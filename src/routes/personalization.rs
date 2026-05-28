use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::{
    auth::middleware::AuthUser,
    preferences::{CategoryContent, VALID_CATEGORIES},
    AppState,
};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/personalization", get(get_profile))
        .route("/personalization", put(put_profile))
        .route("/personalization", delete(delete_profile))
        .route("/preferences", get(get_all_preferences))
        .route("/preferences/{category}", put(put_preference))
        .route("/preferences/{category}", delete(delete_preference))
        .route("/cases/{case_id}/preferences", get(get_case_preferences))
        .route(
            "/cases/{case_id}/preferences/{category}",
            put(put_case_preference),
        )
}

// --- Legacy endpoints (unchanged, for frontend compat) ---

#[derive(Deserialize)]
struct ProfileIn {
    profile_text: String,
}

async fn get_profile(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT profile_text, updated_at FROM user_personalization WHERE user_id = ?",
    )
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(db_err)?;

    match row {
        Some((text, updated_at)) => Ok(Json(json!({
            "profile_text": text,
            "updated_at": updated_at,
        }))),
        None => Ok(Json(json!({
            "profile_text": "",
            "updated_at": null,
        }))),
    }
}

async fn put_profile(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<ProfileIn>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if body.profile_text.len() > 4000 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": "Profile text must be 4000 characters or fewer"})),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO user_personalization (user_id, profile_text, updated_at) \
         VALUES (?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE SET profile_text = excluded.profile_text, updated_at = excluded.updated_at",
    )
    .bind(&auth.user_id)
    .bind(&body.profile_text)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({
        "profile_text": body.profile_text,
        "updated_at": now,
    })))
}

async fn delete_profile(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    sqlx::query("DELETE FROM user_personalization WHERE user_id = ?")
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    Ok(StatusCode::NO_CONTENT)
}

/// Legacy: fetch raw profile text for backwards compat.
pub async fn fetch_profile_text(db: &sqlx::SqlitePool, user_id: &str) -> String {
    sqlx::query_as::<_, (String,)>(
        "SELECT profile_text FROM user_personalization WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .map(|(t,)| t)
    .unwrap_or_default()
}

// --- New categorical preference endpoints ---

async fn get_all_preferences(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT category, content_json, updated_at FROM user_preference_categories WHERE user_id = ?",
    )
    .bind(&auth.user_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let mut out = serde_json::Map::new();
    for &cat in VALID_CATEGORIES {
        let entry = rows.iter().find(|(c, _, _)| c == cat);
        match entry {
            Some((_, json_str, updated_at)) => {
                let content: serde_json::Value =
                    serde_json::from_str(json_str).unwrap_or(json!({}));
                out.insert(
                    cat.to_string(),
                    json!({"content": content, "updated_at": updated_at}),
                );
            }
            None => {
                out.insert(cat.to_string(), serde_json::Value::Null);
            }
        }
    }

    Ok(Json(serde_json::Value::Object(out)))
}

#[derive(Deserialize)]
struct PreferenceBody {
    #[serde(default)]
    critical_rules: Vec<String>,
    #[serde(default)]
    success_metrics: Vec<String>,
    #[serde(default)]
    anti_patterns: Vec<String>,
}

async fn put_preference(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(category): Path<String>,
    Json(body): Json<PreferenceBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !VALID_CATEGORIES.contains(&category.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": format!("Invalid category: {category}. Valid: {VALID_CATEGORIES:?}")})),
        ));
    }

    let content = CategoryContent {
        critical_rules: body.critical_rules,
        success_metrics: body.success_metrics,
        anti_patterns: body.anti_patterns,
    };
    let content_json = serde_json::to_string(&content).unwrap();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO user_preference_categories (user_id, category, content_json, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(user_id, category) DO UPDATE SET content_json = excluded.content_json, updated_at = excluded.updated_at",
    )
    .bind(&auth.user_id)
    .bind(&category)
    .bind(&content_json)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({"category": category, "content": content, "updated_at": now})))
}

async fn delete_preference(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(category): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    if !VALID_CATEGORIES.contains(&category.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": format!("Invalid category: {category}")})),
        ));
    }

    sqlx::query("DELETE FROM user_preference_categories WHERE user_id = ? AND category = ?")
        .bind(&auth.user_id)
        .bind(&category)
        .execute(&state.db)
        .await
        .map_err(db_err)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_case_preferences(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(case_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT category, content_json, updated_at FROM case_preferences WHERE case_id = ?",
    )
    .bind(&case_id)
    .fetch_all(&state.db)
    .await
    .map_err(db_err)?;

    let mut out = serde_json::Map::new();
    for (cat, json_str, updated_at) in &rows {
        let content: serde_json::Value = serde_json::from_str(json_str).unwrap_or(json!({}));
        out.insert(
            cat.clone(),
            json!({"content": content, "updated_at": updated_at}),
        );
    }

    Ok(Json(serde_json::Value::Object(out)))
}

async fn put_case_preference(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path((case_id, category)): Path<(String, String)>,
    Json(body): Json<PreferenceBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if !VALID_CATEGORIES.contains(&category.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"detail": format!("Invalid category: {category}")})),
        ));
    }

    let content = CategoryContent {
        critical_rules: body.critical_rules,
        success_metrics: body.success_metrics,
        anti_patterns: body.anti_patterns,
    };
    let content_json = serde_json::to_string(&content).unwrap();
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO case_preferences (case_id, category, content_json, updated_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(case_id, category) DO UPDATE SET content_json = excluded.content_json, updated_at = excluded.updated_at",
    )
    .bind(&case_id)
    .bind(&category)
    .bind(&content_json)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(db_err)?;

    Ok(Json(json!({"case_id": case_id, "category": category, "content": content, "updated_at": now})))
}

fn db_err(e: sqlx::Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"detail": format!("DB error: {e}")})),
    )
}
