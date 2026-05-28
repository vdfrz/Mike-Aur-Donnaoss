//! Indian Kanoon corpus routes.
//!
//! Mirrors the EUR-Lex citation-jump pattern: cases are fetched from the
//! Indian Kanoon API, cached locally as plain-text files, embedded into
//! the local RAG index, and cited with clickable links that jump to the
//! exact cached document. Files never leave the lawyer's machine.
//!
//!   GET    /indian-kanoon/config              — user's IK API key / enable flag
//!   PUT    /indian-kanoon/config              — save IK API key
//!   POST   /indian-kanoon/search              — natural language → search → synthesize
//!   GET    /indian-kanoon/doc/:tid            — fetch full doc, cache & embed locally
//!   GET    /indian-kanoon/docfragment/:tid    — exact paragraphs matching a query
//!   GET    /indian-kanoon/documents           — list locally cached cases
//!   DELETE /indian-kanoon/documents/:id       — drop a cached case
//!   POST   /indian-kanoon/documents/:id/resync — re-embed an interrupted case

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, storage::make_storage, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

fn storage_root() -> PathBuf {
    PathBuf::from(
        std::env::var("STORAGE_PATH").unwrap_or_else(|_| "./data/storage".to_string()),
    )
}

const CORPUS_ID: &str = "indian-kanoon";
const IK_API_BASE: &str = "https://api.indiankanoon.org";

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/config", get(get_config).put(put_config))
        .route("/search", post(search))
        .route("/meta/{tid}", get(fetch_meta))
        .route("/doc/{tid}", get(fetch_doc))
        .route("/doc-html/{tid}", get(fetch_doc_html))
        .route("/docfragment/{tid}", get(doc_fragment))
        .route("/documents", get(list_documents))
        .route("/documents/{id}", delete(delete_document))
        .route("/documents/{id}/resync", post(resync_document))
}

// ---------------------------------------------------------------------------
// GET /indian-kanoon/config
// ---------------------------------------------------------------------------

async fn get_config(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> ApiResult {
    let row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT enabled, ik_api_key FROM corpus_settings \
         WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Fall back to the global env var key if the user hasn't set one.
    let global_key = std::env::var("IK_API_KEY").ok();
    let (enabled, key) = row
        .map(|(e, k)| (e != 0, k))
        .unwrap_or((global_key.is_some(), None));
    let has_key = key.is_some() || global_key.is_some();

    Ok(Json(json!({
        "enabled": enabled,
        "has_key": has_key,
        // Never expose the raw key to the frontend
    })))
}

// ---------------------------------------------------------------------------
// PUT /indian-kanoon/config
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ConfigPayload {
    enabled: bool,
    ik_api_key: Option<String>,
}

async fn put_config(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<ConfigPayload>,
) -> ApiResult {
    sqlx::query(
        "INSERT INTO corpus_settings (user_id, corpus_id, enabled, ik_api_key, updated_at) \
         VALUES (?, ?, ?, ?, datetime('now')) \
         ON CONFLICT(user_id, corpus_id) DO UPDATE SET \
           enabled = excluded.enabled, \
           ik_api_key = COALESCE(excluded.ik_api_key, ik_api_key), \
           updated_at = excluded.updated_at",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .bind(body.enabled as i64)
    .bind(&body.ik_api_key)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "enabled": body.enabled })))
}

// ---------------------------------------------------------------------------
// POST /indian-kanoon/search — natural language → IK API → synthesized answer
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SearchPayload {
    query: String,
    model: Option<String>,
}

