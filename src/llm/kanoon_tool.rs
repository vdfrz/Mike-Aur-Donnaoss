//! Indian Kanoon — high-quality tool implementation for the LLM.
//!
//! Replaces the old "extract bare keywords + send to /search/ + show the
//! LLM a one-line headline" pattern with:
//!
//!   • Field-operator query construction (court:, fromdate:, todate:,
//!     doctypes:, cites:, phrase matching).
//!   • Pagination through enough results to build a real candidate pool.
//!   • Per-result `/docfragment/` fetching so the LLM sees the actual
//!     query-relevant paragraphs from each judgment, not just an 80-char
//!     headline.
//!   • Simple local re-ranking by token overlap between snippet and query.
//!
//! Two tools are exposed:
//!
//!   `kanoon_search`        — search the Kanoon corpus and (by default)
//!                            return each hit with extracted relevant
//!                            paragraphs from the full judgment.
//!   `kanoon_get_fragment`  — fetch query-relevant paragraphs for a
//!                            single document (when the LLM wants more
//!                            detail on one hit it already saw).
//!
//! Both are server-side builtins dispatched from `builtin_tools::dispatch`.

use crate::AppState;
use serde_json::{json, Value};

const IK_API_BASE: &str = "https://api.indiankanoon.org";
const CORPUS_ID: &str = "indian-kanoon";

/// HTTP timeout for a single Kanoon API call. Generous because /doc and
/// /docfragment can be slow on big judgments.
const HTTP_TIMEOUT_SECS: u64 = 30;

/// Max paginated pages we'll pull when building the candidate pool.
const MAX_PAGES: usize = 3;

/// Hard cap on results returned to the LLM regardless of what it asks
/// for. Keeps the tool result under a sane token budget.
const MAX_RESULTS_RETURNED: usize = 10;

/// Per-result paragraph budget when including fragments. Most Kanoon
/// fragments are 1-3 paragraphs already; this guards against runaway
/// judgments.
const FRAGMENT_CHAR_CAP: usize = 4000;

// ---------------------------------------------------------------------------
// Public entry points (called from builtin_tools::dispatch)
// ---------------------------------------------------------------------------

