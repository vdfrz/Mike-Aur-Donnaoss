//! Citation recorder + deterministic bibliography renderers.
//!
//! As the chat model calls `kanoon_search` / `kanoon_verify_case` /
//! `statute_search`, [`record_tool_citations`] upserts each judgment or statute
//! section into `case_citations` (status `'referred'`). When a case actually
//! ends up cited — its `indiankanoon.org/doc/<tid>` link appears in the
//! assistant's reply, or `kanoon_verify_case` confirms it —
//! [`mark_cited_from_text`] / the verify path raise its status to `'cited'`.
//!
//! [`render_cases_referred`] / [`render_authorities`] then produce the two
//! standard tables deterministically from the table, no LLM involved.
//!
//! The recorder/marker functions MUST NOT break the chat: they swallow all
//! errors internally (logging via `tracing::warn`) and return `()`.

use anyhow::Result;
use regex::Regex;
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Dedupe keys
// ---------------------------------------------------------------------------

/// Dedupe key for a judgment, keyed on its Indian Kanoon tid.
pub fn dedupe_key_judgment(tid: i64) -> String {
    format!("judgment:{tid}")
}

/// Dedupe key for a statute section, e.g. "statute:BNS:318".
pub fn dedupe_key_statute(statute: &str, section: &str) -> String {
    format!("statute:{}:{}", statute.trim(), section.trim())
}

// ---------------------------------------------------------------------------
// Recorder
// ---------------------------------------------------------------------------

/// Record citations implied by a single tool call. No-op for any tool other
/// than `kanoon_search` / `kanoon_verify_case` / `statute_search`. `result` is
/// the JSON string the tool returned. Never returns an error — failures are
/// logged and swallowed so chat can't break.
pub async fn record_tool_citations(
    db: &SqlitePool,
    case_id: &str,
    tool_name: &str,
    _args: &Value,
    result: &str,
) {
    let parsed: Value = match serde_json::from_str(result) {
        Ok(v) => v,
        Err(_) => return, // tool returned non-JSON (shouldn't happen); skip
    };

    let outcome = match tool_name {
        "kanoon_search" => record_kanoon_search(db, case_id, &parsed).await,
        "kanoon_verify_case" => record_kanoon_verify(db, case_id, &parsed).await,
        "statute_search" => record_statute_search(db, case_id, &parsed).await,
        _ => return,
    };
    if let Err(e) = outcome {
        tracing::warn!("[drafting::citations] record {tool_name} failed: {e}");
    }
}

/// `kanoon_search` -> upsert each hit as a referred judgment.
async fn record_kanoon_search(db: &SqlitePool, case_id: &str, parsed: &Value) -> Result<()> {
    let Some(results) = parsed.get("results").and_then(|v| v.as_array()) else {
        return Ok(());
    };
    for hit in results {
        let Some(tid) = hit.get("tid").and_then(|v| v.as_i64()) else {
            continue;
        };
        let upsert = JudgmentUpsert {
            tid,
            title: str_field(hit, "title"),
            court: str_field(hit, "court"),
            decision_date: str_field(hit, "decision_date"),
            kanoon_url: str_field(hit, "kanoon_url")
                .or_else(|| Some(format!("https://indiankanoon.org/doc/{tid}/"))),
            canonical_citation: None,
            status: "referred",
            source_tool: "kanoon_search",
        };
        upsert_judgment(db, case_id, &upsert).await?;
    }
    Ok(())
}

