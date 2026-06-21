//! Session 3 — RAG part 2: agentic INGEST distillation + profile store.
//!
//! Proves the offline-safe contract of the distillation pass against the real
//! 0044 migration:
//!   * an ONLINE import (the model's JSON output is mocked by a canned string)
//!     writes a `corpus_profiles` row with the distilled fields;
//!   * an OFFLINE import (no model output -> `None`) writes NO profile row,
//!     leaves the file's chunks intact, and returns Ok without error.
//!
//! Runs under a plain `cargo test` (the default feature set includes `rag`).

#![cfg(feature = "rag")]

use mike::corpus::ingest::persist_profile_from_raw;
use mike::embeddings::register_sqlite_vec_auto_extension;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

/// In-memory pool with the FULL migration suite applied (so the corpus tables
/// and the new `corpus_profiles` table all exist) and the sqlite-vec extension
/// loaded (0043 needs it). Single connection so the in-memory schema is visible
/// to every query in the pool.
async fn setup_db() -> SqlitePool {
    register_sqlite_vec_auto_extension();
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("open in-memory pool");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    pool
}

/// Insert a user + a ready corpus file + one chunk (so we can assert chunks
/// survive an offline import).
async fn seed_file_with_chunk(pool: &SqlitePool, user_id: &str, file_id: &str) {
    sqlx::query("INSERT OR IGNORE INTO user_profiles (id, username, pin_hash) VALUES (?, ?, 'x')")
        .bind(user_id)
        .bind(user_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO corpus_files (id, user_id, filename, file_type, sha256, status) \
         VALUES (?, ?, ?, 'txt', ?, 'ready')",
    )
    .bind(file_id)
    .bind(user_id)
    .bind(format!("{file_id}.txt"))
    .bind(format!("sha-{file_id}"))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO corpus_chunks (file_id, user_id, seq, heading, section_role, page, text) \
         VALUES (?, ?, 0, NULL, 'argument', NULL, ?)",
    )
    .bind(file_id)
    .bind(user_id)
    .bind("MOST RESPECTFULLY SHEWETH that the complainant is a consumer.")
    .execute(pool)
    .await
    .unwrap();
}

const MOCK_MODEL_JSON: &str = r#"```json
{"summary":"A consumer complaint under the Consumer Protection Act.",
 "structure":["Title and parties","Facts","Grounds","Relief sought","Verification"],
 "style_notes":"Formal register; numbered paragraphs; opens with 'MOST RESPECTFULLY SHEWETH'.",
 "reusable_phrases":["MOST RESPECTFULLY SHEWETH","It is therefore most respectfully prayed"]}
```"#;

#[tokio::test]
async fn online_import_writes_profile() {
    let pool = setup_db().await;
    let (user, file) = ("user-1", "file-1");
    seed_file_with_chunk(&pool, user, file).await;

    let wrote = persist_profile_from_raw(&pool, user, file, Some("petition"), "mock-model", Some(MOCK_MODEL_JSON))
        .await
        .expect("persist must not error");
    assert!(wrote, "online distillation should write a profile row");

    let row: Option<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT file_id, summary, structure, reusable_phrases FROM corpus_profiles WHERE file_id = ?",
    )
    .bind(file)
    .fetch_optional(&pool)
    .await
    .unwrap();
    let row = row.expect("a profile row must exist");
    assert_eq!(row.0, file);
    assert!(row.1.as_deref().unwrap_or_default().contains("Consumer Protection"));
    assert!(row.2.as_deref().unwrap_or_default().contains("Relief sought"));
    assert!(row.3.as_deref().unwrap_or_default().contains("MOST RESPECTFULLY SHEWETH"));
}

#[tokio::test]
async fn offline_import_writes_chunks_no_profile() {
    let pool = setup_db().await;
    let (user, file) = ("user-1", "file-1");
    seed_file_with_chunk(&pool, user, file).await;

    // Offline: the model call yielded nothing.
    let wrote = persist_profile_from_raw(&pool, user, file, Some("petition"), "gemini-2.0-flash", None)
        .await
        .expect("offline persist must not error");
    assert!(!wrote, "offline must write no profile");

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM corpus_profiles WHERE file_id = ?")
        .bind(file)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "no profile row offline");

    // Chunks (written before distillation) survive.
    let chunks: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM corpus_chunks WHERE file_id = ?")
        .bind(file)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(chunks > 0, "offline import still has its chunks");
}
