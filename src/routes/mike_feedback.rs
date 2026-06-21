//! "Mike listens" — the self-rewriting drafting harness, HTTP surface.
//!
//! Ported from lavern's `proposal-feedback.ts`. The lawyer chats about how Mike
//! should draft; Mike triages the message, rewrites its learned drafting rules
//! live (streaming the diff), and queues any feature requests for the dev team.
//! Backed by the `harness_*` tables (migration 0041) and `crate::harness`.
//!
//! POST /mike-feedback/chat        multipart (message, history, optional files) → SSE
//! GET  /mike-feedback/history     rebuild the conversation thread
//! GET  /mike-feedback/lessons     the learned rules + counts
//! GET  /mike-feedback/generations harness generation + lineage
//! GET  /mike-feedback/features    the feature-request queue
//! POST /mike-feedback/features    add a feature request

use axum::{
    extract::{DefaultBodyLimit, Multipart, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::{convert::Infallible, path::Path, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::{auth::middleware::AuthUser, harness, routes::user::fetch_llm_settings, AppState};

const MAX_ATTACHMENT_TEXT: usize = 8000;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/chat", post(chat))
        .route("/history", get(history))
        .route("/lessons", get(lessons))
        .route("/generations", get(generations))
        .route("/features", get(list_features).post(add_feature))
        // Attachments (marked-up printouts, scans) ride the chat turn.
        .layer(DefaultBodyLimit::max(25 * 1024 * 1024))
}

// ── POST /chat (streaming) ───────────────────────────────────────────────────

async fn chat(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    mut multipart: Multipart,
) -> Response {
    let mut message = String::new();
    let mut history: Vec<(String, String)> = Vec::new();
    let mut attachments: Vec<(String, Vec<u8>)> = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("").to_string().as_str() {
            "message" => message = field.text().await.unwrap_or_default(),
            "history" => {
                let raw = field.text().await.unwrap_or_default();
                if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) {
                    for t in arr {
                        let role = t.get("role").and_then(|x| x.as_str()).unwrap_or("");
                        let text = t.get("text").and_then(|x| x.as_str()).unwrap_or("");
                        if (role == "you" || role == "assistant") && !text.is_empty() {
                            history.push((role.to_string(), text.to_string()));
                        }
                    }
                }
            }
            "files" => {
                let fname = field
                    .file_name()
                    .map(String::from)
                    .unwrap_or_else(|| "upload".to_string());
                if let Ok(bytes) = field.bytes().await {
                    attachments.push((fname, bytes.to_vec()));
                }
            }
            _ => {} // chatTurn etc. — client bookkeeping, ignored server-side
        }
    }

    let mut message = message.trim().to_string();

    // Fold any readable attachment text into the message the model triages.
    let mut attach_count = 0usize;
    for (fname, bytes) in &attachments {
        if let Ok((text, _)) = crate::sync::scanner::extract_text_dispatch(Path::new(fname), bytes) {
            if !text.trim().is_empty() {
                attach_count += 1;
                let snippet: String = text.trim().chars().take(MAX_ATTACHMENT_TEXT).collect();
                message.push_str(&format!("\n\n--- ATTACHMENT: {fname} ---\n{snippet}"));
            }
        }
    }
    if message.is_empty() && attach_count > 0 {
        message = "Apply the feedback in the attached document(s).".to_string();
    }
    if message.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Tell Mike what to change, or attach a marked-up document." })),
        )
            .into_response();
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    let user_id = auth.user_id.clone();
    tokio::spawn(async move {
        run_chat(state, user_id, message, history, attach_count, tx).await;
    });

    Sse::new(ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn run_chat(
    state: Arc<AppState>,
    user_id: String,
    message: String,
    history: Vec<(String, String)>,
    attach_count: usize,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    macro_rules! send {
        ($v:expr) => {
            let _ = tx.send(Ok(Event::default().data($v.to_string()))).await;
        };
    }

    // Persist the lawyer's turn (the raw message, sans attachment dumps).
    // Attachment-only turns have nothing left after the strip — store the
    // same placeholder the UI shows instead of an empty bubble.
    let shown = message
        .split("\n\n--- ATTACHMENT:")
        .next()
        .unwrap_or(&message)
        .trim()
        .to_string();
    let shown = if shown.is_empty() {
        "Apply the feedback in the attached document(s).".to_string()
    } else {
        shown
    };
    insert_feedback(&state.db, &user_id, "you", &shown, None, None).await;

    if attach_count > 0 {
        send!(json!({
            "type": "event", "agent": "Mike",
            "text": format!("Read {attach_count} attachment{}", if attach_count == 1 { "" } else { "s" }),
        }));
    }

    let settings = fetch_llm_settings(&state.db, &user_id).await.ok();
    let config = crate::llm::oneshot::config_from_settings(&settings);

    // Show the triage model what Mike already knows, so questions about
    // learned rules answer truthfully and retractions ground to real rules.
    let current_rules = harness::active_lessons(&state.db, &user_id, 12).await;
    let triage = harness::triage(&config, &message, &history, &current_rules).await;
    send!(json!({ "type": "reply", "text": triage.reply }));

    // Pure question — answer and stop. No rules, no harness rewrite.
    let has_signal = !triage.lessons.is_empty() || !triage.retract.is_empty();
    if triage.intent == "question" && !has_signal && triage.feature_requests.is_empty() {
        insert_feedback(&state.db, &user_id, "assistant", &triage.reply, None, None).await;
        send!(json!({ "type": "complete", "answered": true }));
        return;
    }

    // Queue feature requests (out of the way of drafting).
    let mut features_queued = 0;
    for fr in &triage.feature_requests {
        let id = uuid::Uuid::new_v4().to_string();
        if sqlx::query("INSERT INTO harness_features (id, user_id, request) VALUES (?, ?, ?)")
            .bind(&id)
            .bind(&user_id)
            .bind(fr)
            .execute(&state.db)
            .await
            .is_ok()
        {
            features_queued += 1;
        }
    }

    // The live harness rewrite: stream the diff as rules are added/retired.
    let mut generation: Option<i64> = None;
    if has_signal {
        let before = harness::current_generation(&state.db, &user_id).await;
        send!(json!({ "type": "harness", "phase": "start", "generation": before }));
        match harness::evolve(&state.db, &user_id, &triage).await {
            Ok(result) if !result.edits.is_empty() => {
                for edit in &result.edits {
                    send!(json!({ "type": "harness", "phase": "edit", "kind": edit.kind, "text": edit.text }));
                }
                send!(json!({ "type": "harness", "phase": "done", "generation": result.generation }));
                generation = Some(result.generation);
            }
            Ok(_) => {
                send!(json!({ "type": "harness", "phase": "skipped", "reason": "No new rules to apply this turn." }));
            }
            Err(_) => {
                send!(json!({ "type": "harness", "phase": "skipped", "reason": "Harness update failed; your feedback is still saved." }));
            }
        }
    }

    let note = match (generation, features_queued) {
        (Some(g), _) => Some(format!("Harness upgraded to generation {g}.")),
        (None, n) if n > 0 => Some(format!("{n} feature request{} queued.", if n == 1 { "" } else { "s" })),
        _ => None,
    };
    insert_feedback(
        &state.db,
        &user_id,
        "assistant",
        &triage.reply,
        note.as_deref(),
        generation,
    )
    .await;

    let lessons_ui: Vec<serde_json::Value> = harness::active_lessons(&state.db, &user_id, 6)
        .await
        .into_iter()
        .map(|l| json!({ "rule": l.rule, "kind": l.kind, "scope": {}, "effectiveness": l.effectiveness }))
        .collect();

    send!(json!({
        "type": "complete",
        "lessons": lessons_ui,
        "featureRequestsQueued": features_queued,
        "note": note,
    }));
}

async fn insert_feedback(
    db: &sqlx::SqlitePool,
    user_id: &str,
    role: &str,
    text: &str,
    note: Option<&str>,
    generation: Option<i64>,
) {
    let id = uuid::Uuid::new_v4().to_string();
    let _ = sqlx::query(
        "INSERT INTO harness_feedback (id, user_id, role, text, note, generation) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user_id)
    .bind(role)
    .bind(text)
    .bind(note)
    .bind(generation)
    .execute(db)
    .await;
}

// ── GET /history ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct HistoryQuery {
    limit: Option<i64>,
}

async fn history(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<HistoryQuery>,
) -> Json<serde_json::Value> {
    let limit = q.limit.unwrap_or(25).clamp(1, 100);
    let mut rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT role, text, note FROM harness_feedback WHERE user_id = ? ORDER BY created_at DESC, rowid DESC LIMIT ?",
    )
    .bind(&auth.user_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    rows.reverse(); // oldest first for display
    let turns: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(role, text, note)| json!({ "role": role, "text": text, "note": note }))
        .collect();
    Json(json!({ "turns": turns }))
}

