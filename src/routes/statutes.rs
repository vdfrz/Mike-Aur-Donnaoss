//! Indian statute database routes.
//!
//! APIs for searching, browsing, and mapping Indian statutes stored in the
//! local SQLite database (migration 0037), plus self-serve ingestion: a
//! signed-in user can paste a link to any Act and Mike fetches it, reads the
//! sections with the LLM, and indexes them (the FTS index auto-syncs).
//!
//!   GET    /statutes/search                       — FTS5 full-text search
//!   GET    /statutes/acts                         — list all acts (+ section_count)
//!   GET    /statutes/acts/:short_name             — act + all sections
//!   DELETE /statutes/acts/:short_name             — remove an act + its sections
//!   GET    /statutes/section/:statute/:section    — single section + mappings
//!   GET    /statutes/map                          — old↔new section mapping
//!   POST   /statutes/ingest                       — fetch a URL, parse + index it (SSE)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::auth::middleware::AuthUser;
use crate::routes::user::{assert_url_is_external, fetch_llm_settings};
use crate::AppState;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/search", get(search_sections))
        .route("/acts", get(list_acts))
        .route("/acts/{short_name}", get(get_act).delete(delete_act))
        .route("/section/{statute}/{section}", get(get_section))
        .route("/map", get(map_section))
        .route("/ingest", post(ingest_statute))
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct SearchHit {
    statute: String,
    section_number: String,
    title: String,
    snippet: String,
}

#[derive(Serialize)]
struct Statute {
    id: i64,
    short_name: String,
    full_title: String,
    year: i64,
    status: String,
    replaced_by: Option<String>,
    category: Option<String>,
    language: Option<String>,
}

#[derive(Serialize)]
struct Section {
    id: i64,
    statute_id: i64,
    section_number: String,
    title: String,
    body: String,
}

#[derive(Serialize)]
struct Mapping {
    id: i64,
    old_statute: String,
    old_section: String,
    new_statute: String,
    new_section: String,
    mapping_type: Option<String>,
    notes: Option<String>,
}

#[derive(Serialize)]
struct ActDetail {
    #[serde(flatten)]
    act: Statute,
    sections: Vec<Section>,
}

#[derive(Serialize)]
struct SectionDetail {
    #[serde(flatten)]
    section: Section,
    statute_name: String,
    mappings: Vec<Mapping>,
}

#[derive(Serialize)]
struct MappingResult {
    #[serde(flatten)]
    mapping: Mapping,
    target_section: Option<Section>,
}

// ---------------------------------------------------------------------------
// GET /statutes/search
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    limit: Option<i64>,
}

