-- Per-user Gemini region preference. Default null means "use global
-- endpoint". Stable models can be pinned to a region (e.g. europe-west1,
-- us-central1) for data residency. Preview models always use global —
-- the chat dispatch layer overrides the user setting when the selected
-- model is preview-only.

ALTER TABLE user_settings ADD COLUMN gemini_region TEXT;
