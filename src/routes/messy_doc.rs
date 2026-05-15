//! Messy Word / Messy PDF cleanup route.
//!
//! POST /messy-doc/clean (multipart)
//!   Fields:
//!     - file      : the messy .docx or .pdf bytes
//!     - instructions : plain text describing how to clean the document
//!   Returns:
//!     { doc_id, filename, size_bytes }  — the cleaned .docx is stored as a
//!     standalone document and can be downloaded via GET /documents/:id/download.

use axum::{
    extract::{Multipart, State},
    http::StatusCode,
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, AppState};
use crate::llm::{self, types::{LocalConfig, Message, StreamParams}};
use crate::routes::user::fetch_llm_settings;

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/clean", post(clean))
}

async fn clean(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> ApiResult {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_ext = String::from("docx");
    let mut file_original_name = String::from("document");
    let mut instructions = String::new();

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        err(StatusCode::BAD_REQUEST, &format!("multipart error: {e}"))
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                if let Some(fname) = field.file_name() {
                    file_original_name = std::path::Path::new(fname)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("document")
                        .to_string();
                    file_ext = std::path::Path::new(fname)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("docx")
                        .to_ascii_lowercase();
                }
                let bytes = field.bytes().await.map_err(|e| {
                    err(StatusCode::BAD_REQUEST, &format!("read error: {e}"))
                })?;
                file_bytes = Some(bytes.to_vec());
            }
            "instructions" => {
                let text = field.text().await.map_err(|e| {
                    err(StatusCode::BAD_REQUEST, &format!("instructions error: {e}"))
                })?;
                instructions = text;
            }
            _ => {}
        }
    }

    let bytes = file_bytes
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "No file uploaded."))?;

    if bytes.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Uploaded file is empty."));
    }

    if instructions.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Instructions cannot be empty."));
    }

    // Step 1: Extract text from the uploaded document
    let raw_text = match file_ext.as_str() {
        "docx" => {
            crate::pdf::extract_docx_text(&bytes)
                .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, &format!("Could not read .docx: {e}")))?
        }
        "pdf" => {
            #[cfg(feature = "pdf")]
            {
                let tmp_name = format!("messy-doc-{}.pdf", uuid::Uuid::new_v4());
                let tmp = std::env::temp_dir().join(tmp_name);
                std::fs::write(&tmp, &bytes)
                    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("tmp write: {e}")))?;
                let pages = crate::pdf::extract_text(&tmp)
                    .map_err(|e| err(StatusCode::UNPROCESSABLE_ENTITY, &format!("PDF extraction: {e}")))?;
                let _ = std::fs::remove_file(&tmp);
                pages.iter().map(|p| p.text.clone()).collect::<Vec<_>>().join("\n\n")
            }
            #[cfg(not(feature = "pdf"))]
            {
                return Err(err(StatusCode::UNPROCESSABLE_ENTITY, "PDF support is not enabled in this build."));
            }
        }
        "txt" | "md" => String::from_utf8_lossy(&bytes).into_owned(),
        other => {
            return Err(err(
                StatusCode::UNPROCESSABLE_ENTITY,
                &format!("Unsupported file type: .{other}. Please upload a .docx or .pdf."),
            ));
        }
    };

    if raw_text.trim().is_empty() {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Could not extract any text from the uploaded file. \
             If it's a scanned PDF with no embedded text, it cannot be processed.",
        ));
    }

    // Step 2: Load user's LLM settings and pick a model
    let user_settings = fetch_llm_settings(&state.db, &auth.user_id)
        .await
        .ok();

    let (model, local_config) = resolve_model(&user_settings);

    // Step 3: Send to LLM with the user's instructions
    let system_prompt = "You are a professional legal document formatter. \
        Your task is to take messy, poorly-formatted text and produce a clean, \
        well-structured version following the user's specific instructions. \
        Output ONLY valid Markdown — no preamble, no explanation, no commentary. \
        Use headings (## / ###), bullet points, and proper paragraph breaks where appropriate. \
        Preserve all substantive content. Do not add new information.";

    let user_message = format!(
        "INSTRUCTIONS FROM USER:\n{}\n\n\
         DOCUMENT TEXT TO CLEAN:\n\n{}",
        instructions.trim(),
        raw_text.trim()
    );

    let params = StreamParams {
        model: model.clone(),
        system_prompt: system_prompt.to_string(),
        messages: vec![Message::user(user_message)],
        tools: vec![],
        max_iterations: 1,
        enable_thinking: false,
        local_config,
        claude_api_key: user_settings.as_ref().and_then(|s| s.claude_api_key.clone()),
        gemini_api_key: user_settings.as_ref().and_then(|s| s.gemini_api_key.clone()),
        gemini_region: user_settings.as_ref().and_then(|s| s.gemini_region.clone()),
    };

    let cleaned_markdown = match llm::provider_for_model(&model) {
        llm::Provider::Claude => llm::claude::complete(params).await,
        llm::Provider::OpenAI => llm::local::complete(params).await,
        llm::Provider::Gemini => llm::gemini::complete(params).await,
    }
    .map_err(|e| err(StatusCode::BAD_GATEWAY, &format!("LLM error: {e}")))?;

    // Step 4: Convert cleaned Markdown → .docx
    let doc_title = format!("{file_original_name} (cleaned)");
    let docx_bytes = crate::pdf::docx_writer::markdown_to_docx(&doc_title, &cleaned_markdown)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("docx build: {e}")))?;

    // Step 5: Persist as a document so it can be downloaded
    let doc_id = uuid::Uuid::new_v4().to_string();
    let safe_stem: String = file_original_name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .take(50)
        .collect();
    let filename = format!("{safe_stem}_cleaned.docx");
    let storage_path = format!("documents/{}/{}", auth.user_id, doc_id);

    let storage = crate::storage::make_storage()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("storage: {e}")))?;
    storage
        .put(
            &storage_path,
            &docx_bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("storage write: {e}")))?;

    let size = docx_bytes.len() as i64;
    sqlx::query(
        "INSERT INTO documents (id, user_id, project_id, filename, file_type, size_bytes, storage_path, status) \
         VALUES (?, ?, NULL, ?, 'docx', ?, ?, 'ready')",
    )
    .bind(&doc_id)
    .bind(&auth.user_id)
    .bind(&filename)
    .bind(size)
    .bind(&storage_path)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("db: {e}")))?;

    tracing::info!(
        "[messy-doc] user={} original={} ext={} doc_id={} size={}",
        auth.user_id, file_original_name, file_ext, doc_id, size
    );

    Ok(Json(json!({
        "doc_id": doc_id,
        "filename": filename,
        "size_bytes": size,
    })))
}