async fn search_sections(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<Vec<SearchHit>>, StatusCode> {
    let q = params.q.trim();
    if q.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let limit = params.limit.unwrap_or(20).min(100);

    let rows = sqlx::query(
        "SELECT ss.id, s.short_name, ss.section_number, ss.title, \
                snippet(statute_sections_fts, 2, '<mark>', '</mark>', '...', 32) as snippet \
         FROM statute_sections_fts fts \
         JOIN statute_sections ss ON ss.id = fts.rowid \
         JOIN statutes s ON s.id = ss.statute_id \
         WHERE statute_sections_fts MATCH ?1 \
         ORDER BY rank \
         LIMIT ?2",
    )
    .bind(q)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let hits: Vec<SearchHit> = rows
        .iter()
        .map(|r| SearchHit {
            statute: r.get("short_name"),
            section_number: r.get("section_number"),
            title: r.get("title"),
            snippet: r.get("snippet"),
        })
        .collect();

    Ok(Json(hits))
}

// ---------------------------------------------------------------------------
// GET /statutes/acts
// ---------------------------------------------------------------------------

/// Listing row: the act plus how many sections are indexed for it. `year` is
/// nullable here because user-ingested acts may not carry a year.
#[derive(Serialize)]
struct ActSummary {
    id: i64,
    short_name: String,
    full_title: String,
    year: Option<i64>,
    status: String,
    replaced_by: Option<String>,
    category: Option<String>,
    language: Option<String>,
    section_count: i64,
}

async fn list_acts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<ActSummary>>, StatusCode> {
    let rows: Vec<(i64, String, String, Option<i64>, String, Option<String>, Option<String>, Option<String>, i64)> =
        sqlx::query_as(
            "SELECT s.id, s.short_name, s.full_title, s.year, s.status, s.replaced_by, s.category, s.language, \
                    (SELECT COUNT(*) FROM statute_sections ss WHERE ss.statute_id = s.id) AS section_count \
             FROM statutes s \
             ORDER BY s.year DESC, s.short_name",
        )
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let acts: Vec<ActSummary> = rows
        .into_iter()
        .map(|(id, short_name, full_title, year, status, replaced_by, category, language, section_count)| {
            ActSummary { id, short_name, full_title, year, status, replaced_by, category, language, section_count }
        })
        .collect();

    Ok(Json(acts))
}

// ---------------------------------------------------------------------------
// GET /statutes/acts/:short_name
// ---------------------------------------------------------------------------

async fn get_act(
    State(state): State<Arc<AppState>>,
    Path(short_name): Path<String>,
) -> Result<Json<ActDetail>, StatusCode> {
    let act_row: (i64, String, String, i64, String, Option<String>, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT id, short_name, full_title, year, status, replaced_by, category, language \
             FROM statutes \
             WHERE short_name = ?",
        )
        .bind(&short_name)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let act = Statute {
        id: act_row.0,
        short_name: act_row.1,
        full_title: act_row.2,
        year: act_row.3,
        status: act_row.4,
        replaced_by: act_row.5,
        category: act_row.6,
        language: act_row.7,
    };

    let section_rows: Vec<(i64, i64, String, String, String)> = sqlx::query_as(
        "SELECT id, statute_id, section_number, title, body \
         FROM statute_sections \
         WHERE statute_id = ? \
         ORDER BY CAST(section_number AS INTEGER), section_number",
    )
    .bind(act.id)
    .fetch_all(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let sections: Vec<Section> = section_rows
        .into_iter()
        .map(|(id, statute_id, section_number, title, body)| {
            Section { id, statute_id, section_number, title, body }
        })
        .collect();

    Ok(Json(ActDetail { act, sections }))
}

// ---------------------------------------------------------------------------
// GET /statutes/section/:statute/:section
// ---------------------------------------------------------------------------

async fn get_section(
    State(state): State<Arc<AppState>>,
    Path((statute, section)): Path<(String, String)>,
) -> Result<Json<SectionDetail>, StatusCode> {
    let row: (i64, i64, String, String, String) = sqlx::query_as(
        "SELECT ss.id, ss.statute_id, ss.section_number, ss.title, ss.body \
         FROM statute_sections ss \
         JOIN statutes s ON s.id = ss.statute_id \
         WHERE s.short_name = ? AND ss.section_number = ?",
    )
    .bind(&statute)
    .bind(&section)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::NOT_FOUND)?;

    let sec = Section {
        id: row.0,
        statute_id: row.1,
        section_number: row.2,
        title: row.3,
        body: row.4,
    };

    let mapping_rows: Vec<(i64, String, String, String, String, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, old_statute, old_section, new_statute, new_section, mapping_type, notes \
             FROM statute_mappings \
             WHERE (old_statute = ?1 AND old_section = ?2) \
                OR (new_statute = ?1 AND new_section = ?2)",
        )
        .bind(&statute)
        .bind(&section)
        .fetch_all(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mappings: Vec<Mapping> = mapping_rows
        .into_iter()
        .map(|(id, old_statute, old_section, new_statute, new_section, mapping_type, notes)| {
            Mapping { id, old_statute, old_section, new_statute, new_section, mapping_type, notes }
        })
        .collect();

    Ok(Json(SectionDetail {
        statute_name: statute,
        section: sec,
        mappings,
    }))
}

// ---------------------------------------------------------------------------
// GET /statutes/map
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MapParams {
    statute: String,
    section: String,
    direction: Option<String>,
}

async fn map_section(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MapParams>,
) -> Result<Json<Vec<MappingResult>>, StatusCode> {
    let old_to_new = params.direction.as_deref().unwrap_or("old_to_new") == "old_to_new";

    let (query, target_statute_col, target_section_col) = if old_to_new {
        (
            "SELECT id, old_statute, old_section, new_statute, new_section, mapping_type, notes \
             FROM statute_mappings \
             WHERE old_statute = ? AND old_section = ?",
            "new_statute",
            "new_section",
        )
    } else {
        (
            "SELECT id, old_statute, old_section, new_statute, new_section, mapping_type, notes \
             FROM statute_mappings \
             WHERE new_statute = ? AND new_section = ?",
            "old_statute",
            "old_section",
        )
    };

    let mapping_rows: Vec<(i64, String, String, String, String, Option<String>, Option<String>)> =
        sqlx::query_as(query)
            .bind(&params.statute)
            .bind(&params.section)
            .fetch_all(&state.db)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let mut results = Vec::with_capacity(mapping_rows.len());

    for (id, old_statute, old_section, new_statute, new_section, mapping_type, notes) in mapping_rows
    {
        let (target_stat, target_sec) = if old_to_new {
            (&new_statute, &new_section)
        } else {
            (&old_statute, &old_section)
        };

        let target_section: Option<(i64, i64, String, String, String)> = sqlx::query_as(
            "SELECT ss.id, ss.statute_id, ss.section_number, ss.title, ss.body \
             FROM statute_sections ss \
             JOIN statutes s ON s.id = ss.statute_id \
             WHERE s.short_name = ? AND ss.section_number = ?",
        )
        .bind(target_stat)
        .bind(target_sec)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let target = target_section.map(|(sid, statute_id, section_number, title, body)| {
            Section { id: sid, statute_id, section_number, title, body }
        });

        results.push(MappingResult {
            mapping: Mapping {
                id,
                old_statute,
                old_section,
                new_statute,
                new_section,
                mapping_type,
                notes,
            },
            target_section: target,
        });
    }

    Ok(Json(results))
}

// ---------------------------------------------------------------------------
// DELETE /statutes/acts/:short_name
//
// Remove an act and all of its sections. The FTS index is kept in sync by the
// AFTER DELETE trigger on statute_sections (migration 0037). Requires a signed
// in user; the statute DB is a shared, firm-wide resource (no per-user scope).
// ---------------------------------------------------------------------------

type IngestResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn ingest_err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "detail": msg })))
}

