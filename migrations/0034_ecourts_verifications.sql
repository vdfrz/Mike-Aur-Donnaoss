-- eCourts manual-verification records.
--
-- When the user clicks "Verify on eCourts" on a Kanoon citation in chat,
-- they're taken to the official eCourts pdfsearch portal, solve the
-- CAPTCHA themselves, find the matching case, and record the canonical
-- case number back into Mike. This table stores those user-confirmed
-- verifications so they persist across sessions and across chats
-- referencing the same Kanoon case.
--
-- Lookup pattern: `WHERE user_id = ? AND kanoon_tid = ?`.
-- We keep history (multiple rows per tid allowed) so that later
-- re-verification attempts overlay the previous outcome rather than
-- silently overwrite — useful when a lawyer wants to confirm they
-- checked something twice.

CREATE TABLE IF NOT EXISTS ecourts_verifications (
    id                  TEXT PRIMARY KEY,
    user_id             TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    -- Kanoon document id that was being verified. Always present.
    kanoon_tid          INTEGER NOT NULL,
    -- Verbatim title from the original Kanoon hit (for display + audit trail).
    kanoon_title        TEXT NOT NULL,
    -- Court string from the original Kanoon hit.
    kanoon_court        TEXT,
    -- Decision date as Kanoon reported it (free-form string).
    kanoon_decision_date TEXT,
    -- Outcome — what the user recorded after their eCourts search.
    --   'verified'  — found on eCourts, case number recorded.
    --   'not_found' — searched eCourts, case is not indexed there.
    --   'pending'   — opened eCourts but didn't record an outcome yet.
    status              TEXT NOT NULL CHECK (status IN ('verified', 'not_found', 'pending')),
    -- Canonical eCourts case identifier as displayed on the result page.
    -- e.g. "CRL.A. 1124/2020" or a 16-digit CNR. NULL when status != 'verified'.
    ecourts_case_number TEXT,
    -- Optional URL to the official PDF on eCourts (if the user pasted it).
    ecourts_pdf_url     TEXT,
    -- Free-form user notes captured at verification time (e.g. "case name
    -- on eCourts uses a slightly different spelling").
    notes               TEXT,
    verified_at         TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (user_id, kanoon_tid, verified_at)
);

CREATE INDEX IF NOT EXISTS idx_ecourts_verifications_user_tid
    ON ecourts_verifications(user_id, kanoon_tid);
