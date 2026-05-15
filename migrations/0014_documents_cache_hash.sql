-- Hash-keyed cache storage for chat-attached uploads.
--
-- For uploads that come in with `cache=true` (the chat composer), we
-- store the binary AND its extracted plain text under data/storage/cache,
-- both keyed by SHA-256 of the binary:
--
--   data/storage/cache/<hash>.<ext>   ← original file
--   data/storage/cache/<hash>.txt     ← extracted plain text
--
-- Using the hash for the on-disk filename buys two things:
--  1. Same file uploaded across multiple chats reuses one set of
--     physical files (multiple `documents` rows point to the same
--     storage_path / extracted_text_path).
--  2. Same display filename ("contratto.pdf") in different chats can no
--     longer collide on disk — the bytes determine the path, not the
--     filename the user picked.
--
-- A modified file produces a different hash, so the cache transparently
-- regenerates the extracted text on next upload.
--
-- The chat-delete handler ref-counts: it deletes the binary + text
-- files only after the last `documents` row referencing the hash is
-- gone (the cascade from migration 0013 wipes those rows).

ALTER TABLE documents ADD COLUMN content_hash TEXT;
ALTER TABLE documents ADD COLUMN extracted_text_path TEXT;
CREATE INDEX IF NOT EXISTS idx_documents_content_hash ON documents(content_hash);
