//! Text chunking for embedding.
//!
//! We chunk in three passes from coarsest to finest:
//!  1. Split on paragraph breaks (`\n\n+`).
//!  2. If a paragraph still exceeds the target size, split on sentence
//!     boundaries (`. ! ?` followed by whitespace).
//!  3. If a sentence is still too long, hard-split on character windows.
//!
//! All chunks then get a sliding-window overlap so semantic context
//! isn't lost across boundaries.
//!
//! Sizes are in **characters**, not tokens. We could use the e5
//! tokenizer for token-accurate sizing, but: (a) it's an extra
//! dependency in this module, (b) for European text 1 token ≈ 4 chars
//! on average, so a 1600-char target lands comfortably under the 512
//! token limit, with safety margin for outliers (Italian/Latin
//! abbreviations, code samples, URLs).

/// Default chunk size — ~400 tokens of European text. The e5 family
/// supports 512 tokens, this leaves headroom for the `passage:` prefix
/// the model expects on document chunks at embed time.
pub const DEFAULT_CHUNK_CHARS: usize = 1600;

/// Default sliding-window overlap. ~80 tokens — enough to preserve a
/// citation reference or trailing clause that would otherwise be split.
pub const DEFAULT_OVERLAP_CHARS: usize = 320;

#[derive(Debug, Clone)]
pub struct Chunk {
    /// Zero-based index in the document. Stable across re-chunkings of
    /// the same input so we can use `(document_id, chunk_index)` as a
    /// composite key in the vector store.
    pub index: usize,
    pub text: String,
    /// Character offset where this chunk starts in the source document.
    /// Lets the citation layer point back to the exact passage.
    pub start: usize,
    pub end: usize,
    /// 1-based page number this chunk belongs to, when the source has
    /// `[Page N]` markers prefixing each page (PDF scanner output).
    /// `None` for formats without page information (DOCX, XLSX, MD,
    /// TXT, CSV) and for content above the first marker.
    ///
    /// Computed by `chunk_text` from the *source* text BEFORE overlap
    /// is applied — it always reflects the page where the chunk's
    /// first character actually lives, regardless of any leading
    /// overlap tail prepended afterwards.
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Copy)]
pub struct ChunkConfig {
    pub target_chars: usize,
    pub overlap_chars: usize,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            target_chars: DEFAULT_CHUNK_CHARS,
            overlap_chars: DEFAULT_OVERLAP_CHARS,
        }
    }
}

