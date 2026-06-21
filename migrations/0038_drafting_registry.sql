-- Drafting registry: per-case parties, annexures, and citations.
-- Powers @party / #annexure cross-references (resolved at build time) and
-- the auto-maintained "List of Cases Referred" / "List of Authorities".

-- Parties: petitioners/respondents with stable @slugs and per-side serials.
CREATE TABLE IF NOT EXISTS case_parties (
    id           TEXT PRIMARY KEY,
    case_id      TEXT NOT NULL,
    slug         TEXT NOT NULL,                 -- '@' handle: lowercase [a-z0-9_]
    name         TEXT NOT NULL,                 -- "State of Kerala"
    side         TEXT NOT NULL CHECK(side IN ('petitioner','respondent')),
    role_label   TEXT,                          -- display override e.g. "Opposite Party"; NULL = derive from side
    serial_no    INTEGER NOT NULL,              -- 1-based within (case_id, side)
    details_json TEXT,                          -- S/o, address, etc. (future memo-of-parties)
    source       TEXT NOT NULL DEFAULT 'manual' CHECK(source IN ('manual','ai')),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE(case_id, slug),
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_case_parties_case ON case_parties(case_id, side, serial_no);

-- Annexures: attached documents designated as exhibits (P-1, R-1, ...).
CREATE TABLE IF NOT EXISTS case_annexures (
    id           TEXT PRIMARY KEY,
    case_id      TEXT NOT NULL,
    document_id  TEXT NOT NULL,                 -- documents.id of an attached case doc
    slug         TEXT NOT NULL,                 -- '#' handle
    description  TEXT,                          -- "A true copy of Form 26AS dated ..."
    doc_date     TEXT,                          -- DD.MM.YYYY or NULL
    side         TEXT NOT NULL DEFAULT 'P' CHECK(side IN ('P','R','C')),
    serial_no    INTEGER NOT NULL,              -- 1-based within (case_id, side)
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    UNIQUE(case_id, slug),
    UNIQUE(case_id, document_id),
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_case_annexures_case ON case_annexures(case_id, side, serial_no);

-- Citations: judgments + statute sections referenced/cited in this case.
-- dedupe_key is mandatory because SQLite UNIQUE treats NULLs as distinct, so a
-- composite UNIQUE over nullable tid/statute columns would never dedupe.
CREATE TABLE IF NOT EXISTS case_citations (
    id                TEXT PRIMARY KEY,
    case_id           TEXT NOT NULL,
    kind              TEXT NOT NULL CHECK(kind IN ('judgment','statute')),
    dedupe_key        TEXT NOT NULL,            -- 'judgment:<tid>' | 'statute:<short>:<section>'
    status            TEXT NOT NULL DEFAULT 'referred' CHECK(status IN ('referred','cited')),
    -- judgment fields
    kanoon_tid        INTEGER,
    title             TEXT,
    court             TEXT,
    decision_date     TEXT,
    kanoon_url        TEXT,
    canonical_citation TEXT,
    pdf_document_id   TEXT,                      -- documents.id of auto-downloaded PDF / cached text
    -- statute fields
    statute           TEXT,
    section_number    TEXT,
    source_tool       TEXT,                      -- 'kanoon_search'|'kanoon_verify_case'|'statute_search'
    times_cited       INTEGER NOT NULL DEFAULT 1,
    first_cited_at    TEXT NOT NULL,
    last_cited_at     TEXT NOT NULL,
    UNIQUE(case_id, dedupe_key),
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_case_citations_case ON case_citations(case_id, kind, status);
