-- Extend user_settings with per-provider config so the chat backend
-- can resolve credentials/endpoints from the DB instead of process env.

ALTER TABLE user_settings ADD COLUMN openai_api_key   TEXT;
ALTER TABLE user_settings ADD COLUMN openai_model     TEXT;
ALTER TABLE user_settings ADD COLUMN local_base_url   TEXT;
ALTER TABLE user_settings ADD COLUMN local_api_key    TEXT;
ALTER TABLE user_settings ADD COLUMN local_model      TEXT;
ALTER TABLE user_settings ADD COLUMN active_provider  TEXT;
