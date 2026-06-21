//! Firm-knowledge corpus HTTP API.
//!
//! Upload firm documents (past cases, skeletons, templates) into the
//! `corpus_files` table, run them through the ingest pipeline (chunk +
//! tag + index for FTS search), and manage the resulting rows. Templates
//! additionally get a cleaned `{{placeholder}}` markdown skeleton and a
//! `workflows` row so the drafting agent can reuse them.
//!
//! The ingest pipeline (`crate::corpus::ingest`) reads the uploaded bytes
//! from storage at `corpus/{user_id}/{file_id}`, so the upload handler
//! stores them there.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::corpus::ingest::{ingest_file, IngestEvent};
use crate::{auth::middleware::AuthUser, storage::make_storage, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

/// Cap on the joined chunk text fed to the template-cleanup LLM call.
const TEMPLATE_TEXT_CAP: usize = 12_000;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/files", get(list_files).post(upload_files))
        .route("/files/{id}", axum::routing::put(update_file).delete(delete_file))
        .route("/process", post(process_files))
        .layer(DefaultBodyLimit::max(50_usize * 1024 * 1024 * 1024))
}

// ---------------------------------------------------------------------------
// POST /corpus/files — multipart upload of one or more firm documents.
// Fields: file (binary, repeatable), is_template? (text bool).
// ---------------------------------------------------------------------------
async fn upload_files(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> ApiResult {
    let storage = make_storage().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let mut is_template = false;
    // Collected (filename, bytes) for every "file" part so we can apply the
    // is_template flag regardless of multipart field order.
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                let filename = field
                    .file_name()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "upload".to_string());
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                files.push((filename, bytes.to_vec()));
            }
            "is_template" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                is_template = matches!(
                    text.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                );
            }
            _ => {}
        }
    }

    if files.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "No file field in multipart"));
    }

    let mut accepted: Vec<String> = Vec::new();
    let mut duplicates: Vec<String> = Vec::new();

    for (filename, data) in files {
        // SHA-256 of the bytes (same hashing approach as the documents
        // upload cache path in routes/documents.rs).
        let sha256 = {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            format!("{:x}", hasher.finalize())
        };

        // Dedupe on (user_id, sha256) — matches the table's UNIQUE constraint.
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT id FROM corpus_files WHERE user_id = ? AND sha256 = ?")
                .bind(&auth.user_id)
                .bind(&sha256)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        if existing.is_some() {
            duplicates.push(filename);
            continue;
        }

        let ext = filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let file_type = if ext.is_empty() {
            "other".to_string()
        } else {
            ext
        };

        let file_id = uuid::Uuid::new_v4().to_string();
        let storage_key = format!("corpus/{}/{}", auth.user_id, file_id);
        storage
            .put(&storage_key, &data, "application/octet-stream")
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        sqlx::query(
            "INSERT INTO corpus_files (id, user_id, filename, file_type, sha256, is_template, status) \
             VALUES (?, ?, ?, ?, ?, ?, 'pending')",
        )
        .bind(&file_id)
        .bind(&auth.user_id)
        .bind(&filename)
        .bind(&file_type)
        .bind(&sha256)
        .bind(i64::from(is_template))
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        accepted.push(file_id);
    }

    Ok(Json(json!({
        "accepted": accepted,
        "duplicates": duplicates,
    })))
}

// ---------------------------------------------------------------------------
// POST /corpus/process — run ingest for the given file_ids, streaming
// per-stage progress over SSE. Mirrors the analyze_case SSE pattern.
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct ProcessBody {
    file_ids: Vec<String>,
}

