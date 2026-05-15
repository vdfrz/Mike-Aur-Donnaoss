-- Link uploaded documents to a specific chat so chat deletion can
-- cascade-clean both the SQLite row and the on-disk file.
--
-- The chat composer's "+ document" button uploads via /single-documents
-- before any chat exists in the DB (the chat row is materialised on the
-- first message-send). Once the chat is created and the message is sent,
-- the upload is associated with that chat by setting documents.chat_id.
--
-- Storage layout for chat-cached uploads:
--   data/storage/cache/<doc_id>
-- Project-scoped uploads and pre-cache rows keep their existing path
-- (data/storage/documents/<user>/<doc_id>) and are untouched here.
--
-- ON DELETE CASCADE: when the chat row goes, the documents row goes
-- too. The actual file on disk is removed by the chat-delete handler
-- *before* the cascade runs, since SQLite can't reach the filesystem.
--
-- Existing rows have chat_id = NULL — they're either project-scoped or
-- predate this column, so chat-deletion does not touch them.

ALTER TABLE documents ADD COLUMN chat_id TEXT REFERENCES chats(id) ON DELETE CASCADE;
CREATE INDEX IF NOT EXISTS idx_documents_chat_id ON documents(chat_id);
