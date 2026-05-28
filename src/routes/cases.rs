use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::{auth::middleware::AuthUser, pii, AppState};
use crate::llm::{LocalConfig, StreamParams};
use crate::routes::user::fetch_llm_settings;
use crate::agents::case_prep::outputs::{self as case_outputs, OutputConfig};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_cases).post(create_case))
        .route("/{id}", get(get_case).put(update_case).delete(delete_case))
        .route("/{id}/documents", post(attach_documents))
        .route("/{id}/documents/{doc_id}", delete(detach_document))
        .route("/{id}/analyze", post(analyze_case))
        .route("/{id}/findings", get(get_findings))
        .route("/{id}/outputs/brief", post(generate_brief))
        .route("/{id}/outputs/strategy-memo", post(generate_strategy_memo))
        .route("/{id}/outputs/hearing-prep", post(generate_hearing_prep))
        .route("/{id}/outputs", get(list_outputs))
        .route("/{id}/chat", post(case_chat))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn verify_case_ownership(
    state: &AppState,
    case_id: &str,
    user_id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let exists: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM cases WHERE id = ? AND user_id = ?",
    )
    .bind(case_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if exists.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Case not found"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GET /cases — list current user's cases
// ---------------------------------------------------------------------------

async fn list_cases(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> ApiResult {
    let rows: Vec<(String, String, Option<String>, String, String, String)> = sqlx::query_as(
        "SELECT id, title, court, status, created_at, updated_at \
         FROM cases WHERE user_id = ? ORDER BY updated_at DESC",
    )
    .bind(&auth.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let mut cases: Vec<Value> = Vec::with_capacity(rows.len());
    for (id, title, court, status, _created, updated_at) in &rows {
        let doc_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM case_documents WHERE case_id = ?",
        )
        .bind(id)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

        cases.push(json!({
            "id": id,
            "title": title,
            "court": court,
            "status": status,
            "document_count": doc_count.0,
            "updated_at": updated_at,
        }));
    }

    Ok(Json(json!({ "cases": cases })))
}

// ---------------------------------------------------------------------------
// POST /cases — create a case
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateCaseBody {
    title: String,
    court: Option<String>,
    parties: Option<Value>,
}

async fn create_case(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateCaseBody>,
) -> ApiResult {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Title cannot be empty"));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let parties_json = body.parties.as_ref().map(|v| v.to_string());

    sqlx::query(
        "INSERT INTO cases (id, user_id, title, court, parties_json, status, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, 'active', ?, ?)",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(title)
    .bind(&body.court)
    .bind(&parties_json)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "id": id,
        "title": title,
        "court": body.court,
        "parties": body.parties,
        "status": "active",
        "created_at": now,
        "updated_at": now,
    })))
}

// ---------------------------------------------------------------------------
// GET /cases/:id — full case details + attached docs + recent findings
// ---------------------------------------------------------------------------

