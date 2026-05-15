-- MikeRust — SQLite schema (adapted from Postgres 000_one_shot_schema.sql)

CREATE TABLE IF NOT EXISTS user_profiles (
    id              TEXT PRIMARY KEY,
    username        TEXT UNIQUE NOT NULL,
    display_name    TEXT,
    pin_hash        TEXT NOT NULL,
    biometric_enrolled INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS sessions (
    token       TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    expires_at  TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS projects (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS chats (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    project_id      TEXT REFERENCES projects(id) ON DELETE CASCADE,
    title           TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS messages (
    id              TEXT PRIMARY KEY,
    chat_id         TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    role            TEXT NOT NULL CHECK (role IN ('user','assistant','tool')),
    content         TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS documents (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    project_id      TEXT REFERENCES projects(id) ON DELETE SET NULL,
    filename        TEXT NOT NULL,
    file_type       TEXT NOT NULL,
    size_bytes      INTEGER NOT NULL DEFAULT 0,
    storage_path    TEXT,
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS document_versions (
    id              TEXT PRIMARY KEY,
    document_id     TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version_number  INTEGER NOT NULL,
    storage_path    TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS workflows (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    prompt_md       TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS user_settings (
    user_id         TEXT PRIMARY KEY REFERENCES user_profiles(id) ON DELETE CASCADE,
    main_model      TEXT,
    title_model     TEXT,
    tabular_model   TEXT,
    claude_api_key  TEXT,
    gemini_api_key  TEXT,
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
