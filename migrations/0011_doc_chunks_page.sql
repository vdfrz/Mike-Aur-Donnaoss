-- Add a `page` aux column to the doc_chunks vec0 table so PDF citations
-- can jump straight to the right page in the DocPanel.
--
-- The scanner already prefixes each PDF page with `[Page N]\n` markers
-- in the extracted text, but those markers can drift inside chunks
-- (overlap, multi-page chunks). Doing the regex at retrieval time is
-- fragile. The chunker now computes the page authoritatively from the
-- ORIGINAL source text — the byte offset of every chunk start — and
-- stamps it on the chunk row at index time.
--
-- For non-PDF formats the page stays NULL: the DocPanel falls back to
-- text-search highlighting just like before.
--
-- sqlite-vec virtual tables don't support ALTER TABLE … ADD COLUMN, so
-- we drop and recreate. The user hasn't run the smoke test pipeline
-- end-to-end yet (TODO.md), so any existing rows would be re-indexed
-- on first scan anyway.

DROP TABLE IF EXISTS doc_chunks;

CREATE VIRTUAL TABLE doc_chunks USING vec0(
    user_id      text partition key,
    project_id   text partition key,
    embedding    float[768],
    +document_id text,
    +source_path text,
    +chunk_index integer,
    +text        text,
    +page        integer
);
