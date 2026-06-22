use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::{auth::middleware::AuthUser, pii, AppState};
use crate::llm::StreamParams;
use crate::llm::oneshot::resolve_analysis_model;
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
        .route("/{id}/resolve-precedents", post(resolve_precedents))
        .route("/{id}/outputs/brief", post(generate_brief))
        .route("/{id}/outputs/strategy-memo", post(generate_strategy_memo))
        .route("/{id}/outputs/hearing-prep", post(generate_hearing_prep))
        .route("/{id}/outputs/list-of-dates", post(generate_list_of_dates))
        .route("/{id}/outputs/annexure-index", post(generate_annexure_index))
        .route("/{id}/outputs", get(list_outputs))
        .route("/{id}/chat", post(case_chat))
        // --- Drafting registry: parties ---
        .route("/{id}/parties", get(list_parties).post(create_party))
        .route("/{id}/parties/reorder", put(reorder_parties))
        .route("/{id}/parties/ai-populate", post(ai_populate_parties))
        .route("/{id}/parties/{party_id}", put(update_party).delete(delete_party))
        // --- Drafting registry: annexures ---
        .route("/{id}/annexures", get(list_annexures).post(create_annexure))
        .route("/{id}/annexures/reorder", put(reorder_annexures))
        .route("/{id}/annexures/ai-populate", post(ai_populate_annexures))
        .route("/{id}/annexures/{annexure_id}", put(update_annexure).delete(delete_annexure))
        // --- Drafting registry: citations ---
        .route("/{id}/citations", get(list_citations))
        .route("/{id}/citations/{citation_id}", delete(delete_citation))
        // --- Cross-ref resolution + bibliographies + red team ---
        .route("/{id}/resolve-refs", post(resolve_refs))
        .route("/{id}/outputs/cases-referred", post(generate_cases_referred))
        .route("/{id}/outputs/authorities", post(generate_authorities))
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
                ProgressEvent::Truncated { original_tokens, target_tokens, error } => json!({
                    "type": "truncated",
                    "original_tokens": original_tokens,
                    "target_tokens": target_tokens,
                    "error": error,
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

// resolve_analysis_model moved to crate::llm::oneshot (imported above) so it
// can be shared by case-prep and corpus tagging.

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
// POST /cases/:id/resolve-precedents — turn precedent_finder SEARCH
// SUGGESTIONS into real Indian Kanoon cases (tid + url) so the frontend
// can show case text and a "verify in eCourts" badge.
//
// Additive: reads the existing precedent_finder finding, reuses the
// existing kanoon_search tool. Does not touch analysis or any finding.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ResolvePrecedentsBody {
    /// Top Kanoon hits to keep per suggested query (1-3). Defaults to 1.
    results_per_query: Option<usize>,
}

async fn resolve_precedents(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<ResolvePrecedentsBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let per_query = body.results_per_query.unwrap_or(1).clamp(1, 3);

    // LLM config for confidence scoring (same resolution as output generation).
    let user_settings = fetch_llm_settings(&state.db, &auth.user_id).await.ok();
    let (score_model, score_local) = resolve_analysis_model(&user_settings);
    let score_config = OutputConfig {
        model: score_model,
        local_config: score_local,
        claude_api_key: user_settings.as_ref().and_then(|s| s.claude_api_key.clone()).filter(|k| !k.trim().is_empty()),
        gemini_api_key: user_settings.as_ref().and_then(|s| s.gemini_api_key.clone()).filter(|k| !k.trim().is_empty()),
        gemini_region: user_settings.as_ref().and_then(|s| s.gemini_region.clone()),
    };

    // Load the most recent precedent_finder finding for this case.
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT content_json FROM case_findings \
         WHERE case_id = ? AND agent_name = 'precedent_finder' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&case_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (content_json,) = row.ok_or_else(|| {
        err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No precedent findings available. Run analyze first.",
        )
    })?;

    let content: Value = serde_json::from_str(&content_json).unwrap_or(Value::Null);
    let required = content
        .get("required_precedents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut resolved: Vec<Value> = Vec::with_capacity(required.len());
    for precedent in &required {
        let query = precedent
            .get("suggested_search_query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let mut cases: Vec<Value> = if query.is_empty() {
            Vec::new()
        } else {
            let args = json!({
                "query": query,
                "max_results": per_query,
                "include_fragments": true,
            });
            // Reuse the existing Kanoon search tool. Returns a JSON string.
            let raw = crate::llm::kanoon_tool::exec_kanoon_search(&state, &auth.user_id, &args).await;
            let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
            parsed
                .get("results")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .take(per_query)
                        .map(|r| {
                            json!({
                                "tid": r.get("tid"),
                                "title": r.get("title"),
                                "court": r.get("court"),
                                "decision_date": r.get("decision_date"),
                                "snippet": r.get("snippet"),
                                "relevant_paragraphs": r.get("relevant_paragraphs"),
                                "kanoon_url": r.get("kanoon_url"),
                                "relevance_score": r.get("relevance_score"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        };

        // AI-judged confidence: how strongly each case actually supports the point of law.
        let point_of_law = precedent.get("point_of_law").and_then(|v| v.as_str()).unwrap_or("");
        if !cases.is_empty() {
            let scores = case_outputs::score_precedent_cases(&score_config, point_of_law, &cases).await;
            for (i, case) in cases.iter_mut().enumerate() {
                if let (Some(obj), Some((conf, reason))) = (case.as_object_mut(), scores.get(i)) {
                    obj.insert("confidence".to_string(), json!(conf));
                    obj.insert("reason".to_string(), json!(reason));
                }
            }
        }

        resolved.push(json!({
            "point_of_law": precedent.get("point_of_law"),
            "suggested_search_query": query,
            "target_court": precedent.get("target_court"),
            "grounding": precedent.get("grounding"),
            "cases": cases,
        }));
    }

    Ok(Json(json!({ "resolved_precedents": resolved })))
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

    // Call the real LLM-backed output generator.
    // The annexure index lists the attached documents; the others build on analysis findings.
    let doc_id = if output_type == "annexure-index" {
        // The annexure index is built deterministically off the drafting registry
        // (seeded from the attached documents) inside generate_annexure_index. We
        // only guard the empty case here so the user gets a clear 422, not a 500.
        let doc_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM case_documents WHERE case_id = ?",
        )
        .bind(case_id)
        .fetch_one(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

        if doc_count == 0 {
            return Err(err(
                StatusCode::UNPROCESSABLE_ENTITY,
                "No documents attached to this case.",
            ));
        }

        case_outputs::generate_annexure_index(&state.db, case_id, user_id)
            .await
    } else {
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

        match output_type {
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
            "list-of-dates" => {
                case_outputs::generate_list_of_dates(&state.db, case_id, user_id, &findings_json, &config)
                    .await
            }
            _ => return Err(err(StatusCode::BAD_REQUEST, "Unknown output type")),
        }
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

async fn generate_list_of_dates(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<GenerateOutputBody>,
) -> ApiResult {
    generate_output(state, &auth.user_id, &case_id, "list-of-dates", body.redact_pii.unwrap_or(false)).await
}

async fn generate_annexure_index(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<GenerateOutputBody>,
) -> ApiResult {
    generate_output(state, &auth.user_id, &case_id, "annexure-index", body.redact_pii.unwrap_or(false)).await
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

// ===========================================================================
// Drafting registry: parties / annexures / citations
//
// Additive endpoints backing the drafting cross-reference registry
// (migrations/0038). They reuse crate::drafting::{registry, crossrefs,
// citations} for all serial/slug bookkeeping and bibliography rendering.
// ===========================================================================

/// Build a slug from `name` that doesn't collide with any slug already used by
/// the case (parties or annexures share UNIQUE(case_id, slug) within their own
/// table). On collision, append 2, 3, ... — mirrors registry::unique_slug.
async fn unique_slug_for(
    db: &sqlx::SqlitePool,
    case_id: &str,
    table: &str,
    name: &str,
) -> Result<String, (StatusCode, Json<Value>)> {
    let mut base = crate::drafting::crossrefs::slugify(name);
    if base.is_empty() {
        base = "party".to_string();
    }
    let select = if table == "case_annexures" {
        "SELECT slug FROM case_annexures WHERE case_id = ?"
    } else {
        "SELECT slug FROM case_parties WHERE case_id = ?"
    };
    let existing: std::collections::HashSet<String> = sqlx::query_as::<_, (String,)>(select)
        .bind(case_id)
        .fetch_all(db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
        .into_iter()
        .map(|(s,)| s)
        .collect();

    if !existing.contains(&base) {
        return Ok(base);
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}{n}");
        if !existing.contains(&candidate) {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// Rebuild `cases.parties_json` from the current `case_parties` rows so the
/// existing left-sidebar (which reads parties_json) stays in sync with the
/// registry. Shape: a JSON array of {name, role} where role == side.
async fn resync_parties_json(
    db: &sqlx::SqlitePool,
    case_id: &str,
) -> Result<(), (StatusCode, Json<Value>)> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, side FROM case_parties WHERE case_id = ? ORDER BY side, serial_no",
    )
    .bind(case_id)
    .fetch_all(db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let arr: Vec<Value> = rows
        .into_iter()
        .map(|(name, side)| json!({ "name": name, "role": side }))
        .collect();
    let parties_json = Value::Array(arr).to_string();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query("UPDATE cases SET parties_json = ?, updated_at = ? WHERE id = ?")
        .bind(&parties_json)
        .bind(&now)
        .bind(case_id)
        .execute(db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(())
}

/// Load all party rows for a case as JSON, ordered by side, serial_no.
async fn fetch_parties(
    db: &sqlx::SqlitePool,
    case_id: &str,
) -> Result<Vec<Value>, (StatusCode, Json<Value>)> {
    let rows: Vec<(String, String, String, String, String, Option<String>, i64, String, String, String)> =
        sqlx::query_as(
            "SELECT id, case_id, slug, name, side, role_label, serial_no, source, created_at, updated_at \
             FROM case_parties WHERE case_id = ? ORDER BY side, serial_no",
        )
        .bind(case_id)
        .fetch_all(db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|(id, cid, slug, name, side, role_label, serial_no, source, created_at, updated_at)| {
            json!({
                "id": id,
                "case_id": cid,
                "slug": slug,
                "name": name,
                "side": side,
                "role_label": role_label,
                "serial_no": serial_no,
                "source": source,
                "created_at": created_at,
                "updated_at": updated_at,
            })
        })
        .collect())
}

/// Load all annexure rows for a case as JSON, ordered by side, serial_no.
async fn fetch_annexures(
    db: &sqlx::SqlitePool,
    case_id: &str,
) -> Result<Vec<Value>, (StatusCode, Json<Value>)> {
    let rows: Vec<(String, String, String, String, Option<String>, Option<String>, String, i64, String, String)> =
        sqlx::query_as(
            "SELECT id, case_id, document_id, slug, description, doc_date, side, serial_no, created_at, updated_at \
             FROM case_annexures WHERE case_id = ? ORDER BY side, serial_no",
        )
        .bind(case_id)
        .fetch_all(db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|(id, cid, document_id, slug, description, doc_date, side, serial_no, created_at, updated_at)| {
            json!({
                "id": id,
                "case_id": cid,
                "document_id": document_id,
                "slug": slug,
                "description": description,
                "doc_date": doc_date,
                "side": side,
                "serial_no": serial_no,
                "created_at": created_at,
                "updated_at": updated_at,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// GET /cases/:id/parties
// ---------------------------------------------------------------------------

async fn list_parties(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;
    let parties = fetch_parties(&state.db, &case_id).await?;
    Ok(Json(json!({ "parties": parties })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/parties
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreatePartyBody {
    name: String,
    side: String,
    role_label: Option<String>,
}

async fn create_party(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<CreatePartyBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let name = body.name.trim();
    if name.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "name cannot be empty"));
    }
    if body.side != "petitioner" && body.side != "respondent" {
        return Err(err(StatusCode::BAD_REQUEST, "side must be 'petitioner' or 'respondent'"));
    }

    let slug = unique_slug_for(&state.db, &case_id, "case_parties", name).await?;
    let serial_no = crate::drafting::registry::next_serial(&state.db, &case_id, "case_parties", &body.side)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO case_parties \
         (id, case_id, slug, name, side, role_label, serial_no, details_json, source, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 'manual', ?, ?)",
    )
    .bind(&id)
    .bind(&case_id)
    .bind(&slug)
    .bind(name)
    .bind(&body.side)
    .bind(&body.role_label)
    .bind(serial_no)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    resync_parties_json(&state.db, &case_id).await?;

    Ok(Json(json!({
        "id": id,
        "case_id": case_id,
        "slug": slug,
        "name": name,
        "side": body.side,
        "role_label": body.role_label,
        "serial_no": serial_no,
        "source": "manual",
        "created_at": now,
        "updated_at": now,
    })))
}

// ---------------------------------------------------------------------------
// PUT /cases/:id/parties/:party_id
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UpdatePartyBody {
    name: Option<String>,
    role_label: Option<String>,
    slug: Option<String>,
}

async fn update_party(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((case_id, party_id)): Path<(String, String)>,
    Json(body): Json<UpdatePartyBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let now = chrono::Utc::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE case_parties SET \
         name = COALESCE(?, name), \
         role_label = COALESCE(?, role_label), \
         slug = COALESCE(?, slug), \
         updated_at = ? \
         WHERE id = ? AND case_id = ?",
    )
    .bind(&body.name)
    .bind(&body.role_label)
    .bind(&body.slug)
    .bind(&now)
    .bind(&party_id)
    .bind(&case_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Party not found"));
    }

    resync_parties_json(&state.db, &case_id).await?;
    let parties = fetch_parties(&state.db, &case_id).await?;
    let party = parties.into_iter().find(|p| p.get("id").and_then(|v| v.as_str()) == Some(party_id.as_str()));
    Ok(Json(json!({ "party": party })))
}

// ---------------------------------------------------------------------------
// DELETE /cases/:id/parties/:party_id
// ---------------------------------------------------------------------------

async fn delete_party(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((case_id, party_id)): Path<(String, String)>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let side: Option<(String,)> = sqlx::query_as(
        "SELECT side FROM case_parties WHERE id = ? AND case_id = ?",
    )
    .bind(&party_id)
    .bind(&case_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (side,) = side.ok_or_else(|| err(StatusCode::NOT_FOUND, "Party not found"))?;

    sqlx::query("DELETE FROM case_parties WHERE id = ? AND case_id = ?")
        .bind(&party_id)
        .bind(&case_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    crate::drafting::registry::compact_serials(&state.db, &case_id, "case_parties", &side)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    resync_parties_json(&state.db, &case_id).await?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// PUT /cases/:id/parties/reorder
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ReorderBody {
    side: String,
    ordered_ids: Vec<String>,
}

async fn reorder_parties(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<ReorderBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    crate::drafting::registry::reorder(&state.db, &case_id, "case_parties", &body.side, &body.ordered_ids)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let parties = fetch_parties(&state.db, &case_id).await?;
    Ok(Json(json!({ "parties": parties })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/parties/ai-populate
// ---------------------------------------------------------------------------

async fn ai_populate_parties(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let seeded = crate::drafting::registry::seed_parties_from_findings(&state.db, &case_id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    resync_parties_json(&state.db, &case_id).await?;
    let parties = fetch_parties(&state.db, &case_id).await?;
    Ok(Json(json!({ "seeded": seeded, "parties": parties })))
}

// ---------------------------------------------------------------------------
// GET /cases/:id/annexures
// ---------------------------------------------------------------------------

async fn list_annexures(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;
    let annexures = fetch_annexures(&state.db, &case_id).await?;
    Ok(Json(json!({ "annexures": annexures })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/annexures
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateAnnexureBody {
    document_id: String,
    side: Option<String>,
    description: Option<String>,
    doc_date: Option<String>,
}

async fn create_annexure(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<CreateAnnexureBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let side = body.side.clone().unwrap_or_else(|| "P".to_string());
    if side != "P" && side != "R" && side != "C" {
        return Err(err(StatusCode::BAD_REQUEST, "side must be 'P', 'R', or 'C'"));
    }

    // The document must be attached to this case; pull its filename for the slug.
    let filename: Option<(String,)> = sqlx::query_as(
        "SELECT d.filename FROM case_documents cd JOIN documents d ON d.id = cd.document_id \
         WHERE cd.case_id = ? AND cd.document_id = ?",
    )
    .bind(&case_id)
    .bind(&body.document_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (filename,) = filename
        .ok_or_else(|| err(StatusCode::UNPROCESSABLE_ENTITY, "Document is not attached to this case"))?;

    let slug = unique_slug_for(&state.db, &case_id, "case_annexures", &filename).await?;
    let serial_no = crate::drafting::registry::next_serial(&state.db, &case_id, "case_annexures", &side)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO case_annexures \
         (id, case_id, document_id, slug, description, doc_date, side, serial_no, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&case_id)
    .bind(&body.document_id)
    .bind(&slug)
    .bind(&body.description)
    .bind(&body.doc_date)
    .bind(&side)
    .bind(serial_no)
    .bind(&now)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "id": id,
        "case_id": case_id,
        "document_id": body.document_id,
        "slug": slug,
        "description": body.description,
        "doc_date": body.doc_date,
        "side": side,
        "serial_no": serial_no,
        "created_at": now,
        "updated_at": now,
    })))
}

// ---------------------------------------------------------------------------
// PUT /cases/:id/annexures/:annexure_id
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UpdateAnnexureBody {
    description: Option<String>,
    doc_date: Option<String>,
    side: Option<String>,
    slug: Option<String>,
}

async fn update_annexure(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((case_id, annexure_id)): Path<(String, String)>,
    Json(body): Json<UpdateAnnexureBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    if let Some(side) = &body.side {
        if side != "P" && side != "R" && side != "C" {
            return Err(err(StatusCode::BAD_REQUEST, "side must be 'P', 'R', or 'C'"));
        }
    }

    let now = chrono::Utc::now().to_rfc3339();
    let result = sqlx::query(
        "UPDATE case_annexures SET \
         description = COALESCE(?, description), \
         doc_date = COALESCE(?, doc_date), \
         side = COALESCE(?, side), \
         slug = COALESCE(?, slug), \
         updated_at = ? \
         WHERE id = ? AND case_id = ?",
    )
    .bind(&body.description)
    .bind(&body.doc_date)
    .bind(&body.side)
    .bind(&body.slug)
    .bind(&now)
    .bind(&annexure_id)
    .bind(&case_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Annexure not found"));
    }

    let annexures = fetch_annexures(&state.db, &case_id).await?;
    let annexure = annexures.into_iter().find(|a| a.get("id").and_then(|v| v.as_str()) == Some(annexure_id.as_str()));
    Ok(Json(json!({ "annexure": annexure })))
}

// ---------------------------------------------------------------------------
// DELETE /cases/:id/annexures/:annexure_id
// ---------------------------------------------------------------------------

async fn delete_annexure(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((case_id, annexure_id)): Path<(String, String)>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let side: Option<(String,)> = sqlx::query_as(
        "SELECT side FROM case_annexures WHERE id = ? AND case_id = ?",
    )
    .bind(&annexure_id)
    .bind(&case_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (side,) = side.ok_or_else(|| err(StatusCode::NOT_FOUND, "Annexure not found"))?;

    sqlx::query("DELETE FROM case_annexures WHERE id = ? AND case_id = ?")
        .bind(&annexure_id)
        .bind(&case_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    crate::drafting::registry::compact_serials(&state.db, &case_id, "case_annexures", &side)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// PUT /cases/:id/annexures/reorder
// ---------------------------------------------------------------------------

async fn reorder_annexures(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<ReorderBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    crate::drafting::registry::reorder(&state.db, &case_id, "case_annexures", &body.side, &body.ordered_ids)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let annexures = fetch_annexures(&state.db, &case_id).await?;
    Ok(Json(json!({ "annexures": annexures })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/annexures/ai-populate
// ---------------------------------------------------------------------------

async fn ai_populate_annexures(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let seeded = crate::drafting::registry::seed_annexures_from_documents(&state.db, &case_id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let annexures = fetch_annexures(&state.db, &case_id).await?;
    Ok(Json(json!({ "seeded": seeded, "annexures": annexures })))
}

// ---------------------------------------------------------------------------
// GET /cases/:id/citations
// ---------------------------------------------------------------------------

async fn list_citations(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let judgment_rows: Vec<(String, String, Option<i64>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, i64, String, String)> =
        sqlx::query_as(
            "SELECT id, status, kanoon_tid, title, court, decision_date, kanoon_url, canonical_citation, \
                    times_cited, first_cited_at, last_cited_at \
             FROM case_citations WHERE case_id = ? AND kind = 'judgment' \
             ORDER BY title COLLATE NOCASE",
        )
        .bind(&case_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let judgments: Vec<Value> = judgment_rows
        .into_iter()
        .map(|(id, status, tid, title, court, decision_date, url, citation, times_cited, first, last)| {
            json!({
                "id": id,
                "kind": "judgment",
                "status": status,
                "kanoon_tid": tid,
                "title": title,
                "court": court,
                "decision_date": decision_date,
                "kanoon_url": url,
                "canonical_citation": citation,
                "times_cited": times_cited,
                "first_cited_at": first,
                "last_cited_at": last,
            })
        })
        .collect();

    let statute_rows: Vec<(String, String, Option<String>, Option<String>, i64, String, String)> =
        sqlx::query_as(
            "SELECT id, status, statute, section_number, times_cited, first_cited_at, last_cited_at \
             FROM case_citations WHERE case_id = ? AND kind = 'statute' \
             ORDER BY statute COLLATE NOCASE, section_number",
        )
        .bind(&case_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let statutes: Vec<Value> = statute_rows
        .into_iter()
        .map(|(id, status, statute, section, times_cited, first, last)| {
            json!({
                "id": id,
                "kind": "statute",
                "status": status,
                "statute": statute,
                "section_number": section,
                "times_cited": times_cited,
                "first_cited_at": first,
                "last_cited_at": last,
            })
        })
        .collect();

    Ok(Json(json!({ "judgments": judgments, "statutes": statutes })))
}

// ---------------------------------------------------------------------------
// DELETE /cases/:id/citations/:citation_id
// ---------------------------------------------------------------------------

async fn delete_citation(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path((case_id, citation_id)): Path<(String, String)>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let result = sqlx::query("DELETE FROM case_citations WHERE id = ? AND case_id = ?")
        .bind(&citation_id)
        .bind(&case_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Citation not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/resolve-refs — substitute @party / #annexure handles
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ResolveRefsBody {
    markdown: String,
}

async fn resolve_refs(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
    Json(body): Json<ResolveRefsBody>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let resolved =
        crate::drafting::crossrefs::resolve_crossrefs(&state.db, &case_id, &body.markdown).await;

    Ok(Json(json!({
        "markdown": resolved.markdown,
        "unresolved": resolved.unresolved,
    })))
}

// ---------------------------------------------------------------------------
// Persist a deterministic bibliography (cases-referred / authorities) as a
// case_outputs row + a .docx document, mirroring outputs.rs::persist_output.
// ---------------------------------------------------------------------------

async fn persist_bibliography(
    state: &AppState,
    user_id: &str,
    case_id: &str,
    output_type: &str,
    title: &str,
    content_md: &str,
) -> ApiResult {
    let docx_bytes = crate::pdf::docx_writer::markdown_to_docx(title, content_md)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let doc_id = uuid::Uuid::new_v4().to_string();
    let output_id = uuid::Uuid::new_v4().to_string();
    let storage_path = format!("documents/{user_id}/{doc_id}");

    let storage = crate::storage::make_storage()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    storage
        .put(
            &storage_path,
            &docx_bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let size = docx_bytes.len() as i64;
    let now = chrono::Utc::now().to_rfc3339();
    let filename = format!("{title}.docx");

    sqlx::query(
        "INSERT INTO documents (id, user_id, project_id, filename, file_type, size_bytes, storage_path, status) \
         VALUES (?, ?, NULL, ?, 'docx', ?, ?, 'ready')",
    )
    .bind(&doc_id)
    .bind(user_id)
    .bind(&filename)
    .bind(size)
    .bind(&storage_path)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    sqlx::query(
        "INSERT INTO case_outputs (id, case_id, output_type, content_md, docx_document_id, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&output_id)
    .bind(case_id)
    .bind(output_type)
    .bind(content_md)
    .bind(&doc_id)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "output_id": output_id,
        "doc_id": doc_id,
        "content_md": content_md,
    })))
}

// ---------------------------------------------------------------------------
// POST /cases/:id/outputs/cases-referred
// ---------------------------------------------------------------------------

async fn generate_cases_referred(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let md = crate::drafting::citations::render_cases_referred(&state.db, &case_id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    persist_bibliography(
        &state,
        &auth.user_id,
        &case_id,
        "cases_referred",
        "List of Cases Referred",
        &md,
    )
    .await
}

// ---------------------------------------------------------------------------
// POST /cases/:id/outputs/authorities
// ---------------------------------------------------------------------------

async fn generate_authorities(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(case_id): Path<String>,
) -> ApiResult {
    verify_case_ownership(&state, &case_id, &auth.user_id).await?;

    let md = crate::drafting::citations::render_authorities(&state.db, &case_id)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    persist_bibliography(
        &state,
        &auth.user_id,
        &case_id,
        "list_of_authorities",
        "List of Authorities",
        &md,
    )
    .await
}