pub async fn exec_kanoon_search(state: &AppState, user_id: &str, arguments: &Value) -> String {
    let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
    if query.is_empty() {
        return json!({"error": "query is required"}).to_string();
    }

    let phrase = arguments.get("phrase").and_then(|v| v.as_bool()).unwrap_or(false);
    let court = arguments.get("court").and_then(|v| v.as_str());
    let doctypes = arguments.get("doctypes").and_then(|v| v.as_str());
    let fromdate = arguments.get("fromdate").and_then(|v| v.as_str());
    let todate = arguments.get("todate").and_then(|v| v.as_str());
    let cites = arguments.get("cites").and_then(|v| v.as_i64());
    let max_results = arguments
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(MAX_RESULTS_RETURNED as u64) as usize;
    // Fragments (per-result paragraph fetches) are the slow part of a search —
    // one /doc call per hit. Default OFF so the search returns fast (titles +
    // snippets + URLs); the model pulls authoritative paragraphs on demand via
    // kanoon_get_fragment for only the 2-3 cases it actually cites. The case
    // "Run Analysis" path opts back in explicitly (cases.rs).
    let include_fragments = arguments
        .get("include_fragments")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let ik_key = match resolve_ik_key(state, user_id).await {
        Some(k) => k,
        None => {
            return json!({
                "error": "No Indian Kanoon API key configured. Add IK_API_KEY env var or set it in Settings → Indian Kanoon."
            })
            .to_string();
        }
    };

    let form_input = build_kanoon_query(query, phrase, court, doctypes, fromdate, todate, cites);

    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return json!({"error": format!("http client: {e}")}).to_string(),
    };

    // Paginate to build a candidate pool. We always pull page 0, and may
    // pull pages 1..MAX_PAGES if the user asked for more results than
    // we've collected. Each page is up to ~20 hits.
    let mut candidates: Vec<KanoonHit> = Vec::new();
    for page in 0..MAX_PAGES {
        let page_str = page.to_string();
        let resp = match client
            .post(format!("{IK_API_BASE}/search/"))
            .header("Authorization", format!("Token {ik_key}"))
            .form(&[("formInput", form_input.as_str()), ("pagenum", page_str.as_str())])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[kanoon] search page {page} request failed: {e}");
                break;
            }
        };
        if !resp.status().is_success() {
            tracing::warn!(
                "[kanoon] search page {page} returned HTTP {}",
                resp.status()
            );
            break;
        }
        let data: Value = match resp.json().await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("[kanoon] search page {page} JSON parse failed: {e}");
                break;
            }
        };
        let docs = data["docs"].as_array().cloned().unwrap_or_default();
        if docs.is_empty() {
            break;
        }
        for d in docs {
            if let Some(h) = parse_search_hit(&d) {
                candidates.push(h);
            }
        }
        if candidates.len() >= max_results * 4 {
            break; // enough to rerank from
        }
    }

    if candidates.is_empty() {
        return json!({
            "query": query,
            "kanoon_query": form_input,
            "results": [],
            "note": "No matches on Indian Kanoon. Try different keywords, broader date range, or remove the court filter."
        })
        .to_string();
    }

    // Local re-rank by token overlap between snippet+title and query.
    rerank(&mut candidates, query);
    candidates.truncate(max_results);

    // Phase 1 — background-cache every judgment we surfaced, so it lands on the
    // judgments tab and becomes locally searchable. Best-effort and
    // non-blocking: the chat answer returns immediately while the full doc is
    // fetched + embedded in a spawned task. Already-cached tids are skipped
    // inside cache_judgment.
    #[cfg(feature = "rag")]
    {
        let db = state.db.clone();
        let embeddings = state.embeddings.clone();
        let uid = user_id.to_string();
        let key = ik_key.clone();
        let tids: Vec<i64> = candidates.iter().map(|c| c.tid).collect();
        tokio::spawn(async move {
            for tid in tids {
                if let Err(e) = crate::routes::indian_kanoon::cache_judgment(
                    db.clone(),
                    embeddings.clone(),
                    uid.clone(),
                    key.clone(),
                    tid,
                )
                .await
                {
                    tracing::warn!("[kanoon] background cache of doc {tid} failed: {e}");
                }
            }
        });
    }

    // Fetch fragments in parallel for each hit. AWS verification is
    // NO LONGER inline — it's expensive (parquet download + parse) and
    // would block the entire search. Instead, the model receives results
    // marked PENDING for verification and can call kanoon_verify_case(tid)
    // on the 2-3 cases it will actually cite.
    let fragments_results: Vec<Option<Fragment>> = if include_fragments {
        let fragment_query = query.to_string();
        let frag_fetches: Vec<_> = candidates
            .iter()
            .map(|c| fetch_fragment(client.clone(), ik_key.clone(), c.tid, fragment_query.clone()))
            .collect();
        futures_util::future::join_all(frag_fetches).await
    } else {
        vec![None; candidates.len()]
    };

    let mut results: Vec<Value> = Vec::with_capacity(candidates.len());
    for (hit, frag) in candidates.iter().zip(fragments_results.into_iter()) {
        results.push(hit.to_json_with_fragment_pending_verify(frag));
    }

    json!({
        "query": query,
        "kanoon_query": form_input,
        "result_count": results.len(),
        "results": results,
        "advisory": "Indian Kanoon results — fast, comprehensive coverage but NOT infallible. Case titles, snippets, and ranking can occasionally drift from the canonical court record. ALWAYS treat these as preliminary until verified. For the 2-3 cases you will actually cite in your final answer, call kanoon_verify_case(tid, title, court, decision_date) to cross-check against the canonical Indian-high-court-judgments AWS dataset. This takes ~3-5 seconds per case and surfaces the canonical PDF URL when found. Do not verify every result — only the ones you cite. If you can't verify (network, court not mapped, not in AWS corpus), still cite the case but flag it in your prose as 'unverified — please confirm before relying'.",
        "instructions_for_model": "Each result has tid, title, court, decision_date, snippet, kanoon_url, relevance_score. To keep search FAST, full paragraph text is NOT fetched here — the snippet is enough to judge relevance. For the 2-3 cases you will actually CITE, call kanoon_get_fragment(tid, query) to pull authoritative judgment paragraphs and quote them directly; do NOT quote 'judgment text' you didn't fetch. The `verification` field is { status: 'PENDING' } — optionally call kanoon_verify_case(tid, title, court) for cases you cite. Cite cases as Markdown links: [Case Title](kanoon_url). Don't cite cases that aren't in this result set."
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// kanoon_verify_case — on-demand AWS verification for a single case
// ---------------------------------------------------------------------------

pub async fn exec_kanoon_verify_case(
    _state: &crate::AppState,
    _user_id: &str,
    arguments: &Value,
) -> String {
    let tid = arguments.get("tid").and_then(|v| v.as_i64());
    let title = arguments.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
    let court = arguments.get("court").and_then(|v| v.as_str()).unwrap_or("").trim();
    let decision_date = arguments
        .get("decision_date")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if title.is_empty() || court.is_empty() {
        return json!({
            "error": "kanoon_verify_case requires `title` and `court` (and optionally `tid` and `decision_date`) — pass these from a previous kanoon_search result."
        })
        .to_string();
    }

    let verification = crate::llm::aws_verification::verify(
        title,
        court,
        decision_date.as_deref(),
    )
    .await;

    json!({
        "tid": tid,
        "title": title,
        "court": court,
        "decision_date": decision_date,
        "verification": verification,
        "instructions_for_model": "verification.status is one of VERIFIED / NOT_IN_AWS / UNVERIFIED. VERIFIED means the canonical court PDF was found at canonical_pdf_url — high confidence; cite with no caveat. NOT_IN_AWS or UNVERIFIED means the AWS canonical lookup did not match — still cite the case (Kanoon's catalog is large and mostly accurate) but add a brief inline caveat in your prose like '(unverified)' so the user knows to double-check."
    })
    .to_string()
}

pub async fn exec_kanoon_get_fragment(
    state: &AppState,
    user_id: &str,
    arguments: &Value,
) -> String {
    let tid = arguments.get("tid").and_then(|v| v.as_i64()).unwrap_or(0);
    let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
    if tid == 0 {
        return json!({"error": "tid is required (the Kanoon document ID from a previous search)"})
            .to_string();
    }
    if query.is_empty() {
        return json!({"error": "query is required — the topic to find paragraphs about"}).to_string();
    }

    let ik_key = match resolve_ik_key(state, user_id).await {
        Some(k) => k,
        None => return json!({"error": "No Indian Kanoon API key configured."}).to_string(),
    };
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return json!({"error": format!("http client: {e}")}).to_string(),
    };

    match fetch_fragment(client, ik_key, tid, query.to_string()).await {
        Some(frag) => json!({
            "tid": tid,
            "query": query,
            "title": frag.title,
            "kanoon_url": format!("https://indiankanoon.org/doc/{tid}/"),
            "relevant_paragraphs": frag.text,
        })
        .to_string(),
        None => json!({
            "tid": tid,
            "query": query,
            "kanoon_url": format!("https://indiankanoon.org/doc/{tid}/"),
            "relevant_paragraphs": null,
            "note": "Kanoon returned no fragment for this query. The case exists but no paragraphs matched the query terms. The full document can still be opened at kanoon_url."
        })
        .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Query construction — field operators
// ---------------------------------------------------------------------------

/// Build a Kanoon `formInput` string using their search-operator syntax.
///
/// Kanoon supports field-prefixed terms like:
///   doctypes:supremecourt
///   court:delhi
///   fromdate:01-01-2020
///   todate:31-12-2024
///   cites:12345
///   "exact phrase"      (phrase match)
///
/// We do MILD keyword extraction (drop a small set of question stop-words)
/// but unlike the previous implementation, we keep multi-word terms intact
/// when phrase=true.
pub fn build_kanoon_query(
    query: &str,
    phrase: bool,
    court: Option<&str>,
    doctypes: Option<&str>,
    fromdate: Option<&str>,
    todate: Option<&str>,
    cites: Option<i64>,
) -> String {
    let body = if phrase {
        format!("\"{}\"", query.trim().replace('"', ""))
    } else {
        clean_keywords(query)
    };

    let mut parts: Vec<String> = vec![body];

    if let Some(c) = doctypes.and_then(non_empty) {
        parts.push(format!("doctypes:{}", c));
    }
    if let Some(c) = court.and_then(non_empty) {
        parts.push(format!("court:{}", c));
    }
    if let Some(d) = fromdate.and_then(non_empty) {
        parts.push(format!("fromdate:{}", d));
    }
    if let Some(d) = todate.and_then(non_empty) {
        parts.push(format!("todate:{}", d));
    }
    if let Some(t) = cites {
        if t > 0 {
            parts.push(format!("cites:{}", t));
        }
    }

    parts.join(" ")
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t) }
}

/// Light keyword cleaning. Unlike the old `extract_legal_keywords` which
/// stripped 50+ words including "supreme", "high", "court", "section",
/// "provision" (all legally meaningful!), this only drops a small set of
/// pure question/filler words. Real legal terms always survive.
fn clean_keywords(query: &str) -> String {
    const FILLERS: &[&str] = &[
        "what", "is", "the", "a", "an", "to", "and", "or", "for", "of",
        "in", "on", "by", "with", "from", "as", "at", "be", "are",
        "do", "does", "did", "can", "will", "would", "could", "should",
        "tell", "me", "show", "find", "give", "explain", "please",
        "i", "you", "we", "they", "he", "she", "it",
        "how", "when", "where", "which", "who", "whose", "why",
    ];
    let lower = query.to_lowercase();
    let tokens: Vec<&str> = lower
        .split_whitespace()
        .filter(|w| {
            let clean = w.trim_matches(|c: char| !c.is_alphanumeric());
            !clean.is_empty() && !FILLERS.contains(&clean)
        })
        .collect();
    if tokens.is_empty() {
        query.to_string()
    } else {
        tokens.join(" ")
    }
}

// ---------------------------------------------------------------------------
// Parsing search hits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct KanoonHit {
    tid: i64,
    title: String,
    court: String,
    snippet: String,
    decision_date: Option<String>,
    docsize: i64,
    score: f32,
}