async fn get_case(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(String, String, String, Option<String>, Option<String>, String, String, String)> =
        sqlx::query_as(
            "SELECT id, user_id, title, court, parties_json, status, created_at, updated_at \
             FROM cases WHERE id = ? AND user_id = ?",
        )
        .bind(&id)
        .bind(&auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (case_id, user_id, title, court, parties_json, status, created_at, updated_at) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Case not found"))?;

    let parties: Option<Value> = parties_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    let docs: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT cd.document_id, cd.document_type, cd.attached_at, \
                d.filename, d.file_type, d.status, d.size_bytes, d.page_count \
         FROM case_documents cd \
         LEFT JOIN documents d ON d.id = cd.document_id \
         WHERE cd.case_id = ?",
    )
    .bind(&case_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let documents: Vec<Value> = docs
        .into_iter()
        .map(|(doc_id, doc_type, attached_at, filename, file_type, status, size_bytes, page_count)| {
            json!({
                "case_id": case_id,
                "document_id": doc_id,
                "document_type": doc_type,
                "attached_at": attached_at,
                "filename": filename,
                "file_type": file_type,
                "status": status,
                "size_bytes": size_bytes,
                "page_count": page_count,
            })
        })
        .collect();

    let findings_rows: Vec<(String, String, String, String, String, Option<String>, String)> =
        sqlx::query_as(
            "SELECT id, case_id, agent_name, finding_type, content_json, grounding_json, created_at \
             FROM case_findings WHERE case_id = ? ORDER BY created_at DESC LIMIT 50",
        )
        .bind(&case_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let findings: Vec<Value> = findings_rows
        .into_iter()
        .map(|(fid, fcase_id, agent, finding_type, content, grounding, created)| {
            json!({
                "id": fid,
                "case_id": fcase_id,
                "agent_name": agent,
                "finding_type": finding_type,
                "content_json": content,
                "grounding_json": grounding,
                "created_at": created,
            })
        })
        .collect();

    let output_rows: Vec<(String, String, String, String, Option<String>, String)> =
        sqlx::query_as(
            "SELECT id, case_id, output_type, content_md, docx_document_id, created_at \
             FROM case_outputs WHERE case_id = ? ORDER BY created_at DESC",
        )
        .bind(&case_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let outputs: Vec<Value> = output_rows
        .into_iter()
        .map(|(oid, ocase_id, otype, content_md, docx_id, created)| {
            json!({
                "id": oid,
                "case_id": ocase_id,
                "output_type": otype,
                "content_md": content_md,
                "docx_document_id": docx_id,
                "created_at": created,
            })
        })
        .collect();

    Ok(Json(json!({
        "case_info": {
            "id": case_id,
            "user_id": user_id,
            "title": title,
            "court": court,
            "parties_json": parties_json,
            "status": status,
            "created_at": created_at,
            "updated_at": updated_at,
        },
        "documents": documents,
        "findings": findings,
        "outputs": outputs,
    })))
}

// ---------------------------------------------------------------------------
// PUT /cases/:id — update title/court/parties/status
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UpdateCaseBody {
    title: Option<String>,
    court: Option<String>,
    parties: Option<Value>,
    status: Option<String>,
}

async fn update_case(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateCaseBody>,
) -> ApiResult {
    verify_case_ownership(&state, &id, &auth.user_id).await?;

    let now = chrono::Utc::now().to_rfc3339();
    let parties_json = body.parties.as_ref().map(|v| v.to_string());

    sqlx::query(
        "UPDATE cases SET \
         title = COALESCE(?, title), \
         court = COALESCE(?, court), \
         parties_json = COALESCE(?, parties_json), \
         status = COALESCE(?, status), \
         updated_at = ? \
         WHERE id = ? AND user_id = ?",
    )
    .bind(&body.title)
    .bind(&body.court)
    .bind(&parties_json)
    .bind(&body.status)
    .bind(&now)
    .bind(&id)
    .bind(&auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true, "updated_at": now })))
}

// ---------------------------------------------------------------------------
// DELETE /cases/:id — cascade delete
// ---------------------------------------------------------------------------

async fn delete_case(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &id, &auth.user_id).await?;

    // Remove case-scoped embeddings from RAG store if available
    #[cfg(feature = "rag")]
    if let Some(emb) = state.embeddings.as_ref() {
        let doc_ids: Vec<(String,)> = sqlx::query_as(
            "SELECT document_id FROM case_documents WHERE case_id = ?",
        )
        .bind(&id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for (doc_id,) in &doc_ids {
            let _ = emb.delete_document(&auth.user_id, doc_id).await;
        }
    }

    // FK CASCADE handles case_documents, case_findings, case_outputs
    let result = sqlx::query("DELETE FROM cases WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Case not found"));
    }

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/documents — attach existing docs to case
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AttachDocumentsBody {
    document_ids: Vec<String>,
    document_types: Option<std::collections::HashMap<String, String>>,
}