async fn delete_act(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(short_name): Path<String>,
) -> IngestResult {
    let id: Option<i64> = sqlx::query_scalar("SELECT id FROM statutes WHERE short_name = ?")
        .bind(&short_name)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| ingest_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let Some(id) = id else {
        return Err(ingest_err(StatusCode::NOT_FOUND, "statute not found"));
    };

    // Sections first (FTS delete-trigger fires), then the act row.
    sqlx::query("DELETE FROM statute_sections WHERE statute_id = ?")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| ingest_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    sqlx::query("DELETE FROM statutes WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| ingest_err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// POST /statutes/ingest
//
// Fetch a user-supplied URL, strip it to text, ask the LLM to read out the
// Act's metadata and its sections, and index them. Progress streams back over
// SSE so the UI can show the same elapsed-timer treatment as the other
// indexing pages. Mirrors the corpus /process SSE shape.
// ---------------------------------------------------------------------------

/// Cap on the page text we feed the LLM (~400 pages). Surfaced via the
/// `truncated` flag on the done event rather than silently dropped.
const MAX_TEXT_CHARS: usize = 1_200_000;
/// Window size per section-extraction call. Deliberately small so the JSON the
/// model emits for one chunk stays far under SECTION_MAX_TOKENS — that, plus the
/// split-retry below, is what stops output truncation from ever losing sections.
const CHUNK_CHARS: usize = 8_000;
/// Output-token cap per section-extraction call. With CHUNK_CHARS ~2k input
/// tokens, real output sits ~3-4k, well under this ceiling.
const SECTION_MAX_TOKENS: u32 = 8_192;
/// Floor for the split-retry: never subdivide a chunk below this.
const MIN_CHUNK_CHARS: usize = 1_500;
/// How many section-extraction calls run concurrently.
const CONCURRENCY: usize = 6;

#[derive(Deserialize)]
struct IngestBody {
    url: String,
}

async fn ingest_statute(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<IngestBody>,
) -> Response {
    let user_id = auth.user_id.clone();
    let url = body.url.trim().to_string();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        run_ingest(state, user_id, url, tx).await;
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn send_event(tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>, v: Value) {
    let _ = tx.send(Ok(Event::default().data(v.to_string()))).await;
}

async fn run_ingest(
    state: Arc<AppState>,
    user_id: String,
    url: String,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    macro_rules! fail {
        ($msg:expr) => {{
            send_event(&tx, json!({ "type": "error", "message": $msg })).await;
            let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
            return;
        }};
    }

    if url.is_empty() {
        fail!("Please paste a link.".to_string());
    }
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_default();

    send_event(&tx, json!({ "type": "stage", "stage": "fetching", "host": host })).await;

    // SSRF guard: reject loopback/private/link-local/metadata targets before
    // any request leaves the box. Reuses the same check as MCP probing.
    if let Err(e) = assert_url_is_external(&url).await {
        fail!(format!("That link can't be fetched: {e}"));
    }

    let client = match reqwest::Client::builder()
        .user_agent("MikeRust/0.1 (statute indexer)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
    {
        Ok(c) => c,
        Err(e) => fail!(format!("Could not start the fetch: {e}")),
    };

    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => fail!(format!("Could not reach that link: {e}")),
    };
    if !resp.status().is_success() {
        fail!(format!("That link returned HTTP {}.", resp.status()));
    }
    // Capture the content type before consuming the body, then read raw bytes
    // (statutes are very often served as PDFs, especially on India Code).
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = match resp.bytes().await {
        Ok(b) => b.to_vec(),
        Err(e) => fail!(format!("Could not download that link: {e}")),
    };

    send_event(&tx, json!({ "type": "stage", "stage": "reading", "host": host })).await;

    // Pick an extraction path. PDF is detected by magic bytes or content type
    // (a query string ending in .pdf is not reliable). Anything else falls back
    // to HTML/plain-text stripping.
    let url_lower = url.to_ascii_lowercase();
    let ext = if bytes.starts_with(b"%PDF") || content_type.contains("pdf") {
        "pdf"
    } else if content_type.contains("wordprocessingml") || url_lower.ends_with(".docx") {
        "docx"
    } else if content_type.contains("rtf") || url_lower.ends_with(".rtf") {
        "rtf"
    } else {
        ""
    };

    let mut text = if ext.is_empty() {
        strip_html(&String::from_utf8_lossy(&bytes))
    } else {
        // extract_text_dispatch keys off the file extension, so write the bytes
        // to a temp file with the right suffix, extract, then clean up.
        let tmp = std::env::temp_dir().join(format!("statute-{}.{ext}", uuid::Uuid::new_v4()));
        if let Err(e) = std::fs::write(&tmp, &bytes) {
            fail!(format!("Could not buffer the download: {e}"));
        }
        let extracted = crate::sync::scanner::extract_text_dispatch(&tmp, &bytes);
        let _ = std::fs::remove_file(&tmp);
        match extracted {
            Ok((t, _)) => t,
            Err(e) => fail!(format!("Could not read that file: {e}")),
        }
    };
    let truncated = text.chars().count() > MAX_TEXT_CHARS;
    if truncated {
        text = text.chars().take(MAX_TEXT_CHARS).collect();
        tracing::warn!("[statutes] ingest text truncated to {MAX_TEXT_CHARS} chars for {url}");
    }
    if text.trim().is_empty() {
        fail!("Couldn't read any text from that link.".to_string());
    }

    let settings = fetch_llm_settings(&state.db, &user_id).await.ok();
    let config = crate::llm::oneshot::config_from_settings(&settings);

    send_event(&tx, json!({ "type": "stage", "stage": "parsing", "host": host })).await;

    // 1) Identify the Act from the first window of text.
    let meta_input: String = text.chars().take(8_000).collect();
    let meta_system = "You are given the text of an Indian statute or Act. Return ONLY a compact \
JSON object (no markdown fences, no prose) with keys: short_name (a short uppercase abbreviation \
or slug, e.g. \"BNS\" or \"IT_ACT_2000\"), full_title (the official title), year (integer or null), \
category (a short lowercase tag like \"criminal\", \"civil\", \"tax\", or null).";
    let meta_raw = match crate::llm::oneshot::complete_with_max(&config, meta_system, &meta_input, 4096).await {
        Ok(s) => s,
        Err(e) => fail!(format!("Mike couldn't read the statute: {e}")),
    };
    let meta = parse_json_value(&meta_raw);
    let short_name = meta
        .as_ref()
        .and_then(|m| m.get("short_name"))
        .and_then(|v| v.as_str())
        .map(sanitize_short_name)
        .unwrap_or_default();
    if short_name.is_empty() {
        fail!("Mike couldn't tell which Act this is. Try a cleaner source link.".to_string());
    }
    let full_title = meta
        .as_ref()
        .and_then(|m| m.get("full_title"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| short_name.clone());
    let year = meta.as_ref().and_then(|m| m.get("year")).and_then(json_to_year);
    let category = meta
        .as_ref()
        .and_then(|m| m.get("category"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // Don't clobber an already-indexed act (including the curated seed data).
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM statutes WHERE short_name = ?")
        .bind(&short_name)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
    if existing.is_some() {
        fail!(format!(
            "\"{short_name}\" is already indexed. Delete it first if you want to re-index it."
        ));
    }

    // 2) Extract sections. We chunk the text only to bound each call's INPUT;
    //    each call's OUTPUT is kept well under the token cap by the small chunk
    //    size, and any chunk that still comes back truncated (or unparseable) is
    //    split in half and retried -- so no section is ever lost to a clipped
    //    reply, however large the Act is. Chunks run CONCURRENCY at a time.
    let chars: Vec<char> = text.chars().collect();
    let sec_system = "You are given an excerpt from an Indian statute or Act. Extract EVERY numbered \
section that appears in this excerpt. Return ONLY a JSON array (no markdown fences, no prose). Each \
element: {\"section_number\": string (e.g. \"420\" or \"498A\"), \"title\": string (may be empty), \
\"body\": string (the full text of that section as it appears)}. Only include sections whose text \
actually appears in this excerpt.";

    // Initial windows covering the whole (capped) text.
    let mut queue: Vec<(usize, usize)> = Vec::new();
    let mut cur = 0usize;
    while cur < chars.len() {
        let end = (cur + CHUNK_CHARS).min(chars.len());
        queue.push((cur, end));
        cur = end;
    }
    let initial_chunks = queue.len().max(1);

    let mut sections: Vec<(String, String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut done_chunks = 0usize;

    while !queue.is_empty() {
        let mut next_round: Vec<(usize, usize)> = Vec::new();
        for batch in queue.chunks(CONCURRENCY) {
            let mut handles = Vec::new();
            for &(rs, re) in batch {
                let chunk: String = chars[rs..re].iter().collect();
                let cfg = config.clone();
                handles.push(tokio::spawn(async move {
                    let raw = crate::llm::oneshot::complete_with_max(
                        &cfg, sec_system, &chunk, SECTION_MAX_TOKENS,
                    )
                    .await;
                    (rs, re, raw)
                }));
            }
            for h in handles {
                let (rs, re, raw) = match h.await {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let raw = match raw {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let parsed = parse_json_value(&raw);
                // Clipped output, or a reply we couldn't parse, on a chunk still
                // big enough to divide -> split and retry so nothing is dropped.
                let clipped = raw.contains("truncated at token limit") || parsed.is_none();
                if clipped && (re - rs) > MIN_CHUNK_CHARS {
                    let mid = rs + (re - rs) / 2;
                    next_round.push((rs, mid));
                    next_round.push((mid, re));
                    continue;
                }
                if let Some(Value::Array(arr)) = parsed {
                    for item in arr {
                        let num = item
                            .get("section_number")
                            .and_then(|v| v.as_str())
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();
                        if num.is_empty() {
                            continue;
                        }
                        let title =
                            item.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        let bod =
                            item.get("body").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        if bod.is_empty() {
                            continue;
                        }
                        if seen.insert(num.clone()) {
                            sections.push((num, title, bod));
                        }
                    }
                }
            }
            done_chunks += batch.len();
            send_event(
                &tx,
                json!({ "type": "stage", "stage": "parsing", "host": host, "chunk": done_chunks, "chunks": initial_chunks }),
            )
            .await;
        }
        queue = next_round;
    }

    if sections.is_empty() {
        fail!("Mike read the page but couldn't find any statute sections in it.".to_string());
    }

    let total = sections.len() as i64;
    send_event(
        &tx,
        json!({ "type": "parsed", "total": total, "short_name": short_name, "full_title": full_title }),
    )
    .await;

    // 3) Insert the act, then its sections (FTS index syncs via triggers).
    let statute_id: i64 = match sqlx::query_scalar(
        "INSERT INTO statutes (short_name, full_title, year, status, category, language) \
         VALUES (?, ?, ?, 'active', ?, 'en') RETURNING id",
    )
    .bind(&short_name)
    .bind(&full_title)
    .bind(year)
    .bind(&category)
    .fetch_one(&state.db)
    .await
    {
        Ok(id) => id,
        Err(e) => fail!(format!("Couldn't save the statute: {e}")),
    };

    let mut inserted = 0i64;
    for (num, title, bod) in &sections {
        let r = sqlx::query(
            "INSERT OR IGNORE INTO statute_sections (statute_id, section_number, title, body) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(statute_id)
        .bind(num)
        .bind(title)
        .bind(bod)
        .execute(&state.db)
        .await;
        if r.is_ok() {
            inserted += 1;
            if inserted % 10 == 0 || inserted == total {
                send_event(&tx, json!({ "type": "progress", "indexed": inserted, "total": total })).await;
            }
        }
    }

    send_event(
        &tx,
        json!({
            "type": "done",
            "short_name": short_name,
            "full_title": full_title,
            "sections": inserted,
            "truncated": truncated,
        }),
    )
    .await;
    let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
}

// ---------------------------------------------------------------------------
// Ingest helpers
// ---------------------------------------------------------------------------

/// Strip HTML tags to plain text and collapse blank lines. Same approach as the
/// Indian Kanoon route's `strip_html_tags`.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    result
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalise an LLM-supplied short name into a safe uppercase slug.
fn sanitize_short_name(s: &str) -> String {
    let mapped: String = s
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect();
    mapped
        .split('_')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .take(40)
        .collect()
}

/// Coerce a JSON year (number, numeric string, or null) into an optional i64.
fn json_to_year(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Best-effort extraction of a single JSON value from an LLM reply: strip code
/// fences, then fall back to the outermost `{...}` / `[...]` span.
fn parse_json_value(raw: &str) -> Option<Value> {
    let t = raw.trim();
    let t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    let start = t.find(|c: char| c == '{' || c == '[')?;
    let open = t.as_bytes()[start] as char;
    let close = if open == '{' { '}' } else { ']' };
    let end = t.rfind(close)?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&t[start..=end]).ok()
}