/// `kanoon_verify_case` -> upsert the verified case as cited, capturing the
/// canonical PDF URL / citation when the AWS lookup succeeded.
async fn record_kanoon_verify(db: &SqlitePool, case_id: &str, parsed: &Value) -> Result<()> {
    let Some(tid) = parsed.get("tid").and_then(|v| v.as_i64()) else {
        // No tid means we can't form a judgment dedupe key; nothing to record.
        return Ok(());
    };
    let verification = parsed.get("verification");
    let canonical_citation = verification
        .and_then(|v| v.get("canonical_citation"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    // Surface the canonical PDF URL as the citation's link when present; it's
    // the authoritative court record. Fall back to the Kanoon URL.
    let canonical_pdf = verification
        .and_then(|v| v.get("canonical_pdf_url"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let upsert = JudgmentUpsert {
        tid,
        title: str_field(parsed, "title"),
        court: str_field(parsed, "court"),
        decision_date: str_field(parsed, "decision_date"),
        kanoon_url: canonical_pdf
            .clone()
            .or_else(|| Some(format!("https://indiankanoon.org/doc/{tid}/"))),
        canonical_citation,
        status: "cited",
        source_tool: "kanoon_verify_case",
    };
    upsert_judgment(db, case_id, &upsert).await?;
    Ok(())
}

/// `statute_search` -> upsert each result section as a referred statute.
async fn record_statute_search(db: &SqlitePool, case_id: &str, parsed: &Value) -> Result<()> {
    let Some(results) = parsed.get("results").and_then(|v| v.as_array()) else {
        return Ok(());
    };
    for item in results {
        let statute = item.get("statute").and_then(|v| v.as_str()).unwrap_or("");
        let section = item
            .get("section_number")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if statute.is_empty() || section.is_empty() {
            continue;
        }
        upsert_statute(db, case_id, statute, section, "referred", "statute_search").await?;
    }
    Ok(())
}

/// Optional string field, empty -> None.
fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

struct JudgmentUpsert {
    tid: i64,
    title: Option<String>,
    court: Option<String>,
    decision_date: Option<String>,
    kanoon_url: Option<String>,
    canonical_citation: Option<String>,
    status: &'static str,
    source_tool: &'static str,
}

/// Upsert a judgment row. On conflict: bump `times_cited`, refresh
/// `last_cited_at`, fill in any newly-known fields, and raise status to
/// `'cited'` if the incoming record is cited (never downgrade a cited row back
/// to referred). On insert, `first_cited_at == last_cited_at`.
async fn upsert_judgment(db: &SqlitePool, case_id: &str, u: &JudgmentUpsert) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let id = uuid::Uuid::new_v4().to_string();
    let dedupe_key = dedupe_key_judgment(u.tid);

    sqlx::query(
        "INSERT INTO case_citations \
         (id, case_id, kind, dedupe_key, status, kanoon_tid, title, court, decision_date, \
          kanoon_url, canonical_citation, source_tool, times_cited, first_cited_at, last_cited_at) \
         VALUES (?, ?, 'judgment', ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?) \
         ON CONFLICT(case_id, dedupe_key) DO UPDATE SET \
            times_cited = times_cited + 1, \
            last_cited_at = excluded.last_cited_at, \
            status = CASE WHEN excluded.status = 'cited' THEN 'cited' ELSE case_citations.status END, \
            title = COALESCE(case_citations.title, excluded.title), \
            court = COALESCE(case_citations.court, excluded.court), \
            decision_date = COALESCE(case_citations.decision_date, excluded.decision_date), \
            kanoon_url = COALESCE(excluded.kanoon_url, case_citations.kanoon_url), \
            canonical_citation = COALESCE(excluded.canonical_citation, case_citations.canonical_citation)",
    )
    .bind(&id)
    .bind(case_id)
    .bind(&dedupe_key)
    .bind(u.status)
    .bind(u.tid)
    .bind(&u.title)
    .bind(&u.court)
    .bind(&u.decision_date)
    .bind(&u.kanoon_url)
    .bind(&u.canonical_citation)
    .bind(u.source_tool)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await?;
    Ok(())
}

/// Record a precedent the user resolved from a `precedent_finder` suggestion
/// (via `kanoon_search` in `resolve_precedents`) as a *referred* judgment, so
/// it shows up in "List of Cases Referred" even before the chat model actually
/// cites it. The `precedent_finder` finding itself carries no concrete cases —
/// only points of law and search queries — so this resolution step is the
/// earliest point a real Kanoon `tid` exists to record. Non-fatal: logs and
/// swallows errors so precedent resolution can never break.
pub async fn record_resolved_precedent(
    db: &SqlitePool,
    case_id: &str,
    tid: i64,
    title: Option<String>,
    court: Option<String>,
    decision_date: Option<String>,
    kanoon_url: Option<String>,
) {
    let upsert = JudgmentUpsert {
        tid,
        title,
        court,
        decision_date,
        kanoon_url: kanoon_url
            .filter(|s| !s.is_empty())
            .or_else(|| Some(format!("https://indiankanoon.org/doc/{tid}/"))),
        canonical_citation: None,
        status: "referred",
        source_tool: "precedent_finder",
    };
    if let Err(e) = upsert_judgment(db, case_id, &upsert).await {
        tracing::warn!("[drafting::citations] record resolved precedent {tid} failed: {e}");
    }
}

/// Upsert a statute section row. Same conflict semantics as judgments.
async fn upsert_statute(
    db: &SqlitePool,
    case_id: &str,
    statute: &str,
    section: &str,
    status: &str,
    source_tool: &str,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let id = uuid::Uuid::new_v4().to_string();
    let dedupe_key = dedupe_key_statute(statute, section);

    sqlx::query(
        "INSERT INTO case_citations \
         (id, case_id, kind, dedupe_key, status, statute, section_number, source_tool, \
          times_cited, first_cited_at, last_cited_at) \
         VALUES (?, ?, 'statute', ?, ?, ?, ?, ?, 1, ?, ?) \
         ON CONFLICT(case_id, dedupe_key) DO UPDATE SET \
            times_cited = times_cited + 1, \
            last_cited_at = excluded.last_cited_at, \
            status = CASE WHEN excluded.status = 'cited' THEN 'cited' ELSE case_citations.status END",
    )
    .bind(&id)
    .bind(case_id)
    .bind(&dedupe_key)
    .bind(status)
    .bind(statute)
    .bind(section)
    .bind(source_tool)
    .bind(&now)
    .bind(&now)
    .execute(db)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Promote referred -> cited from the assistant's final text
// ---------------------------------------------------------------------------

// indiankanoon.org/doc/<tid> — flips judgment rows actually linked in the reply.
static RE_KANOON_DOC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"indiankanoon\.org/doc/(\d+)").unwrap());

// "<Statute> s.<section>" — e.g. "BNS s.318", "IPC s. 420", "NI s.138(2)".
static RE_STATUTE_CITE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Z][A-Za-z]{1,15})\s+s\.?\s?(\d+[A-Za-z]?(?:\(\d+\))?)").unwrap()
});

