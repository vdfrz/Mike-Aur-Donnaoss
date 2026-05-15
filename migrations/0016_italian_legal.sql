-- Italian-legal-corpus bulk metadata index.
--
-- We mirror only the metadata columns (no `text` body) of the rows we
-- care about (Normattiva + Corte Costituzionale) from the
-- `dossier-legal/italian-legal-corpus` HuggingFace dataset. ~80 MB on
-- disk for ~91k rows. The full text is fetched on demand from the
-- HuggingFace `/rows` API when the user picks a doc to index — the
-- per-row fetch is small (a few KB to ~100 KB per act).
--
-- Search lives in a virtual FTS5 table that mirrors `title`,
-- `authority`, `number` for fast keyword ranking. The non-indexed
-- columns stay on `italian_corpus` so we can filter by source /
-- doc_type / year via plain SQL after the FTS5 narrowing.
--
-- `italian_corpus_meta` carries the import bookkeeping — pin the
-- dataset commit SHA for reproducibility, store the last-import
-- timestamp + row count.

CREATE TABLE IF NOT EXISTS italian_corpus (
    -- HuggingFace row identifier from the dataset's `id` field. Acts
    -- as our primary key; doubles as the lookup key for /rows.
    hf_id          TEXT PRIMARY KEY,
    -- Row offset in the train split — used by /rows?offset=N&length=1
    -- to fetch the full text on demand without re-scanning Parquet.
    row_offset     INTEGER NOT NULL,
    source         TEXT NOT NULL,
    doc_type       TEXT,
    title          TEXT,
    authority      TEXT,
    -- `number` is a string in the source dataset (mixed formats like
    -- "274", "798/85") — we keep it as TEXT and let the user search
    -- it as opaque.
    number         TEXT,
    year           INTEGER,
    date           TEXT,
    ecli           TEXT,
    text_length    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_italian_corpus_source     ON italian_corpus(source);
CREATE INDEX IF NOT EXISTS idx_italian_corpus_doc_type   ON italian_corpus(doc_type);
CREATE INDEX IF NOT EXISTS idx_italian_corpus_year       ON italian_corpus(year);

CREATE VIRTUAL TABLE IF NOT EXISTS italian_corpus_fts USING fts5(
    hf_id    UNINDEXED,
    title,
    authority,
    number
);

-- Single-row table (id=1) carrying import bookkeeping. The unique
-- constraint on `id=1` keeps us at most one row.
CREATE TABLE IF NOT EXISTS italian_corpus_meta (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    last_import_at      TEXT,
    row_count           INTEGER NOT NULL DEFAULT 0,
    dataset_revision    TEXT,
    -- Import-job state — surfaced by the /italian-legal/import-status
    -- endpoint to drive the UI progress bar:
    --   idle | downloading | importing | ready | failed
    job_state           TEXT NOT NULL DEFAULT 'idle',
    job_current_shard   INTEGER NOT NULL DEFAULT 0,
    job_total_shards    INTEGER NOT NULL DEFAULT 0,
    job_rows_imported   INTEGER NOT NULL DEFAULT 0,
    job_error           TEXT
);

-- Seed the singleton row so the route layer can blindly UPDATE.
INSERT OR IGNORE INTO italian_corpus_meta (id, job_state) VALUES (1, 'idle');

-- Per-user enable flag for the corpus, plus chosen sources. Stored as
-- JSON to keep the schema flat — the only sources we recognise today
-- are 'normattiva' and 'corte_costituzionale', but a JSON list lets
-- us add 'openga' later without another migration.
CREATE TABLE IF NOT EXISTS italian_corpus_settings (
    user_id     TEXT NOT NULL PRIMARY KEY REFERENCES user_profiles(id) ON DELETE CASCADE,
    enabled     INTEGER NOT NULL DEFAULT 0,
    sources     TEXT NOT NULL DEFAULT '["normattiva","corte_costituzionale"]',
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