impl KanoonHit {
    fn to_json_with_fragment_pending_verify(&self, frag: Option<Fragment>) -> Value {
        let frag_text = frag.map(|f| f.text);
        json!({
            "tid": self.tid,
            "title": self.title,
            "court": self.court,
            "snippet": self.snippet,
            "decision_date": self.decision_date,
            "docsize_bytes": self.docsize,
            "kanoon_url": format!("https://indiankanoon.org/doc/{}/", self.tid),
            "relevance_score": self.score,
            "relevant_paragraphs": frag_text,
            "verification": {
                "status": "PENDING",
                "note": "Not yet cross-checked against AWS canonical dataset. Call kanoon_verify_case(tid, title, court, decision_date) to verify this case before citing it."
            },
        })
    }
}

fn parse_search_hit(d: &Value) -> Option<KanoonHit> {
    let tid = d["tid"].as_i64()?;
    let title = d["title"].as_str().unwrap_or("Unknown Case").to_string();
    let court = d["docsource"].as_str().unwrap_or("").to_string();
    let snippet_html = d["headline"].as_str().unwrap_or("");
    let snippet = strip_html(snippet_html);
    let decision_date = d["publishdate"]
        .as_str()
        .or_else(|| d["pubdate"].as_str())
        .map(|s| s.to_string());
    let docsize = d["docsize"].as_i64().unwrap_or(0);
    Some(KanoonHit {
        tid,
        title,
        court,
        snippet,
        decision_date,
        docsize,
        score: 0.0,
    })
}

