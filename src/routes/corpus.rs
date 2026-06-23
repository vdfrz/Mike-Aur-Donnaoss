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
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::corpus::ingest::{ingest_file, IngestEvent};
use crate::corpus::{
    upload_skip_reason, FIRM_SUPPORTED_EXTS, FIRM_UPLOAD_MAX_DOCS, FIRM_UPLOAD_MAX_FILE_BYTES,
};
use crate::{auth::middleware::AuthUser, storage::make_storage, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

/// Cap on the joined chunk text fed to the template-cleanup LLM call.
const TEMPLATE_TEXT_CAP: usize = 12_000;

/// How many files ingest at once during a folder drop. Bounded so 1 vs 500
/// degrades gracefully and the shared embedder is not stampeded.
// ponytail: a fixed pool of 4; raise toward 8 only if ingest throughput on a
// big folder becomes the bottleneck and the embedder can take the load.
const FIRM_INGEST_CONCURRENCY: usize = 4;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/limits", get(get_limits))
        .route("/files", get(list_files).post(upload_files))
        .route("/files/{id}", axum::routing::put(update_file).delete(delete_file))
        .route("/batches/{batch_id}", axum::routing::delete(delete_batch))
        .route("/process", post(process_files))
        .layer(DefaultBodyLimit::max(50_usize * 1024 * 1024 * 1024))
}

