//! Structure-aware chunking for Indian legal documents.
//!
//! Pure regex/heuristics, NO LLM and NO DB. Splits a full document into
//! ordered chunks at the structural boundaries Indian pleadings actually
//! use (ALL-CAPS section headings, the canonical drafting markers like
//! "MOST RESPECTFULLY SHOWETH" / "PRAYER", and numbered `1. That,` paras),
//! carrying the nearest preceding heading and a heuristic `section_role`
//! onto each chunk. Oversized blocks are hard-split at sentence
//! boundaries (~1800 char cap) and very short adjacent chunks under the
//! same heading are merged so the FTS index isn't full of one-line slivers.
//!
//! The heading vocabulary mirrors `case_prep::INDIAN_LEGAL_CONTEXT`.

/// One unit of a chunked document. `seq` is the 0-based order within the
/// file; `heading` is the nearest preceding section heading (if any);
/// `section_role` is a coarse classification of that heading; `page` is
/// the `[Page N]` marker in force when the chunk starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub seq: i64,
    pub heading: Option<String>,
    pub section_role: Option<String>,
    pub page: Option<i64>,
    pub text: String,
}

/// Soft cap on chunk size; blocks larger than this are hard-split at
/// sentence boundaries.
const MAX_CHARS: usize = 1800;
/// Chunks shorter than this get merged into an adjacent chunk under the
/// same heading.
const MIN_MERGE_CHARS: usize = 300;

/// The canonical drafting markers that begin a new section even when they
/// aren't in ALL CAPS. Matched case-insensitively against the start of a
/// trimmed line.
const MARKERS: &[&str] = &[
    "MOST RESPECTFULLY SHOWETH",
    "RESPECTFULLY SHOWETH",
    "PRELIMINARY OBJECTIONS",
    "PARAWISE REPLY",
    "GROUNDS",
    "PRAYER",
    "VERIFICATION",
    "SYNOPSIS",
    "LIST OF DATES",
    "STATEMENT OF FACTS",
    "WHEREAS",
    "NOW THIS DEED WITNESSETH",
];

/// Build the chunks for a full document body.
pub fn chunk_legal_text(full_text: &str) -> Vec<Chunk> {
    // Pass 1: walk the lines, tracking the running page and the current
    // heading, emitting a "block" each time a heading/marker starts a new
    // section. A block is (heading, section_role, page, text).
    struct Block {
        heading: Option<String>,
        section_role: Option<String>,
        page: Option<i64>,
        text: String,
    }

    let mut blocks: Vec<Block> = Vec::new();
    let mut cur_heading: Option<String> = None;
    let mut cur_role: Option<String> = None;
    let mut cur_page: Option<i64> = None;
    let mut buf = String::new();
    // The page in force at the moment the *current* buffer started.
    let mut buf_page: Option<i64> = None;

    macro_rules! flush {
        () => {
            if !buf.trim().is_empty() {
                blocks.push(Block {
                    heading: cur_heading.clone(),
                    section_role: cur_role.clone(),
                    page: buf_page,
                    text: buf.trim().to_string(),
                });
            }
            buf.clear();
        };
    }

    for raw_line in full_text.lines() {
        // Form-feed (page break) — treat as an implicit page increment when
        // we don't have explicit [Page N] markers.
        let has_ff = raw_line.contains('\u{000C}');
        let line = raw_line.replace('\u{000C}', "").trim().to_string();

        if let Some(p) = parse_page_marker(&line) {
            cur_page = Some(p);
            // A page marker is structural, not content; don't add it to a
            // chunk. If the buffer is empty, future text inherits this page.
            if buf.trim().is_empty() {
                buf_page = Some(p);
            }
            continue;
        }
        if has_ff {
            cur_page = Some(cur_page.unwrap_or(1) + 1);
            if buf.trim().is_empty() {
                buf_page = cur_page;
            }
        }

        if let Some((heading, role)) = detect_heading(&line) {
            // A new section starts here: flush whatever we accumulated under
            // the previous heading, then set the new heading.
            flush!();
            cur_heading = Some(heading);
            cur_role = Some(role);
            buf_page = cur_page;
            // Keep the heading line itself as the first text of the new
            // block so search still hits it and expand_chunk shows it.
            buf.push_str(&line);
            buf.push('\n');
            continue;
        }

        if is_numbered_para_start(&line) {
            // Numbered "1. That," paragraphs start a fresh chunk but stay
            // under the same heading.
            flush!();
            buf_page = cur_page;
            buf.push_str(&line);
            buf.push('\n');
            continue;
        }

        if buf.trim().is_empty() {
            buf_page = cur_page;
        }
        buf.push_str(&line);
        buf.push('\n');
    }
    flush!();

    // Pass 2: hard-split oversized blocks at sentence boundaries.
    let mut split_blocks: Vec<Block> = Vec::new();
    for b in blocks {
        if b.text.chars().count() <= MAX_CHARS {
            split_blocks.push(b);
            continue;
        }
        for piece in hard_split(&b.text) {
            split_blocks.push(Block {
                heading: b.heading.clone(),
                section_role: b.section_role.clone(),
                page: b.page,
                text: piece,
            });
        }
    }

    // Pass 3: merge very short adjacent blocks that share a heading.
    let mut merged: Vec<Block> = Vec::new();
    for b in split_blocks {
        if let Some(last) = merged.last_mut() {
            let same_heading = last.heading == b.heading;
            let same_page = last.page == b.page;
            let last_short = last.text.chars().count() < MIN_MERGE_CHARS;
            let combined = last.text.chars().count() + b.text.chars().count();
            if same_heading && same_page && last_short && combined <= MAX_CHARS {
                last.text.push('\n');
                last.text.push_str(&b.text);
                continue;
            }
        }
        merged.push(b);
    }

    merged
        .into_iter()
        .enumerate()
        .map(|(i, b)| Chunk {
            seq: i as i64,
            heading: b.heading,
            section_role: b.section_role,
            page: b.page,
            text: b.text,
        })
        .collect()
}