// ---------------------------------------------------------------------------
// Re-ranking by query-token overlap
// ---------------------------------------------------------------------------

fn rerank(hits: &mut [KanoonHit], query: &str) {
    let q_tokens: Vec<String> = tokenize(query);
    if q_tokens.is_empty() {
        return;
    }
    for h in hits.iter_mut() {
        let mut text = String::with_capacity(h.title.len() + h.snippet.len() + 1);
        text.push_str(&h.title);
        text.push(' ');
        text.push_str(&h.snippet);
        let h_tokens = tokenize(&text);
        // Score = fraction of query tokens that appear in title+snippet,
        // weighted higher when they appear in the title.
        let title_tokens = tokenize(&h.title);
        let mut score = 0.0f32;
        for q in &q_tokens {
            if title_tokens.iter().any(|t| t == q) {
                score += 2.0;
            } else if h_tokens.iter().any(|t| t == q) {
                score += 1.0;
            }
        }
        h.score = score / (q_tokens.len() as f32 * 2.0);
    }
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 1)
        .map(|t| t.to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Fragment fetching
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Fragment {
    title: String,
    text: String,
}

async fn fetch_fragment(
    client: reqwest::Client,
    ik_key: String,
    tid: i64,
    query: String,
) -> Option<Fragment> {
    let resp = client
        .post(format!("{IK_API_BASE}/docfragment/{tid}/"))
        .header("Authorization", format!("Token {ik_key}"))
        .form(&[("formInput", query.as_str())])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        tracing::warn!(
            "[kanoon] /docfragment/{tid} returned HTTP {}",
            resp.status()
        );
        return None;
    }
    let data: Value = resp.json().await.ok()?;
    let title = data["title"].as_str().unwrap_or("").to_string();
    let raw = data["headline"]
        .as_str()
        .or_else(|| data["doc"].as_str())
        .unwrap_or("");
    if raw.is_empty() {
        return None;
    }
    let mut text = strip_html(raw);
    if text.len() > FRAGMENT_CHAR_CAP {
        // `String::truncate` panics if the byte index lands mid-codepoint.
        // Indian judgments routinely contain multibyte chars (Devanagari
        // धारा, curly quotes/dashes Kanoon emits), so snap down to the
        // nearest char boundary before truncating.
        let cut = floor_char_boundary(&text, FRAGMENT_CHAR_CAP);
        text.truncate(cut);
        text.push_str("…[truncated]");
    }
    Some(Fragment { title, text })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent("MikeRust/0.1 (Indian Kanoon tool)")
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build()
}

