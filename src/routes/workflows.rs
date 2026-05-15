use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use crate::{auth::middleware::AuthUser, AppState};

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_workflows).post(create_workflow))
        .route("/hidden", get(list_hidden).post(hide_workflow))
        .route("/hidden/{id}", axum::routing::delete(unhide_workflow))
        .route(
            "/{id}",
            get(get_workflow)
                .put(update_workflow)
                .patch(update_workflow)
                .delete(delete_workflow),
        )
}

/// Tuple shape returned by the SELECT statements below. Wrapping it in a
/// type alias keeps the long row signatures readable.
type WorkflowRow = (
    String,         // id
    String,         // user_id
    String,         // title
    Option<String>, // prompt_md (NULL if blank string)
    String,         // type
    Option<String>, // practice
    String,         // columns_config (JSON text, parsed on the way out)
    String,         // created_at
    String,         // updated_at
);

const SELECT_COLS: &str =
    "id, user_id, title, NULLIF(prompt_md, '') AS prompt_md, type, practice, columns_config, created_at, updated_at";

fn row_to_json(row: WorkflowRow, current_user: &str) -> Value {
    let (id, user_id, title, prompt_md, ty, practice, columns_config, created_at, _updated_at) =
        row;
    let cols: Value = serde_json::from_str(&columns_config).unwrap_or_else(|_| json!([]));
    let is_owner = user_id == current_user;
    json!({
        "id": id,
        "user_id": user_id,
        "title": title,
        "type": ty,
        "prompt_md": prompt_md,
        "columns_config": cols,
        "practice": practice,
        "created_at": created_at,
        "is_system": false,
        "is_owner": is_owner,
    })
}

// ---------------------------------------------------------------------------
// GET /workflow?type=assistant|tabular
// ---------------------------------------------------------------------------
async fn list_workflows(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult {
    // Optional filter on workflow type. The frontend always sets this via
    // listWorkflows(type), so we honour it when present.
    let type_filter: Option<String> = params
        .get("type")
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let rows: Vec<WorkflowRow> = if let Some(t) = type_filter {
        sqlx::query_as(&format!(
            "SELECT {SELECT_COLS} FROM workflows \
             WHERE user_id = ? AND type = ? ORDER BY updated_at DESC"
        ))
        .bind(&auth.user_id)
        .bind(t)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    } else {
        sqlx::query_as(&format!(
            "SELECT {SELECT_COLS} FROM workflows \
             WHERE user_id = ? ORDER BY updated_at DESC"
        ))
        .bind(&auth.user_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?
    };

    let workflows: Vec<Value> = rows
        .into_iter()
        .map(|r| row_to_json(r, &auth.user_id))
        .collect();

    Ok(Json(json!({ "workflows": workflows })))
}

// ---------------------------------------------------------------------------
// POST /workflow
// Body: { title, type?, prompt_md?, practice?, columns_config? }
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct CreateWorkflowBody {
    title: String,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    prompt_md: Option<String>,
    #[serde(default)]
    practice: Option<String>,
    #[serde(default)]
    columns_config: Option<Value>,
}

async fn create_workflow(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateWorkflowBody>,
) -> ApiResult {
    if body.title.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "Workflow title cannot be empty"));
    }
    let id = uuid::Uuid::new_v4().to_string();

    // Default to "assistant" when the client omits the type — the modal
    // always sends one, but other call sites (built-in promotion, future
    // import flows) might not.
    let ty = body.r#type.unwrap_or_else(|| "assistant".to_string());
    if ty != "assistant" && ty != "tabular" {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "type must be 'assistant' or 'tabular'",
        ));
    }

    // Empty-string sentinel for prompt_md keeps the original NOT NULL
    // constraint happy without requiring a destructive table rebuild.
    // The SELECT side rewrites empty strings back to NULL via NULLIF()
    // so the frontend sees a real Option<String>.
    let prompt_md = body.prompt_md.unwrap_or_default();

    let cols_text = body
        .columns_config
        .map(|v| v.to_string())
        .unwrap_or_else(|| "[]".to_string());

    sqlx::query(
        "INSERT INTO workflows (id, user_id, title, prompt_md, type, practice, columns_config) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(body.title.trim())
    .bind(&prompt_md)
    .bind(&ty)
    .bind(&body.practice)
    .bind(&cols_text)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let row: WorkflowRow = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM workflows WHERE id = ?"
    ))
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(row_to_json(row, &auth.user_id)))
}

// ---------------------------------------------------------------------------
// GET /workflow/:id
// ---------------------------------------------------------------------------
async fn get_workflow(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<WorkflowRow> = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM workflows WHERE id = ? AND user_id = ?"
    ))
    .bind(&id)
    .bind(&auth.user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let row = row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Workflow not found"))?;
    Ok(Json(row_to_json(row, &auth.user_id)))
}

// ---------------------------------------------------------------------------
// PUT|PATCH /workflow/:id
// Body: { title?, prompt_md?, practice?, columns_config? }
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct UpdateWorkflowBody {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    prompt_md: Option<String>,
    #[serde(default)]
    practice: Option<String>,
    #[serde(default)]
    columns_config: Option<Value>,
}

async fn update_workflow(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<UpdateWorkflowBody>,
) -> ApiResult {
    let cols_text: Option<String> = body.columns_config.map(|v| v.to_string());

    let result = sqlx::query(
        "UPDATE workflows SET \
           title          = COALESCE(?, title), \
           prompt_md      = COALESCE(?, prompt_md), \
           practice       = COALESCE(?, practice), \
           columns_config = COALESCE(?, columns_config), \
           updated_at = datetime('now') \
         WHERE id = ? AND user_id = ?",
    )
    .bind(&body.title)
    .bind(&body.prompt_md)
    .bind(&body.practice)
    .bind(&cols_text)
    .bind(&id)
    .bind(&auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Workflow not found"));
    }

    let row: WorkflowRow = sqlx::query_as(&format!(
        "SELECT {SELECT_COLS} FROM workflows WHERE id = ?"
    ))
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(row_to_json(row, &auth.user_id)))
}

// ---------------------------------------------------------------------------
// DELETE /workflow/:id
// ---------------------------------------------------------------------------
async fn delete_workflow(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let result = sqlx::query("DELETE FROM workflows WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Workflow not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// GET /workflow/hidden
// ---------------------------------------------------------------------------
async fn list_hidden(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> ApiResult {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT workflow_id FROM workflow_hidden WHERE user_id = ?")
            .bind(&auth.user_id)
            .fetch_all(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let ids: Vec<String> = rows.into_iter().map(|(id,)| id).collect();
    Ok(Json(json!(ids)))
}

// ---------------------------------------------------------------------------
// POST /workflow/hidden  — Body: { workflow_id }
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct HideWorkflowBody {
    workflow_id: String,
}

async fn hide_workflow(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<HideWorkflowBody>,
) -> ApiResult {
    sqlx::query(
        "INSERT OR IGNORE INTO workflow_hidden (user_id, workflow_id) VALUES (?, ?)",
    )
    .bind(&auth.user_id)
    .bind(&body.workflow_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// DELETE /workflow/hidden/:id
// ---------------------------------------------------------------------------
async fn unhide_workflow(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    sqlx::query("DELETE FROM workflow_hidden WHERE user_id = ? AND workflow_id = ?")
        .bind(&auth.user_id)
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "ok": true })))
}
