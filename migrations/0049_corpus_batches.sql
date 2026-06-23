-- Folder-upload batching for the firm corpus.
--
-- A "drop a folder" upload tags every file it creates with a shared batch_id
-- and a human label (the dropped folder's name) so the UI can group the batch,
-- show its progress, and offer remove-the-whole-batch. Existing single-file
-- uploads leave both NULL and render as individual files. Both columns are
-- nullable additive columns, so this migration is backward compatible.

ALTER TABLE corpus_files ADD COLUMN batch_id TEXT;
ALTER TABLE corpus_files ADD COLUMN batch_label TEXT;

CREATE INDEX IF NOT EXISTS idx_corpus_files_batch
    ON corpus_files (user_id, batch_id);