async fn search(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<SearchPayload>,
) -> ApiResult {
    let query = body.query.trim();
    if query.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Query cannot be empty."));
    }

    // Resolve the IK API key: user-specific → env fallback
    let ik_key: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT ik_api_key FROM corpus_settings WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .and_then(|(k,)| k)
    .or_else(|| std::env::var("IK_API_KEY").ok());

    let ik_key = ik_key.ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No Indian Kanoon API key configured. Add IK_API_KEY to your .env or set it in Settings.",
        )
    })?;

    // Step 1: Extract legal keywords from the natural-language query.
    // Indian Kanoon's search API works best with simple space-separated keywords
    // rather than full natural-language questions. We strip common question words
    // and legal boilerplate, keeping only the substantive legal terms.
    let search_keywords = extract_legal_keywords(query);

    tracing::info!(
        "[indian-kanoon] search user={} query={:?} keywords={:?}",
        auth.user_id,
        query,
        search_keywords
    );

    // Step 2: Hit the Indian Kanoon search API.
    let client = reqwest::Client::builder()
        .user_agent("MikeRust/0.1 (Indian Kanoon integration)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let ik_response = client
        .post(format!("{IK_API_BASE}/search/"))
        .header("Authorization", format!("Token {ik_key}"))
        .form(&[("formInput", search_keywords.as_str()), ("pagenum", "0")])
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Indian Kanoon API error: {e}")))?;

    if !ik_response.status().is_success() {
        let status = ik_response.status();
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!("Indian Kanoon returned HTTP {status}"),
        ));
    }

    let ik_data: Value = ik_response
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to parse IK response: {e}")))?;

    let docs = ik_data["docs"].as_array().cloned().unwrap_or_default();

    if docs.is_empty() {
        return Ok(Json(json!({
            "answer": "I could not find any relevant cases on Indian Kanoon for your query. \
                       Try using more specific legal terms (e.g. 'dishonour cheque NI Act 138').",
            "cases": [],
            "search_keywords": search_keywords,
        })));
    }

    // Return the top 5 cases with citation-jump metadata.
    // Each case includes a `docfragment_url` that the frontend can call
    // to get exact paragraph-level matches for the user's query.
    let top_cases: Vec<Value> = docs
        .iter()
        .take(5)
        .map(|doc| {
            let tid = doc["tid"].as_i64().unwrap_or(0);
            let docsize = doc["docsize"].as_i64().unwrap_or(0);
            json!({
                "tid": tid,
                "title": doc["title"].as_str().unwrap_or("Unknown Case"),
                "court": doc["docsource"].as_str().unwrap_or(""),
                "snippet": doc["headline"].as_str().unwrap_or(""),
                "docsize": docsize,
                "source_url": format!("https://indiankanoon.org/doc/{tid}/"),
                "docfragment_url": format!("/indian-kanoon/docfragment/{tid}?formInput={}",
                    urlencoding(&search_keywords)),
            })
        })
        .collect();

    // Synthesized answer — formatted as Markdown with citations.
    let answer = format_answer(query, &top_cases);

    Ok(Json(json!({
        "answer": answer,
        "cases": top_cases,
        "search_keywords": search_keywords,
    })))
}

/// Extract 3-5 core legal keywords from a natural-language query.
/// Strips common question words and legal boilerplate, keeping only
/// substantive terms that Indian Kanoon's search API can use effectively.
fn extract_legal_keywords(query: &str) -> String {
    let lower = query.to_lowercase();

    // Common question/boilerplate words to strip
    let stop_words = [
        "what", "is", "the", "a", "an", "in", "of", "for", "on", "to",
        "and", "or", "by", "with", "from", "as", "at", "it", "be", "has",
        "have", "been", "was", "were", "are", "does", "do", "did", "can",
        "will", "shall", "may", "would", "could", "should", "about",
        "tell", "me", "show", "find", "give", "explain", "how", "when",
        "where", "which", "who", "whom", "whose", "why", "please",
        "according", "section", "under", "provision", "provisions",
        "related", "relevant", "case", "cases", "law", "laws", "legal",
        "judgment", "judgements", "judgement", "court", "supreme",
        "high", "india", "indian", "there", "any", "this", "that",
        "these", "those", "i", "you", "we", "they", "he", "she",
    ];

    let words: Vec<&str> = lower
        .split_whitespace()
        .filter(|w| {
            let clean = w.trim_matches(|c: char| !c.is_alphanumeric());
            !stop_words.contains(&clean) && clean.len() > 1
        })
        .collect();

    if words.is_empty() {
        return query.to_string();
    }

    // Take up to 5 keywords
    let keywords: Vec<&str> = words.into_iter().take(5).collect();
    keywords.join(" ")
}

