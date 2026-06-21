//! `statute_search` builtin tool.
//!
//! Full-text search over the local curated Indian statute database
//! (migration 0037: BNS / IPC / CrPC / BNSS / Evidence Act, …). Returns
//! matching sections ordered by relevance, each annotated with its current
//! equivalent when the section has been repealed and remapped (e.g.
//! IPC s.420 → BNS s.318). This is the statute counterpart to
//! `kanoon_search` (case law): the model uses it to ground answers about
//! what a section says and which provision replaced an older one.

use crate::AppState;
use serde_json::{json, Value};

/// Strip FTS5 operator characters so an LLM-supplied query can't produce a
/// MATCH syntax error. Keeps alphanumerics and spaces; everything else
/// becomes a space. The remaining bare terms are combined with FTS5's
/// implicit AND.
fn sanitize_fts_query(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Pull section-number-like tokens out of the raw query (e.g. "420",
/// "498A", "318(4)") so we can resolve old↔new mappings even when the
/// query is something like "what replaced IPC 420". A token qualifies if
/// it starts with a digit.
fn extract_section_tokens(raw: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.split_whitespace()
        .map(|t| t.trim_matches(|c: char| !c.is_alphanumeric() && c != '(' && c != ')'))
        .filter(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .filter(|t| seen.insert(t.to_string()))
        .map(|t| t.to_string())
        .collect()
}

pub async fn exec_statute_search(state: &AppState, arguments: &Value) -> String {
    let raw_query = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if raw_query.is_empty() {
        return json!({"error": "query is required"}).to_string();
    }
    let query = sanitize_fts_query(raw_query);
    if query.is_empty() {
        return json!({
            "results": [],
            "count": 0,
            "note": "Query had no searchable terms after sanitization."
        })
        .to_string();
    }
    let limit = arguments
        .get("max_results")
        .and_then(|v| v.as_i64())
        .unwrap_or(8)
        .clamp(1, 15);

    let rows: Result<Vec<(i64, String, Option<String>, String, String)>, _> = sqlx::query_as(
        "SELECT ss.id, s.short_name, ss.title, ss.section_number, ss.body \
         FROM statute_sections_fts \
         JOIN statute_sections ss ON ss.id = statute_sections_fts.rowid \
         JOIN statutes s ON s.id = ss.statute_id \
         WHERE statute_sections_fts MATCH ?1 \
         ORDER BY bm25(statute_sections_fts, 1.0, 10.0, 1.0) \
         LIMIT ?2",
    )
    .bind(&query)
    .bind(limit)
    .fetch_all(&state.db)
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => return json!({"error": format!("Statute search failed: {e}")}).to_string(),
    };

    let mut results: Vec<Value> = Vec::with_capacity(rows.len());
    for (_id, short_name, title, section_number, body) in &rows {
        // Full section text, capped so a very long procedural section can't
        // blow the model's context; note when truncated.
        let (text, truncated) = if body.chars().count() > 6000 {
            (body.chars().take(6000).collect::<String>(), true)
        } else {
            (body.clone(), false)
        };
        let mapping: Option<(String, String)> = sqlx::query_as(
            "SELECT new_statute, new_section FROM statute_mappings \
             WHERE old_statute = ?1 AND old_section = ?2 LIMIT 1",
        )
        .bind(short_name)
        .bind(section_number)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        let display_title = title
            .as_deref()
            .map(|t| format!("Section {section_number} \u{2014} {t}"))
            .unwrap_or_else(|| format!("Section {section_number}"));

        let mut item = json!({
            "id": format!("{short_name}/{section_number}"),
            "statute": short_name,
            "section_number": section_number,
            "title": display_title,
            "text": text,
            "truncated": truncated,
        });
        if let Some((new_statute, new_section)) = mapping {
            item["mapped_to"] = json!({"statute": new_statute, "section": new_section});
        }
        results.push(item);
    }

    // Resolve old↔new mappings for any section numbers in the query. This
    // works even with no section bodies loaded, so it covers "what replaced
    // IPC 420 / what's the old equivalent of BNS 318" questions directly.
    let mut mappings: Vec<Value> = Vec::new();
    let mut seen_maps = std::collections::HashSet::new();
    for token in extract_section_tokens(raw_query) {
        let like = format!("{token}(%");
        let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
            "SELECT old_statute, old_section, new_statute, new_section, mapping_type \
             FROM statute_mappings \
             WHERE old_section = ?1 OR new_section = ?1 \
                OR old_section LIKE ?2 OR new_section LIKE ?2 \
             LIMIT 5",
        )
        .bind(&token)
        .bind(&like)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
        for (os, osec, ns, nsec, mtype) in rows {
            let key = format!("{os}{osec}->{ns}{nsec}");
            if seen_maps.insert(key) {
                // Pull the in-force successor section's verbatim text so the
                // model is grounded on current law even when the OLD code's
                // body was never loaded (e.g. a query for "IPC 420" resolves
                // to BNS 318(4) and carries the BNS text to draft from).
                // Sections are stored at the BASE number (e.g. "318"), with
                // sub-clauses inside the body, while a mapping's new_section
                // can be sub-clause-specific ("318(4)") — try the exact number
                // first, then fall back to the base number before the "(".
                let base_nsec: String =
                    nsec.split('(').next().unwrap_or(nsec.as_str()).to_string();
                let current_text: Option<String> = sqlx::query_scalar(
                    "SELECT ss.body FROM statute_sections ss \
                     JOIN statutes s ON s.id = ss.statute_id \
                     WHERE s.short_name = ?1 AND (ss.section_number = ?2 OR ss.section_number = ?3) \
                     ORDER BY CASE WHEN ss.section_number = ?2 THEN 0 ELSE 1 END LIMIT 1",
                )
                .bind(&ns)
                .bind(&nsec)
                .bind(&base_nsec)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .filter(|t: &String| !t.trim().is_empty());
                let mut m = json!({
                    "old": if osec.is_empty() { Value::Null } else { json!(format!("{os} s.{osec}")) },
                    "new": format!("{ns} s.{nsec}"),
                    "mapping_type": mtype,
                });
                if let Some(body) = current_text {
                    let capped: String = if body.chars().count() > 6000 {
                        body.chars().take(6000).collect()
                    } else {
                        body
                    };
                    m["current_text"] = json!(capped);
                }
                mappings.push(m);
            }
        }
    }

    if results.is_empty() && mappings.is_empty() {
        return json!({
            "results": [],
            "mappings": [],
            "count": 0,
            "note": "Nothing in the local statute database matched. It currently holds the 2023 codes (BNS, BNSS, BSA) in full; for other statutes or case law, use kanoon_search. Do not invent section text."
        })
        .to_string();
    }

    json!({
        "results": results,
        "mappings": mappings,
        "count": results.len() + mappings.len(),
        "instructions_for_model": "`results[].text` (and a mapping's `current_text`) is the verbatim bare-act text of the section (BNS/BNSS/BSA fully loaded; if `truncated` is true the section was long and cut at 6000 chars). USE THIS TEXT TO GROUND YOURSELF — get the section NUMBER and the legal STANDARD right — but DO NOT block-quote it by default. When ANSWERING a question about what a section says, quote freely. When DRAFTING a document, cite the section inline (e.g. 'Section 318(4) BNS') and state the legal standard in your OWN prose; reproduce the bare-act words verbatim ONLY where the exact wording is itself at issue (a legal notice's formal demand, a ground that turns on the language, a contested element) — never paste '...which states \"...\"' after every section. Never invent section text. `mappings` are authoritative old↔new section correspondences (e.g. IPC s.420 → BNS s.318(4)); when a mapping carries `current_text`, that is the in-force successor's verbatim text — ground post-1-July-2024 drafting on it. Cite sections as '<statute> s.<section>'."
    })
    .to_string()
}
