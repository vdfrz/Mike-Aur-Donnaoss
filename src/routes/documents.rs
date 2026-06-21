use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, storage::make_storage, AppState};

fn storage_root() -> PathBuf {
    PathBuf::from(
        std::env::var("STORAGE_PATH").unwrap_or_else(|_| "./data/storage".to_string()),
    )
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

pub fn router() -> Router<Arc<AppState>> {
    use axum::routing::post;
    Router::new()
        .route("/", get(list_documents).post(upload_document))
        .route("/{id}", get(get_document).delete(delete_document))
        .route("/{id}/display", get(display_document))
        .route("/{id}/docx", get(display_document))
        .route("/{id}/text", get(display_document))
        .route("/{id}/url", get(get_document_url))
        .route("/{id}/tracked-change-ids", get(tracked_change_ids))
        .route("/{id}/render-word", post(render_word))
        .route("/{id}/edits/{edit_id}/accept", post(resolve_edit_accept))
        .route("/{id}/edits/{edit_id}/reject", post(resolve_edit_reject))
        .layer(DefaultBodyLimit::max(50_usize * 1024 * 1024 * 1024))
}

// ---------------------------------------------------------------------------
// GET /document?project_id=…
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct ListQuery {
    project_id: Option<String>,
}

async fn list_documents(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult {
    let rows: Vec<(String, String, String, i64, Option<String>, String)> = if let Some(pid) = &q.project_id {
        sqlx::query_as(
            "SELECT id, filename, file_type, size_bytes, status, created_at \
             FROM documents WHERE user_id = ? AND project_id = ? ORDER BY created_at DESC",
        )
        .bind(&auth.user_id)
        .bind(pid)
        .fetch_all(&state.db)
        .await
    } else {
        sqlx::query_as(
            "SELECT id, filename, file_type, size_bytes, status, created_at \
             FROM documents WHERE user_id = ? ORDER BY created_at DESC",
        )
        .bind(&auth.user_id)
        .fetch_all(&state.db)
        .await
    }
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let docs: Vec<Value> = rows
        .into_iter()
        .map(|(id, filename, file_type, size, status, created_at)| {
            json!({ "id": id, "filename": filename, "file_type": file_type,
                    "size_bytes": size, "status": status, "created_at": created_at })
        })
        .collect();

    Ok(Json(json!({ "documents": docs })))
}

// ---------------------------------------------------------------------------
// POST /document  — multipart upload
// Fields: file (binary), project_id? (text)
// ---------------------------------------------------------------------------
async fn upload_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> ApiResult {
    tracing::info!("[upload] POST /document user={}", auth.user_id);
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut filename: Option<String> = None;
    let mut project_id: Option<String> = None;
    // `cache=true` is the chat-composer signal: store the binary +
    // extracted text under data/storage/cache, keyed by SHA-256 of the
    // bytes. The chat row may not exist at upload time (the composer
    // materialises the chat on first send), so chat_id is wired up
    // later by the /chat send handler — and the chat-delete handler
    // ref-counts by content_hash before unlinking the on-disk files.
    let mut cache = false;
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        tracing::warn!("[upload] multipart parse error: {e}");
        err(StatusCode::BAD_REQUEST, &e.to_string())
    })? {
        let field_name = field.name().unwrap_or("").to_string();
        match field_name.as_str() {
            "file" => {
                filename = field.file_name().map(|s| s.to_string());
                let bytes = field.bytes().await.map_err(|e| {
                    tracing::warn!(
                        "[upload] failed reading file field (filename={:?}): {e}",
                        filename
                    );
                    err(StatusCode::BAD_REQUEST, &e.to_string())
                })?;
                tracing::info!(
                    "[upload] received file field name={:?} size={} bytes",
                    filename,
                    bytes.len()
                );
                file_bytes = Some(bytes.to_vec());
            }
            "project_id" => {
                let text = field.text().await.map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                if !text.trim().is_empty() {
                    project_id = Some(text.trim().to_string());
                }
            }
            "cache" => {
                let text = field.text().await.map_err(|e| err(StatusCode::BAD_REQUEST, &e.to_string()))?;
                cache = matches!(text.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes");
            }
            _ => {}
        }
    }

    let data = file_bytes.ok_or_else(|| err(StatusCode::BAD_REQUEST, "No file field in multipart"))?;
    let fname = filename.unwrap_or_else(|| "upload".to_string());
    let ext = fname.rsplit('.').next().unwrap_or("").to_lowercase();
    let file_type = match ext.as_str() {
        "pdf" => "pdf",
        "docx" => "docx",
        "rtf" => "rtf",
        "xlsx" => "xlsx",
        "xls" => "xls",
        "xlsb" => "xlsb",
        "ods" => "ods",
        "csv" => "csv",
        "txt" => "txt",
        "md" => "md",
        "png" => "png",
        "jpg" | "jpeg" => "jpeg",
        "tif" | "tiff" => "tiff",
        _ => "other",
    };

    let doc_id = uuid::Uuid::new_v4().to_string();
    let storage = make_storage().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let size = data.len() as i64;

    // Cache uploads (chat-attached): key files by SHA-256 of the
    // binary so re-uploads of identical content dedupe and same
    // user-facing filename across different chats can't collide on
    // disk. We also extract plain text once per unique hash so the
    // chat send handler doesn't re-parse a 200-page PDF on every
    // turn. Skip extraction silently if the binary or text already
    // exist on disk — same hash means identical bytes.
    let (storage_key, content_hash, extracted_text_path) = if cache {
        let hash = {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            format!("{:x}", hasher.finalize())
        };
        let bin_ext = if ext.is_empty() { "bin".to_string() } else { ext.clone() };
        let bin_key = format!("cache/{}.{}", hash, bin_ext);
        let txt_key = format!("cache/{}.txt", hash);

        let root = storage_root();
        let bin_abs = root.join(bin_key.replace('/', std::path::MAIN_SEPARATOR_STR));
        let txt_abs = root.join(txt_key.replace('/', std::path::MAIN_SEPARATOR_STR));

        if !bin_abs.exists() {
            storage
                .put(&bin_key, &data, "application/octet-stream")
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
            tracing::info!("[upload] cache binary written: {} ({} bytes)", bin_key, data.len());
        } else {
            tracing::info!("[upload] cache binary already exists, reusing: {}", bin_key);
        }

        if !txt_abs.exists() {
            // extract_text_dispatch keys off the path's extension, so
            // the absolute path of the binary we just wrote is the
            // right thing to feed it (pdfium also needs an on-disk
            // path for PDFs).
            match crate::sync::scanner::extract_text_dispatch(&bin_abs, &data) {
                Ok((text, skip_reason)) => {
                    if let Some(reason) = skip_reason {
                        tracing::info!(
                            "[upload] cache text extraction skipped for {} ({}): {}",
                            fname,
                            hash,
                            reason
                        );
                    }
                    storage
                        .put(&txt_key, text.as_bytes(), "text/plain; charset=utf-8")
                        .await
                        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
                    tracing::info!(
                        "[upload] cache text written: {} ({} chars)",
                        txt_key,
                        text.len()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[upload] cache text extraction failed for {} ({}): {}",
                        fname,
                        hash,
                        e
                    );
                    // Drop a marker so we don't retry on every reload —
                    // an empty .txt is a valid "we tried" signal.
                    let _ = storage
                        .put(&txt_key, b"", "text/plain; charset=utf-8")
                        .await;
                }
            }
        } else {
            tracing::info!("[upload] cache text already exists, reusing: {}", txt_key);
        }

        (bin_key, Some(hash), Some(txt_key))
    } else {
        // Legacy (non-cache) layout: per-user, per-doc-id. No hashing,
        // no text extraction — the existing pipeline handles those
        // documents on demand.
        let key = format!("documents/{}/{}", auth.user_id, doc_id);
        storage
            .put(&key, &data, "application/octet-stream")
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
        (key, None, None)
    };

    sqlx::query(
        "INSERT INTO documents (id, user_id, project_id, filename, file_type, size_bytes, storage_path, status, content_hash, extracted_text_path) \
         VALUES (?, ?, ?, ?, ?, ?, ?, 'ready', ?, ?)",
    )
    .bind(&doc_id)
    .bind(&auth.user_id)
    .bind(&project_id)
    .bind(&fname)
    .bind(file_type)
    .bind(size)
    .bind(&storage_key)
    .bind(&content_hash)
    .bind(&extracted_text_path)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "id": doc_id,
        "filename": fname,
        "file_type": file_type,
        "size_bytes": size,
        "status": "ready"
    })))
}

