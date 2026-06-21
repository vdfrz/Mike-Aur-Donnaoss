-- Semantic-search vector store for firm-corpus chunks (Session 2: RAG part 1).
--
-- Mirrors the `doc_chunks` vec0 table (migrations 0009/0011) but scoped to the
-- firm corpus. One row per `corpus_chunks` row, holding that chunk's
-- multilingual-e5-base embedding so `search_firm_corpus` can do hybrid
-- retrieval (vector-KNN ∪ FTS5/BM25, re-ranked) instead of keyword-only search.
--
-- Layout (vec0 syntax):
--   * `user_id` is a PARTITION KEY — sqlite-vec only allows WHERE filters on
--     partition keys during a KNN MATCH, and per-user partition pruning keeps
--     the cosine pass cheap. Every corpus query is already user-scoped.
--   * `embedding float[768]` — the e5 vector (same dim/metric as doc_chunks;
--     fastembed L2-normalises its output so the default metric ranks like cosine).
--   * `+chunk_id` — auxiliary link back to `corpus_chunks.id` (the join target
--     used to fetch the chunk text + file metadata for a hit).
--   * `+file_id` — auxiliary, so ingest can `DELETE ... WHERE file_id = ?` to
--     clear a file's prior vectors on idempotent re-ingest. Virtual tables
--     can't carry FKs, so the ON DELETE CASCADE from `corpus_files` can't reach
--     here; ingest clears these rows explicitly before re-embedding.
--
-- Requires the sqlite-vec extension to be loaded on the connection running this
-- migration (AppState::new registers it as an auto-extension before migrating,
-- exactly as migrations 0009/0011 already rely on).
CREATE VIRTUAL TABLE IF NOT EXISTS corpus_chunks_vec USING vec0(
    user_id    text partition key,
    embedding  float[768],
    +chunk_id  integer,
    +file_id   text
);