// ---------------------------------------------------------------------------
// GET /corpus/limits — the single source of truth for the upload caps, so the
// browser preflight enforces the exact numbers the server does.
// ---------------------------------------------------------------------------
async fn get_limits(_auth: AuthUser) -> ApiResult {
    Ok(Json(json!({
        "max_docs": FIRM_UPLOAD_MAX_DOCS,
        "max_file_bytes": FIRM_UPLOAD_MAX_FILE_BYTES,
        "supported_exts": FIRM_SUPPORTED_EXTS,
    })))
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
    // Folder-drop grouping: every file in one drop shares a batch_id and a
    // human label (the dropped folder's name) so the UI can group, track and
    // remove the whole batch. Single-file uploads leave these None.
    let mut batch_id: Option<String> = None;
    let mut batch_label: Option<String> = None;
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
            "batch_id" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                let text = text.trim();
                if !text.is_empty() {
                    batch_id = Some(text.to_string());
                }
            }
            "batch_label" => {
                let text = field
                    .text()
                    .await
                    .map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                let text = text.trim();
                if !text.is_empty() {
                    batch_label = Some(text.to_string());
                }
            }
            _ => {}
        }
    }

    if files.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "No file field in multipart"));
    }

    let mut accepted: Vec<String> = Vec::new();
    let mut duplicates: Vec<String> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();

    // The 500-doc cap is per folder batch. Count what the batch already holds
    // so a chunked upload (10 files per request) still enforces the limit
    // across requests instead of resetting each time.
    let mut batch_count: usize = if let Some(ref bid) = batch_id {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM corpus_files WHERE user_id = ? AND batch_id = ?",
        )
        .bind(&auth.user_id)
        .bind(bid)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        n as usize
    } else {
        0
    };

    for (filename, data) in files {
        // Server-side enforcement of the same type/size rule the browser
        // preflight uses — defense in depth, since the client can be bypassed.
        // Never a silent drop: every skip is reported with its reason.
        let size = u64::try_from(data.len()).unwrap_or(u64::MAX);
        if let Some(reason) = upload_skip_reason(&filename, size) {
            skipped.push(json!({"filename": filename, "reason": reason}));
            continue;
        }

        // Enforce the folder cap. Over the limit we report, never truncate.
        if let Some(reason) = crate::corpus::batch_cap_skip(batch_id.is_some(), batch_count) {
            skipped.push(json!({"filename": filename, "reason": reason}));
            continue;
        }

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

        // upload_skip_reason already guaranteed a supported extension.
        let file_type = crate::corpus::file_ext(&filename);

        let file_id = uuid::Uuid::new_v4().to_string();
        let storage_key = format!("corpus/{}/{}", auth.user_id, file_id);
        storage
            .put(&storage_key, &data, "application/octet-stream")
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        sqlx::query(
            "INSERT INTO corpus_files \
                (id, user_id, filename, file_type, sha256, is_template, status, batch_id, batch_label) \
             VALUES (?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
        )
        .bind(&file_id)
        .bind(&auth.user_id)
        .bind(&filename)
        .bind(&file_type)
        .bind(&sha256)
        .bind(i64::from(is_template))
        .bind(&batch_id)
        .bind(&batch_label)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        accepted.push(file_id);
        batch_count += 1;
    }

    Ok(Json(json!({
        "accepted": accepted,
        "duplicates": duplicates,
        "skipped": skipped,
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
    // Bounded worker pool: at most FIRM_INGEST_CONCURRENCY files ingest at
    // once. One file failing is isolated inside process_one, so a corrupt or
    // locked file never aborts the batch.
    stream::iter(file_ids)
        .for_each_concurrent(FIRM_INGEST_CONCURRENCY, |file_id| {
            let state = state.clone();
            let user_id = user_id.clone();
            let tx = tx.clone();
            async move {
                process_one(&state, &user_id, &file_id, &tx).await;
            }
        })
        .await;

    // Terminal markers, matching the chat/analyze SSE convention.
    let _ = tx
        .send(Ok(Event::default().data(json!({"type": "done"}).to_string())))
        .await;
    let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
}

/// Ingest one file and (if it is a ready template) build its template. All
/// failures are contained here so a sibling in the batch keeps going.
async fn process_one(
    state: &Arc<AppState>,
    user_id: &str,
    file_id: &str,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
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

    let result = ingest_file(state, user_id, file_id, Some(&prog_tx)).await;
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
        return;
    }

    // Template cleanup: only for files that finished `ready` AND are
    // flagged is_template. Non-fatal — log and continue on any failure.
    let row: Option<(String, i64, Option<String>)> = sqlx::query_as(
        "SELECT status, is_template, template_md FROM corpus_files WHERE id = ? AND user_id = ?",
    )
    .bind(file_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    if let Some((status, is_template, template_md)) = row {
        if status == "ready" && is_template == 1 && template_md.is_none() {
            if let Err(e) = build_template(state, user_id, file_id).await {
                tracing::warn!("[corpus] template cleanup failed for {file_id}: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// GET /corpus/files[?batch_id=…] — list the caller's corpus files, newest
// first. The optional batch_id filter backs the folder-upload progress poll.
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct ListQuery {
    batch_id: Option<String>,
}

type FileRow = (
    String,         // id
    String,         // filename
    Option<String>, // doc_type
    Option<String>, // case_type
    Option<String>, // court
    String,         // status
    i64,            // chunk_count
    i64,            // is_template
    String,         // created_at
    Option<String>, // error
    Option<String>, // batch_id
    Option<String>, // batch_label
);

async fn list_files(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult {
    const COLS: &str = "id, filename, doc_type, case_type, court, status, chunk_count, \
         is_template, created_at, error, batch_id, batch_label";

    let rows: Vec<FileRow> = if let Some(bid) = q.batch_id.as_ref() {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM corpus_files WHERE user_id = ? AND batch_id = ? ORDER BY created_at DESC"
        ))
        .bind(&auth.user_id)
        .bind(bid)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM corpus_files WHERE user_id = ? ORDER BY created_at DESC"
        ))
        .bind(&auth.user_id)
        .fetch_all(&state.db)
        .await
    }
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let files: Vec<Value> = rows
        .into_iter()
        .map(
            |(
                id,
                filename,
                doc_type,
                case_type,
                court,
                status,
                chunk_count,
                is_template,
                created_at,
                error,
                batch_id,
                batch_label,
            )| {
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
                    "error": error,
                    "batch_id": batch_id,
                    "batch_label": batch_label,
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
// Purge one corpus file completely: derived workflow, embedding vectors, the
// row (chunks + FTS cascade via FK), and the stored object. Shared by single-
// file and whole-batch deletion so neither path leaves orphan chunks/vectors.
// Caller is responsible for the ownership check.
// ---------------------------------------------------------------------------
async fn purge_corpus_file(
    state: &Arc<AppState>,
    user_id: &str,
    id: &str,
) -> Result<(), sqlx::Error> {
    // Remove any workflow derived from this template.
    let _ = sqlx::query("DELETE FROM workflows WHERE corpus_file_id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await;

    // Clear this file's embedding vectors. corpus_chunks_vec is a sqlite-vec
    // virtual table, so the ON DELETE CASCADE from corpus_files (which cleans
    // corpus_chunks + its FTS mirror) can't reach it — we delete by file_id
    // explicitly, exactly as ingest does on re-ingest.
    #[cfg(feature = "rag")]
    let _ = sqlx::query("DELETE FROM corpus_chunks_vec WHERE file_id = ?")
        .bind(id)
        .execute(&state.db)
        .await;

    // Delete the row — corpus_chunks cascade via FK.
    sqlx::query("DELETE FROM corpus_files WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user_id)
        .execute(&state.db)
        .await?;

    // Best-effort storage cleanup.
    if let Ok(storage) = make_storage() {
        let _ = storage.delete(&format!("corpus/{user_id}/{id}")).await;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// DELETE /corpus/files/:id — remove one file and everything derived from it.
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

    purge_corpus_file(&state, &auth.user_id, &id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// DELETE /corpus/batches/:batch_id — remove a whole folder upload, including
// every file's chunks and embedding vectors (no orphans left behind).
// ---------------------------------------------------------------------------
async fn delete_batch(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(batch_id): Path<String>,
) -> ApiResult {
    let ids: Vec<(String,)> =
        sqlx::query_as("SELECT id FROM corpus_files WHERE user_id = ? AND batch_id = ?")
            .bind(&auth.user_id)
            .bind(&batch_id)
            .fetch_all(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // No rows for this (user, batch): unknown batch or not the caller's —
    // 404, mirroring delete_file, so the client can tell it apart from a
    // real deletion.
    if ids.is_empty() {
        return Err(err(StatusCode::NOT_FOUND, "Batch not found"));
    }

    let mut deleted = 0usize;
    for (id,) in &ids {
        // One file failing to purge does not abort the rest of the batch.
        match purge_corpus_file(&state, &auth.user_id, id).await {
            Ok(()) => deleted += 1,
            Err(e) => tracing::warn!("[corpus] purge of {id} in batch {batch_id} failed: {e}"),
        }
    }

    Ok(Json(json!({ "ok": true, "deleted": deleted })))
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
