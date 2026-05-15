-- Extend the workflows table to match the frontend MikeWorkflow shape.
--
-- The original table (migration 0001) only had id/user_id/title/prompt_md.
-- The Next.js port assumes:
--   * `type`           — 'assistant' (chat-style) or 'tabular' (column grid)
--   * `practice`       — free-form practice area label ("Corporate", …)
--   * `columns_config` — JSON array of {index, name, prompt} for tabular
--
-- Without these the create_workflow handler can only accept a bare
-- {title, prompt_md} body and fails to deserialize the richer payload
-- the modal sends. Adding them as nullable / defaulted columns is
-- backwards-compatible with existing rows.
--
-- We also relax `prompt_md` from NOT NULL to nullable: workflows can be
-- created with just a title + type + practice (the prompt is filled in
-- via the editor afterwards). Existing NULL-rejecting INSERTs in the
-- code keep working since they bind a non-null value.

ALTER TABLE workflows ADD COLUMN type TEXT NOT NULL DEFAULT 'assistant';
ALTER TABLE workflows ADD COLUMN practice TEXT;
ALTER TABLE workflows ADD COLUMN columns_config TEXT NOT NULL DEFAULT '[]';

-- SQLite can't drop a NOT NULL constraint without table-rebuild. We
-- preserve the constraint at the schema level and accept Some("") at
-- the application level to mean "no prompt yet" — the route layer
-- coerces None → "" before binding so this stays compatible.
