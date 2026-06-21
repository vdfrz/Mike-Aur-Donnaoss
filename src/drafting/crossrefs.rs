//! Build-time cross-reference resolver.
//!
//! Drafts written in the editor use lightweight handles:
//!   `@stateofkerala`  → a party  → rendered "Respondent No. 2"
//!   `#form26as`       → an annexure → rendered "Annexure P-3"
//!
//! [`resolve_crossrefs`] loads the case's parties + annexures from the
//! registry and substitutes every recognised handle with its canonical
//! rendering. Unknown handles are left verbatim and reported in
//! [`ResolvedMarkdown::unresolved`] so the caller can warn the user.
//!
//! The token regexes require a non-word, non-`@`/`#` character immediately
//! before the sigil so that emails (`user@x.com`) and Markdown headings
//! (`## Heading`, `#Heading`) are never mistaken for handles.

use regex::Regex;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Result of resolving a draft's cross-references.
pub struct ResolvedMarkdown {
    /// Markdown with every recognised `@`/`#` handle substituted.
    pub markdown: String,
    /// Slugs that looked like handles but matched no party/annexure.
    /// Deduplicated, in first-seen order.
    pub unresolved: Vec<String>,
}

// A handle is `@`/`#` immediately preceded by start-of-string or a char that
// is NOT a word char, `@`, or `/`. The leading char is captured (group 1) and
// re-emitted so we don't eat surrounding text. The slug (group 2) starts with
// an alphanumeric then 2..40 of `[a-z0-9_]`, mirroring `slugify`'s output.
static RE_PARTY_HANDLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^\w@/])@([a-z0-9][a-z0-9_]{2,40})").unwrap());
static RE_ANNEXURE_HANDLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[^\w@/])#([a-z0-9][a-z0-9_]{2,40})").unwrap());

/// Default display label for a party side when no `role_label` override exists.
fn default_role_label(side: &str) -> &'static str {
    match side {
        "petitioner" => "Petitioner",
        _ => "Respondent",
    }
}

/// Resolve `@party` / `#annexure` handles in `markdown` against the case's
/// registry. Pure substitution apart from the two registry reads; never
/// mutates the DB. On a DB read error the corresponding map is empty, so every
/// handle of that kind falls through to `unresolved` (markdown unchanged).
pub async fn resolve_crossrefs(db: &SqlitePool, case_id: &str, markdown: &str) -> ResolvedMarkdown {
    // slug -> "Respondent No. 2"
    let mut party_map: HashMap<String, String> = HashMap::new();
    let party_rows: Vec<(String, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT slug, side, role_label, serial_no FROM case_parties WHERE case_id = ?",
    )
    .bind(case_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    for (slug, side, role_label, serial_no) in party_rows {
        let label = role_label
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| default_role_label(&side).to_string());
        party_map.insert(slug, format!("{label} No. {serial_no}"));
    }

    // slug -> "Annexure P-3"
    let mut annexure_map: HashMap<String, String> = HashMap::new();
    let annexure_rows: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT slug, side, serial_no FROM case_annexures WHERE case_id = ?",
    )
    .bind(case_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();
    for (slug, side, serial_no) in annexure_rows {
        annexure_map.insert(slug, format!("Annexure {side}-{serial_no}"));
    }

    let mut unresolved: Vec<String> = Vec::new();

    // Resolve `@party` handles first, then `#annexure` handles. Order is
    // irrelevant because the two sigils never overlap.
    let after_parties = substitute(&RE_PARTY_HANDLE, markdown, &party_map, &mut unresolved);
    let after_annexures =
        substitute(&RE_ANNEXURE_HANDLE, &after_parties, &annexure_map, &mut unresolved);

    ResolvedMarkdown {
        markdown: after_annexures,
        unresolved,
    }
}

/// Replace every handle matched by `re` using `map`. The leading boundary char
/// (capture group 1) is preserved; unknown slugs (capture group 2) are left as
/// the original token and pushed to `unresolved` (deduped).
fn substitute(
    re: &Regex,
    input: &str,
    map: &HashMap<String, String>,
    unresolved: &mut Vec<String>,
) -> String {
    re.replace_all(input, |caps: &regex::Captures| {
        let lead = &caps[1];
        let slug = &caps[2];
        match map.get(slug) {
            Some(rendering) => format!("{lead}{rendering}"),
            None => {
                if !unresolved.iter().any(|s| s == slug) {
                    unresolved.push(slug.to_string());
                }
                // Re-emit the whole original match (lead + sigil + slug).
                caps[0].to_string()
            }
        }
    })
    .into_owned()
}

