-- Per-user Gemini model preference. Previously the Gemini model was
-- only stored client-side, so it was lost on every restart. Stable and
-- preview models alike are saved here as a free-form string (e.g.
-- "gemini-2.5-flash", "gemini-3.1-pro-preview").

ALTER TABLE user_settings ADD COLUMN gemini_model TEXT;
