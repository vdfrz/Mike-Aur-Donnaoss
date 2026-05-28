-- Structured categorical preferences (replaces single profile_text blob)

CREATE TABLE IF NOT EXISTS user_preference_categories (
    user_id TEXT NOT NULL,
    category TEXT NOT NULL,
    content_json TEXT NOT NULL DEFAULT '{}',
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, category)
);

CREATE TABLE IF NOT EXISTS case_preferences (
    case_id TEXT NOT NULL,
    category TEXT NOT NULL,
    content_json TEXT NOT NULL DEFAULT '{}',
    updated_at TEXT NOT NULL,
    PRIMARY KEY (case_id, category),
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS preference_observations (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    observation_type TEXT NOT NULL,
    dedup_hash TEXT NOT NULL,
    details_json TEXT NOT NULL,
    times_observed INTEGER DEFAULT 1,
    effectiveness_score REAL DEFAULT 0.5,
    status TEXT DEFAULT 'tentative',
    created_at TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    dismissed_until TEXT
);

CREATE INDEX IF NOT EXISTS idx_preference_observations_dedup
    ON preference_observations(user_id, dedup_hash);

-- Migrate existing profile_text into practice_specialization category
INSERT OR IGNORE INTO user_preference_categories (user_id, category, content_json, updated_at)
SELECT
    user_id,
    'practice_specialization',
    json_object('critical_rules', json_array(profile_text), 'success_metrics', json_array(), 'anti_patterns', json_array()),
    updated_at
FROM user_personalization
WHERE profile_text != '';
