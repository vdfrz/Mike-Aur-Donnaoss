//! eCourts manual-verification routes.
//!
//! When the user clicks "Verify on eCourts" against a Kanoon citation
//! in chat, the frontend opens judgments.ecourts.gov.in/pdfsearch in a
//! new window. The user solves the CAPTCHA, finds the matching case,
//! and pastes the canonical case number back into Mike. The frontend
//! then POSTs that outcome here so it persists across sessions.
//!
//! The actual eCourts portal interaction happens entirely in the
//! frontend — this module only stores the user's recorded outcomes.
//! Mike NEVER tries to scrape, automate, or bypass eCourts on the
//! server side: only the user-in-the-loop result lands here.
//!
//!   POST   /ecourts-verify             — record a verification outcome
//!   GET    /ecourts-verify/:kanoon_tid — latest outcome for a case
//!   DELETE /ecourts-verify/:kanoon_tid — clear an outcome (audit reset)

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", post(record_verification))
        .route("/{kanoon_tid}", get(latest_for_tid).delete(clear_for_tid))
}

#[derive(Deserialize)]
struct VerifyPayload {
    kanoon_tid: i64,
    kanoon_title: String,
    #[serde(default)]
    kanoon_court: Option<String>,
    #[serde(default)]
    kanoon_decision_date: Option<String>,
    /// One of: "verified" | "not_found" | "pending"
    status: String,
    #[serde(default)]
    ecourts_case_number: Option<String>,
    #[serde(default)]
    ecourts_pdf_url: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Serialize)]
struct VerifyRecord {
    id: String,
    kanoon_tid: i64,
    kanoon_title: String,
    kanoon_court: Option<String>,
    kanoon_decision_date: Option<String>,
    status: String,
    ecourts_case_number: Option<String>,
    ecourts_pdf_url: Option<String>,
    notes: Option<String>,
    verified_at: String,
}

async fn record_verification(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<VerifyPayload>,
) -> ApiResult {
    // Validate status — defense in depth even though SQLite has CHECK.
    if !matches!(body.status.as_str(), "verified" | "not_found" | "pending") {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "status must be one of: verified, not_found, pending",
        ));
    }
    // For 'verified' status, a case number is required.
    if body.status == "verified"
        && body
            .ecourts_case_number
            .as_ref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(true)
    {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "ecourts_case_number is required when status is 'verified'",
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO ecourts_verifications \
            (id, user_id, kanoon_tid, kanoon_title, kanoon_court, kanoon_decision_date, \
             status, ecourts_case_number, ecourts_pdf_url, notes) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(body.kanoon_tid)
    .bind(&body.kanoon_title)
    .bind(&body.kanoon_court)
    .bind(&body.kanoon_decision_date)
    .bind(&body.status)
    .bind(&body.ecourts_case_number)
    .bind(&body.ecourts_pdf_url)
    .bind(&body.notes)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({
        "id": id,
        "kanoon_tid": body.kanoon_tid,
        "status": body.status,
        "ecourts_case_number": body.ecourts_case_number,
    })))
}

async fn latest_for_tid(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(tid): Path<i64>,
) -> ApiResult {
    let row: Option<(
        String, i64, String, Option<String>, Option<String>,
        String, Option<String>, Option<String>, Option<String>, String,
    )> = sqlx::query_as(
        "SELECT id, kanoon_tid, kanoon_title, kanoon_court, kanoon_decision_date, \
                status, ecourts_case_number, ecourts_pdf_url, notes, verified_at \
         FROM ecourts_verifications \
         WHERE user_id = ? AND kanoon_tid = ? \
         ORDER BY verified_at DESC LIMIT 1",
    )
    .bind(&auth.user_id)
    .bind(tid)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let Some((id, kanoon_tid, kanoon_title, kanoon_court, kanoon_decision_date,
        status, ecourts_case_number, ecourts_pdf_url, notes, verified_at)) = row else {
        return Ok(Json(json!({ "verification": null })));
    };

    let record = VerifyRecord {
        id, kanoon_tid, kanoon_title, kanoon_court, kanoon_decision_date,
        status, ecourts_case_number, ecourts_pdf_url, notes, verified_at,
    };
    Ok(Json(json!({ "verification": record })))
}

async fn clear_for_tid(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(tid): Path<i64>,
) -> ApiResult {
    sqlx::query(
        "DELETE FROM ecourts_verifications WHERE user_id = ? AND kanoon_tid = ?",
    )
    .bind(&auth.user_id)
    .bind(tid)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    Ok(Json(json!({ "ok": true, "kanoon_tid": tid })))
}
