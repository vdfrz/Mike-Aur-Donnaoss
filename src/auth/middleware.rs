use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde_json::json;
use std::sync::Arc;

use crate::AppState;

/// The authenticated local user, extracted from a valid session token.
#[derive(Clone, Debug)]
pub struct AuthUser {
    pub user_id: String,
    pub username: String,
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Bypass auth entirely when MIKE_BYPASS_AUTH=true (used by word-addin).
        // This is safe only on a local machine — do NOT set in production/Tauri builds.
        if std::env::var("MIKE_BYPASS_AUTH").as_deref() == Ok("true") {
            return Ok(AuthUser {
                user_id: "local-user".into(),
                username: "local-user".into(),
            });
        }

        let auth = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        if !auth.starts_with("Bearer ") {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"detail": "Missing or invalid session token"})),
            )
                .into_response());
        }

        let token = auth[7..].trim();
        match state.sessions.validate(token).await {
            Ok(Some(session)) => {
                // Fetch username from DB
                let row: Option<(String,)> =
                    sqlx::query_as("SELECT username FROM user_profiles WHERE id = ?")
                        .bind(&session.user_id)
                        .fetch_optional(&state.db)
                        .await
                        .unwrap_or(None);
                let username = row.map(|r| r.0).unwrap_or_default();
                Ok(AuthUser { user_id: session.user_id, username })
            }
            _ => Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"detail": "Invalid or expired session"})),
            )
                .into_response()),
        }
    }
}