/// Simple URL-encoding for query parameters (avoids pulling in a full crate).
fn urlencoding(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            ' ' => "%20".to_string(),
            other => format!("%{:02X}", other as u8),
        })
        .collect()
}

/// Format a readable cited answer from search results.
/// Used both by the web frontend and the Donna WhatsApp bot.
fn format_answer(query: &str, cases: &[Value]) -> String {
    if cases.is_empty() {
        return r#"<p style="color:#888;">No relevant cases found on Indian Kanoon.</p>"#.to_string();
    }

    let mut answer = format!(
        r#"<div style="font-family:-apple-system,BlinkMacSystemFont,sans-serif;">
<p style="color:#666;font-size:13px;margin-bottom:12px;">Query: <em>{q}</em></p>
<p style="color:#333;font-size:14px;font-weight:600;margin-bottom:10px;">Results from Indian Kanoon ({n} case(s)):</p>
<ol style="padding-left:18px;margin:0;">"#,
        q = html_escape(query),
        n = cases.len()
    );

    for case in cases {
        let title = case["title"].as_str().unwrap_or("Unknown Case");
        let court = case["court"].as_str().unwrap_or("");
        let snippet = case["snippet"].as_str().unwrap_or("");
        let docfragment_url = case["docfragment_url"].as_str().unwrap_or("");
        let source_url = case["source_url"].as_str().unwrap_or("");

        answer.push_str(&format!(
            r#"<li style="margin-bottom:14px;">
<span style="font-weight:600;font-size:13px;">{title}</span><br>
<span style="color:#c2410c;font-size:11px;">{court}</span><br>
<span style="color:#555;font-size:12px;">{snippet}</span><br>
<a href="{frag}" style="color:#2563eb;font-size:12px;text-decoration:underline;" target="_blank">View exact matching paragraphs</a>
<span style="color:#999;font-size:11px;"> | </span>
<a href="{src}" style="color:#888;font-size:11px;" target="_blank">Full document on IK</a>
</li>"#,
            title = html_escape(title),
            court = html_escape(court),
            snippet = html_escape(snippet),
            frag = html_escape(docfragment_url),
            src = html_escape(source_url),
        ));
    }

    answer.push_str("</ol>");
    answer.push_str(
        r#"<p style="color:#888;font-size:11px;margin-top:10px;font-style:italic;">Sources: Indian Kanoon API. Click blue links to see exact paragraphs.</p>"#
    );
    answer.push_str("</div>");
    answer
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ---------------------------------------------------------------------------
// GET /indian-kanoon/doc/:tid — fetch full doc from IK API, cache & embed
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DocFragmentQuery {
    #[serde(default)]
    #[allow(dead_code)]
    formInput: Option<String>,
}

// ---------------------------------------------------------------------------
// GET /indian-kanoon/meta/:tid — lightweight court metadata (no doc ingestion)
// ---------------------------------------------------------------------------

async fn fetch_meta(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(tid): Path<i64>,
) -> ApiResult {
    // Resolve the IK API key.
    let ik_key: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT ik_api_key FROM corpus_settings WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .and_then(|(k,)| k)
    .or_else(|| std::env::var("IK_API_KEY").ok());

    let ik_key = ik_key.ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No Indian Kanoon API key configured.",
        )
    })?;

    let client = reqwest::Client::builder()
        .user_agent("MikeRust/0.1 (Indian Kanoon integration)")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let resp = client
        .post(format!("{IK_API_BASE}/doc/{tid}/"))
        .header("Authorization", format!("Token {ik_key}"))
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("IK API error: {e}")))?;

    if !resp.status().is_success() {
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!("Indian Kanoon returned HTTP {}", resp.status()),
        ));
    }

    let data: Value = resp
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to parse IK response: {e}")))?;

    Ok(Json(json!({
        "tid": tid,
        "title": data["title"].as_str().unwrap_or(""),
        "docsource": data["docsource"].as_str().unwrap_or(""),
        "publishdate": data["publishdate"].as_str().unwrap_or(""),
    })))
}