async fn process_files(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<ProcessBody>,
) -> Response {
    let user_id = auth.user_id.clone();
    let file_ids = body.file_ids;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        run_process(state, user_id, file_ids, tx).await;
    });

    let sse_stream = ReceiverStream::new(rx);
    Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn run_process(
    state: Arc<AppState>,
    user_id: String,
    file_ids: Vec<String>,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    for file_id in &file_ids {
        // Forward IngestEvents from the ingest pipeline to the SSE stream.
        let (prog_tx, mut prog_rx) = tokio::sync::mpsc::channel::<IngestEvent>(64);
        let sse_tx = tx.clone();
        let forward = tokio::spawn(async move {
            while let Some(ev) = prog_rx.recv().await {
                let payload = match ev {
                    IngestEvent::Stage { file_id, stage } => {
                        json!({"type": "stage", "file_id": file_id, "stage": stage})
                    }
                    IngestEvent::Done { file_id, chunk_count, doc_type } => {
                        json!({"type": "done", "file_id": file_id, "chunk_count": chunk_count, "doc_type": doc_type})
                    }
                    IngestEvent::Error { file_id, message } => {
                        json!({"type": "error", "file_id": file_id, "message": message})
                    }
                };
                let _ = sse_tx.send(Ok(Event::default().data(payload.to_string()))).await;
            }
        });

        let result = ingest_file(&state, &user_id, file_id, Some(&prog_tx)).await;
        // Drop the sender so the forwarder task can drain and exit.
        drop(prog_tx);
        let _ = forward.await;

        if let Err(e) = result {
            // Hard IO/DB failure — stamp the row failed and surface it.
            let _ = sqlx::query("UPDATE corpus_files SET status = 'failed', error = ? WHERE id = ?")
                .bind(e.to_string())
                .bind(file_id)
                .execute(&state.db)
                .await;
            let payload = json!({"type": "error", "file_id": file_id, "message": e.to_string()});
            let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
            continue;
        }

        // Template cleanup: only for files that finished `ready` AND are
        // flagged is_template. Non-fatal — log and continue on any failure.
        let row: Option<(String, i64, Option<String>)> = sqlx::query_as(
            "SELECT status, is_template, template_md FROM corpus_files WHERE id = ? AND user_id = ?",
        )
        .bind(file_id)
        .bind(&user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        if let Some((status, is_template, template_md)) = row {
            if status == "ready" && is_template == 1 && template_md.is_none() {
                if let Err(e) = build_template(&state, &user_id, file_id).await {
                    tracing::warn!("[corpus] template cleanup failed for {file_id}: {e}");
                }
            }
        }
    }

    // Terminal markers, matching the chat/analyze SSE convention.
    let _ = tx
        .send(Ok(Event::default().data(json!({"type": "done"}).to_string())))
        .await;
    let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
}

// ---------------------------------------------------------------------------
// GET /corpus/files — list the caller's corpus files, newest first.
// ---------------------------------------------------------------------------
async fn list_files(State(state): State<Arc<AppState>>, auth: AuthUser) -> ApiResult {
    let rows: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        i64,
        i64,
        String,
    )> = sqlx::query_as(
        "SELECT id, filename, doc_type, case_type, court, status, chunk_count, is_template, created_at \
         FROM corpus_files WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(&auth.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let files: Vec<Value> = rows
        .into_iter()
        .map(
            |(id, filename, doc_type, case_type, court, status, chunk_count, is_template, created_at)| {
                json!({
                    "id": id,
                    "filename": filename,
                    "doc_type": doc_type,
                    "case_type": case_type,
                    "court": court,
                    "status": status,
                    "chunk_count": chunk_count,
                    "is_template": is_template != 0,
                    "created_at": created_at,
                })
            },
        )
        .collect();

    Ok(Json(json!({ "files": files })))
}

// ---------------------------------------------------------------------------
// PUT /corpus/files/:id — update provided metadata fields. Flipping
// is_template on (when no template_md exists yet) triggers template cleanup.
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct UpdateFileBody {
    is_template: Option<bool>,
    doc_type: Option<String>,
    case_type: Option<String>,
}

async fn update_file(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateFileBody>,
) -> ApiResult {
    // Ownership check + current template state.
    let row: Option<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT status, is_template, template_md FROM corpus_files WHERE id = ? AND user_id = ?",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (status, prev_is_template, template_md) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Corpus file not found"))?;

    if let Some(v) = body.is_template {
        sqlx::query("UPDATE corpus_files SET is_template = ? WHERE id = ? AND user_id = ?")
            .bind(i64::from(v))
            .bind(&id)
            .bind(&auth.user_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }
    if let Some(ref v) = body.doc_type {
        sqlx::query("UPDATE corpus_files SET doc_type = ? WHERE id = ? AND user_id = ?")
            .bind(v)
            .bind(&id)
            .bind(&auth.user_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }
    if let Some(ref v) = body.case_type {
        sqlx::query("UPDATE corpus_files SET case_type = ? WHERE id = ? AND user_id = ?")
            .bind(v)
            .bind(&id)
            .bind(&auth.user_id)
            .execute(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    }

    // If is_template flips to true and we have no template_md yet, build it.
    // Only meaningful once the file is `ready` (chunks exist to summarize).
    let flipped_on = body.is_template == Some(true) && prev_is_template == 0;
    if flipped_on && template_md.is_none() && status == "ready" {
        if let Err(e) = build_template(&state, &auth.user_id, &id).await {
            tracing::warn!("[corpus] template cleanup failed for {id}: {e}");
        }
    }

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// DELETE /corpus/files/:id — drop the row (chunks cascade via FK), clean up
// any derived workflow, and best-effort delete the stored object.
// ---------------------------------------------------------------------------
async fn delete_file(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let owns: Option<(String,)> =
        sqlx::query_as("SELECT id FROM corpus_files WHERE id = ? AND user_id = ?")
            .bind(&id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if owns.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Corpus file not found"));
    }

    // Remove any workflow derived from this template.
    let _ = sqlx::query("DELETE FROM workflows WHERE corpus_file_id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await;

    // Clear this file's embedding vectors. corpus_chunks_vec is a sqlite-vec
    // virtual table, so the ON DELETE CASCADE from corpus_files (which cleans
    // corpus_chunks + its FTS mirror) can't reach it — we delete by file_id
    // explicitly, exactly as ingest does on re-ingest.
    #[cfg(feature = "rag")]
    let _ = sqlx::query("DELETE FROM corpus_chunks_vec WHERE file_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await;

    // Delete the row — corpus_chunks cascade via FK.
    sqlx::query("DELETE FROM corpus_files WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Best-effort storage cleanup.
    if let Ok(storage) = make_storage() {
        let _ = storage.delete(&format!("corpus/{}/{}", auth.user_id, id)).await;
    }

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Template cleanup helper.
//
// Joins a ready template file's chunks, asks the LLM to turn it into a
// reusable {{placeholder}} markdown skeleton, stores it in template_md, and
// creates a workflows row pointing back at the file. Fully best-effort:
// returns Err so the caller can log, but never aborts the surrounding flow.
// ---------------------------------------------------------------------------
async fn build_template(state: &Arc<AppState>, user_id: &str, file_id: &str) -> anyhow::Result<()> {
    // Gather chunk text in document order, capped.
    let chunks: Vec<(String,)> =
        sqlx::query_as("SELECT text FROM corpus_chunks WHERE file_id = ? ORDER BY seq")
            .bind(file_id)
            .fetch_all(&state.db)
            .await?;

    if chunks.is_empty() {
        anyhow::bail!("no chunks to build template from");
    }

    let mut joined = String::new();
    for (text,) in &chunks {
        if joined.len() >= TEMPLATE_TEXT_CAP {
            break;
        }
        if !joined.is_empty() {
            joined.push_str("\n\n");
        }
        joined.push_str(text);
    }
    let joined: String = joined.chars().take(TEMPLATE_TEXT_CAP).collect();

    // Filename (for the workflow title).
    let filename: String =
        sqlx::query_scalar("SELECT filename FROM corpus_files WHERE id = ?")
            .bind(file_id)
            .fetch_one(&state.db)
            .await?;

    // LLM call to clean the document into a reusable template.
    let settings = crate::routes::user::fetch_llm_settings(&state.db, user_id).await.ok();
    let config = crate::llm::oneshot::config_from_settings(&settings);

    let system = "Convert this legal document into a clean reusable Markdown template. \
Replace every case-specific fact (names, dates, amounts, addresses, case numbers) with a \
descriptive {{snake_case_placeholder}}. Preserve the structure, headings and stock phrases \
verbatim. Output ONLY the markdown template.";

    let template_md = crate::llm::oneshot::complete(&config, system, &joined).await?;
    let template_md = template_md.trim().to_string();
    if template_md.is_empty() {
        anyhow::bail!("template cleanup produced empty output");
    }

    // Store the cleaned template.
    sqlx::query("UPDATE corpus_files SET template_md = ? WHERE id = ?")
        .bind(&template_md)
        .bind(file_id)
        .execute(&state.db)
        .await?;

    // Create the derived workflow row.
    let title_base = filename
        .rsplit_once('.')
        .map(|(stem, _ext)| stem)
        .unwrap_or(&filename);
    let title = format!("Firm Template — {title_base}");
    let prompt_md = format!(
        "Draft using this firm template. Keep its structure and stock phrasing; fill the \
{{{{placeholders}}}} from the user's facts and leave ________ where a fact is unknown.\n\n{template_md}"
    );

    let workflow_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO workflows (id, user_id, title, prompt_md, corpus_file_id, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, datetime('now'), datetime('now'))",
    )
    .bind(&workflow_id)
    .bind(user_id)
    .bind(&title)
    .bind(&prompt_md)
    .bind(file_id)
    .execute(&state.db)
    .await?;

    sqlx::query("UPDATE corpus_files SET workflow_id = ? WHERE id = ?")
        .bind(&workflow_id)
        .bind(file_id)
        .execute(&state.db)
        .await?;

    Ok(())
}
