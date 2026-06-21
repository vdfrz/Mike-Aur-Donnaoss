//! Integrity test for POST /document/:id/save-markdown.
//!
//! Reproduces the "save-markdown swallows the storage_path pointer-flip
//! failure" finding (documents.rs:553): the load-bearing UPDATE that points
//! the document at the freshly-written version bytes had its Result
//! discarded with `let _ = …`, so a failed flip still returned 200 with a
//! download_url while GET /document/:id/docx kept serving the OLD bytes.
//!
//! Here we force ONLY that UPDATE to fail (a BEFORE UPDATE trigger that
//! raises ABORT on `documents`) — the prior ownership SELECT and the
//! storage `put` still succeed — and assert the handler surfaces a 500
//! instead of reporting success.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use mike::AppState;
use serde_json::{json, Value};
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Arc;
use tower::ServiceExt; // for `oneshot`

async fn fresh_app() -> (axum::Router, Arc<AppState>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("docs.db");
    let url = format!(
        "sqlite://{}?mode=rwc",
        db_path.display().to_string().replace('\\', "/")
    );

    // Storage backend: point at a tempdir so make_storage() resolves to
    // LocalStorage and the version `put` actually succeeds (we want the
    // FLIP to be the only failing step, not the write before it).
    let storage_dir = tempfile::tempdir().expect("storage tempdir");
    // SAFETY: tests in this file run single-threaded relative to env reads
    // (make_storage reads STORAGE_PATH at call time inside the handler we
    // drive synchronously); no other thread mutates the environment here.
    unsafe {
        std::env::set_var("STORAGE_PATH", storage_dir.path());
    }

    #[cfg(feature = "rag")]
    mike::embeddings::register_sqlite_vec_auto_extension();

    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect sqlite");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");

    let sessions = mike::auth::SessionStore::new(pool.clone());
    let state = AppState {
        db: pool,
        sessions,
        biometric_tx: None,
        no_tools_models: Default::default(),
        mcp_discovery_cache: Default::default(),
        client_tool_tx: Default::default(),
        #[cfg(feature = "rag")]
        embeddings: None,
        #[cfg(feature = "rag")]
        scans: Default::default(),
        #[cfg(feature = "rag")]
        ik_reindex: Arc::new(tokio::sync::RwLock::new(Default::default())),
    };
    let state = Arc::new(state);

    let app = axum::Router::new()
        .nest("/document", mike::routes::documents::router())
        .with_state(state.clone());

    // Keep the tempdirs alive for the duration of the test.
    std::mem::forget(dir);
    std::mem::forget(storage_dir);
    (app, state)
}

async fn make_user_and_token(state: &AppState) -> String {
    let user_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO user_profiles (id, username, display_name, pin_hash) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(&user_id)
    .bind(format!("doc-{}", &user_id[..8]))
    .bind("Doc")
    .bind("dummy-not-a-real-hash")
    .execute(&state.db)
    .await
    .expect("insert user");

    state.sessions.create(&user_id).await.expect("create session")
}

/// Insert a document row owned by `user_id` and return its id.
async fn insert_document(state: &AppState, token: &str) -> String {
    // Resolve the user_id from the session token.
    let user_id: String =
        sqlx::query_scalar("SELECT user_id FROM sessions WHERE token = ?")
            .bind(token)
            .fetch_one(&state.db)
            .await
            .expect("resolve user from session");

    let doc_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO documents (id, user_id, filename, file_type, size_bytes, storage_path, status) \
         VALUES (?, ?, ?, ?, ?, ?, 'ready')",
    )
    .bind(&doc_id)
    .bind(&user_id)
    .bind("affidavit.docx")
    .bind("docx")
    .bind(1234_i64)
    .bind(format!("original/{doc_id}"))
    .execute(&state.db)
    .await
    .expect("insert document");

    doc_id
}

#[tokio::test]
async fn save_markdown_surfaces_failed_pointer_flip() {
    let (app, state) = fresh_app().await;
    let token = make_user_and_token(&state).await;
    let doc_id = insert_document(&state, &token).await;

    // Force the load-bearing pointer-flip UPDATE to fail while leaving the
    // ownership SELECT (and the storage put) working: a BEFORE UPDATE
    // trigger that aborts every UPDATE on `documents`.
    sqlx::query(
        "CREATE TRIGGER block_doc_update BEFORE UPDATE ON documents \
         BEGIN SELECT RAISE(ABORT, 'pointer flip blocked'); END;",
    )
    .execute(&state.db)
    .await
    .expect("install trigger");

    let body = json!({ "markdown": "# Edited\n\nNew body text." });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/document/{doc_id}/save-markdown"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    // With the bug (`let _ = …`) the handler ignores the failed UPDATE and
    // returns 200 with a download_url; with the fix it propagates a 500.
    assert_eq!(
        resp.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "a failed storage_path pointer-flip must surface as an error, not a successful save"
    );

    // And the failed save must NOT have left a version row claiming success.
    let version_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM document_versions WHERE document_id = ?")
            .bind(&doc_id)
            .fetch_one(&state.db)
            .await
            .expect("count versions");
    assert_eq!(
        version_count, 0,
        "no version row should be recorded when the pointer-flip failed"
    );
}

#[tokio::test]
async fn save_markdown_succeeds_when_flip_works() {
    // Guards the happy path so the error-propagation fix doesn't break it:
    // a normal save returns 200, flips storage_path, and records a version.
    let (app, state) = fresh_app().await;
    let token = make_user_and_token(&state).await;
    let doc_id = insert_document(&state, &token).await;

    let body = json!({ "markdown": "# Edited\n\nNew body text." });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/document/{doc_id}/save-markdown"))
                .header("Authorization", format!("Bearer {token}"))
                .header("Content-Type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK, "normal save should succeed");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["document_id"], doc_id);
    assert_eq!(v["version_number"], 1);

    // The pointer-flip actually happened.
    let storage_path: String =
        sqlx::query_scalar("SELECT storage_path FROM documents WHERE id = ?")
            .bind(&doc_id)
            .fetch_one(&state.db)
            .await
            .expect("fetch storage_path");
    assert!(
        storage_path.starts_with("versions/"),
        "storage_path should be flipped to the new version, got {storage_path}"
    );

    let version_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM document_versions WHERE document_id = ?")
            .bind(&doc_id)
            .fetch_one(&state.db)
            .await
            .expect("count versions");
    assert_eq!(version_count, 1, "one version row should be recorded");
}
