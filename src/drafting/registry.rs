//! Serial bookkeeping + AI seeding for the drafting registry.
//!
//! Parties and annexures each carry a 1-based `serial_no` that is unique
//! within `(case_id, side)`. These helpers allocate the next serial, compact
//! after deletes, reorder in one transaction, and seed the registry from
//! case-prep findings / attached documents.
//!
//! `table` is always one of `"case_parties"` | `"case_annexures"`. Callers are
//! internal, so the table name is interpolated into the SQL directly after a
//! whitelist check rather than bound (SQLite can't bind identifiers).

use anyhow::{bail, Result};
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashSet;

use crate::drafting::crossrefs::slugify;

/// Validate a caller-supplied table name. Both registry tables share the
/// `(case_id, side, serial_no, slug)` shape these helpers rely on.
fn check_table(table: &str) -> Result<()> {
    match table {
        "case_parties" | "case_annexures" => Ok(()),
        other => bail!("unsupported registry table: {other}"),
    }
}

/// Next 1-based serial for `(case_id, side)` in `table`. Returns 1 when none
/// exist yet. Not transactional on its own — callers that insert immediately
/// after should hold a transaction if concurrent inserts are possible.
pub async fn next_serial(db: &SqlitePool, case_id: &str, table: &str, side: &str) -> Result<i64> {
    check_table(table)?;
    let sql = format!(
        "SELECT COALESCE(MAX(serial_no), 0) + 1 FROM {table} WHERE case_id = ? AND side = ?"
    );
    let next: i64 = sqlx::query_scalar(&sql)
        .bind(case_id)
        .bind(side)
        .fetch_one(db)
        .await?;
    Ok(next)
}