async fn attach_documents(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<AttachDocumentsBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    if body.document_ids.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "document_ids cannot be empty"));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut attached = 0u64;

    for doc_id in &body.document_ids {
        // Verify document belongs to user
        let doc_exists: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM documents WHERE id = ? AND user_id = ?",
        )
        .bind(doc_id)
        .bind(&auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        if doc_exists.is_none() {
            continue;
        }

        let doc_type = body
            .document_types
            .as_ref()
            .and_then(|m| m.get(doc_id))
            .map(|s| s.as_str());

        let res = sqlx::query(
            "INSERT OR IGNORE INTO case_documents (case_id, document_id, document_type, attached_at) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&case_id)
        .bind(doc_id)
        .bind(doc_type)
        .bind(&now)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        attached += res.rows_affected();
    }

    // Touch case updated_at
    let _ = sqlx::query("UPDATE cases SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&case_id)
        .execute(&state.db)
        .await;

    Ok(Json(json!({ "attached": attached })))
}

// ---------------------------------------------------------------------------
// DELETE /cases/:id/documents/:doc_id — detach
// ---------------------------------------------------------------------------

async fn detach_document(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((case_id, doc_id)): Path<(String, String)>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let result = sqlx::query(
        "DELETE FROM case_documents WHERE case_id = ? AND document_id = ?",
    )
    .bind(&case_id)
    .bind(&doc_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Document not attached to this case"));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let _ = sqlx::query("UPDATE cases SET updated_at = ? WHERE id = ?")
        .bind(&now)
        .bind(&case_id)
        .execute(&state.db)
        .await;

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/analyze — trigger 7-agent orchestrator (returns 202)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AnalyzeCaseBody {
    redact_pii: Option<bool>,
}

async fn analyze_case(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<AnalyzeCaseBody>,
) -> Response {
    if let Err(e) = verify_case_ownership(&state, &case_id, &auth.user_id).await {
        return e.into_response();
    }

    let redact = body.redact_pii.unwrap_or(false);
    let user_id = auth.user_id.clone();
    let cid = case_id.clone();
    let db = state.db.clone();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        run_case_analysis(db, &user_id, &cid, redact, tx).await;
    });

    let sse_stream = ReceiverStream::new(rx);
    Sse::new(sse_stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn run_case_analysis(
    db: sqlx::SqlitePool,
    user_id: &str,
    case_id: &str,
    redact_pii: bool,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    // --- PII redaction pass (if enabled) ---
    if redact_pii {
        let doc_ids: Vec<(String,)> = sqlx::query_as(
            "SELECT document_id FROM case_documents WHERE case_id = ?",
        )
        .bind(case_id)
        .fetch_all(&db)
        .await
        .unwrap_or_default();

        if !doc_ids.is_empty() {
            tracing::info!("[cases/analyze] case={} PII redaction enabled, scrubbing {} doc(s)", case_id, doc_ids.len());
            let storage = crate::storage::make_storage().ok();
            if let Some(storage) = &storage {
                for (doc_id,) in &doc_ids {
                    let row: Option<(Option<String>,)> = sqlx::query_as(
                        "SELECT extracted_text_path FROM documents WHERE id = ?",
                    )
                    .bind(doc_id)
                    .fetch_optional(&db)
                    .await
                    .unwrap_or(None);

                    if let Some((Some(txt_key),)) = row {
                        if let Ok(raw_bytes) = storage.get(&txt_key).await {
                            let text = String::from_utf8_lossy(&raw_bytes);
                            let scrubbed = pii::scrub_pii(&text);
                            let redacted_key = format!("{}.redacted.txt", txt_key.trim_end_matches(".txt"));
                            let _ = storage.put(&redacted_key, scrubbed.scrubbed_text.as_bytes(), "text/plain").await;
                        }
                    }
                }
            }
        }
    }

    // --- Resolve LLM settings ---
    let user_settings = fetch_llm_settings(&db, user_id).await.ok();
    let (model, local_config) = resolve_analysis_model(&user_settings);
    let claude_key = user_settings.as_ref().and_then(|s| s.claude_api_key.clone()).filter(|k| !k.trim().is_empty());
    let gemini_key = user_settings.as_ref().and_then(|s| s.gemini_api_key.clone()).filter(|k| !k.trim().is_empty());
    let gemini_region = user_settings.as_ref().and_then(|s| s.gemini_region.clone());

    let llm_params = StreamParams {
        model,
        system_prompt: String::new(),
        system_volatile: String::new(),
        messages: vec![],
        tools: vec![],
        max_iterations: 1,
        enable_thinking: false,
        local_config,
        claude_api_key: claude_key,
        gemini_api_key: gemini_key,
        gemini_region,
    };

    // --- Clear previous findings for this case ---
    let _ = sqlx::query("DELETE FROM case_findings WHERE case_id = ?")
        .bind(case_id)
        .execute(&db)
        .await;

    // --- Emit pending status for all agents immediately so UI shows the
    // --- checklist while we extract text (which can take a while for
    // --- scanned PDFs that require OCR).
    for agent in &[
        "case_summary", "strengths_weaknesses", "evidence_gap",
        "opposition_predictor", "strategy_recommender", "precedent_finder",
        "risk_assessor",
    ] {
        let evt = json!({
            "type": "agent_status",
            "agent_name": agent,
            "status": "pending",
        });
        let _ = tx.send(Ok(Event::default().data(evt.to_string()))).await;
    }
    let prep_evt = json!({
        "type": "stage",
        "stage": "extracting",
        "message": "Extracting text from documents (this may take a few minutes for scanned PDFs)…",
    });
    let _ = tx.send(Ok(Event::default().data(prep_evt.to_string()))).await;

    // --- Run the orchestrator, streaming progress events back as SSE ---
    use crate::agents::case_prep::orchestrator::ProgressEvent;
    let (prog_tx, mut prog_rx) = tokio::sync::mpsc::channel::<ProgressEvent>(64);

    // Forward progress events from the orchestrator to the SSE stream.
    let sse_tx = tx.clone();
    let forwarder = tokio::spawn(async move {
        while let Some(evt) = prog_rx.recv().await {
            let json_evt = match evt {
                ProgressEvent::ExtractingDoc { filename, doc_index, total_docs } => json!({
                    "type": "extracting_doc",
                    "filename": filename,
                    "doc_index": doc_index,
                    "total_docs": total_docs,
                }),
                ProgressEvent::ExtractedDoc { filename, doc_index, total_docs, page_count, needed_ocr } => json!({
                    "type": "extracted_doc",
                    "filename": filename,
                    "doc_index": doc_index,
                    "total_docs": total_docs,
                    "page_count": page_count,
                    "needed_ocr": needed_ocr,
                }),
                ProgressEvent::Compressing { original_tokens, target_tokens } => json!({
                    "type": "compressing",
                    "original_tokens": original_tokens,
                    "target_tokens": target_tokens,
                }),
                ProgressEvent::Estimate { total_pages, estimated_seconds, has_ocr } => json!({
                    "type": "estimate",
                    "total_pages": total_pages,
                    "estimated_seconds": estimated_seconds,
                    "has_ocr": has_ocr,
                }),
                ProgressEvent::AgentStarted { agent_name } => json!({
                    "type": "agent_status",
                    "agent_name": agent_name,
                    "status": "running",
                }),
                ProgressEvent::AgentThinking { agent_name, snippet } => json!({
                    "type": "agent_thinking",
                    "agent_name": agent_name,
                    "snippet": snippet,
                }),
                ProgressEvent::AgentDone { finding } => {
                    let _ = sse_tx.send(Ok(Event::default().data(json!({
                        "type": "agent_status",
                        "agent_name": finding.agent_name,
                        "status": "done",
                    }).to_string()))).await;
                    let content: Value = serde_json::from_str(&finding.content_json).unwrap_or(Value::Null);
                    let grounding: Option<Value> = finding.grounding_json.as_ref()
                        .and_then(|g| serde_json::from_str(g).ok());
                    json!({
                        "type": "finding",
                        "finding": {
                            "id": finding.id,
                            "case_id": finding.case_id,
                            "agent_name": finding.agent_name,
                            "finding_type": finding.finding_type,
                            "content_json": content,
                            "grounding_json": grounding,
                            "created_at": finding.created_at,
                        }
                    })
                }
                ProgressEvent::AgentError { agent_name, error } => json!({
                    "type": "agent_status",
                    "agent_name": agent_name,
                    "status": "error",
                    "error": error,
                }),
            };
            let _ = sse_tx.send(Ok(Event::default().data(json_evt.to_string()))).await;
        }
    });

    match crate::agents::case_prep::orchestrator::analyze_case(
        case_id, user_id, &db, llm_params, redact_pii, Some(prog_tx)
    ).await {
        Ok(_) => {}
        Err(e) => {
            tracing::error!("[cases/analyze] orchestrator failed: {e}");
            let evt = json!({
                "type": "agent_status",
                "agent_name": "orchestrator",
                "status": "error",
                "error": e.to_string(),
            });
            let _ = tx.send(Ok(Event::default().data(evt.to_string()))).await;
        }
    }
    // Drop the orchestrator's sender clone by waiting for the forwarder; the
    // orchestrator has returned so its tx is dropped, closing the channel,
    // which lets the forwarder exit.
    let _ = forwarder.await;

    // --- Update case timestamp ---
    let now = chrono::Utc::now().to_rfc3339();
    let _ = sqlx::query("UPDATE cases SET updated_at = ? WHERE id = ? AND user_id = ?")
        .bind(&now)
        .bind(case_id)
        .bind(user_id)
        .execute(&db)
        .await;

    let _ = tx.send(Ok(Event::default().data(json!({"type": "done"}).to_string()))).await;
    let _ = tx.send(Ok(Event::default().data("[DONE]".to_string()))).await;

    tracing::info!("[cases/analyze] case={} analysis complete (pii_redacted={})", case_id, redact_pii);
}

fn resolve_analysis_model(
    settings: &Option<crate::routes::user::LlmSettings>,
) -> (String, Option<LocalConfig>) {
    // 1. Try user-configured provider from DB settings
    if let Some(s) = settings {
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

        if s.active_provider.as_deref() == Some("openai") {
            if let (Some(m), Some(k)) = (&s.openai_model, &s.openai_api_key) {
                if !k.trim().is_empty() {
                    let cfg = LocalConfig {
                        base_url: "https://api.openai.com/v1".to_string(),
                        api_key: Some(k.clone()),
                        model: m.clone(),
                    };
                    return (format!("openai:{m}"), Some(cfg));
                }
            }
        }

        if s.active_provider.as_deref() == Some("deepseek") {
            if let Some(ref m) = s.local_model {
                let cfg = LocalConfig {
                    base_url: "https://api.deepseek.com/v1".to_string(),
                    api_key: s.local_api_key.clone().filter(|k| !k.trim().is_empty()),
                    model: m.clone(),
                };
                return (format!("local:{m}"), Some(cfg));
            }
        }

        if let Some(ref m) = s.local_model {
            if let Some(ref b) = s.local_base_url {
                if !b.trim().is_empty() {
                    let cfg = LocalConfig {
                        base_url: b.clone(),
                        api_key: s.local_api_key.clone(),
                        model: m.clone(),
                    };
                    return (format!("local:{m}"), Some(cfg));
                }
            }
        }
    }

    // 2. Fallback: check env vars for an available provider
    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        if !key.trim().is_empty() {
            return ("gemini-2.0-flash".to_string(), None);
        }
    }

    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        if !key.trim().is_empty() {
            let model = "deepseek-chat".to_string();
            let cfg = LocalConfig {
                base_url: "https://api.deepseek.com/v1".to_string(),
                api_key: Some(key),
                model: model.clone(),
            };
            return (format!("local:{model}"), Some(cfg));
        }
    }

    if let Ok(base) = std::env::var("VLLM_BASE_URL") {
        if !base.trim().is_empty() {
            let model = std::env::var("VLLM_MAIN_MODEL").unwrap_or_else(|_| "default".to_string());
            let cfg = LocalConfig {
                base_url: base,
                api_key: std::env::var("VLLM_API_KEY").ok(),
                model: model.clone(),
            };
            return (format!("local:{model}"), Some(cfg));
        }
    }

    ("gemini-2.0-flash".to_string(), None)
}

// ---------------------------------------------------------------------------
// GET /cases/:id/findings — all findings grouped by agent_name
// ---------------------------------------------------------------------------

async fn get_findings(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let rows: Vec<(String, String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, case_id, agent_name, content_json, grounding_json, created_at \
         FROM case_findings WHERE case_id = ? ORDER BY created_at ASC",
    )
    .bind(&case_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let mut grouped: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
    for (fid, _, agent, content, grounding, created) in rows {
        grouped.entry(agent.clone()).or_default().push(json!({
            "id": fid,
            "agent_name": agent,
            "content": serde_json::from_str::<Value>(&content).unwrap_or(Value::Null),
            "grounding": grounding.and_then(|g| serde_json::from_str::<Value>(&g).ok()),
            "created_at": created,
        }));
    }

    Ok(Json(json!({ "findings": grouped })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/outputs/brief
// POST /cases/:id/outputs/strategy-memo
// POST /cases/:id/outputs/hearing-prep
// ---------------------------------------------------------------------------

async fn generate_output(
    state: Arc<AppState>,
    user_id: &str,
    case_id: &str,
    output_type: &str,
    redact_pii: bool,
) -> ApiResult {
    verify_case_ownership(&state, case_id, user_id).await?;

    let findings: Vec<(String, String)> = sqlx::query_as(
        "SELECT agent_name, content_json FROM case_findings WHERE case_id = ? ORDER BY created_at ASC",
    )
    .bind(case_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if findings.is_empty() {
        return Err(err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No findings available. Run analyze first.",
        ));
    }

    // Build findings as JSON values for the output generators
    let findings_json: Vec<serde_json::Value> = findings
        .iter()
        .map(|(agent_name, content_json)| {
            let parsed = serde_json::from_str::<serde_json::Value>(content_json)
                .unwrap_or_else(|_| json!(content_json));
            json!({
                "agent_name": agent_name,
                "finding_type": "analysis",
                "content_json": parsed.to_string(),
            })
        })
        .collect();

    // Resolve LLM settings (same pattern as analyze)
    let user_settings = fetch_llm_settings(&state.db, user_id).await.ok();
    let (model, local_config) = resolve_analysis_model(&user_settings);
    let claude_key = user_settings.as_ref().and_then(|s| s.claude_api_key.clone()).filter(|k| !k.trim().is_empty());
    let gemini_key = user_settings.as_ref().and_then(|s| s.gemini_api_key.clone()).filter(|k| !k.trim().is_empty());
    let gemini_region = user_settings.as_ref().and_then(|s| s.gemini_region.clone());

    let config = OutputConfig {
        model,
        local_config,
        claude_api_key: claude_key,
        gemini_api_key: gemini_key,
        gemini_region,
    };

    // Call the real LLM-backed output generator
    let doc_id = match output_type {
        "brief" => {
            case_outputs::generate_case_brief(&state.db, case_id, user_id, &findings_json, &config)
                .await
        }
        "strategy-memo" => {
            case_outputs::generate_strategy_memo(&state.db, case_id, user_id, &findings_json, &config)
                .await
        }
        "hearing-prep" => {
            case_outputs::generate_hearing_prep(&state.db, case_id, user_id, &findings_json, None, &config)
                .await
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "Unknown output type")),
    }
    .map_err(|e| {
        tracing::error!("[cases] generate_output({output_type}) failed: {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Output generation failed: {e}"))
    })?;

    // Fetch the persisted output to return (outputs.rs already saved it)
    let row: Option<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, output_type, content_md, docx_document_id, created_at \
         FROM case_outputs WHERE case_id = ? AND docx_document_id = ? \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(case_id)
    .bind(&doc_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (output_id, _otype, mut content_md, docx_doc_id, created_at) = row
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "Output saved but not found in DB"))?;

    // Optional PII redaction
    let mut pii_counts: Option<std::collections::HashMap<String, usize>> = None;
    if redact_pii {
        let result = pii::scrub_pii(&content_md);
        content_md = result.scrubbed_text;
        pii_counts = Some(result.counts);
        // Update the stored markdown with redacted version
        let _ = sqlx::query("UPDATE case_outputs SET content_md = ? WHERE id = ?")
            .bind(&content_md)
            .bind(&output_id)
            .execute(&state.db)
            .await;
    }

    let _ = sqlx::query("UPDATE cases SET updated_at = ? WHERE id = ?")
        .bind(&created_at)
        .bind(case_id)
        .execute(&state.db)
        .await;

    Ok(Json(json!({
        "id": output_id,
        "output_type": output_type,
        "content_md": content_md,
        "docx_document_id": docx_doc_id,
        "redacted_pii": redact_pii,
        "pii_counts": pii_counts,
        "created_at": created_at,
    })))
}

#[derive(Deserialize)]
struct GenerateOutputBody {
    redact_pii: Option<bool>,
}

async fn generate_brief(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<GenerateOutputBody>,
) -> ApiResult {
    generate_output(state, &auth.user_id, &case_id, "brief", body.redact_pii.unwrap_or(false)).await
}

async fn generate_strategy_memo(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<GenerateOutputBody>,
) -> ApiResult {
    generate_output(state, &auth.user_id, &case_id, "strategy-memo", body.redact_pii.unwrap_or(false)).await
}

async fn generate_hearing_prep(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<GenerateOutputBody>,
) -> ApiResult {
    generate_output(state, &auth.user_id, &case_id, "hearing-prep", body.redact_pii.unwrap_or(false)).await
}

// ---------------------------------------------------------------------------
// GET /cases/:id/outputs — list all generated outputs
// ---------------------------------------------------------------------------

async fn list_outputs(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let rows: Vec<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT id, output_type, content_md, docx_document_id, created_at \
         FROM case_outputs WHERE case_id = ? ORDER BY created_at DESC",
    )
    .bind(&case_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let outputs: Vec<Value> = rows
        .into_iter()
        .map(|(id, output_type, content_md, docx_id, created)| {
            json!({
                "id": id,
                "output_type": output_type,
                "content_md": content_md,
                "docx_document_id": docx_id,
                "created_at": created,
            })
        })
        .collect();

    Ok(Json(json!({ "outputs": outputs })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/chat — case-scoped chat (full chat infrastructure)
// ---------------------------------------------------------------------------

async fn case_chat(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if let Err(e) = verify_case_ownership(&state, &case_id, &auth.user_id).await {
        return e.into_response();
    }

    // Load case metadata
    let case_row: Option<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT title, court, parties_json, status FROM cases WHERE id = ? AND user_id = ?",
    )
    .bind(&case_id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((title, court, parties_json, status)) = case_row else {
        return err(StatusCode::NOT_FOUND, "Case not found").into_response();
    };

    let parties: Option<Value> = parties_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());

    // Load case documents with filenames
    let doc_rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT cd.document_id, COALESCE(d.filename, 'unknown'), cd.document_type \
         FROM case_documents cd \
         LEFT JOIN documents d ON d.id = cd.document_id \
         WHERE cd.case_id = ?",
    )
    .bind(&case_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Load latest findings
    let findings: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT agent_name, finding_type, content_json FROM case_findings \
         WHERE case_id = ? ORDER BY created_at ASC",
    )
    .bind(&case_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    // Build case system prompt
    let case_system_prompt = super::chat::build_case_system_prompt(
        &title,
        court.as_deref(),
        parties.as_ref(),
        &status,
        &doc_rows,
        &findings,
    );

    // Build case-doc label map (case-doc-0, case-doc-1, …)
    let case_doc_ids: Vec<String> = doc_rows.iter().map(|(id, _, _)| id.clone()).collect();
    let mut case_doc_labels = std::collections::HashMap::new();
    for (i, (doc_id, _, _)) in doc_rows.iter().enumerate() {
        case_doc_labels.insert(format!("case-doc-{i}"), doc_id.clone());
    }

    let case_ctx = super::chat::CaseContext {
        case_id: case_id.clone(),
        case_system_prompt,
        case_doc_ids,
        case_doc_labels,
    };

    // If the frontend didn't specify a model, inject the same model
    // the analysis uses so case chat doesn't fall back to local LLM.
    let mut body = body;
    if body.get("model").and_then(|v| v.as_str()).is_none() {
        let user_settings = fetch_llm_settings(&state.db, &auth.user_id).await.ok();
        let (resolved_model, _) = resolve_analysis_model(&user_settings);
        body.as_object_mut().map(|m| m.insert("model".to_string(), Value::String(resolved_model)));
    }

    super::chat::stream_chat_root(state, auth, body, Some(case_ctx)).await
}
