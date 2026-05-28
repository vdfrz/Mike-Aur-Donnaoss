CREATE TABLE IF NOT EXISTS user_personalization (
    user_id TEXT PRIMARY KEY NOT NULL,
    profile_text TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL
);
