//! Session 2 — RAG part 1: semantic FIND over the firm corpus.
//!
//! The `search_firm_corpus` agent tool used to be FTS5/BM25-only: a query had
//! to share keywords with a chunk to surface it. This test proves the new
//! HYBRID path (vector-KNN ∪ BM25, re-ranked) recovers the right passage even
//! when the query shares **no** keywords with it — the whole point of semantic
//! search.
//!
//! Like `onnx_inference.rs`, the live-model test downloads ~280 MB of
//! multilingual-e5-base weights on first run, so it's `#[ignore]`d and run
//! explicitly:
//!
//!     cargo test --features rag --test corpus_semantic_search -- --ignored --nocapture
//!
//! The migration/wiring is also covered by an in-module deterministic test in
//! `src/corpus/tools.rs` (fabricated vectors, no model) that runs under a
//! plain `cargo test`.

#![cfg(feature = "rag")]

use mike::corpus::ingest::index_corpus_chunks;
use mike::corpus::tools::hybrid_search_firm_corpus;
use mike::embeddings::{register_sqlite_vec_auto_extension, EmbeddingService};
use serde_json::{json, Value};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

/// In-memory pool with the FULL migration suite applied (so the corpus tables,
/// their FTS5 mirror, and the new `corpus_chunks_vec` table all exist) and the
/// sqlite-vec extension loaded. Single connection so the in-memory schema is
/// visible to every query in the pool.
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

/// Insert a user + a ready corpus file + one chunk.
async fn seed_chunk(
    pool: &SqlitePool,
    user_id: &str,
    file_id: &str,
    filename: &str,
    text: &str,
) {
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
    .bind(filename)
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
    .bind(text)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "downloads ~280MB ONNX model on first run; run with --ignored"]
async fn corpus_semantic_search() {
    let pool = setup_db().await;
    let svc = EmbeddingService::new(pool.clone());
    let user = "user-1";

    // The semantically-correct chunk: an air-travel injury / compensation
    // passage. The unrelated chunk: a cooking recipe. Crucially, the QUERY
    // below shares ZERO tokens with EITHER chunk — so BM25 alone returns
    // nothing and only the vector half can surface the right passage.
    seed_chunk(
        &pool,
        user,
        "file-air",
        "carriage_by_air.txt",
        "Damages payable to an air traveller hurt while flying, claimed against \
         the carrier under the limits of the Convention governing such carriage.",
    )
    .await;
    seed_chunk(
        &pool,
        user,
        "file-food",
        "recipe.txt",
        "Two cups flour, a pinch salt, fresh basil leaves; knead, rest, then \
         bake twenty minutes until golden.",
    )
    .await;

    // Embed both files' chunks into corpus_chunks_vec (the ingest hook).
    index_corpus_chunks(&svc, &pool, user, "file-air").await.unwrap();
    index_corpus_chunks(&svc, &pool, user, "file-food").await.unwrap();

    // Query worded entirely differently from the air-travel chunk (no shared
    // keyword), yet semantically about the same thing.
    let args = json!({ "query": "compensation for aviation accident victims suing airlines" });
    let out = hybrid_search_firm_corpus(&pool, &svc, user, &args).await;
    let v: Value = serde_json::from_str(&out).expect("tool returns JSON");

    let results = v["results"].as_array().expect("results array");
    assert!(
        !results.is_empty(),
        "hybrid search must surface the air-travel chunk despite no keyword overlap; got: {out}"
    );

    let top_text = results[0]["text"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        top_text.contains("carrier")
            || top_text.contains("traveller")
            || top_text.contains("convention"),
        "top hit should be the air-travel chunk, got: {}",
        results[0]["text"]
    );

    // And the recipe must rank strictly below it (if present at all).
    if let Some(second) = results.get(1) {
        let second_text = second["text"].as_str().unwrap_or_default().to_lowercase();
        assert!(
            second_text.contains("flour") || second_text.contains("basil"),
            "the unrelated recipe chunk should rank below the air-travel chunk"
        );
    }
}