// ── GET /lessons ─────────────────────────────────────────────────────────────

async fn lessons(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Json<serde_json::Value> {
    let selected: Vec<serde_json::Value> = harness::active_lessons(&state.db, &auth.user_id, 20)
        .await
        .into_iter()
        .map(|l| json!({ "rule": l.rule, "kind": l.kind, "scope": {}, "effectiveness": l.effectiveness }))
        .collect();
    let (total, active): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(CASE WHEN deprecated = 0 THEN 1 ELSE 0 END), 0) \
         FROM harness_lessons WHERE user_id = ?",
    )
    .bind(&auth.user_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0, 0));
    Json(json!({
        "selected": selected,
        "stats": { "total": total, "active": active, "deprecated": total - active },
    }))
}

// ── GET /generations ─────────────────────────────────────────────────────────

async fn generations(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Json<serde_json::Value> {
    let generation = harness::current_generation(&state.db, &auth.user_id).await;
    let compiled = harness::active_lessons(&state.db, &auth.user_id, 1000).await.len();
    let lineage: Vec<String> = (0..=generation).map(|n| format!("gen-{n:03}")).collect();
    Json(json!({
        "active": {
            "generation": generation,
            "planOverrides": [],
            "extraRules": 0,
            "compiledLessons": compiled,
        },
        "generations": lineage,
    }))
}

// ── /features ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FeaturesQuery {
    status: Option<String>,
}

async fn list_features(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<FeaturesQuery>,
) -> Json<serde_json::Value> {
    let status = q.status.unwrap_or_else(|| "open".to_string());
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, request, status, created_at FROM harness_features \
         WHERE user_id = ? AND status = ? ORDER BY created_at DESC",
    )
    .bind(&auth.user_id)
    .bind(&status)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let requests: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, request, status, created_at)| {
            json!({ "id": id, "request": request, "status": status, "createdAt": created_at })
        })
        .collect();
    Json(json!({ "requests": requests }))
}

#[derive(Deserialize)]
struct FeatureIn {
    request: String,
}

async fn add_feature(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<FeatureIn>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let request = body.request.trim();
    if request.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "A feature request can't be empty." })),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO harness_features (id, user_id, request) VALUES (?, ?, ?)")
        .bind(&id)
        .bind(&auth.user_id)
        .bind(request)
        .execute(&state.db)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
        })?;
    Ok(Json(json!({ "ok": true, "id": id })))
}