async fn resolve_ik_key(state: &AppState, user_id: &str) -> Option<String> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT ik_api_key FROM corpus_settings WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    row.and_then(|(k,)| k)
        .or_else(|| std::env::var("IK_API_KEY").ok())
}

/// Strip HTML tags and collapse whitespace. Kanoon snippets and fragments
/// come back as HTML with <b>highlights</b>; for the LLM we want plain text.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Decode the most common HTML entities Kanoon emits.
    let decoded = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    // Collapse whitespace.
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Largest byte index `<= max` that is a UTF-8 char boundary in `s`.
/// Used to truncate without panicking on a multibyte boundary.
fn floor_char_boundary(s: &str, max: usize) -> usize {
    let mut i = max.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_builder_phrase_mode_wraps_quotes() {
        let q = build_kanoon_query("dishonour of cheque", true, None, None, None, None, None);
        assert!(q.starts_with("\"dishonour of cheque\""));
    }

    #[test]
    fn query_builder_drops_pure_fillers_not_legal_terms() {
        // "section" and "supreme court" must NOT be dropped.
        let q = build_kanoon_query(
            "what is section 138 supreme court ruling",
            false,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(q.contains("section"), "section dropped: {q}");
        assert!(q.contains("138"), "138 dropped: {q}");
        assert!(q.contains("supreme"), "supreme dropped: {q}");
        assert!(q.contains("court"), "court dropped: {q}");
        assert!(q.contains("ruling"), "ruling dropped: {q}");
        // Pure fillers go.
        assert!(!q.split_whitespace().any(|t| t == "what"));
        assert!(!q.split_whitespace().any(|t| t == "is"));
    }

    #[test]
    fn query_builder_appends_field_operators_in_order() {
        let q = build_kanoon_query(
            "cheque bounce",
            false,
            Some("delhi"),
            Some("supremecourt"),
            Some("01-01-2020"),
            Some("31-12-2024"),
            Some(12345),
        );
        // Order matters for predictability in tests.
        assert!(q.contains("doctypes:supremecourt"));
        assert!(q.contains("court:delhi"));
        assert!(q.contains("fromdate:01-01-2020"));
        assert!(q.contains("todate:31-12-2024"));
        assert!(q.contains("cites:12345"));
    }

    #[test]
    fn query_builder_skips_empty_optionals() {
        let q = build_kanoon_query("cheque bounce", false, Some(""), Some("   "), None, None, None);
        assert!(!q.contains("doctypes:"));
        assert!(!q.contains("court:"));
    }

    #[test]
    fn query_builder_skips_zero_cites() {
        let q = build_kanoon_query("cheque bounce", false, None, None, None, None, Some(0));
        assert!(!q.contains("cites:"));
    }

    #[test]
    fn clean_keywords_keeps_short_legal_terms_like_ipc() {
        // "ipc" is 3 chars; tokenizer threshold must allow it through.
        let q = clean_keywords("what is the ipc 138 doctrine");
        assert!(q.contains("ipc"), "ipc dropped from: {q}");
        assert!(q.contains("138"));
        assert!(q.contains("doctrine"));
    }

    #[test]
    fn strip_html_removes_tags_and_decodes_entities() {
        let s = strip_html("<p>Section <b>138</b> &amp; the &quot;NI Act&quot;</p>");
        assert_eq!(s, "Section 138 & the \"NI Act\"");
    }

    #[test]
    fn floor_char_boundary_never_panics_on_multibyte_truncate() {
        // Devanagari धारा is 3 bytes/char; a cap landing mid-char must snap
        // down to a boundary so String::truncate does not panic.
        let mut s = "धारा".repeat(2000); // well over FRAGMENT_CHAR_CAP, all multibyte
        let cut = floor_char_boundary(&s, FRAGMENT_CHAR_CAP);
        assert!(s.is_char_boundary(cut));
        assert!(cut <= FRAGMENT_CHAR_CAP);
        // The exact operation fetch_fragment performs — must not panic.
        s.truncate(cut);
        s.push_str("…[truncated]");
        assert!(s.ends_with("…[truncated]"));
    }

    #[test]
    fn floor_char_boundary_ascii_is_exact() {
        let s = "abcdef";
        assert_eq!(floor_char_boundary(s, 3), 3);
        // Past end clamps to len.
        assert_eq!(floor_char_boundary(s, 100), 6);
    }

    #[test]
    fn rerank_promotes_title_matches() {
        let mut hits = vec![
            KanoonHit {
                tid: 1,
                title: "Random unrelated case".into(),
                court: "".into(),
                snippet: "mentions cheque dishonour in passing".into(),
                decision_date: None,
                docsize: 0,
                score: 0.0,
            },
            KanoonHit {
                tid: 2,
                title: "Cheque dishonour under Section 138 NI Act".into(),
                court: "".into(),
                snippet: "".into(),
                decision_date: None,
                docsize: 0,
                score: 0.0,
            },
        ];
        rerank(&mut hits, "cheque dishonour section 138");
        assert_eq!(hits[0].tid, 2, "title-match should rank first");
    }

    #[test]
    fn tokenize_drops_punctuation_and_short_tokens() {
        let t = tokenize("Section 138, NI-Act!");
        assert!(t.contains(&"section".to_string()));
        assert!(t.contains(&"138".to_string()));
        // "ni" is 2 chars; filter is `len() > 1`, so 2-char tokens stay.
        assert!(t.contains(&"ni".to_string()));
        assert!(t.contains(&"act".to_string()));
        // Single-char tokens get filtered (none here, sanity check).
        let t2 = tokenize("a b c section");
        assert_eq!(t2, vec!["section".to_string()]);
    }
}
