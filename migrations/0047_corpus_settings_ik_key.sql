-- BYOK: per-user Indian Kanoon API key.
-- The /indian-kanoon/config routes already read & write
-- corpus_settings.ik_api_key, but the column was never created (0015 made
-- corpus_settings without it), so the in-app key path silently failed.
ALTER TABLE corpus_settings ADD COLUMN ik_api_key TEXT;
