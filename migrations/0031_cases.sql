CREATE TABLE IF NOT EXISTS cases (
    id          TEXT PRIMARY KEY,
    user_id     TEXT NOT NULL,
    title       TEXT NOT NULL,
    court       TEXT,
    parties_json TEXT,
    status      TEXT DEFAULT 'active',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS case_documents (
    case_id       TEXT NOT NULL,
    document_id   TEXT NOT NULL,
    document_type TEXT,
    attached_at   TEXT,
    PRIMARY KEY (case_id, document_id),
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS case_findings (
    id              TEXT PRIMARY KEY,
    case_id         TEXT NOT NULL,
    agent_name      TEXT NOT NULL,
    finding_type    TEXT NOT NULL,
    content_json    TEXT NOT NULL,
    grounding_json  TEXT,
    created_at      TEXT NOT NULL,
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS case_outputs (
    id                TEXT PRIMARY KEY,
    case_id           TEXT NOT NULL,
    output_type       TEXT NOT NULL,
    content_md        TEXT NOT NULL,
    docx_document_id  TEXT,
    created_at        TEXT NOT NULL,
    FOREIGN KEY (case_id) REFERENCES cases(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_cases_user_id ON cases(user_id);
CREATE INDEX IF NOT EXISTS idx_case_findings_case_id ON case_findings(case_id);
CREATE INDEX IF NOT EXISTS idx_case_findings_case_agent ON case_findings(case_id, agent_name);