// ---------------------------------------------------------------------------
// GET /document/:id
// ---------------------------------------------------------------------------
async fn get_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(String, String, String, i64, Option<String>, Option<String>, String)> =
        sqlx::query_as(
            "SELECT id, filename, file_type, size_bytes, storage_path, status, created_at \
             FROM documents WHERE id = ? AND user_id = ?",
        )
        .bind(&id)
        .bind(&auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (id, filename, file_type, size, storage_path, status, created_at) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Document not found"))?;

    Ok(Json(json!({
        "id": id,
        "filename": filename,
        "file_type": file_type,
        "size_bytes": size,
        "storage_path": storage_path,
        "status": status,
        "created_at": created_at,
    })))
}

// ---------------------------------------------------------------------------
// GET /document/:id/display, /docx, /text — stream raw bytes for the viewer
// ---------------------------------------------------------------------------
async fn display_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> Response {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT filename, file_type, storage_path FROM documents WHERE id = ? AND user_id = ?",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((filename, file_type, Some(storage_path))) = row else {
        return (StatusCode::NOT_FOUND, "Document not found").into_response();
    };

    let storage = match crate::storage::make_storage() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let bytes = match storage.get(&storage_path).await {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    let content_type = match file_type.as_str() {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "rtf" => "application/rtf",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "csv" => "text/csv; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "png" => "image/png",
        "jpeg" | "jpg" => "image/jpeg",
        "tiff" | "tif" => "image/tiff",
        _ => "application/octet-stream",
    };

    let mut resp = Response::new(Body::from(bytes));
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(disp) = HeaderValue::from_str(&format!("inline; filename=\"{filename}\"")) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, disp);
    }
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=60"),
    );
    resp
}

