-- Markdown-first drafting.
--
-- Mike now drafts in Markdown, which is the persistent WORKING COPY of a
-- document. The .docx is rendered on demand (only when the user approves),
-- so the markdown must survive a chat reload — previously the markdown body
-- lived only in the live SSE stream and was lost when the user left, which is
-- why reopening a chat showed raw markdown instead of the document card.
--
-- `markdown_source` holds the CURRENT markdown for a document (source of truth
-- for re-rendering and for the formatted panel). `message_id` links a draft to
-- the assistant message that produced it so chat reload can rebuild the
-- doc_created event (card + formatted panel) instead of dumping raw prose.
ALTER TABLE documents ADD COLUMN markdown_source TEXT;
ALTER TABLE documents ADD COLUMN message_id TEXT REFERENCES messages(id) ON DELETE SET NULL;
CREATE INDEX IF NOT EXISTS idx_documents_message_id ON documents(message_id);

-- Append-only snapshots of every draft/re-draft so a chat-driven edit
-- ("change this name throughout") never destroys the prior draft — the
-- pre-edit version stays recoverable. `markdown_source` above is the latest;
-- the highest version_no here mirrors it. Cascades with the document.
CREATE TABLE IF NOT EXISTS document_markdown_versions (
    id              TEXT PRIMARY KEY,
    document_id     TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    version_no      INTEGER NOT NULL,
    markdown        TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_doc_md_versions_doc
    ON document_markdown_versions(document_id, version_no);
