-- Session 3 (RAG part 2): agentic INGEST. A distilled style/structure
-- "profile" of each imported firm draft, produced by an online cloud-model
-- pass at ingest time so future drafting can reuse the lawyer's own
-- structure and phrasing. Local-only, one row per corpus file. No PII
-- scrubbing — this is the firm's own private data on their own machine.
--
-- file_id is the PK and FKs corpus_files with ON DELETE CASCADE, so deleting
-- a corpus file (or re-ingesting it) cleans up its profile automatically
-- (sqlx enables PRAGMA foreign_keys by default).
CREATE TABLE IF NOT EXISTS corpus_profiles (
    file_id          TEXT PRIMARY KEY
                         REFERENCES corpus_files(id) ON DELETE CASCADE,
    user_id          TEXT NOT NULL,
    doc_type         TEXT,            -- doc type at distill time (from tagging)
    summary          TEXT,            -- 1-2 sentence "what this draft is"
    structure        TEXT,            -- ordered section outline, newline-joined
    style_notes      TEXT,            -- tone / register / formatting habits
    reusable_phrases TEXT,            -- JSON array of stock phrases worth reusing
    model            TEXT,            -- model that produced it (provenance)
    created_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_corpus_profiles_user
    ON corpus_profiles(user_id);