/// Honorifics stripped from the front of a name before slugifying.
const HONORIFICS: &[&str] = &[
    "sh.", "shri", "smt.", "smt", "m/s", "mr.", "mrs.", "ms.", "dr.",
    "sh", "mr", "mrs", "ms", "dr",
];

/// Turn a human name / filename into a stable `@`/`#` slug.
///
/// Lowercase, strip a leading honorific, drop the file extension and all
/// non-`[a-z0-9]` characters, truncate to 30 chars. e.g.
///   "State of Kerala" -> "stateofkerala"
///   "Sh. Ram Kumar"   -> "ramkumar"
///   "Form 26AS.pdf"   -> "form26as"
pub fn slugify(name: &str) -> String {
    let mut s = name.trim().to_lowercase();

    // Drop a file extension so "Form 26AS.pdf" and "Form 26AS" agree.
    if let Some(dot) = s.rfind('.') {
        let ext = &s[dot + 1..];
        if (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            s.truncate(dot);
        }
    }

    // Strip a single leading honorific (token-aware, so "shrikant" survives).
    for h in HONORIFICS {
        if let Some(rest) = s.strip_prefix(h) {
            if rest.starts_with(|c: char| c.is_whitespace()) {
                s = rest.trim_start().to_string();
                break;
            }
        }
    }

    let cleaned: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    cleaned.chars().take(30).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn party(slug: &str, rendering: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(slug.to_string(), rendering.to_string());
        m
    }

    #[test]
    fn email_is_not_a_party_handle() {
        let map = party("xyz", "Respondent No. 1");
        let mut un = Vec::new();
        let out = substitute(&RE_PARTY_HANDLE, "contact user@xyz.com today", &map, &mut un);
        assert_eq!(out, "contact user@xyz.com today");
        assert!(un.is_empty(), "email must not register as unresolved");
    }

    #[test]
    fn markdown_headings_are_not_annexure_handles() {
        let map: HashMap<String, String> = HashMap::new();
        let mut un = Vec::new();
        // "## Heading" and "#Heading" — neither should be touched, and the
        // letters of "Heading" aren't valid slug chars anyway (uppercase).
        let out = substitute(&RE_ANNEXURE_HANDLE, "## Heading\n#intro text", &map, &mut un);
        assert_eq!(out, "## Heading\n#intro text");
        // "#intro" sits after '\n' (a boundary) so it IS a candidate handle,
        // but since it's unknown it must be left verbatim + reported once.
        assert_eq!(un, vec!["intro".to_string()]);
    }

    #[test]
    fn basic_party_resolution() {
        let map = party("stateofkerala", "Respondent No. 2");
        let mut un = Vec::new();
        let out = substitute(
            &RE_PARTY_HANDLE,
            "served upon @stateofkerala on time",
            &map,
            &mut un,
        );
        assert_eq!(out, "served upon Respondent No. 2 on time");
        assert!(un.is_empty());
    }

    #[test]
    fn handle_at_start_of_string_resolves() {
        let map = party("ramkumar", "Petitioner No. 1");
        let mut un = Vec::new();
        let out = substitute(&RE_PARTY_HANDLE, "@ramkumar filed the suit", &map, &mut un);
        assert_eq!(out, "Petitioner No. 1 filed the suit");
    }

    #[test]
    fn basic_annexure_resolution() {
        let map = party("form26as", "Annexure P-3");
        let mut un = Vec::new();
        let out = substitute(
            &RE_ANNEXURE_HANDLE,
            "is annexed as #form26as hereto",
            &map,
            &mut un,
        );
        assert_eq!(out, "is annexed as Annexure P-3 hereto");
    }

    #[test]
    fn unknown_slug_passes_through_and_is_reported() {
        let map = party("known", "Respondent No. 1");
        let mut un = Vec::new();
        let out = substitute(
            &RE_PARTY_HANDLE,
            "see @known and @missing and @missing again",
            &map,
            &mut un,
        );
        assert_eq!(out, "see Respondent No. 1 and @missing and @missing again");
        // Deduped to a single entry despite two occurrences.
        assert_eq!(un, vec!["missing".to_string()]);
    }

    #[test]
    fn slugify_cases() {
        assert_eq!(slugify("State of Kerala"), "stateofkerala");
        assert_eq!(slugify("Form 26AS.pdf"), "form26as");
        assert_eq!(slugify("Sh. Ram Kumar"), "ramkumar");
        assert_eq!(slugify("M/s Acme Pvt. Ltd."), "acmepvtltd");
        // Honorific embedded in a real word must not be stripped.
        assert_eq!(slugify("Shrikant"), "shrikant");
        // Truncation to 30 chars.
        assert_eq!(slugify(&"a".repeat(50)).len(), 30);
    }
}