/// Split a document into overlapping chunks suitable for embedding.
///
/// The returned slices retain the original character offsets so the
/// retrieval layer can highlight the exact range in the source file.
/// Empty / whitespace-only inputs yield an empty vector.
pub fn chunk_text(text: &str, cfg: ChunkConfig) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    // Pass 1: paragraph-level chunks. We track byte offsets relative to
    // the original text so citations stay accurate.
    let mut paragraphs: Vec<(usize, &str)> = Vec::new();
    let mut cursor = 0;
    for raw in split_paragraphs(text) {
        // Find this paragraph in the source text starting from cursor —
        // split_paragraphs returns string slices that share the same
        // backing buffer, so we can compute the offset via pointer math.
        let para_start = offset_in(text, raw).unwrap_or(cursor);
        cursor = para_start + raw.len();
        if !raw.trim().is_empty() {
            paragraphs.push((para_start, raw));
        }
    }

    // Pass 2: pack paragraphs into chunks up to target_chars. When a
    // single paragraph exceeds the target, it goes through sentence /
    // hard split.
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut buf = String::new();
    let mut buf_start: Option<usize> = None;
    let mut buf_end: usize = 0;
    let push_buf =
        |chunks: &mut Vec<Chunk>, buf: &mut String, start: &mut Option<usize>, end: usize| {
            if let Some(s) = *start {
                if !buf.trim().is_empty() {
                    let idx = chunks.len();
                    chunks.push(Chunk {
                        index: idx,
                        text: std::mem::take(buf),
                        start: s,
                        end,
                        page: None, // overwritten in pass 2.5
                    });
                }
            }
            buf.clear();
            *start = None;
        };

    for (start, para) in paragraphs {
        if para.len() > cfg.target_chars {
            // Flush any in-progress buffer first.
            push_buf(&mut chunks, &mut buf, &mut buf_start, buf_end);
            for sub in split_oversized(para, cfg.target_chars, start) {
                let idx = chunks.len();
                chunks.push(Chunk {
                    index: idx,
                    text: sub.text,
                    start: sub.start,
                    end: sub.end,
                    page: None, // overwritten in pass 2.5
                });
            }
            continue;
        }
        // Will adding this paragraph blow the target? Flush + start fresh.
        if !buf.is_empty() && buf.len() + para.len() + 2 > cfg.target_chars {
            push_buf(&mut chunks, &mut buf, &mut buf_start, buf_end);
        }
        if buf.is_empty() {
            buf_start = Some(start);
        } else {
            buf.push_str("\n\n");
        }
        buf.push_str(para);
        buf_end = start + para.len();
    }
    push_buf(&mut chunks, &mut buf, &mut buf_start, buf_end);

    // Pass 2.5: stamp the page number on each chunk BEFORE we apply
    // overlap. Doing it before overlap is critical — the prepended tail
    // can carry a stale `[Page N]` marker from the previous page and a
    // naïve regex on the final text would pick that one up instead of
    // the page where the chunk actually begins.
    let page_index = build_page_index(text);
    for c in chunks.iter_mut() {
        c.page = page_for_offset(&page_index, c.start);
    }

    // Pass 3: sliding-window overlap. We rebuild chunks so that each
    // (except the first) starts `overlap_chars` before its current
    // start, taken from the previous chunk's tail. Index re-numbering
    // happens implicitly because we only mutate the `text` field.
    if cfg.overlap_chars > 0 && chunks.len() > 1 {
        for i in 1..chunks.len() {
            // Compute the overlap from the previous chunk into an owned
            // String first; otherwise we'd hold an immutable borrow of
            // `chunks` while taking a mutable one of `chunks[i]`.
            let overlap: String = {
                let prev = &chunks[i - 1];
                let take_n = cfg.overlap_chars.min(prev.text.len());
                char_safe_tail(&prev.text, take_n).to_string()
            };
            let cur = &mut chunks[i];
            cur.text = format!("{overlap}\n…\n{}", cur.text);
        }
    }

    chunks
}

