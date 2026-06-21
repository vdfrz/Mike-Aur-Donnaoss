use axum::{extract::State, http::StatusCode, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/open-word", axum::routing::post(open_in_word))
}

#[derive(Deserialize)]
struct OpenWordBody {
    document_id: String,
}

/// POST /desktop/open-word
/// Opens a project document in Microsoft Word on the local machine.
/// Resolves the storage path from the DB, then shells out to `open -a`.
async fn open_in_word(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<OpenWordBody>,
) -> ApiResult {
    // Scope the lookup to the authenticated user so one user cannot open
    // another user's document by guessing its id (IDOR).
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT filename, storage_path FROM documents WHERE id = ? AND user_id = ?",
    )
    .bind(&body.document_id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (filename, storage_path) = match row {
        Some((f, Some(s))) => (f, s),
        Some((f, None)) => {
            return Err(err(
                StatusCode::BAD_REQUEST,
                &format!("Document '{f}' has no local file (stored in S3/R2)"),
            ));
        }
        None => return Err(err(StatusCode::NOT_FOUND, "Document not found")),
    };

    let storage_base = std::env::var("STORAGE_PATH")
        .unwrap_or_else(|_| "./data/storage".to_string());
    let full_path = std::path::PathBuf::from(&storage_base).join(&storage_path);

    if !full_path.exists() {
        return Err(err(
            StatusCode::NOT_FOUND,
            &format!("File not found on disk: {}", full_path.display()),
        ));
    }

    // Spawn `open -a "Microsoft Word" /path/to/file` (macOS only).
    // On Windows this would be `start winword.exe`.
    match std::process::Command::new("open")
        .args(["-a", "Microsoft Word", &full_path.to_string_lossy()])
        .spawn()
    {
        Ok(_) => Ok(Json(json!({
            "ok": true,
            "filename": filename,
            "path": full_path.to_string_lossy(),
        }))),
        Err(e) => Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to launch Word: {e}"),
        )),
    }
}