async fn fetch_doc(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(tid): Path<i64>,
) -> ApiResult {
    // Resolve the IK API key.
    let ik_key: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT ik_api_key FROM corpus_settings WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .and_then(|(k,)| k)
    .or_else(|| std::env::var("IK_API_KEY").ok());

    let ik_key = ik_key.ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No Indian Kanoon API key configured.",
        )
    })?;

    // Check if already cached.
    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT id, status FROM documents \
         WHERE user_id = ? AND corpus_id = ? AND corpus_identifier = ?",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .bind(&tid.to_string())
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if let Some((id, status)) = existing {
        return Ok(Json(json!({
            "id": id,
            "tid": tid,
            "status": status,
            "already_cached": true,
            "source_url": format!("https://indiankanoon.org/doc/{tid}/"),
        })));
    }

    // Fetch the full document from IK API.
    let client = reqwest::Client::builder()
        .user_agent("MikeRust/0.1 (Indian Kanoon integration)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let doc_response = client
        .post(format!("{IK_API_BASE}/doc/{tid}/"))
        .header("Authorization", format!("Token {ik_key}"))
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("IK API error: {e}")))?;

    if !doc_response.status().is_success() {
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!("Indian Kanoon returned HTTP {}", doc_response.status()),
        ));
    }

    let doc_data: Value = doc_response
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to parse IK doc: {e}")))?;

    let title = doc_data["title"].as_str().unwrap_or("Untitled Case").to_string();
    let doc_html = doc_data["doc"].as_str().unwrap_or("");

    if doc_html.is_empty() {
        return Err(err(StatusCode::BAD_GATEWAY, "Empty document body from Indian Kanoon."));
    }

    // Strip HTML tags to get plain text for embedding.
    let plain_text = strip_html_tags(doc_html);

    // Hash + store.
    let hash = {
        let mut h = Sha256::new();
        h.update(plain_text.as_bytes());
        format!("{:x}", h.finalize())
    };
    let bin_key = format!("cache/{}.txt", hash);

    let storage = make_storage()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let bin_abs = storage_root().join(bin_key.replace('/', std::path::MAIN_SEPARATOR_STR));
    if !bin_abs.exists() {
        storage
            .put(&bin_key, plain_text.as_bytes(), "text/plain; charset=utf-8")
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    let doc_id = uuid::Uuid::new_v4().to_string();
    let filename = format!("{}.txt", sanitize_filename(&title));
    let size = plain_text.len() as i64;

    // Insert with status='syncing'.
    sqlx::query(
        "INSERT INTO documents (\
            id, user_id, corpus_id, corpus_identifier, filename, \
            storage_path, extracted_text_path, content_hash, size_bytes, \
            status, created_at, updated_at\
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'syncing', datetime('now'), datetime('now'))",
    )
    .bind(&doc_id)
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .bind(&tid.to_string())
    .bind(&filename)
    .bind(&bin_key)
    .bind(&bin_key)
    .bind(&hash)
    .bind(size)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Run embedding/indexing.
    let source_path = bin_abs.to_string_lossy().to_string();

    #[cfg(feature = "rag")]
    let (chunks_indexed, final_status) = if let Some(emb) = state.embeddings.clone() {
        match emb.index_document(&auth.user_id, None, &doc_id, &source_path, &plain_text).await {
            Ok(n) => (n, "ready".to_string()),
            Err(e) => {
                tracing::warn!("[indian-kanoon] embed for {} failed: {}", doc_id, e);
                (0usize, "interrupted".to_string())
            }
        }
    } else {
        (0usize, "ready".to_string())
    };

    #[cfg(not(feature = "rag"))]
    let (chunks_indexed, final_status) = (0usize, "ready".to_string());

    sqlx::query("UPDATE documents SET status = ? WHERE id = ?")
        .bind(&final_status)
        .bind(&doc_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "id": doc_id,
        "tid": tid,
        "filename": filename,
        "title": title,
        "size_bytes": size,
        "chunks_indexed": chunks_indexed,
        "status": final_status,
        "source_url": format!("https://indiankanoon.org/doc/{tid}/"),
    })))
}

