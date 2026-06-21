-- The self-rewriting drafting harness (ported from the lavern feedback loop).
--
-- "Mike listens": the lawyer talks to Mike about how it drafts, and Mike
-- rewrites its own drafting instructions live. Per user we keep a generation
-- counter, a set of learned drafting lessons (the harness the loop evolves), a
-- feedback chat log (so the conversation survives reloads), and a feature
-- request queue (capabilities for the dev team — these never change drafting).
--
-- The active (non-deprecated) lessons are injected into every draft's system
-- prompt. Unlike lavern there are no scored proposal sections to revise, so we
-- keep only the compounding-lessons + generation-lineage half of the loop.

-- One row per user: the current harness generation (0 = built-in defaults).
CREATE TABLE IF NOT EXISTS harness_state (
    user_id     TEXT PRIMARY KEY REFERENCES user_profiles(id) ON DELETE CASCADE,
    generation  INTEGER NOT NULL DEFAULT 0,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Distilled drafting rules. id = 'lsn-' + sha1(normalized rule) so two rules
-- that differ only in case/punctuation dedupe to one row (occurrences bumps).
CREATE TABLE IF NOT EXISTS harness_lessons (
    user_id            TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    id                 TEXT NOT NULL,
    rule               TEXT NOT NULL,
    kind               TEXT NOT NULL DEFAULT 'do' CHECK(kind IN ('do','dont')),
    effectiveness      REAL NOT NULL DEFAULT 0.5,   -- confidence from reinforcement (EWMA)
    occurrences        INTEGER NOT NULL DEFAULT 1,
    deprecated         INTEGER NOT NULL DEFAULT 0,
    deprecation_reason TEXT,
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at       TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, id)
);
CREATE INDEX IF NOT EXISTS idx_harness_lessons_user ON harness_lessons(user_id, deprecated);

-- The feedback conversation, rebuilt into the chat thread on page load. One row
-- per turn; assistant turns may carry a note and the generation reached.
CREATE TABLE IF NOT EXISTS harness_feedback (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK(role IN ('you','assistant')),
    text        TEXT NOT NULL,
    note        TEXT,
    generation  INTEGER,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_harness_feedback_user ON harness_feedback(user_id, created_at);

-- Feature requests: new app capabilities, queued for the dev team. Kept apart
-- from lessons on purpose — "Mike listens" only improves drafting; new features
-- live here and never touch the harness.
CREATE TABLE IF NOT EXISTS harness_features (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    request     TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open','done','declined')),
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_harness_features_user ON harness_features(user_id, created_at);
