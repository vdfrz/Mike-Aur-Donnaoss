-- Firm knowledge corpus: uploaded firm documents (past cases, skeleton
-- arguments, templates), chunked with metadata and full-text searchable.
-- The drafting agent searches this via tools rather than blind top-k injection.

CREATE TABLE IF NOT EXISTS corpus_files (
    id           TEXT PRIMARY KEY,
    user_id      TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    document_id  TEXT,                          -- optional link to a documents row
    filename     TEXT NOT NULL,
    file_type    TEXT NOT NULL,
    sha256       TEXT NOT NULL,
    doc_type     TEXT,                          -- petition|written_statement|judgment|skeleton_argument|template|deed|notice|other
    case_type    TEXT,                          -- matrimonial|consumer|criminal|writ|service|ni_act|civil|other
    court        TEXT,
    doc_date     TEXT,
    language     TEXT,
    is_template  INTEGER NOT NULL DEFAULT 0,
    template_md  TEXT,                          -- cleaned {{placeholder}} skeleton when is_template=1
    workflow_id  TEXT,                          -- workflows row created for the template
    status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK(status IN ('pending','extracting','chunking','tagging','ready','failed','unsupported')),
    error        TEXT,
    chunk_count  INTEGER NOT NULL DEFAULT 0,
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, sha256)
);
CREATE INDEX IF NOT EXISTS idx_corpus_files_user ON corpus_files(user_id, status);

CREATE TABLE IF NOT EXISTS corpus_chunks (
    id           INTEGER PRIMARY KEY,           -- rowid, FTS content sync
    file_id      TEXT NOT NULL REFERENCES corpus_files(id) ON DELETE CASCADE,
    user_id      TEXT NOT NULL,
    seq          INTEGER NOT NULL,              -- order within file (expand_chunk walks this)
    heading      TEXT,                          -- nearest heading / paragraph label
    section_role TEXT,                          -- ground|prayer|argument|clause|facts|verification|cause_title|other
    page         INTEGER,
    text         TEXT NOT NULL,
    UNIQUE(file_id, seq)
);
CREATE INDEX IF NOT EXISTS idx_corpus_chunks_file ON corpus_chunks(file_id, seq);

-- FTS5 mirror of corpus_chunks (content-sync mode, same pattern as 0037).
CREATE VIRTUAL TABLE IF NOT EXISTS corpus_chunks_fts USING fts5(
    heading,
    text,
    content=corpus_chunks,
    content_rowid=id
);

CREATE TRIGGER IF NOT EXISTS corpus_chunks_ai AFTER INSERT ON corpus_chunks BEGIN
    INSERT INTO corpus_chunks_fts(rowid, heading, text)
    VALUES (new.id, new.heading, new.text);
END;

CREATE TRIGGER IF NOT EXISTS corpus_chunks_ad AFTER DELETE ON corpus_chunks BEGIN
    INSERT INTO corpus_chunks_fts(corpus_chunks_fts, rowid, heading, text)
    VALUES ('delete', old.id, old.heading, old.text);
END;

CREATE TRIGGER IF NOT EXISTS corpus_chunks_au AFTER UPDATE ON corpus_chunks BEGIN
    INSERT INTO corpus_chunks_fts(corpus_chunks_fts, rowid, heading, text)
    VALUES ('delete', old.id, old.heading, old.text);
    INSERT INTO corpus_chunks_fts(rowid, heading, text)
    VALUES (new.id, new.heading, new.text);
END;

-- Provenance link so deleting a corpus template can clean up its workflow row.
ALTER TABLE workflows ADD COLUMN corpus_file_id TEXT;