// ---------------------------------------------------------------------------
// GET /indian-kanoon/docfragment/:tid — exact paragraphs matching a query
// ---------------------------------------------------------------------------

async fn doc_fragment(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(tid): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    let form_input = params.get("formInput").cloned().unwrap_or_default();
    if form_input.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Missing formInput query parameter."));
    }

    // Resolve the IK API key.
    let ik_key: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT ik_api_key FROM corpus_settings WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .and_then(|(k,)| k)
    .or_else(|| std::env::var("IK_API_KEY").ok());

    let ik_key = ik_key.ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No Indian Kanoon API key configured.",
        )
    })?;

    let client = reqwest::Client::builder()
        .user_agent("MikeRust/0.1 (Indian Kanoon integration)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let frag_response = client
        .post(format!("{IK_API_BASE}/docfragment/{tid}/"))
        .header("Authorization", format!("Token {ik_key}"))
        .form(&[("formInput", form_input.as_str())])
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("IK API error: {e}")))?;

    if !frag_response.status().is_success() {
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!("Indian Kanoon returned HTTP {}", frag_response.status()),
        ));
    }

    let frag_data: Value = frag_response
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to parse IK fragment: {e}")))?;

    let title = frag_data["title"].as_str().unwrap_or("Unknown Case").to_string();
    let headline = frag_data["headline"].as_str().unwrap_or("").to_string();

    Ok(Json(json!({
        "tid": tid,
        "title": title,
        "formInput": form_input,
        "headline": headline,
        "source_url": format!("https://indiankanoon.org/doc/{tid}/"),
    })))
}

// ---------------------------------------------------------------------------
// GET /indian-kanoon/doc-html/:tid — proxy-fetch the IK doc and return HTML
// for rendering in the side panel (avoids X-Frame-Options blocking).
// ---------------------------------------------------------------------------

async fn fetch_doc_html(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(tid): Path<i64>,
) -> ApiResult {
    let ik_key: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT ik_api_key FROM corpus_settings WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .and_then(|(k,)| k)
    .or_else(|| std::env::var("IK_API_KEY").ok());

    let ik_key = ik_key.ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No Indian Kanoon API key configured.",
        )
    })?;

    let client = reqwest::Client::builder()
        .user_agent("MikeRust/0.1 (Indian Kanoon integration)")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let doc_response = client
        .post(format!("{IK_API_BASE}/doc/{tid}/"))
        .header("Authorization", format!("Token {ik_key}"))
        .send()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("IK API error: {e}")))?;

    if !doc_response.status().is_success() {
        return Err(err(
            StatusCode::BAD_GATEWAY,
            &format!("Indian Kanoon returned HTTP {}", doc_response.status()),
        ));
    }

    let doc_data: Value = doc_response
        .json()
        .await
        .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("Failed to parse IK doc: {e}")))?;

    let title = doc_data["title"].as_str().unwrap_or("Untitled Case");
    let doc_html = doc_data["doc"].as_str().unwrap_or("");

    Ok(Json(json!({
        "tid": tid,
        "title": title,
        "html": doc_html,
        "source_url": format!("https://indiankanoon.org/doc/{tid}/"),
    })))
}

/// Strip HTML tags, leaving plain text. Simple but effective for IK's HTML output.
fn strip_html_tags(html: &str) -> String {
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
    // Collapse whitespace.
    let collapsed: String = result
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    collapsed
}

/// Sanitize a string for use as a filename.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .take(200)
        .collect()
}

// ---------------------------------------------------------------------------
// GET /indian-kanoon/documents — list locally cached cases
// ---------------------------------------------------------------------------