// ---------------------------------------------------------------------------
// GET /document/:id/url — frontend convenience: returns a URL the viewer
// can fetch later. In MikeRust it's just an absolute /display URL because
// storage is local; remote-storage backends could return a presigned URL.
// ---------------------------------------------------------------------------
async fn get_document_url(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let owns: Option<(String,)> =
        sqlx::query_as("SELECT id FROM documents WHERE id = ? AND user_id = ?")
            .bind(&id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if owns.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Document not found"));
    }
    let api_base = std::env::var("API_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:3001".to_string());
    Ok(Json(json!({
        "url": format!("{api_base}/document/{id}/display"),
    })))
}

// ---------------------------------------------------------------------------
// DELETE /document/:id
// ---------------------------------------------------------------------------
async fn delete_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT storage_path FROM documents WHERE id = ? AND user_id = ?")
            .bind(&id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (storage_path,) = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Document not found"))?;

    // Delete from storage
    if let Some(key) = storage_path {
        if let Ok(storage) = make_storage() {
            let _ = storage.delete(&key).await;
        }
    }

    sqlx::query("DELETE FROM documents WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// Render a Markdown draft to a stored .docx.
//
// Shared by the `render_word` chat tool and the `POST /document/:id/render-word`
// route below. Reads `markdown_source` (the persistent working copy), renders it
// via the existing `markdown_to_docx`, stores the bytes, and flips the row to a
// ready, downloadable document. The Err string is a readable message the caller
// surfaces (409 when there is no markdown to render — e.g. an uploaded file).
// Returns `(filename, download_url)`.
// ---------------------------------------------------------------------------
pub async fn render_document_to_docx(
    state: &AppState,
    user_id: &str,
    id: &str,
) -> Result<(String, String), String> {
    let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT filename, markdown_source, storage_path FROM documents WHERE id = ? AND user_id = ?",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    let (filename, markdown_source, storage_path) =
        row.ok_or_else(|| "document not found".to_string())?;

    // 409 case: nothing to render (uploaded file / legacy doc with no markdown).
    let markdown = markdown_source.filter(|m| !m.trim().is_empty()).ok_or_else(|| {
        "no markdown source to render — this document has no editable draft (it is likely an uploaded file)".to_string()
    })?;

    // Title = filename without the .docx extension (matches how drafts are named).
    let title = filename.strip_suffix(".docx").unwrap_or(&filename).to_string();

    let bytes = crate::pdf::docx_writer::markdown_to_docx(&title, &markdown)
        .map_err(|e| format!("docx build: {e}"))?;

    let key = storage_path.unwrap_or_else(|| format!("documents/{user_id}/{id}"));
    let storage = make_storage().map_err(|e| e.to_string())?;
    storage
        .put(
            &key,
            &bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await
        .map_err(|e| format!("storage write: {e}"))?;

    let size = bytes.len() as i64;
    sqlx::query(
        "UPDATE documents SET storage_path = ?, size_bytes = ?, status = 'ready' \
         WHERE id = ? AND user_id = ?",
    )
    .bind(&key)
    .bind(size)
    .bind(id)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(|e| e.to_string())?;

    Ok((filename, format!("/document/{id}/docx")))
}

// ---------------------------------------------------------------------------
// POST /document/:id/render-word  — render a Markdown draft to a .docx
// ---------------------------------------------------------------------------
async fn render_word(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    match render_document_to_docx(&state, &auth.user_id, &id).await {
        Ok((_filename, download_url)) => {
            Ok(Json(json!({ "document_id": id, "download_url": download_url })))
        }
        Err(msg) if msg == "document not found" => Err(err(StatusCode::NOT_FOUND, &msg)),
        Err(msg) if msg.starts_with("no markdown source") => {
            Err(err(StatusCode::CONFLICT, &msg))
        }
        Err(msg) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, &msg)),
    }
}

// ---------------------------------------------------------------------------
// GET /document/:id/tracked-change-ids?version_id=…
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct TrackedChangeQuery {
    version_id: Option<String>,
}

async fn tracked_change_ids(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Query(q): Query<TrackedChangeQuery>,
) -> ApiResult {
    let storage_path = match &q.version_id {
        Some(vid) => {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT dv.storage_path FROM document_versions dv \
                 JOIN documents d ON d.id = dv.document_id \
                 WHERE dv.id = ? AND d.id = ? AND d.user_id = ?"
            )
                .bind(vid)
                .bind(&id)
                .bind(&auth.user_id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
            row.map(|(p,)| p)
        }
        None => {
            let row: Option<(Option<String>,)> = sqlx::query_as(
                "SELECT storage_path FROM documents WHERE id = ? AND user_id = ?"
            )
                .bind(&id)
                .bind(&auth.user_id)
                .fetch_optional(&state.db)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
            row.and_then(|(p,)| p)
        }
    };

    let storage_path = storage_path
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Document or version not found"))?;

    let storage = make_storage().map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let bytes = storage.get(&storage_path).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let ids = crate::pdf::docx_writer::extract_tracked_change_ids(&bytes)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let ids_json: Vec<Value> = ids.iter()
        .map(|(kind, w_id)| json!({"kind": kind, "w_id": w_id}))
        .collect();

    Ok(Json(json!({ "ids": ids_json })))
}

// ---------------------------------------------------------------------------
// POST /document/:id/edits/:edit_id/accept
// POST /document/:id/edits/:edit_id/reject
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct EditPath {
    id: String,
    edit_id: String,
}

async fn resolve_edit_accept(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(EditPath { id, edit_id }): Path<EditPath>,
) -> ApiResult {
    resolve_edit(state, &auth.user_id, &id, &edit_id, true).await
}

async fn resolve_edit_reject(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(EditPath { id, edit_id }): Path<EditPath>,
) -> ApiResult {
    resolve_edit(state, &auth.user_id, &id, &edit_id, false).await
}

async fn resolve_edit(
    state: Arc<AppState>,
    user_id: &str,
    doc_id: &str,
    edit_id: &str,
    accept: bool,
) -> ApiResult {
    // Look up the edit record
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT de.del_w_id, de.ins_w_id, de.version_id FROM document_edits de \
         JOIN documents d ON d.id = de.document_id \
         WHERE de.id = ? AND de.document_id = ? AND d.user_id = ?"
    )
        .bind(edit_id)
        .bind(doc_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (del_w_id, ins_w_id, _version_id) = row
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Edit not found"))?;

    // Fetch current document bytes
    let doc_row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT storage_path FROM documents WHERE id = ? AND user_id = ?"
    )
        .bind(doc_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let storage_path = doc_row
        .and_then(|(p,)| p)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "Document not found"))?;

    let storage = make_storage()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    let bytes = storage.get(&storage_path).await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    // Resolve the tracked change — try both del and ins w:ids
    let w_id = if accept { &ins_w_id } else { &del_w_id };
    let other_id = if accept { &del_w_id } else { &ins_w_id };

    let mut new_bytes = bytes.clone();

    // Accept: unwrap ins (keep content), remove del
    // Reject: unwrap del (keep content, delText→t), remove ins
    if !w_id.is_empty() {
        if let Ok(Some(b)) = crate::pdf::docx_writer::resolve_tracked_change(&new_bytes, w_id, accept) {
            new_bytes = b;
        }
    }
    if !other_id.is_empty() {
        if let Ok(Some(b)) = crate::pdf::docx_writer::resolve_tracked_change(&new_bytes, other_id, accept) {
            new_bytes = b;
        }
    }

    // Write back
    storage.put(
        &storage_path,
        &new_bytes,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ).await.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let new_size = new_bytes.len() as i64;
    let _ = sqlx::query("UPDATE documents SET size_bytes = ? WHERE id = ? AND user_id = ?")
        .bind(new_size)
        .bind(doc_id)
        .bind(user_id)
        .execute(&state.db)
        .await;

    let status_str = if accept { "accepted" } else { "rejected" };
    let _ = sqlx::query("UPDATE document_edits SET status = ? WHERE id = ?")
        .bind(status_str)
        .bind(edit_id)
        .execute(&state.db)
        .await;

    Ok(Json(json!({
        "ok": true,
        "status": status_str,
        "download_url": format!("/document/{}/docx", doc_id),
    })))
}
