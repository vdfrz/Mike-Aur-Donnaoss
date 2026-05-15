-- Folder-sync RAG state.
--
-- Two tables:
--
-- `sync_folders`  — what the user wants indexed. Multiple roots
--   supported so a user can mix e.g. "Drive\\Lavoro\\Contratti" and
--   "C:\\Casi\\2026". Each row is a watched directory tree;
--   sub-directories are walked recursively when `recursive=1`.
--
-- `synced_files`  — what we've already indexed, keyed by absolute path.
--   Tracks sha256, mtime and the document_id that owns the chunks in
--   the Lance vector store, so a rescan can decide quickly whether to
--   re-extract / re-embed (mtime changed → recompute hash → if hash
--   changed, re-index; else mark seen).

CREATE TABLE IF NOT EXISTS sync_folders (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL,
    path         TEXT NOT NULL,
    recursive    INTEGER NOT NULL DEFAULT 1,
    enabled      INTEGER NOT NULL DEFAULT 1,
    last_scan_at TEXT,
    -- Optional human label shown in the UI; falls back to basename(path).
    label        TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, path)
);

CREATE INDEX IF NOT EXISTS idx_sync_folders_user ON sync_folders(user_id);

CREATE TABLE IF NOT EXISTS synced_files (
    id            TEXT PRIMARY KEY,
    user_id       TEXT NOT NULL,
    folder_id     TEXT NOT NULL REFERENCES sync_folders(id) ON DELETE CASCADE,
    path          TEXT NOT NULL,
    sha256        TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL,
    mtime         TEXT NOT NULL,
    -- Stable id used as the foreign key into the LanceDB chunks table.
    -- Same value for the lifetime of the file, even after re-embedding.
    document_id   TEXT NOT NULL UNIQUE,
    -- "ready" | "extracted" | "skipped" | "failed"
    status        TEXT NOT NULL,
    -- Reason shown to the user when status != ready (e.g. "scanned PDF",
    -- "format not supported", "extraction error: ..."). Null on ready.
    skip_reason   TEXT,
    chunk_count   INTEGER NOT NULL DEFAULT 0,
    indexed_at    TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, path)
);

CREATE INDEX IF NOT EXISTS idx_synced_files_user ON synced_files(user_id);
CREATE INDEX IF NOT EXISTS idx_synced_files_folder ON synced_files(folder_id);
CREATE INDEX IF NOT EXISTS idx_synced_files_doc ON synced_files(document_id);
