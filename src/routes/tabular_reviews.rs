use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
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
        .route("/", get(list_tabular_reviews).post(create_tabular_review))
        .route("/{id}", get(get_tabular_review).delete(delete_tabular_review))
}

// ---------------------------------------------------------------------------
// GET /tabular-review?project_id=...
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct ListQuery {
    project_id: Option<String>,
}

async fn list_tabular_reviews(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult {
    let rows: Vec<(String, String, Option<String>, Option<String>, String, String, String)> =
        if let Some(ref pid) = q.project_id {
            sqlx::query_as(
                "SELECT id, title, project_id, workflow_id, columns_config, created_at, updated_at \
                 FROM tabular_reviews WHERE user_id = ? AND project_id = ? ORDER BY updated_at DESC",
            )
            .bind(&auth.user_id)
            .bind(pid)
            .fetch_all(&state.db)
            .await
        } else {
            sqlx::query_as(
                "SELECT id, title, project_id, workflow_id, columns_config, created_at, updated_at \
                 FROM tabular_reviews WHERE user_id = ? ORDER BY updated_at DESC",
            )
            .bind(&auth.user_id)
            .fetch_all(&state.db)
            .await
        }
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let reviews: Vec<Value> = rows
        .into_iter()
        .map(|(id, title, project_id, workflow_id, columns_config, created_at, updated_at)| {
            json!({
                "id": id,
                "title": title,
                "project_id": project_id,
                "workflow_id": workflow_id,
                "columns_config": serde_json::from_str::<Value>(&columns_config).unwrap_or(json!([])),
                "created_at": created_at,
                "updated_at": updated_at
            })
        })
        .collect();

    Ok(Json(json!(reviews)))
}

// ---------------------------------------------------------------------------
// POST /tabular-review
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct CreateTabularReviewBody {
    title: Option<String>,
    project_id: Option<String>,
    workflow_id: Option<String>,
    columns_config: Option<Value>,
}

async fn create_tabular_review(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateTabularReviewBody>,
) -> ApiResult {
    let id = uuid::Uuid::new_v4().to_string();
    let title = body.title.unwrap_or_else(|| "Untitled Review".to_string());
    let columns_config = body.columns_config.unwrap_or(json!([])).to_string();

    sqlx::query(
        "INSERT INTO tabular_reviews (id, user_id, project_id, workflow_id, title, columns_config) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(&body.project_id)
    .bind(&body.workflow_id)
    .bind(&title)
    .bind(&columns_config)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "id": id, "title": title })))
}

// ---------------------------------------------------------------------------
// GET /tabular-review/:id
// ---------------------------------------------------------------------------
async fn get_tabular_review(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(String, String, Option<String>, Option<String>, String, String, String)> =
        sqlx::query_as(
            "SELECT id, title, project_id, workflow_id, columns_config, created_at, updated_at \
             FROM tabular_reviews WHERE id = ? AND user_id = ?",
        )
        .bind(&id)
        .bind(&auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (id, title, project_id, workflow_id, columns_config, created_at, updated_at) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Tabular review not found"))?;

    Ok(Json(json!({
        "id": id,
        "title": title,
        "project_id": project_id,
        "workflow_id": workflow_id,
        "columns_config": serde_json::from_str::<Value>(&columns_config).unwrap_or(json!([])),
        "created_at": created_at,
        "updated_at": updated_at
    })))
}

// ---------------------------------------------------------------------------
// DELETE /tabular-review/:id
// ---------------------------------------------------------------------------
async fn delete_tabular_review(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let result = sqlx::query("DELETE FROM tabular_reviews WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Tabular review not found"));
    }
    Ok(Json(json!({ "ok": true })))
}