/// Scan the assistant's final text and flip every cited judgment/statute from
/// `'referred'` to `'cited'`. Non-fatal throughout — logs and returns on error.
pub async fn mark_cited_from_text(db: &SqlitePool, case_id: &str, assistant_text: &str) {
    let now = chrono::Utc::now().to_rfc3339();

    // Judgments: collect tids linked in the text, then flip by dedupe_key.
    let mut tids: Vec<i64> = Vec::new();
    for caps in RE_KANOON_DOC.captures_iter(assistant_text) {
        if let Ok(tid) = caps[1].parse::<i64>() {
            if !tids.contains(&tid) {
                tids.push(tid);
            }
        }
    }
    for tid in tids {
        let dedupe_key = dedupe_key_judgment(tid);
        if let Err(e) = sqlx::query(
            "UPDATE case_citations SET status = 'cited', last_cited_at = ? \
             WHERE case_id = ? AND dedupe_key = ? AND status <> 'cited'",
        )
        .bind(&now)
        .bind(case_id)
        .bind(&dedupe_key)
        .execute(db)
        .await
        {
            tracing::warn!("[drafting::citations] mark_cited judgment {tid} failed: {e}");
        }
    }

    // Statutes: best-effort. The text's "<Statute> s.<section>" rarely matches
    // the stored short_name exactly (e.g. "BNS" vs "Bharatiya Nyaya Sanhita"),
    // so we match on section_number and a case-insensitive prefix of statute.
    let mut seen: Vec<(String, String)> = Vec::new();
    for caps in RE_STATUTE_CITE.captures_iter(assistant_text) {
        let statute = caps[1].to_string();
        let section = caps[2].to_string();
        let pair = (statute.clone(), section.clone());
        if seen.contains(&pair) {
            continue;
        }
        seen.push(pair);

        let like = format!("{statute}%");
        if let Err(e) = sqlx::query(
            "UPDATE case_citations SET status = 'cited', last_cited_at = ? \
             WHERE case_id = ? AND kind = 'statute' AND section_number = ? \
               AND statute LIKE ? COLLATE NOCASE AND status <> 'cited'",
        )
        .bind(&now)
        .bind(case_id)
        .bind(&section)
        .bind(&like)
        .execute(db)
        .await
        {
            tracing::warn!(
                "[drafting::citations] mark_cited statute {statute} s.{section} failed: {e}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Bibliography renderers
// ---------------------------------------------------------------------------

/// Render the "List of Cases Referred" — cited judgments, alphabetical by
/// title. A judgment with no canonical citation falls back to its court.
pub async fn render_cases_referred(db: &SqlitePool, case_id: &str) -> Result<String> {
    let rows: Vec<(Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT title, canonical_citation, court, kanoon_url FROM case_citations \
             WHERE case_id = ? AND kind = 'judgment' AND status = 'cited' \
             ORDER BY title COLLATE NOCASE",
        )
        .bind(case_id)
        .fetch_all(db)
        .await?;

    let mut out = String::from("# LIST OF CASES REFERRED\n\n");
    if rows.is_empty() {
        out.push_str("No cases cited yet.");
        return Ok(out);
    }
    for (i, (title, citation, court, url)) in rows.iter().enumerate() {
        let title = title.as_deref().unwrap_or("(untitled case)");
        let cite = citation
            .as_deref()
            .filter(|s| !s.is_empty())
            .or(court.as_deref())
            .unwrap_or("");
        let url = url.as_deref().unwrap_or("");
        out.push_str(&format!("{}. {title}, {cite} \u{2014} {url}\n", i + 1));
    }
    Ok(out)
}

/// Render the "List of Authorities" — statute sections grouped by statute,
/// each statute a `##` heading with one bullet per section. Includes both
/// referred and cited statutes.
pub async fn render_authorities(db: &SqlitePool, case_id: &str) -> Result<String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT statute, section_number FROM case_citations \
         WHERE case_id = ? AND kind = 'statute' AND statute IS NOT NULL AND section_number IS NOT NULL \
         ORDER BY statute COLLATE NOCASE, section_number",
    )
    .bind(case_id)
    .fetch_all(db)
    .await?;

    let mut out = String::from("# LIST OF AUTHORITIES\n\n");
    if rows.is_empty() {
        out.push_str("No authorities cited yet.");
        return Ok(out);
    }
    let mut current: Option<String> = None;
    for (statute, section) in &rows {
        if current.as_deref() != Some(statute.as_str()) {
            if current.is_some() {
                out.push('\n');
            }
            out.push_str(&format!("## {statute}\n"));
            current = Some(statute.clone());
        }
        out.push_str(&format!("- Section {section}\n"));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedupe_keys() {
        assert_eq!(dedupe_key_judgment(12345), "judgment:12345");
        assert_eq!(dedupe_key_statute("BNS", "318"), "statute:BNS:318");
        // Trims surrounding whitespace so keys are stable.
        assert_eq!(dedupe_key_statute(" BNS ", " 318 "), "statute:BNS:318");
    }

    #[test]
    fn kanoon_doc_regex_extracts_tid() {
        let text = "See [Foo v Bar](https://indiankanoon.org/doc/98765/) at para 4.";
        let caps = RE_KANOON_DOC.captures(text).unwrap();
        assert_eq!(&caps[1], "98765");
    }

    #[test]
    fn statute_cite_regex_matches_section_forms() {
        let text = "punishable under BNS s.318 and IPC s. 420 and NI s.138(2)";
        let found: Vec<(String, String)> = RE_STATUTE_CITE
            .captures_iter(text)
            .map(|c| (c[1].to_string(), c[2].to_string()))
            .collect();
        assert!(found.contains(&("BNS".to_string(), "318".to_string())));
        assert!(found.contains(&("IPC".to_string(), "420".to_string())));
        assert!(found.contains(&("NI".to_string(), "138(2)".to_string())));
    }
}
