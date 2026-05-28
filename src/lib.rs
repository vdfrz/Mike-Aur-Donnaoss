pub mod agents;
pub mod auth;
pub mod corpora;
pub mod db;
pub mod embeddings;
pub mod llm;
pub mod mcp;
pub mod mikeprj;
pub mod pdf;
pub mod pii;
pub mod preferences;
pub mod routes;
pub mod storage;
pub mod sync;

pub use db::AppState;

use axum::{Router, http::Method};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

/// Start the axum HTTP server on the given port.
/// Blocks until the server shuts down.
/// Intended to be called from a dedicated tokio task or thread.
pub use db::BiometricRequest;

pub async fn run_server(port: u16) -> anyhow::Result<()> {
    run_server_with_bio_tx(port, None).await
}

/// Load `.env` from a known-good location regardless of cwd.
///
/// Tauri spawns the bundled exe with `cwd = src-tauri/`, where there's no
/// `.env`. Plain `dotenvy::dotenv()` only checks cwd, so the env vars we
/// rely on (DATABASE_URL, STORAGE_PATH, …) silently failed to load and
/// the DB ended up wherever the relative fallback resolved to. We walk
/// up from both cwd and the executable directory until we find a `.env`.
fn load_dotenv() {
    fn try_walk_up(start: std::path::PathBuf) -> bool {
        let mut current: Option<std::path::PathBuf> = Some(start);
        while let Some(dir) = current {
            let candidate = dir.join(".env");
            if candidate.is_file() {
                if dotenvy::from_path(&candidate).is_ok() {
                    tracing::info!("[env] loaded {}", candidate.display());
                    return true;
                }
            }
            current = dir.parent().map(|p| p.to_path_buf());
        }
        false
    }

    if let Ok(cwd) = std::env::current_dir() {
        if try_walk_up(cwd) {
            return;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            try_walk_up(parent.to_path_buf());
        }
    }
}

/// Pin fastembed's model cache to a stable directory **outside the
/// workspace**, otherwise the ~280MB of `.part` chunks downloaded on
/// first run land under the cwd (= `src-tauri/` for Tauri dev) and
/// trigger the file watcher repeatedly during the download.
///
/// Honours `FASTEMBED_CACHE_DIR` if the user already set it in `.env`;
/// otherwise points at `<userdata>/mikerust-data/fastembed`. Either
/// way the directory is created so fastembed doesn't fail on first
/// `try_new`.
///
/// Called from `run_server_with_bio_tx` immediately after `load_dotenv`,
/// so the override takes effect before the embedding service spins up.
fn ensure_fastembed_cache_dir() {
    if std::env::var("FASTEMBED_CACHE_DIR").is_ok() {
        return;
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(home)
        .join("mikerust-data")
        .join("fastembed");
    let _ = std::fs::create_dir_all(&path);
    // SAFETY: single-threaded process startup before the runtime spins
    // up — no concurrent reads of std::env to race with.
    unsafe {
        std::env::set_var("FASTEMBED_CACHE_DIR", &path);
    }
    tracing::info!("[rag] fastembed cache pinned to {}", path.display());
}

pub async fn run_server_with_bio_tx(
    port: u16,
    biometric_tx: Option<tokio::sync::mpsc::Sender<BiometricRequest>>,
) -> anyhow::Result<()> {
    load_dotenv();
    ensure_fastembed_cache_dir();

    let mut state = AppState::new().await?;
    state.biometric_tx = biometric_tx;
    let state = Arc::new(state);
    state.run_migrations().await?;

    // Startup recovery: any document still flagged as `syncing` from a
    // previous session can't actually be in flight any more — there's
    // no embedding task running for it. Flip those rows to
    // `interrupted` so the UI surfaces the resync button instead of
    // leaving them stuck with a spinner that never moves.
    let recovered = sqlx::query(
        "UPDATE documents SET status = 'interrupted' WHERE status = 'syncing'",
    )
    .execute(&state.db)
    .await
    .map(|r| r.rows_affected())
    .unwrap_or(0);
    if recovered > 0 {
        tracing::info!(
            "[startup] recovered {recovered} doc(s) from stale 'syncing' state \
             → marked 'interrupted' (resync from the UI when ready)"
        );
    }

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::PATCH, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any);

    let app = Router::new()
        .nest("/auth",     routes::auth::router())
        .nest("/user",     routes::user::router())
        .nest("/chat",     routes::chat::router())
        .nest("/project",  routes::projects::router())
        .nest("/document", routes::documents::router())
        // Alias used by the upstream-Mike frontend for standalone documents.
        .nest("/single-documents", routes::documents::router())
        .nest("/workflow",  routes::workflows::router())
        .nest("/tabular-review", routes::tabular_reviews::router())
        .nest("/sync",     routes::sync::router())
        .nest("/eurlex",   routes::eurlex::router())
        .nest("/indian-kanoon", routes::indian_kanoon::router())
        .nest("/ecourts-verify", routes::ecourts::router())
        .nest("/cases", routes::cases::router())
        .nest("/messy-doc", routes::messy_doc::router())
        .merge(routes::personalization::router())
        .nest("/desktop",  routes::desktop::router())
        .layer(cors)
        .with_state(state);

    let addr = format!("127.0.0.1:{port}");
    tracing::info!("API listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