/// Parse a `[Page N]` marker (the form `extract_text_dispatch` emits for
/// PDFs). Returns the page number when the whole line is just that marker.
fn parse_page_marker(line: &str) -> Option<i64> {
    let t = line.trim();
    let inner = t.strip_prefix('[')?.strip_suffix(']')?;
    let rest = inner.trim().strip_prefix("Page").or_else(|| inner.trim().strip_prefix("PAGE"))?;
    rest.trim().parse::<i64>().ok()
}

/// Whether a line is a section heading, and its coarse `section_role`.
///
/// Two ways to qualify:
///  * it begins with one of the canonical drafting `MARKERS`, or
///  * it is a short ALL-CAPS line (a hand-typed section banner).
fn detect_heading(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() {
        return None;
    }
    let upper = t.to_uppercase();

    for m in MARKERS {
        if upper.starts_with(m) {
            return Some((t.to_string(), role_for(&upper)));
        }
    }

    // ALL-CAPS heading: short line, has letters, and every cased letter is
    // uppercase. Guard against very long all-caps paragraphs (OCR) and
    // numbered "That," lines that happen to be shouted.
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    let is_all_caps = !letters.is_empty()
        && letters.iter().all(|c| c.is_uppercase())
        && t.chars().count() <= 80
        && t.chars().count() >= 3;
    if is_all_caps && !is_numbered_para_start(t) {
        return Some((t.to_string(), role_for(&upper)));
    }
    None
}

/// Heuristic mapping from a heading's text to a coarse role tag.
fn role_for(upper: &str) -> String {
    if upper.contains("GROUND") {
        "ground"
    } else if upper.contains("PRAYER") || upper.contains("RELIEF") {
        "prayer"
    } else if upper.contains("VERIFICATION") {
        "verification"
    } else if upper.contains("FACTS") || upper.contains("SHOWETH") || upper.contains("STATEMENT OF FACTS") {
        "facts"
    } else if upper.contains("WITNESSETH") || upper.contains("WHEREAS") || upper.contains("CLAUSE") {
        "clause"
    } else if upper.contains("IN THE COURT")
        || upper.contains("IN THE HIGH COURT")
        || upper.contains("HIGH COURT")
        || upper.contains("SUPREME COURT")
        || upper.contains("BEFORE THE")
        || upper.contains("TRIBUNAL")
        || upper.contains("VERSUS")
        || upper.contains(" VS ")
        || upper.contains("IN THE MATTER OF")
    {
        "cause_title"
    } else if upper.contains("ARGUMENT")
        || upper.contains("SUBMISSION")
        || upper.contains("OBJECTION")
        || upper.contains("REPLY")
    {
        "argument"
    } else {
        "other"
    }
    .to_string()
}

/// Whether a line begins a numbered paragraph like `1. That,` / `12) That`
/// or a bare `1.` / `(a)` enumerated clause.
fn is_numbered_para_start(line: &str) -> bool {
    let t = line.trim_start();
    let mut chars = t.char_indices().peekable();

    // (a) / (i) style clause openers.
    if t.starts_with('(') {
        if let Some(close) = t.find(')') {
            let inner = &t[1..close];
            if !inner.is_empty()
                && inner.len() <= 4
                && inner.chars().all(|c| c.is_alphanumeric())
            {
                return true;
            }
        }
    }

    // Leading digits then `.` or `)`.
    let mut saw_digit = false;
    while let Some(&(_, c)) = chars.peek() {
        if c.is_ascii_digit() {
            saw_digit = true;
            chars.next();
        } else {
            break;
        }
    }
    if saw_digit {
        if let Some(&(_, c)) = chars.peek() {
            if c == '.' || c == ')' {
                return true;
            }
        }
    }
    false
}