/// Renumber `(case_id, side)` serials to a dense 1..n after a delete, keeping
/// the existing relative order. Runs in one transaction.
pub async fn compact_serials(
    db: &SqlitePool,
    case_id: &str,
    table: &str,
    side: &str,
) -> Result<()> {
    check_table(table)?;
    let select_sql = format!(
        "SELECT id FROM {table} WHERE case_id = ? AND side = ? ORDER BY serial_no, created_at"
    );
    let ids: Vec<(String,)> = sqlx::query_as(&select_sql)
        .bind(case_id)
        .bind(side)
        .fetch_all(db)
        .await?;

    let now = chrono::Utc::now().to_rfc3339();
    let update_sql = format!("UPDATE {table} SET serial_no = ?, updated_at = ? WHERE id = ?");
    let mut tx = db.begin().await?;
    for (idx, (id,)) in ids.iter().enumerate() {
        sqlx::query(&update_sql)
            .bind((idx + 1) as i64)
            .bind(&now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Apply an explicit ordering: `ordered_ids[i]` gets `serial_no = i + 1`.
/// Only rows in `ordered_ids` are touched; any omitted rows keep their old
/// serials (caller is responsible for passing the full set if a dense
/// renumber is desired). One transaction.
pub async fn reorder(
    db: &SqlitePool,
    case_id: &str,
    table: &str,
    side: &str,
    ordered_ids: &[String],
) -> Result<()> {
    check_table(table)?;
    let now = chrono::Utc::now().to_rfc3339();
    // Scope the WHERE to (case_id, side) so a stray id from another case/side
    // can't be renumbered through this helper.
    let update_sql = format!(
        "UPDATE {table} SET serial_no = ?, updated_at = ? \
         WHERE id = ? AND case_id = ? AND side = ?"
    );
    let mut tx = db.begin().await?;
    for (idx, id) in ordered_ids.iter().enumerate() {
        sqlx::query(&update_sql)
            .bind((idx + 1) as i64)
            .bind(&now)
            .bind(id)
            .bind(case_id)
            .bind(side)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Seeding: parties from case_summary findings + cases.parties_json
// ---------------------------------------------------------------------------

/// A party gathered from a finding before it's persisted.
struct SeedParty {
    name: String,
    side: &'static str,
}

/// Pull party names out of one value under `parties.<key>`. The case_summary
/// agent emits strings (sometimes comma-joined) for petitioner/respondent and
/// an array for `other`, but we accept array-of-string, array-of-{name}, and a
/// single string defensively.
fn extract_names(value: Option<&Value>) -> Vec<String> {
    let mut out = Vec::new();
    let Some(v) = value else { return out };
    match v {
        Value::String(s) => {
            // A single field may hold several comma-separated names.
            for part in s.split(',') {
                let t = clean_name(part);
                if !t.is_empty() {
                    out.push(t);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(s) => {
                        let t = clean_name(s);
                        if !t.is_empty() {
                            out.push(t);
                        }
                    }
                    Value::Object(_) => {
                        if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                            let t = clean_name(name);
                            if !t.is_empty() {
                                out.push(t);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

/// Trim surrounding whitespace and stray punctuation/quotes from a name.
fn clean_name(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '.' || c == ';')
        .trim()
        .to_string()
}

/// Normalised name key for dedupe: lowercase alphanumerics only.
fn name_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Seed `case_parties` from the latest `case_summary` finding plus
/// `cases.parties_json`. Dedupes by normalised name, skips slugs already in
/// the registry, assigns per-side serials in encounter order, source `'ai'`.
/// Returns the number of new parties inserted.
pub async fn seed_parties_from_findings(db: &SqlitePool, case_id: &str) -> Result<usize> {
    let mut seeds: Vec<SeedParty> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    // Pre-seed with existing party names so re-running this is idempotent — a
    // party already in the registry (by normalised name) is never re-inserted.
    let existing_names: Vec<(String,)> =
        sqlx::query_as("SELECT name FROM case_parties WHERE case_id = ?")
            .bind(case_id)
            .fetch_all(db)
            .await?;
    for (name,) in existing_names {
        seen_names.insert(name_key(&name));
    }

    // 1. Latest case_summary finding -> parties object.
    let finding: Option<(String,)> = sqlx::query_as(
        "SELECT content_json FROM case_findings \
         WHERE case_id = ? AND agent_name = 'case_summary' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(case_id)
    .fetch_optional(db)
    .await?;

    if let Some((content_json,)) = finding {
        if let Ok(parsed) = serde_json::from_str::<Value>(&content_json) {
            let parties = parsed.get("parties");
            collect_side(parties, "petitioner", "petitioner", &mut seeds, &mut seen_names);
            collect_side(parties, "respondent", "respondent", &mut seeds, &mut seen_names);
            // 'other' and any unknown role bucket map to the respondent side.
            collect_side(parties, "other", "respondent", &mut seeds, &mut seen_names);
        }
    }

    // 2. cases.parties_json — a JSON array of {name, role}.
    let parties_json: Option<(Option<String>,)> =
        sqlx::query_as("SELECT parties_json FROM cases WHERE id = ?")
            .bind(case_id)
            .fetch_optional(db)
            .await?;
    if let Some((Some(raw),)) = parties_json {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&raw) {
            for item in items {
                let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                let name = clean_name(name);
                if name.is_empty() {
                    continue;
                }
                let role = item
                    .get("role")
                    .and_then(|r| r.as_str())
                    .unwrap_or("")
                    .to_lowercase();
                let side = if role.contains("petition") || role.contains("plaintiff") || role.contains("applicant") || role.contains("complainant") {
                    "petitioner"
                } else {
                    // respondent, defendant, opposite party, other, unknown
                    "respondent"
                };
                let key = name_key(&name);
                if seen_names.insert(key) {
                    seeds.push(SeedParty { name, side });
                }
            }
        }
    }

    if seeds.is_empty() {
        return Ok(0);
    }

    // Existing slugs in this case so we can skip duplicates and avoid slug
    // collisions across BOTH sides (slug is unique per case_id).
    let mut existing_slugs: HashSet<String> = sqlx::query_as::<_, (String,)>(
        "SELECT slug FROM case_parties WHERE case_id = ?",
    )
    .bind(case_id)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|(s,)| s)
    .collect();

    // Per-side serial cursors, starting after whatever already exists.
    let mut next_pet = next_serial(db, case_id, "case_parties", "petitioner").await?;
    let mut next_res = next_serial(db, case_id, "case_parties", "respondent").await?;

    let now = chrono::Utc::now().to_rfc3339();
    let mut inserted = 0usize;
    let mut tx = db.begin().await?;
    for seed in seeds {
        let slug = unique_slug(&seed.name, &existing_slugs);
        // unique_slug never returns an existing slug, so this is always a new
        // party; reserve the slug for subsequent iterations.
        existing_slugs.insert(slug.clone());

        let serial = if seed.side == "petitioner" {
            let s = next_pet;
            next_pet += 1;
            s
        } else {
            let s = next_res;
            next_res += 1;
            s
        };

        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO case_parties \
             (id, case_id, slug, name, side, role_label, serial_no, details_json, source, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, NULL, ?, NULL, 'ai', ?, ?)",
        )
        .bind(&id)
        .bind(case_id)
        .bind(&slug)
        .bind(&seed.name)
        .bind(seed.side)
        .bind(serial)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        inserted += 1;
    }
    tx.commit().await?;
    Ok(inserted)
}

/// Gather one side from the `parties` object and append unseen names.
fn collect_side(
    parties: Option<&Value>,
    key: &str,
    side: &'static str,
    seeds: &mut Vec<SeedParty>,
    seen_names: &mut HashSet<String>,
) {
    let value = parties.and_then(|p| p.get(key));
    for name in extract_names(value) {
        if seen_names.insert(name_key(&name)) {
            seeds.push(SeedParty { name, side });
        }
    }
}

/// Build a slug from `name` that doesn't collide with `existing`. On collision,
/// append 2, 3, ... A non-empty base is guaranteed by falling back to "party".
fn unique_slug(name: &str, existing: &HashSet<String>) -> String {
    let mut base = slugify(name);
    if base.is_empty() {
        base = "party".to_string();
    }
    if !existing.contains(&base) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}{n}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

// ---------------------------------------------------------------------------
// Seeding: annexures from attached case documents
// ---------------------------------------------------------------------------

/// Seed `case_annexures` from `case_documents` not yet registered. Each new
/// document becomes one P-side annexure: slug from its filename, description
/// "A true copy of {filename}", serial_no the next per side 'P'. Returns the
/// number of new annexures inserted.
pub async fn seed_annexures_from_documents(db: &SqlitePool, case_id: &str) -> Result<usize> {
    // Attached docs joined to their filenames, excluding any already registered
    // as an annexure (UNIQUE(case_id, document_id) would also reject them).
    let docs: Vec<(String, String)> = sqlx::query_as(
        "SELECT cd.document_id, d.filename \
         FROM case_documents cd \
         JOIN documents d ON d.id = cd.document_id \
         WHERE cd.case_id = ? \
           AND cd.document_id NOT IN \
               (SELECT document_id FROM case_annexures WHERE case_id = ?) \
         ORDER BY cd.attached_at",
    )
    .bind(case_id)
    .bind(case_id)
    .fetch_all(db)
    .await?;

    if docs.is_empty() {
        return Ok(0);
    }

    let mut existing_slugs: HashSet<String> = sqlx::query_as::<_, (String,)>(
        "SELECT slug FROM case_annexures WHERE case_id = ?",
    )
    .bind(case_id)
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|(s,)| s)
    .collect();

    let mut next_serial = next_serial(db, case_id, "case_annexures", "P").await?;
    let now = chrono::Utc::now().to_rfc3339();
    let mut inserted = 0usize;
    let mut tx = db.begin().await?;
    for (document_id, filename) in docs {
        let slug = unique_slug(&filename, &existing_slugs);
        existing_slugs.insert(slug.clone());
        let description = format!("A true copy of {filename}");

        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO case_annexures \
             (id, case_id, document_id, slug, description, doc_date, side, serial_no, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, NULL, 'P', ?, ?, ?)",
        )
        .bind(&id)
        .bind(case_id)
        .bind(&document_id)
        .bind(&slug)
        .bind(&description)
        .bind(next_serial)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        next_serial += 1;
        inserted += 1;
    }
    tx.commit().await?;
    Ok(inserted)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_table_whitelist() {
        assert!(check_table("case_parties").is_ok());
        assert!(check_table("case_annexures").is_ok());
        assert!(check_table("documents").is_err());
        assert!(check_table("case_parties; DROP TABLE cases").is_err());
    }

    #[test]
    fn extract_names_handles_shapes() {
        // single string
        assert_eq!(
            extract_names(Some(&serde_json::json!("State of Kerala"))),
            vec!["State of Kerala".to_string()]
        );
        // comma-joined string -> split
        assert_eq!(
            extract_names(Some(&serde_json::json!("Ram, Shyam"))),
            vec!["Ram".to_string(), "Shyam".to_string()]
        );
        // array of strings
        assert_eq!(
            extract_names(Some(&serde_json::json!(["A", "B"]))),
            vec!["A".to_string(), "B".to_string()]
        );
        // array of {name}
        assert_eq!(
            extract_names(Some(&serde_json::json!([{"name": "Acme"}, {"name": "Beta"}]))),
            vec!["Acme".to_string(), "Beta".to_string()]
        );
        // missing -> empty
        assert!(extract_names(None).is_empty());
    }

    #[test]
    fn name_key_normalises() {
        assert_eq!(name_key("State of Kerala"), name_key("state  of kerala!"));
        assert_ne!(name_key("Ram Kumar"), name_key("Shyam Kumar"));
    }

    #[test]
    fn unique_slug_appends_on_collision() {
        let mut existing = HashSet::new();
        existing.insert("ramkumar".to_string());
        let s = unique_slug("Ram Kumar", &existing);
        assert_eq!(s, "ramkumar2");
        existing.insert(s);
        assert_eq!(unique_slug("Ram Kumar", &existing), "ramkumar3");
    }

    #[test]
    fn unique_slug_falls_back_when_empty() {
        let existing = HashSet::new();
        // A name with no alphanumerics slugifies to "" -> "party".
        assert_eq!(unique_slug("!!!", &existing), "party");
    }
}