async fn list_documents(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> ApiResult {
    let rows: Vec<(String, String, Option<String>, i64, String, String)> = sqlx::query_as(
        "SELECT id, filename, corpus_identifier, size_bytes, created_at, status \
         FROM documents \
         WHERE user_id = ? AND corpus_id = ? \
         ORDER BY created_at DESC",
    )
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let docs: Vec<Value> = rows
        .into_iter()
        .map(|(id, filename, ident, size, created, status)| {
            let source_url = ident.as_ref().map(|tid| {
                format!("https://indiankanoon.org/doc/{tid}/")
            });
            json!({
                "id": id,
                "filename": filename,
                "corpus_identifier": ident,
                "size_bytes": size,
                "created_at": created,
                "status": status,
                "source_url": source_url,
            })
        })
        .collect();

    Ok(Json(json!({ "documents": docs })))
}

// ---------------------------------------------------------------------------
// DELETE /indian-kanoon/documents/:id — drop a cached case
// ---------------------------------------------------------------------------

async fn delete_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT storage_path, content_hash FROM documents \
         WHERE id = ? AND user_id = ? AND corpus_id = ?",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (storage_path, content_hash) = row.ok_or_else(|| {
        err(StatusCode::NOT_FOUND, "Indian Kanoon case not found.")
    })?;

    // Drop embedding chunks first.
    let _ = sqlx::query("DELETE FROM doc_chunks WHERE document_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    sqlx::query("DELETE FROM documents WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Ref-count the cache file before deleting from disk.
    if let Some(hash) = content_hash {
        let still: Option<(i64,)> = sqlx::query_as(
            "SELECT 1 FROM documents WHERE content_hash = ? LIMIT 1",
        )
        .bind(&hash)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        if still.is_none() {
            if let (Ok(storage), Some(key)) = (make_storage(), storage_path) {
                let _ = storage.delete(&key).await;
            }
        }
    }

    Ok(Json(json!({ "ok": true, "id": id })))
}

// ---------------------------------------------------------------------------
// POST /indian-kanoon/documents/:id/resync — re-embed an interrupted case
// ---------------------------------------------------------------------------

async fn resync_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT extracted_text_path, corpus_identifier, status \
         FROM documents \
         WHERE id = ? AND user_id = ? AND corpus_id = ?",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(CORPUS_ID)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (text_key, _tid, prev_status) = row.ok_or_else(|| {
        err(StatusCode::NOT_FOUND, "Indian Kanoon case not found.")
    })?;
    let text_key = text_key.ok_or_else(|| {
        err(StatusCode::CONFLICT, "No cached text found. Please re-fetch the case.")
    })?;

    let _ = sqlx::query("UPDATE documents SET status = 'syncing' WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    let storage = make_storage()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let bytes = storage
        .get(&text_key)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();

    let source_path = storage_root()
        .join(text_key.replace('/', std::path::MAIN_SEPARATOR_STR))
        .to_string_lossy()
        .to_string();

    // Re-run indexing (same pattern as EUR-Lex resync).
    #[cfg(feature = "rag")]
    let (chunks_indexed, indexing_error, final_status) = if let Some(emb) = state.embeddings.clone() {
        match emb.index_document(&auth.user_id, None, &id, &source_path, &text).await {
            Ok(n) => (n, None, "ready".to_string()),
            Err(e) => {
                tracing::warn!("[indian-kanoon] resync embed for {} failed: {}", id, e);
                (0, Some(e.to_string()), "interrupted".to_string())
            }
        }
    } else {
        (0usize, None, "ready".to_string())
    };

    #[cfg(not(feature = "rag"))]
    let (chunks_indexed, indexing_error, final_status) = (0usize, None::<String>, "ready".to_string());

    sqlx::query("UPDATE documents SET status = ? WHERE id = ?")
        .bind(&final_status)
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "id": id,
        "previous_status": prev_status,
        "status": final_status,
        "chunks_indexed": chunks_indexed,
        "indexing_error": indexing_error,
    })))
}
