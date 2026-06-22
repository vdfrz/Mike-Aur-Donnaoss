-- Link each document version to the assistant message that produced it, so
-- chat reload can rebuild the doc_edited card (the per-change Accept/Reject
-- EditCards) for that turn. Nullable + no backfill: versions created before
-- this migration stay NULL and simply produce no card on reload (the prior
-- behaviour), which is correct since they predate the fix.
ALTER TABLE document_versions ADD COLUMN message_id TEXT;
CREATE INDEX IF NOT EXISTS idx_document_versions_message_id
    ON document_versions(message_id);
