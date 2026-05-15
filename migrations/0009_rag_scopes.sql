-- RAG scoping + sqlite-vec virtual table.
--
-- Three-tier scope model:
--  1. Global pool       (synced_files.project_id IS NULL):
--     Folders not bound to any project. Visible from any chat.
--  2. Project pool      (synced_files.project_id = '<proj>'):
--     Folders bound to a project. Visible from that project's chats.
--     If the project's `isolation_mode = 'shared'` (default), the
--     chat ALSO sees the global pool. If `isolation_mode = 'strict'`,
--     only the project's own pool is visible — useful for cases with
--     conflict-of-interest concerns.
--  3. Per-turn attached (current behaviour, unchanged):
--     User-attached docs flow through the chat input and get full-text
--     concatenation in the system prompt. RAG retrieval is additive.

ALTER TABLE sync_folders ADD COLUMN project_id TEXT;
ALTER TABLE synced_files ADD COLUMN project_id TEXT;
CREATE INDEX IF NOT EXISTS idx_sync_folders_project ON sync_folders(project_id);
CREATE INDEX IF NOT EXISTS idx_synced_files_project ON synced_files(project_id);

-- 'shared' (default): project chats see global + own.
-- 'strict':           project chats see only own.
ALTER TABLE projects ADD COLUMN isolation_mode TEXT NOT NULL DEFAULT 'shared';

-- sqlite-vec virtual table holding the embedding vectors.
--
-- Layout (vec0 syntax):
--   * `embedding float[768]` — the vector itself.
--   * `user_id` / `project_id` are **PARTITION KEYs**, not auxiliary
--     columns. sqlite-vec only allows WHERE filters on partition keys
--     during KNN MATCH queries; auxiliary `+columns` are returnable but
--     can't appear in a WHERE clause next to MATCH (raises "illegal
--     WHERE constraint" at runtime). Marking these as partition keys
--     also makes the per-user / per-project KNN search cheaper because
--     vec0 prunes whole partitions before the cosine pass.
--   * `+document_id`, `+source_path`, `+chunk_index`, `+text` remain
--     auxiliary — we read them out of the result rows but never filter
--     on them inline.
CREATE VIRTUAL TABLE IF NOT EXISTS doc_chunks USING vec0(
    user_id      text partition key,
    project_id   text partition key,
    embedding    float[768],
    +document_id text,
    +source_path text,
    +chunk_index integer,
    +text        text
);
