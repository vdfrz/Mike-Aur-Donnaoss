-- Tabular reviews
CREATE TABLE IF NOT EXISTS tabular_reviews (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    project_id      TEXT REFERENCES projects(id) ON DELETE SET NULL,
    workflow_id     TEXT REFERENCES workflows(id) ON DELETE SET NULL,
    title           TEXT NOT NULL DEFAULT 'Untitled Review',
    columns_config  TEXT NOT NULL DEFAULT '[]',
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tabular_review_rows (
    id                  TEXT PRIMARY KEY,
    tabular_review_id   TEXT NOT NULL REFERENCES tabular_reviews(id) ON DELETE CASCADE,
    document_id         TEXT REFERENCES documents(id) ON DELETE CASCADE,
    row_index           INTEGER NOT NULL DEFAULT 0,
    cells               TEXT NOT NULL DEFAULT '[]',
    status              TEXT NOT NULL DEFAULT 'pending',
    created_at          TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Hidden workflows per user
CREATE TABLE IF NOT EXISTS workflow_hidden (
    user_id     TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, workflow_id)
);