/// Split an oversized block into <=MAX_CHARS pieces, preferring sentence
/// boundaries (`.`/`?`/`!` followed by whitespace). Falls back to a hard
/// char cut only when a single "sentence" is itself larger than the cap.
fn hard_split(text: &str) -> Vec<String> {
    let sentences = split_sentences(text);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for s in sentences {
        if cur.chars().count() + s.chars().count() > MAX_CHARS && !cur.is_empty() {
            out.push(cur.trim().to_string());
            cur = String::new();
        }
        if s.chars().count() > MAX_CHARS {
            // A single monster sentence — cut it at the char cap.
            for piece in char_chunks(&s, MAX_CHARS) {
                if !cur.is_empty() {
                    out.push(cur.trim().to_string());
                    cur = String::new();
                }
                out.push(piece);
            }
            continue;
        }
        cur.push_str(&s);
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// Split into sentence-ish units, keeping the terminator attached.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        cur.push(c);
        if (c == '.' || c == '?' || c == '!' || c == '\n')
            && chars.get(i + 1).map(|n| n.is_whitespace()).unwrap_or(true)
        {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Hard char-bounded split (last resort for a single huge token/sentence).
fn char_chunks(text: &str, max: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(max)
        .map(|c| c.iter().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_grounds_section_and_role() {
        let text = "\
IN THE HIGH COURT OF DELHI

GROUNDS

A. Because the impugned order is contrary to law and the settled position laid down by the Hon'ble Supreme Court.
B. Because the Ld. Trial Court failed to appreciate the evidence on record.";
        let chunks = chunk_legal_text(text);
        let grounds: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| c.section_role.as_deref() == Some("ground"))
            .collect();
        assert!(!grounds.is_empty(), "expected a ground chunk: {chunks:#?}");
        assert!(grounds[0]
            .heading
            .as_deref()
            .unwrap()
            .to_uppercase()
            .contains("GROUNDS"));
        // Cause title should be its own role.
        assert!(chunks
            .iter()
            .any(|c| c.section_role.as_deref() == Some("cause_title")));
    }

    #[test]
    fn splits_numbered_that_paragraphs() {
        let text = "\
MOST RESPECTFULLY SHOWETH:

1. That, the complainant is a permanent resident of Delhi and is gainfully employed.
2. That, the accused issued a cheque dated 01.01.2024 which was dishonoured on presentation.
3. That, a legal demand notice was duly served upon the accused.";
        let chunks = chunk_legal_text(text);
        // Each numbered para (after merge) should be discoverable; at least
        // the facts heading is present and the paras are under it.
        let facts: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| c.section_role.as_deref() == Some("facts"))
            .collect();
        assert!(!facts.is_empty(), "expected facts chunks: {chunks:#?}");
        let joined: String = chunks.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join("\n");
        assert!(joined.contains("dishonoured"));
        assert!(joined.contains("legal demand notice"));
    }

    #[test]
    fn detects_prayer_section() {
        let text = "\
GROUNDS

A. Because the order is bad in law.

PRAYER

It is therefore most respectfully prayed that this Hon'ble Court may be pleased to set aside the impugned order and grant such other relief as deemed fit.";
        let chunks = chunk_legal_text(text);
        assert!(
            chunks.iter().any(|c| c.section_role.as_deref() == Some("prayer")),
            "expected a prayer chunk: {chunks:#?}"
        );
    }

    #[test]
    fn tracks_page_markers() {
        let text = "\
[Page 1]
IN THE COURT OF THE METROPOLITAN MAGISTRATE

Some introductory text on the first page.

[Page 2]
2. That, the facts continue onto the second page here.";
        let chunks = chunk_legal_text(text);
        // A chunk that starts on page 2 should carry page == 2.
        assert!(
            chunks.iter().any(|c| c.page == Some(2) && c.text.contains("second page")),
            "expected a page-2 chunk: {chunks:#?}"
        );
        // And the cause title should be page 1.
        assert!(chunks.iter().any(|c| c.page == Some(1)));
    }

    #[test]
    fn seqs_are_contiguous_from_zero() {
        let text = "\
GROUNDS
A. Because.
PRAYER
Pray for relief here in a slightly longer sentence so it survives the merge pass.";
        let chunks = chunk_legal_text(text);
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.seq, i as i64);
        }
    }

    #[test]
    fn hard_splits_oversized_block() {
        // One heading, then a very long body of many sentences > 1800 chars.
        let sentence = "That this is a fairly long sentence used to pad the chunk well beyond the cap. ";
        let body = sentence.repeat(60); // ~ 60 * 78 chars ≈ 4680
        let text = format!("STATEMENT OF FACTS\n{body}");
        let chunks = chunk_legal_text(&text);
        assert!(chunks.len() >= 2, "oversized block should split: {} chunks", chunks.len());
        for c in &chunks {
            assert!(
                c.text.chars().count() <= MAX_CHARS + 200,
                "chunk too large: {}",
                c.text.chars().count()
            );
        }
    }

    #[test]
    fn merges_short_adjacent_chunks() {
        // Several tiny lines under one heading should merge rather than
        // producing many one-line chunks.
        let text = "\
VERIFICATION
Verified at Delhi.
On 01.01.2024.
Contents are true.";
        let chunks = chunk_legal_text(text);
        let verif: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| c.section_role.as_deref() == Some("verification"))
            .collect();
        assert_eq!(verif.len(), 1, "short verification lines should merge: {chunks:#?}");
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_legal_text("").is_empty());
        assert!(chunk_legal_text("   \n  \n").is_empty());
    }
}
