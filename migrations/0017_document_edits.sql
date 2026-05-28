CREATE TABLE IF NOT EXISTS document_edits (
    id              TEXT PRIMARY KEY,
    document_id     TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version_id      TEXT NOT NULL REFERENCES document_versions(id) ON DELETE CASCADE,
    change_id       TEXT NOT NULL,
    del_w_id        TEXT,
    ins_w_id        TEXT,
    deleted_text    TEXT NOT NULL DEFAULT '',
    inserted_text   TEXT NOT NULL DEFAULT '',
    context_before  TEXT,
    context_after   TEXT,
    reason          TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
