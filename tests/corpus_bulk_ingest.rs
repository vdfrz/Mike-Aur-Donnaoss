//! Folder bulk-upload — reversibility guarantees.
//!
//! The folder-drop feature must let a lawyer undo what they imported without
//! leaving orphans: deleting a file (or a whole batch) has to take its chunks
//! AND its embedding vectors with it, and must not touch sibling files. The
//! `corpus_chunks` FTS mirror cascades via FK, but `corpus_chunks_vec` is a
//! sqlite-vec virtual table that the cascade can't reach, so the delete path
//! clears it explicitly by file_id. These tests pin that exact contract — the
//! same statements `routes::corpus::purge_corpus_file` runs.
//!
//!     cargo test --features rag --test corpus_bulk_ingest

#![cfg(feature = "rag")]

use mike::embeddings::register_sqlite_vec_auto_extension;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

/// Raw little-endian f32 bytes — the blob layout sqlite-vec's `float[768]`
/// column expects (mirrors the crate-private `embeddings::service::vec_to_blob`).
fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

async fn setup_db() -> SqlitePool {
    register_sqlite_vec_auto_extension();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::from_str("sqlite::memory:")
                .unwrap()
                .create_if_missing(true),
        )
        .await
        .expect("open in-memory pool");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations");
    pool
}

/// Seed a ready corpus file with one chunk + its embedding vector, optionally
/// tagged into a batch.
async fn seed_file(pool: &SqlitePool, user: &str, file_id: &str, batch: Option<&str>) {
    sqlx::query("INSERT OR IGNORE INTO user_profiles (id, username, pin_hash) VALUES (?, ?, 'x')")
        .bind(user)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO corpus_files (id, user_id, filename, file_type, sha256, status, batch_id) \
         VALUES (?, ?, ?, 'txt', ?, 'ready', ?)",
    )
    .bind(file_id)
    .bind(user)
    .bind(format!("{file_id}.txt"))
    .bind(format!("sha-{file_id}"))
    .bind(batch)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO corpus_chunks (file_id, user_id, seq, section_role, text) \
         VALUES (?, ?, 0, 'argument', 'some indexed firm text')",
    )
    .bind(file_id)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    let chunk_id: i64 = sqlx::query_scalar("SELECT id FROM corpus_chunks WHERE file_id = ?")
        .bind(file_id)
        .fetch_one(pool)
        .await
        .unwrap();
    let mut v = vec![0.0_f32; 768];
    v[0] = 1.0;
    sqlx::query(
        "INSERT INTO corpus_chunks_vec (embedding, user_id, chunk_id, file_id) VALUES (?, ?, ?, ?)",
    )
    .bind(vec_to_blob(&v))
    .bind(user)
    .bind(chunk_id)
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
}

/// The exact purge the delete handlers run for one file.
async fn purge(pool: &SqlitePool, user: &str, file_id: &str) {
    sqlx::query("DELETE FROM workflows WHERE corpus_file_id = ? AND user_id = ?")
        .bind(file_id)
        .bind(user)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM corpus_chunks_vec WHERE file_id = ?")
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM corpus_files WHERE id = ? AND user_id = ?")
        .bind(file_id)
        .bind(user)
        .execute(pool)
        .await
        .unwrap();
}

async fn count(pool: &SqlitePool, sql: &str, file_id: &str) -> i64 {
    sqlx::query_scalar(sql).bind(file_id).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn delete_file_removes_chunks_and_vectors_no_orphans() {
    let pool = setup_db().await;
    let user = "u1";
    seed_file(&pool, user, "f1", None).await;

    // Precondition: chunk + vector present.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM corpus_chunks WHERE file_id = ?", "f1").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM corpus_chunks_vec WHERE file_id = ?", "f1").await, 1);

    purge(&pool, user, "f1").await;

    // The row, its chunks (FK cascade) and its vectors are all gone.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM corpus_files WHERE id = ?", "f1").await, 0);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM corpus_chunks WHERE file_id = ?", "f1").await, 0, "chunks must cascade");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM corpus_chunks_vec WHERE file_id = ?", "f1").await,
        0,
        "no orphan vectors may survive"
    );
}

#[tokio::test]
async fn delete_batch_removes_all_its_files_but_leaves_siblings() {
    let pool = setup_db().await;
    let user = "u1";
    // Two files in batch B, one unrelated individual file.
    seed_file(&pool, user, "b1", Some("batchB")).await;
    seed_file(&pool, user, "b2", Some("batchB")).await;
    seed_file(&pool, user, "solo", None).await;

    // Purge the whole batch (what DELETE /corpus/batches/{id} does).
    let ids: Vec<(String,)> =
        sqlx::query_as("SELECT id FROM corpus_files WHERE user_id = ? AND batch_id = ?")
            .bind(user)
            .bind("batchB")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ids.len(), 2, "both batch files are found");
    for (id,) in &ids {
        purge(&pool, user, id).await;
    }

    // The batch is gone with no orphan chunks/vectors.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM corpus_files WHERE batch_id = ?", "batchB").await, 0);
    let orphan_chunks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM corpus_chunks WHERE file_id IN ('b1','b2')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orphan_chunks, 0, "batch chunks must be gone");
    let orphan_vecs: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM corpus_chunks_vec WHERE file_id IN ('b1','b2')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orphan_vecs, 0, "batch vectors must be gone");

    // The unrelated individual file is untouched.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM corpus_files WHERE id = ?", "solo").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM corpus_chunks_vec WHERE file_id = ?", "solo").await, 1);
}