/// Build a sorted list of `(byte_offset, page_number)` pairs from
/// `[Page N]\n` markers found in the source text. The first marker is
/// at offset O₀ → page N₀; everything before O₀ has no page (None).
///
/// We tolerate the marker either at the very start of the buffer or
/// preceded by `\n` — that's how `sync::scanner` emits them.
fn build_page_index(text: &str) -> Vec<(usize, u32)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Look for the literal "[Page " token.
        if bytes[i] == b'[' && bytes.get(i..i + 6) == Some(b"[Page ") {
            // Marker must be at start-of-buffer or right after a `\n`.
            if i == 0 || bytes[i - 1] == b'\n' {
                let num_start = i + 6;
                let mut j = num_start;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > num_start && bytes.get(j) == Some(&b']') {
                    if let Ok(n) = text[num_start..j].parse::<u32>() {
                        out.push((i, n));
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Return the page number for a chunk that starts at `offset` in the
/// source text — the most recent `[Page N]` marker at or before that
/// offset. None when no marker has been seen yet (content above the
/// first marker, or document with no page markers at all).
fn page_for_offset(index: &[(usize, u32)], offset: usize) -> Option<u32> {
    if index.is_empty() {
        return None;
    }
    // Binary search for the largest marker offset ≤ offset.
    let mut lo = 0usize;
    let mut hi = index.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        if index[mid].0 <= offset {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        None
    } else {
        Some(index[lo - 1].1)
    }
}

/// Split on `\n\n+` with paragraph slices preserved. We keep the slices
/// (rather than collecting Strings) so offset math works.
fn split_paragraphs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // Lookahead for one or more additional newlines (with
            // optional spaces in between, common in Word-converted text).
            let mut j = i + 1;
            let mut found_break = false;
            while j < bytes.len() {
                match bytes[j] {
                    b'\n' => {
                        found_break = true;
                        j += 1;
                    }
                    b' ' | b'\t' | b'\r' => j += 1,
                    _ => break,
                }
            }
            if found_break {
                out.push(&text[start..i]);
                start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(&text[start..]);
    }
    out
}

struct SizedPiece {
    text: String,
    start: usize,
    end: usize,
}

/// Break a paragraph that exceeds `target` chars into sentence-sized or
/// hard-cut pieces. `base_start` is the offset of `para` in the
/// original document, so each piece records absolute offsets.
fn split_oversized(para: &str, target: usize, base_start: usize) -> Vec<SizedPiece> {
    let mut out: Vec<SizedPiece> = Vec::new();
    let mut buf = String::new();
    let mut buf_start_in_para: usize = 0;
    let mut cursor: usize = 0; // byte position inside `para`

    for sentence_range in split_sentences(para) {
        let sentence = &para[sentence_range.clone()];
        let len = sentence.len();
        if len > target {
            // Flush whatever is buffered.
            if !buf.is_empty() {
                out.push(SizedPiece {
                    text: std::mem::take(&mut buf),
                    start: base_start + buf_start_in_para,
                    end: base_start + cursor,
                });
            }
            // Hard-cut on `target`-char windows. Use char_safe so we
            // don't slice in the middle of a multi-byte UTF-8 codepoint.
            let mut s = sentence_range.start;
            while s < sentence_range.end {
                let e = char_safe_end(para, (s + target).min(sentence_range.end));
                out.push(SizedPiece {
                    text: para[s..e].to_string(),
                    start: base_start + s,
                    end: base_start + e,
                });
                s = e;
            }
            cursor = sentence_range.end;
            continue;
        }
        if buf.is_empty() {
            buf_start_in_para = sentence_range.start;
        } else if buf.len() + len + 1 > target {
            out.push(SizedPiece {
                text: std::mem::take(&mut buf),
                start: base_start + buf_start_in_para,
                end: base_start + cursor,
            });
            buf_start_in_para = sentence_range.start;
        } else {
            buf.push(' ');
        }
        buf.push_str(sentence);
        cursor = sentence_range.end;
    }
    if !buf.is_empty() {
        out.push(SizedPiece {
            text: buf,
            start: base_start + buf_start_in_para,
            end: base_start + cursor,
        });
    }
    out
}

/// Return byte ranges of sentence-like spans. Sentences end at
/// `. ! ?` followed by whitespace or end-of-input. We avoid splitting
/// after common European abbreviations (e.g. "art.", "n.", "Sig.")
/// to reduce false breaks in legal text.
fn split_sentences(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i];
        if ch == b'.' || ch == b'!' || ch == b'?' {
            let next = bytes.get(i + 1).copied();
            let is_break = matches!(next, Some(b' ') | Some(b'\n') | Some(b'\t')) || next.is_none();
            if is_break && !looks_like_abbrev(text, i) {
                let end = (i + 1).min(bytes.len());
                if end > start {
                    out.push(start..end);
                }
                start = end;
                while start < bytes.len() && matches!(bytes[start], b' ' | b'\n' | b'\t') {
                    start += 1;
                }
                i = start;
                continue;
            }
        }
        i += 1;
    }
    if start < bytes.len() {
        out.push(start..bytes.len());
    }
    out
}

const ABBREVS: &[&str] = &[
    "art", "n", "sig", "sigg", "dott", "dr", "ing", "avv",
    "p", "pp", "vol", "ed", "es", "cfr", "cap", "v",
    "no", "vs", "etc", "e.g", "i.e",
];

fn looks_like_abbrev(text: &str, dot_pos: usize) -> bool {
    let bytes = text.as_bytes();
    // Walk back from the dot collecting alphabetic chars.
    let mut s = dot_pos;
    while s > 0 {
        let c = bytes[s - 1];
        if c.is_ascii_alphabetic() {
            s -= 1;
        } else {
            break;
        }
    }
    let token = &text[s..dot_pos];
    if token.is_empty() {
        return false;
    }
    let lower = token.to_ascii_lowercase();
    ABBREVS.contains(&lower.as_str())
}

/// Move `i` backwards (or stay) so that we land on a UTF-8 char boundary.
fn char_safe_end(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Take the last N bytes of `s` aligned to a char boundary.
fn char_safe_tail(s: &str, n: usize) -> &str {
    if n >= s.len() {
        return s;
    }
    let mut start = s.len() - n;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// Find the byte offset of `slice` inside `parent` using pointer
/// arithmetic. Returns None if the slice doesn't belong to parent.
fn offset_in(parent: &str, slice: &str) -> Option<usize> {
    let p_start = parent.as_ptr() as usize;
    let s_start = slice.as_ptr() as usize;
    if s_start < p_start || s_start > p_start + parent.len() {
        return None;
    }
    Some(s_start - p_start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_text("", ChunkConfig::default()).is_empty());
        assert!(chunk_text("    \n\n  ", ChunkConfig::default()).is_empty());
    }

    #[test]
    fn small_doc_produces_single_chunk() {
        let chunks = chunk_text("Hello world.", ChunkConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "Hello world.");
        assert_eq!(chunks[0].start, 0);
    }

    #[test]
    fn paragraphs_are_packed() {
        let cfg = ChunkConfig {
            target_chars: 30,
            overlap_chars: 0,
        };
        let text = "Una.\n\nDue.\n\nTre lunghissima frase ancora più lunga.";
        let chunks = chunk_text(text, cfg);
        assert!(chunks.len() >= 2);
        // First chunk packs "Una." + "Due." together (under 30 chars).
        assert!(chunks[0].text.contains("Una"));
        assert!(chunks[0].text.contains("Due"));
    }

    #[test]
    fn oversized_paragraph_is_hard_split() {
        let cfg = ChunkConfig {
            target_chars: 20,
            overlap_chars: 0,
        };
        let text = "a".repeat(100);
        let chunks = chunk_text(&text, cfg);
        assert!(chunks.len() >= 5);
        assert!(chunks.iter().all(|c| c.text.len() <= 20));
    }

    #[test]
    fn abbrev_does_not_break_sentence() {
        // A naive sentence splitter would break here on "art."
        let text = "Vedi l'art. 1234 del codice civile.";
        let cfg = ChunkConfig {
            target_chars: 100,
            overlap_chars: 0,
        };
        let chunks = chunk_text(text, cfg);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn overlap_is_added_between_chunks() {
        let cfg = ChunkConfig {
            target_chars: 30,
            overlap_chars: 5,
        };
        let text = "Primo paragrafo.\n\nSecondo paragrafo qui.\n\nTerzo paragrafo.";
        let chunks = chunk_text(text, cfg);
        if chunks.len() > 1 {
            // Each non-first chunk should contain the overlap marker.
            for c in &chunks[1..] {
                assert!(c.text.contains('…'));
            }
        }
    }

    #[test]
    fn indices_are_sequential_starting_at_zero() {
        let cfg = ChunkConfig { target_chars: 20, overlap_chars: 0 };
        let text = "Una.\n\nDue.\n\nTre.\n\nQuattro.\n\nCinque.";
        let chunks = chunk_text(text, cfg);
        assert!(chunks.len() >= 2);
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.index, i, "chunk indices must be 0..n");
        }
    }

    #[test]
    fn offsets_are_within_source_length() {
        let cfg = ChunkConfig { target_chars: 30, overlap_chars: 0 };
        let text = "Primo paragrafo qui.\n\nSecondo molto più lungo del primo paragrafo.";
        let chunks = chunk_text(text, cfg);
        for c in &chunks {
            assert!(c.start <= text.len(), "start must be inside source");
            assert!(c.end <= text.len(), "end must be inside source");
            assert!(c.start <= c.end, "start <= end");
        }
    }

    #[test]
    fn utf8_multibyte_does_not_panic() {
        // Italian + emoji + accented chars all sharing a single oversized
        // paragraph so we hit the hard-cut path.
        let text = "àèìòù ñ ç € 你好 🚀 ".repeat(50);
        let cfg = ChunkConfig { target_chars: 32, overlap_chars: 8 };
        let chunks = chunk_text(&text, cfg);
        assert!(!chunks.is_empty());
        // Every chunk must be valid UTF-8 (implicit if it's a String, but
        // the hard-cut code path in particular must not panic on
        // multi-byte boundaries).
        for c in &chunks {
            assert!(c.text.is_char_boundary(0));
            assert!(c.text.is_char_boundary(c.text.len()));
        }
    }

    #[test]
    fn paragraph_separator_preserved_when_packed() {
        let cfg = ChunkConfig { target_chars: 200, overlap_chars: 0 };
        let chunks = chunk_text("Primo.\n\nSecondo.", cfg);
        assert_eq!(chunks.len(), 1);
        // Both paragraphs joined by `\n\n` per pack_buf logic.
        assert!(chunks[0].text.contains("Primo."));
        assert!(chunks[0].text.contains("Secondo."));
        assert!(chunks[0].text.contains("\n\n"));
    }

    #[test]
    fn whitespace_only_paragraphs_are_dropped() {
        let cfg = ChunkConfig::default();
        let text = "Una.\n\n   \n\n   \n\nDue.";
        let chunks = chunk_text(text, cfg);
        assert!(chunks.iter().all(|c| !c.text.trim().is_empty()));
    }

    #[test]
    fn overlap_zero_does_not_inject_separator() {
        let cfg = ChunkConfig { target_chars: 15, overlap_chars: 0 };
        let chunks = chunk_text("Una.\n\nDue.\n\nTre.", cfg);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(!c.text.contains('…'), "no overlap separator with overlap_chars=0");
        }
    }

    #[test]
    fn extra_blank_lines_collapse_to_single_break() {
        let cfg = ChunkConfig { target_chars: 200, overlap_chars: 0 };
        let chunks = chunk_text("Una.\n\n\n\n\nDue.\n\n\n\n\n\nTre.", cfg);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("Una."));
        assert!(chunks[0].text.contains("Due."));
        assert!(chunks[0].text.contains("Tre."));
    }

    #[test]
    fn many_short_sentences_pack_into_few_chunks() {
        let cfg = ChunkConfig { target_chars: 100, overlap_chars: 0 };
        let mut text = String::new();
        for i in 0..50 {
            text.push_str(&format!("Frase numero {i}.\n\n"));
        }
        let chunks = chunk_text(&text, cfg);
        // Each chunk must respect the target (paragraph-packing path).
        assert!(chunks.iter().all(|c| c.text.len() <= 100 + 4));
        // We packed 50 short paragraphs, so we should not get 50 chunks.
        assert!(chunks.len() < 50);
    }

    #[test]
    fn config_default_values_match_constants() {
        let cfg = ChunkConfig::default();
        assert_eq!(cfg.target_chars, DEFAULT_CHUNK_CHARS);
        assert_eq!(cfg.overlap_chars, DEFAULT_OVERLAP_CHARS);
    }

    #[test]
    fn split_paragraphs_yields_non_empty_slices() {
        let parts = split_paragraphs("a\n\nb\n\nc");
        assert_eq!(parts, vec!["a", "b", "c"]);
    }

    #[test]
    fn split_paragraphs_handles_trailing_newlines() {
        let parts = split_paragraphs("a\n\nb\n\n");
        assert_eq!(parts, vec!["a", "b"]);
    }

    #[test]
    fn looks_like_abbrev_recognizes_common_legal_terms() {
        // Each abbreviation followed by its dot.
        for s in &["art.", "n.", "Sig.", "cfr.", "etc."] {
            let dot = s.len() - 1;
            assert!(looks_like_abbrev(s, dot), "should recognize {s}");
        }
        // Random word should not match.
        assert!(!looks_like_abbrev("hello.", 5));
    }

    #[test]
    fn char_safe_end_lands_on_boundary() {
        let s = "café";
        // byte 3 is in the middle of the 'é' (2-byte UTF-8). Land on 3.
        assert_eq!(char_safe_end(s, 4), 3);
        assert_eq!(char_safe_end(s, 100), s.len());
    }

    #[test]
    fn char_safe_tail_does_not_split_codepoint() {
        let s = "café";
        // n=3 would land mid-codepoint; expect aligned slice.
        let tail = char_safe_tail(s, 3);
        // Just verify it's a valid UTF-8 suffix.
        assert!(s.ends_with(tail));
    }

    #[test]
    fn offset_in_works_for_subslice() {
        let parent = "hello world";
        let sub = &parent[6..];
        assert_eq!(offset_in(parent, sub), Some(6));
    }

    #[test]
    fn offset_in_rejects_unrelated_slice() {
        let parent = "hello";
        let other = String::from("hello");
        assert_eq!(offset_in(parent, &other), None);
    }

    // ---- page extraction ----

    #[test]
    fn build_page_index_finds_markers_at_buffer_start() {
        let text = "[Page 1]\nContenuto pagina uno.";
        let idx = build_page_index(text);
        assert_eq!(idx, vec![(0, 1)]);
    }

    #[test]
    fn build_page_index_finds_markers_after_newline() {
        let text = "[Page 1]\nfirst\n\n[Page 2]\nsecond";
        let idx = build_page_index(text);
        assert_eq!(idx.len(), 2);
        assert_eq!(idx[0].1, 1);
        assert_eq!(idx[1].1, 2);
        // Marker 2 must come after marker 1's offset.
        assert!(idx[1].0 > idx[0].0);
    }

    #[test]
    fn build_page_index_ignores_marker_inside_text() {
        // Not preceded by `\n` or start-of-buffer → not a real marker.
        let text = "see [Page 99] in the body";
        let idx = build_page_index(text);
        assert!(idx.is_empty());
    }

    #[test]
    fn build_page_index_handles_multi_digit_pages() {
        let text = "[Page 12]\nfoo\n\n[Page 345]\nbar";
        let idx = build_page_index(text);
        assert_eq!(idx.iter().map(|(_, n)| *n).collect::<Vec<_>>(), vec![12, 345]);
    }

    #[test]
    fn page_for_offset_returns_none_before_first_marker() {
        let idx = vec![(10, 1u32), (50, 2u32)];
        assert_eq!(page_for_offset(&idx, 0), None);
        assert_eq!(page_for_offset(&idx, 9), None);
    }

    #[test]
    fn page_for_offset_returns_marker_at_or_before() {
        let idx = vec![(10, 1u32), (50, 2u32), (100, 3u32)];
        assert_eq!(page_for_offset(&idx, 10), Some(1));
        assert_eq!(page_for_offset(&idx, 49), Some(1));
        assert_eq!(page_for_offset(&idx, 50), Some(2));
        assert_eq!(page_for_offset(&idx, 99), Some(2));
        assert_eq!(page_for_offset(&idx, 100), Some(3));
        assert_eq!(page_for_offset(&idx, 9999), Some(3));
    }

    #[test]
    fn chunk_text_stamps_page_on_pdf_style_input() {
        // Mimic sync::scanner output: paragraph-separated `[Page N]\n<text>`.
        let mut text = String::new();
        for i in 1..=4 {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            text.push_str(&format!("[Page {i}]\n"));
            // Each page is 200 chars so several chunks straddle page boundaries.
            text.push_str(&"x".repeat(200));
        }
        let cfg = ChunkConfig {
            target_chars: 250,
            overlap_chars: 0,
        };
        let chunks = chunk_text(&text, cfg);
        assert!(!chunks.is_empty());
        // The first chunk must report page 1.
        assert_eq!(chunks[0].page, Some(1));
        // Pages must be monotonically non-decreasing across chunks
        // (chunks are emitted in document order).
        let mut last = 0u32;
        for c in &chunks {
            let p = c.page.expect("each chunk should have a page");
            assert!(p >= last, "page must be monotonic, got {p} after {last}");
            last = p;
        }
        // We should observe at least pages 1 and 2 in the chunk stream.
        let pages: std::collections::BTreeSet<u32> = chunks
            .iter()
            .filter_map(|c| c.page)
            .collect();
        assert!(pages.contains(&1));
        assert!(pages.contains(&2));
    }

    #[test]
    fn chunk_text_returns_none_for_text_without_page_markers() {
        let text = "Un documento senza marcatori di pagina.\n\nSecondo paragrafo.";
        let chunks = chunk_text(text, ChunkConfig::default());
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].page, None);
    }

    #[test]
    fn chunk_page_unaffected_by_overlap_tail() {
        // Pages 1 and 2 with enough text that overlap from prev chunk
        // (page 1) leaks into the start of the next chunk's text. The
        // page assignment must still reflect where the chunk's *content*
        // begins (page 2), not what the prepended overlap says.
        let mut text = String::new();
        text.push_str("[Page 1]\n");
        text.push_str(&"a".repeat(400));
        text.push_str("\n\n[Page 2]\n");
        text.push_str(&"b".repeat(400));
        let cfg = ChunkConfig { target_chars: 200, overlap_chars: 50 };
        let chunks = chunk_text(&text, cfg);
        assert!(chunks.len() >= 4);
        // Find the first chunk whose source-position-start is on page 2.
        // page index for offset of "[Page 2]" in the text:
        let p2_offset = text.find("[Page 2]").unwrap();
        let p2_chunk = chunks.iter().find(|c| c.start >= p2_offset).unwrap();
        assert_eq!(p2_chunk.page, Some(2),
            "chunk starting at page 2 byte offset must be tagged page 2 \
             even though its text begins with overlap from page 1");
    }
}
