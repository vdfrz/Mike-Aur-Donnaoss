-- Authoritative-corpus tracking on the documents table.
--
-- When a document is ingested from an external legal corpus (EUR-Lex,
-- Retsinformation, Légifrance, ...) we keep enough metadata to: avoid
-- re-fetching the same act (corpus_id + corpus_identifier is unique),
-- show the user which language was actually fetched (corpus_language),
-- and surface a "fell back to English" indicator when the requested
-- language wasn't available (fetched_with_fallback).
--
-- All four columns are nullable: every existing documents row, as well
-- as future user uploads and project-library docs, has corpus_id NULL
-- and is unaffected by this migration.

ALTER TABLE documents ADD COLUMN corpus_id TEXT;
ALTER TABLE documents ADD COLUMN corpus_identifier TEXT;
ALTER TABLE documents ADD COLUMN corpus_language TEXT;
ALTER TABLE documents ADD COLUMN fetched_with_fallback INTEGER NOT NULL DEFAULT 0;

-- Lookup by (corpus_id, corpus_identifier) is the primary dedupe path:
-- "have I already indexed CELEX 32016R0679?". Per-user uniqueness is
-- enforced application-side rather than via UNIQUE so the same act
-- can be indexed by different users on a multi-tenant install.
CREATE INDEX IF NOT EXISTS idx_documents_corpus
    ON documents(corpus_id, corpus_identifier);

-- Per-user EUR-Lex preferences (enabled, reference language). Kept in
-- a dedicated table rather than as columns on user_profiles so other
-- corpora (Retsinformation, Légifrance, ...) can reuse the same shape
-- without further schema churn.
CREATE TABLE IF NOT EXISTS corpus_settings (
    user_id     TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    corpus_id   TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 0,
    language    TEXT,
    -- "Fall back to English when the chosen language is unavailable" —
    -- always-on for V1 of EUR-Lex; column kept so the user can opt out
    -- later if they specifically want hard failures.
    fallback_en INTEGER NOT NULL DEFAULT 1,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, corpus_id)
);
