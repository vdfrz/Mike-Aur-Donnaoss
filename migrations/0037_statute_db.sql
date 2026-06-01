-- Indian statute database: acts, section-level text, full-text search,
-- and old-to-new code mappings (IPC→BNS, CrPC→BNSS, IEA→BSA).

CREATE TABLE IF NOT EXISTS statutes (
    id            INTEGER PRIMARY KEY,
    short_name    TEXT UNIQUE NOT NULL,
    full_title    TEXT NOT NULL,
    year          INTEGER,
    status        TEXT DEFAULT 'active' CHECK(status IN ('active','repealed','replaced')),
    replaced_by   TEXT REFERENCES statutes(short_name),
    category      TEXT,
    language      TEXT DEFAULT 'en'
);

CREATE TABLE IF NOT EXISTS statute_sections (
    id              INTEGER PRIMARY KEY,
    statute_id      INTEGER NOT NULL REFERENCES statutes(id),
    section_number  TEXT NOT NULL,
    title           TEXT,
    body            TEXT NOT NULL,
    UNIQUE(statute_id, section_number)
);

CREATE INDEX IF NOT EXISTS idx_statute_sections_statute_id ON statute_sections(statute_id);

CREATE TABLE IF NOT EXISTS statute_mappings (
    id              INTEGER PRIMARY KEY,
    old_statute     TEXT NOT NULL,
    old_section     TEXT NOT NULL,
    new_statute     TEXT NOT NULL,
    new_section     TEXT NOT NULL,
    mapping_type    TEXT DEFAULT 'replaced' CHECK(mapping_type IN ('replaced','merged','split','new','deleted')),
    notes           TEXT
);

CREATE INDEX IF NOT EXISTS idx_statute_mappings_old ON statute_mappings(old_statute, old_section);
CREATE INDEX IF NOT EXISTS idx_statute_mappings_new ON statute_mappings(new_statute, new_section);

-- FTS5 virtual table for full-text search on section body + title.
-- Uses content-sync mode: the real data lives in statute_sections,
-- FTS5 mirrors it via triggers below.
CREATE VIRTUAL TABLE IF NOT EXISTS statute_sections_fts USING fts5(
    section_number,
    title,
    body,
    content=statute_sections,
    content_rowid=id
);

-- Keep FTS index in sync with statute_sections.

CREATE TRIGGER IF NOT EXISTS statute_sections_ai AFTER INSERT ON statute_sections BEGIN
    INSERT INTO statute_sections_fts(rowid, section_number, title, body)
    VALUES (new.id, new.section_number, new.title, new.body);
END;

CREATE TRIGGER IF NOT EXISTS statute_sections_ad AFTER DELETE ON statute_sections BEGIN
    INSERT INTO statute_sections_fts(statute_sections_fts, rowid, section_number, title, body)
    VALUES ('delete', old.id, old.section_number, old.title, old.body);
END;

CREATE TRIGGER IF NOT EXISTS statute_sections_au AFTER UPDATE ON statute_sections BEGIN
    INSERT INTO statute_sections_fts(statute_sections_fts, rowid, section_number, title, body)
    VALUES ('delete', old.id, old.section_number, old.title, old.body);
    INSERT INTO statute_sections_fts(rowid, section_number, title, body)
    VALUES (new.id, new.section_number, new.title, new.body);
END;