/// Resolve the best available LLM model from the user's saved settings.
fn resolve_model(
    settings: &Option<crate::routes::user::LlmSettings>,
) -> (String, Option<LocalConfig>) {
    let Some(s) = settings else {
        return ("gemini-2.0-flash".to_string(), None);
    };

    if s.active_provider.as_deref() == Some("deepseek") {
        if let Some(ref m) = s.local_model {
            let model = format!("local:{m}");
            let cfg = LocalConfig {
                base_url: "https://api.deepseek.com/v1".to_string(),
                api_key: s.local_api_key.clone().filter(|k| !k.trim().is_empty()),
                model: m.clone(),
            };
            return (model, Some(cfg));
        }
    }

    if s.active_provider.as_deref() == Some("openai") {
        if let (Some(m), Some(k)) = (&s.openai_model, &s.openai_api_key) {
            if !k.trim().is_empty() {
                let model = format!("openai:{m}");
                let cfg = LocalConfig {
                    base_url: "https://api.openai.com/v1".to_string(),
                    api_key: Some(k.clone()),
                    model: m.clone(),
                };
                return (model, Some(cfg));
            }
        }
    }

    if s.active_provider.as_deref() == Some("claude") {
        if s.claude_api_key.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false) {
            let m = s.main_model.clone().unwrap_or_else(|| "claude-sonnet-4-6".to_string());
            return (m, None);
        }
    }

    if s.active_provider.as_deref() == Some("gemini") {
        if s.gemini_api_key.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false) {
            return ("gemini-2.0-flash".to_string(), None);
        }
    }

    if let Some(ref m) = s.local_model {
        if let Some(ref b) = s.local_base_url {
            if !b.trim().is_empty() {
                let model = format!("local:{m}");
                let cfg = LocalConfig {
                    base_url: b.clone(),
                    api_key: s.local_api_key.clone(),
                    model: m.clone(),
                };
                return (model, Some(cfg));
            }
        }
    }

    ("gemini-2.0-flash".to_string(), None)
}
