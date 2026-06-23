//! Firm-corpus agent tools.
//!
//! Two exec fns the drafting model calls to ground itself on the firm's
//! OWN prior work (past pleadings, skeleton arguments, deeds, templates)
//! instead of relying on blind top-k injection:
//!
//! * `search_firm_corpus` — FTS5 search over chunked corpus, with optional
//!   doc_type / case_type / section_role filters.
//! * `expand_chunk` — pull a hit chunk plus its seq-neighbours from the same
//!   file, so the model can read surrounding context after a search.
//!
//! Return values are JSON strings (same contract as `builtin_tools.rs`):
//! errors come back as `json!({"error": "..."}).to_string()`.

use crate::AppState;
use serde_json::{json, Value};
use sqlx::SqlitePool;
#[cfg(feature = "rag")]
use std::collections::HashMap;

/// Strip FTS5 operator characters so an LLM-supplied query can't produce a
/// MATCH syntax error. Mirrors `statute_tool::sanitize_fts_query`: keeps
/// alphanumerics and spaces, everything else becomes a space, remaining
/// bare terms are joined (implicit AND).
fn sanitize_fts_query(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whitelist the model-supplied enum filters so they can only ever match
/// values our own pipeline writes. Returns the validated value or `None`
/// (which the caller treats as "no filter").
fn whitelist(value: Option<&str>, allowed: &[&str]) -> Option<String> {
    let v = value?.trim().to_lowercase();
    if allowed.contains(&v.as_str()) {
        Some(v)
    } else {
        None
    }
}

const DOC_TYPES: &[&str] = &[
    "petition",
    "written_statement",
    "judgment",
    "skeleton_argument",
    "template",
    "deed",
    "notice",
    "other",
];
const CASE_TYPES: &[&str] = &[
    "matrimonial",
    "consumer",
    "criminal",
    "writ",
    "service",
    "ni_act",
    "civil",
    "other",
];
const SECTION_ROLES: &[&str] = &[
    "ground",
    "prayer",
    "argument",
    "clause",
    "facts",
    "verification",
    "cause_title",
    "other",
];

/// `search_firm_corpus` — args `{query, doc_type?, case_type?, section_role?, max_results?}`.
///
/// Hybrid retrieval when an embedding service is available (the `rag`
/// feature is on and the model is reachable): the FTS5/BM25 ranking is
/// fused with a vector-KNN ranking over `corpus_chunks_vec` via Reciprocal
/// Rank Fusion, so a query can surface the right passage even with no
/// keyword overlap. Falls back to BM25-only otherwise — byte-identical to
/// the original keyword search.
pub async fn exec_search_firm_corpus(state: &AppState, user_id: &str, args: &Value) -> String {
    #[cfg(feature = "rag")]
    if let Some(emb) = state.embeddings.as_deref() {
        return hybrid_search_firm_corpus(&state.db, emb, user_id, args).await;
    }
    bm25_search_firm_corpus(&state.db, user_id, args).await
}

/// Validated, whitelisted metadata filters shared by the BM25 and vector
/// candidate fetches.
#[derive(Default)]
struct Filters {
    doc_type: Option<String>,
    case_type: Option<String>,
    section_role: Option<String>,
    /// Restrict to reusable templates. Set when the model asks for
    /// `doc_type = "template"`: see `parse` for why that maps here.
    template_only: bool,
}

impl Filters {
    fn parse(args: &Value) -> Self {
        // `doc_type = "template"` is special. The ingest pipeline flags a
        // reusable skeleton via the separate `is_template` column and keeps
        // its CONTENT doc_type (a template is still e.g. a "deed"); it never
        // stores doc_type = "template". So the tool's advertised
        // doc_type="template" filter — the natural choice when the lawyer says
        // "use my templates" — would match zero rows as `f.doc_type =
        // 'template'`. Translate it into an `is_template = 1` filter instead.
        let doc_type = whitelist(args.get("doc_type").and_then(|v| v.as_str()), DOC_TYPES);
        let template_only = doc_type.as_deref() == Some("template");
        Filters {
            doc_type: if template_only { None } else { doc_type },
            case_type: whitelist(args.get("case_type").and_then(|v| v.as_str()), CASE_TYPES),
            section_role: whitelist(
                args.get("section_role").and_then(|v| v.as_str()),
                SECTION_ROLES,
            ),
            template_only,
        }
    }
}

fn parse_limit(args: &Value) -> i64 {
    args.get("max_results")
        .and_then(|v| v.as_i64())
        .unwrap_or(8)
        .clamp(1, 20)
}

/// One corpus search hit, independent of which ranker surfaced it.
struct CorpusHit {
    id: i64,
    filename: String,
    doc_type: Option<String>,
    case_type: Option<String>,
    court: Option<String>,
    doc_date: Option<String>,
    heading: Option<String>,
    section_role: Option<String>,
    page: Option<i64>,
    text: String,
}

/// The 10 hit columns, in SELECT order. (BM25 appends a trailing score.)
type HitRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    String,
);

impl CorpusHit {
    fn from_row(r: HitRow) -> Self {
        CorpusHit {
            id: r.0,
            filename: r.1,
            doc_type: r.2,
            case_type: r.3,
            court: r.4,
            doc_date: r.5,
            heading: r.6,
            section_role: r.7,
            page: r.8,
            text: r.9,
        }
    }

    fn to_json(&self, score: f64) -> Value {
        // Cap the returned snippet in one pass — chars().take returns the
        // whole text when it's already under the cap.
        let snippet: String = self.text.chars().take(1500).collect();
        json!({
            "chunk_id": self.id,
            "filename": self.filename,
            "doc_type": self.doc_type,
            "case_type": self.case_type,
            "court": self.court,
            "doc_date": self.doc_date,
            "heading": self.heading,
            "section_role": self.section_role,
            "page": self.page,
            "text": snippet,
            "score": score,
        })
    }
}

/// FTS5/BM25 candidates: the hit columns + the bm25 score, joined and
/// filtered, best-first. Shared by the BM25-only path and the hybrid
/// path's keyword half. The whitelisted filter values are bound params;
/// only the presence/absence of fixed clauses is dynamic SQL.
async fn bm25_candidates(
    db: &SqlitePool,
    user_id: &str,
    query: &str,
    filters: &Filters,
    limit: i64,
) -> Result<Vec<(CorpusHit, f64)>, sqlx::Error> {
    let mut sql = String::from(
        "SELECT c.id, f.filename, f.doc_type, f.case_type, f.court, f.doc_date, \
                c.heading, c.section_role, c.page, c.text, \
                bm25(corpus_chunks_fts, 5.0, 1.0) AS score \
         FROM corpus_chunks_fts \
         JOIN corpus_chunks c ON c.id = corpus_chunks_fts.rowid \
         JOIN corpus_files f ON f.id = c.file_id \
         WHERE corpus_chunks_fts MATCH ?1 \
           AND c.user_id = ?2 \
           AND f.status = 'ready'",
    );
    if filters.doc_type.is_some() {
        sql.push_str(" AND f.doc_type = ?3");
    }
    if filters.case_type.is_some() {
        sql.push_str(" AND f.case_type = ?4");
    }
    if filters.section_role.is_some() {
        sql.push_str(" AND c.section_role = ?5");
    }
    if filters.template_only {
        sql.push_str(" AND f.is_template = 1");
    }
    sql.push_str(" ORDER BY score LIMIT ?6");

    let rows: Vec<(
        i64,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        String,
        f64,
    )> = sqlx::query_as(&sql)
        .bind(query)
        .bind(user_id)
        .bind(filters.doc_type.as_deref().unwrap_or(""))
        .bind(filters.case_type.as_deref().unwrap_or(""))
        .bind(filters.section_role.as_deref().unwrap_or(""))
        .bind(limit)
        .fetch_all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let score = r.10;
            (
                CorpusHit::from_row((r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9)),
                score,
            )
        })
        .collect())
}

/// BM25-only firm-corpus search. The fallback when the `rag` feature is off
/// or no embedding service is available. Output is identical to the
/// original keyword-only tool.
pub(crate) async fn bm25_search_firm_corpus(db: &SqlitePool, user_id: &str, args: &Value) -> String {
    let raw_query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
    if raw_query.is_empty() {
        return json!({"error": "query is required"}).to_string();
    }
    let query = sanitize_fts_query(raw_query);
    if query.is_empty() {
        return json!({
            "results": [],
            "count": 0,
            "note": "Query had no searchable terms after sanitization."
        })
        .to_string();
    }
    let limit = parse_limit(args);
    let filters = Filters::parse(args);

    match bm25_candidates(db, user_id, &query, &filters, limit).await {
        Ok(hits) => {
            let results: Vec<Value> = hits.iter().map(|(h, score)| h.to_json(*score)).collect();
            let count = results.len();
            json!({ "results": results, "count": count }).to_string()
        }
        Err(e) => json!({"error": format!("corpus search failed: {e}")}).to_string(),
    }
}

/// Reciprocal Rank Fusion constant. The standard k=60: large enough that
/// the score depends mostly on rank ordering, not the exact rank values,
/// which lets us fuse BM25 (negative scores) and cosine distance (0..2)
/// without normalising their incomparable scales.
#[cfg(feature = "rag")]
const RRF_K: f64 = 60.0;

/// Hybrid firm-corpus search: fuse the BM25 keyword ranking with a
/// vector-KNN ranking via Reciprocal Rank Fusion. Embeds the query with the
/// e5 model; if embedding fails we degrade gracefully to BM25-only so a
/// transient model problem never breaks search.
#[cfg(feature = "rag")]
pub async fn hybrid_search_firm_corpus(
    db: &SqlitePool,
    emb: &crate::embeddings::EmbeddingService,
    user_id: &str,
    args: &Value,
) -> String {
    let raw_query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
    if raw_query.is_empty() {
        return json!({"error": "query is required"}).to_string();
    }
    let qvec = match emb.embed_query(raw_query).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[corpus] query embed failed, falling back to BM25-only: {e}");
            return bm25_search_firm_corpus(db, user_id, args).await;
        }
    };
    hybrid_search_core(db, &qvec, user_id, args).await
}

/// The embedding-agnostic fusion core: given a pre-computed query vector,
/// run both rankers and re-rank by RRF. Separated so tests can drive it
/// with a fabricated vector (no model download).
#[cfg(feature = "rag")]
async fn hybrid_search_core(db: &SqlitePool, qvec: &[f32], user_id: &str, args: &Value) -> String {
    let raw_query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
    let limit = parse_limit(args);
    let filters = Filters::parse(args);
    // Over-fetch from each ranker so the fusion has enough candidates to
    // work with, then truncate to the caller's limit.
    let cand = (limit * 4).clamp(10, 50);

    // Keyword half — skipped when the sanitised query is empty (the vector
    // half still carries the search).
    let fts_query = sanitize_fts_query(raw_query);
    let bm25_hits: Vec<(CorpusHit, f64)> = if fts_query.is_empty() {
        Vec::new()
    } else {
        match bm25_candidates(db, user_id, &fts_query, &filters, cand).await {
            Ok(h) => h,
            Err(e) => return json!({"error": format!("corpus search failed: {e}")}).to_string(),
        }
    };

    // Vector half — KNN ids in distance order, then a metadata fetch that
    // re-applies the same filters (vec0 can't filter on the joined columns).
    let knn = match vec_knn(db, qvec, user_id, cand).await {
        Ok(k) => k,
        Err(e) => return json!({"error": format!("corpus vector search failed: {e}")}).to_string(),
    };
    let knn_ids: Vec<i64> = knn.iter().map(|(id, _)| *id).collect();
    let vec_hits: HashMap<i64, CorpusHit> =
        match fetch_hits_by_ids(db, user_id, &knn_ids, &filters).await {
            Ok(m) => m,
            Err(e) => {
                return json!({"error": format!("corpus vector search failed: {e}")}).to_string()
            }
        };

    // Rank lists for RRF (vector list filtered to rows that survived the
    // metadata filter, preserving distance order).
    let bm25_ids: Vec<i64> = bm25_hits.iter().map(|(h, _)| h.id).collect();
    let vec_ids: Vec<i64> = knn_ids
        .into_iter()
        .filter(|id| vec_hits.contains_key(id))
        .collect();

    // Unified row data keyed by chunk id (a chunk found by both rankers
    // carries identical data, so either insert wins).
    let mut hits: HashMap<i64, CorpusHit> = HashMap::new();
    for (h, _) in bm25_hits {
        hits.insert(h.id, h);
    }
    for (id, h) in vec_hits {
        hits.entry(id).or_insert(h);
    }

    let fused = fuse_rrf(&bm25_ids, &vec_ids, RRF_K);
    let results: Vec<Value> = fused
        .iter()
        .take(limit as usize)
        .filter_map(|(id, score)| hits.get(id).map(|h| h.to_json(*score)))
        .collect();
    let count = results.len();
    json!({ "results": results, "count": count }).to_string()
}

/// Top-`k` nearest corpus chunks for `qvec`, as `(chunk_id, distance)`
/// ordered by ascending distance, partition-filtered to the user.
#[cfg(feature = "rag")]
async fn vec_knn(
    db: &SqlitePool,
    qvec: &[f32],
    user_id: &str,
    k: i64,
) -> Result<Vec<(i64, f32)>, sqlx::Error> {
    let blob = crate::embeddings::service::vec_to_blob(qvec);
    sqlx::query_as(
        "SELECT chunk_id, distance \
         FROM corpus_chunks_vec \
         WHERE user_id = ? AND embedding MATCH ? AND k = ? \
         ORDER BY distance",
    )
    .bind(user_id)
    .bind(&blob[..])
    .bind(k)
    .fetch_all(db)
    .await
}

/// Fetch the hit columns for a set of chunk ids, re-applying the metadata
/// filters + the user/ready guards. Returned as a map for O(1) lookup
/// during fusion.
#[cfg(feature = "rag")]
async fn fetch_hits_by_ids(
    db: &SqlitePool,
    user_id: &str,
    ids: &[i64],
    filters: &Filters,
) -> Result<HashMap<i64, CorpusHit>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "SELECT c.id, f.filename, f.doc_type, f.case_type, f.court, f.doc_date, \
                c.heading, c.section_role, c.page, c.text \
         FROM corpus_chunks c \
         JOIN corpus_files f ON f.id = c.file_id \
         WHERE c.user_id = ? \
           AND f.status = 'ready' \
           AND c.id IN ({placeholders})"
    );
    if filters.doc_type.is_some() {
        sql.push_str(" AND f.doc_type = ?");
    }
    if filters.case_type.is_some() {
        sql.push_str(" AND f.case_type = ?");
    }
    if filters.section_role.is_some() {
        sql.push_str(" AND c.section_role = ?");
    }
    if filters.template_only {
        sql.push_str(" AND f.is_template = 1");
    }

    let mut q = sqlx::query_as::<_, HitRow>(&sql).bind(user_id);
    for id in ids {
        q = q.bind(*id);
    }
    if let Some(dt) = &filters.doc_type {
        q = q.bind(dt.as_str());
    }
    if let Some(ct) = &filters.case_type {
        q = q.bind(ct.as_str());
    }
    if let Some(sr) = &filters.section_role {
        q = q.bind(sr.as_str());
    }

    let rows: Vec<HitRow> = q.fetch_all(db).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let hit = CorpusHit::from_row(r);
            (hit.id, hit)
        })
        .collect())
}

/// Reciprocal Rank Fusion: combine two ranked id lists into one, scoring
/// each id by the sum of `1 / (k + rank + 1)` (rank is 0-indexed) across the
/// lists it appears in. Ties (same fused score) break by ascending id for
/// deterministic output.
#[cfg(feature = "rag")]
fn fuse_rrf(bm25_ids: &[i64], vec_ids: &[i64], k: f64) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for (rank, &id) in bm25_ids.iter().enumerate() {
        *scores.entry(id).or_insert(0.0) += 1.0 / (k + rank as f64 + 1.0);
    }
    for (rank, &id) in vec_ids.iter().enumerate() {
        *scores.entry(id).or_insert(0.0) += 1.0 / (k + rank as f64 + 1.0);
    }
    let mut out: Vec<(i64, f64)> = scores.into_iter().collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    out
}

/// `expand_chunk` — args `{chunk_id, before?, after?}`. Returns the chunk plus
/// its seq-neighbours from the same file, with file-level metadata.
pub async fn exec_expand_chunk(state: &AppState, user_id: &str, args: &Value) -> String {
    let chunk_id = match args.get("chunk_id").and_then(|v| v.as_i64()) {
        Some(id) => id,
        // Tolerate a stringified id from a loose model.
        None => match args
            .get("chunk_id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.trim().parse::<i64>().ok())
        {
            Some(id) => id,
            None => return json!({"error": "chunk_id (integer) is required"}).to_string(),
        },
    };

    let before = args.get("before").and_then(|v| v.as_i64()).unwrap_or(2).clamp(0, 5);
    let after = args.get("after").and_then(|v| v.as_i64()).unwrap_or(2).clamp(0, 5);

    // Resolve the anchor chunk (scoped to the user) → its file + seq.
    let anchor: Option<(String, i64)> = sqlx::query_as(
        "SELECT file_id, seq FROM corpus_chunks WHERE id = ? AND user_id = ?",
    )
    .bind(chunk_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let Some((file_id, seq)) = anchor else {
        return json!({"error": format!("chunk {chunk_id} not found")}).to_string();
    };

    // File metadata.
    let file_meta: Option<(
        String,         // filename
        Option<String>, // doc_type
        Option<String>, // case_type
        Option<String>, // court
        Option<String>, // doc_date
        Option<String>, // language
        i64,            // chunk_count
    )> = sqlx::query_as(
        "SELECT filename, doc_type, case_type, court, doc_date, language, chunk_count \
         FROM corpus_files WHERE id = ? AND user_id = ?",
    )
    .bind(&file_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();

    let file_json = match file_meta {
        Some((filename, dt, ct, court, doc_date, language, chunk_count)) => json!({
            "file_id": file_id,
            "filename": filename,
            "doc_type": dt,
            "case_type": ct,
            "court": court,
            "doc_date": doc_date,
            "language": language,
            "chunk_count": chunk_count,
        }),
        None => json!({"file_id": file_id}),
    };

    let lo = seq - before;
    let hi = seq + after;
    let chunk_rows: Vec<(i64, Option<String>, Option<String>, Option<i64>, String)> =
        sqlx::query_as(
            "SELECT seq, heading, section_role, page, text \
             FROM corpus_chunks \
             WHERE file_id = ? AND seq >= ? AND seq <= ? \
             ORDER BY seq",
        )
        .bind(&file_id)
        .bind(lo)
        .bind(hi)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let chunks: Vec<Value> = chunk_rows
        .into_iter()
        .map(|(seq, heading, role, page, text)| {
            json!({
                "seq": seq,
                "heading": heading,
                "section_role": role,
                "page": page,
                "text": text,
            })
        })
        .collect();

    json!({
        "file": file_json,
        "chunks": chunks,
    })
    .to_string()
}

// ── Direct injection helpers (small / offline models) ────────────────────────
//
// Small local models get no tool schemas, so they can never call
// `search_firm_corpus`. These helpers let the chat route pull the same firm
// knowledge directly and inject a trimmed copy into the system prompt instead.

/// A firm template the lawyer marked reusable, cleaned into a `{{placeholder}}`
/// markdown skeleton at upload time (see `routes::corpus::build_template`).
pub struct FirmTemplate {
    pub filename: String,
    pub template_md: String,
}

/// Ready firm templates that already have a built skeleton, newest first.
/// Empty on any error — template injection is best-effort.
pub async fn firm_templates(db: &SqlitePool, user_id: &str, limit: i64) -> Vec<FirmTemplate> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT filename, template_md FROM corpus_files \
         WHERE user_id = ? AND status = 'ready' AND is_template = 1 \
           AND template_md IS NOT NULL AND template_md != '' \
         ORDER BY created_at DESC LIMIT ?",
    )
    .bind(user_id)
    .bind(limit.clamp(1, 5))
    .fetch_all(db)
    .await
    .unwrap_or_else(|e| {
        tracing::warn!("[corpus] firm_templates query failed, drafting will proceed without firm templates: {e}");
        Vec::new()
    })
    .into_iter()
    .map(|(filename, template_md)| FirmTemplate { filename, template_md })
    .collect()
}

/// One firm-corpus passage for direct prompt injection.
pub struct FirmSnippet {
    pub filename: String,
    pub heading: Option<String>,
    pub text: String,
}

/// Top firm-corpus passages for a query, BM25-only so it needs no embedding
/// model (works offline). Empty on no match or any error.
pub async fn top_firm_snippets(
    db: &SqlitePool,
    user_id: &str,
    query: &str,
    limit: i64,
) -> Vec<FirmSnippet> {
    let q = sanitize_fts_query(query);
    if q.trim().is_empty() {
        return Vec::new();
    }
    let filters = Filters::default();
    match bm25_candidates(db, user_id, &q, &filters, limit.clamp(1, 6)).await {
        Ok(hits) => hits
            .into_iter()
            .map(|(h, _score)| FirmSnippet {
                filename: h.filename,
                heading: h.heading,
                text: h.text,
            })
            .collect(),
        Err(e) => {
            tracing::warn!("[corpus] top_firm_snippets BM25 search failed, drafting will proceed without firm snippets: {e}");
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_fts_operators() {
        assert_eq!(sanitize_fts_query("dishonour AND \"cheque\"*"), "dishonour AND cheque");
        assert_eq!(sanitize_fts_query("   "), "");
        assert_eq!(sanitize_fts_query("(420) OR -bad"), "420 OR bad");
    }

    #[test]
    fn whitelist_only_accepts_known_values() {
        assert_eq!(whitelist(Some("Petition"), DOC_TYPES).as_deref(), Some("petition"));
        assert_eq!(whitelist(Some("nonsense"), DOC_TYPES), None);
        assert_eq!(whitelist(None, DOC_TYPES), None);
        assert_eq!(whitelist(Some("GROUND"), SECTION_ROLES).as_deref(), Some("ground"));
        assert_eq!(whitelist(Some("ni_act"), CASE_TYPES).as_deref(), Some("ni_act"));
    }

    #[cfg(feature = "rag")]
    #[test]
    fn rrf_rewards_agreement_and_breaks_ties_by_id() {
        // id 2 is ranked #1 by BM25 AND #1 by the vector list → highest fused
        // score. ids 1 and 3 each appear in only one list at rank #2, so they
        // tie and break by ascending id.
        let fused = fuse_rrf(&[2, 1], &[2, 3], RRF_K);
        assert_eq!(fused[0].0, 2, "the chunk both rankers agree on ranks first");
        assert_eq!(fused[1].0, 1, "equal single-list scores break by ascending id");
        assert_eq!(fused[2].0, 3);
        assert!(fused[0].1 > fused[1].1, "agreement scores strictly higher");
    }

    /// Deterministic end-to-end of the hybrid path with FABRICATED vectors —
    /// proves the 0043 migration, the `corpus_chunks_vec` KNN, the join/filter
    /// and the RRF fusion without loading the e5 model. The live-model semantic
    /// proof lives in `tests/corpus_semantic_search.rs`.
    #[cfg(feature = "rag")]
    #[tokio::test]
    async fn hybrid_surfaces_vector_only_match() {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr;

        crate::embeddings::register_sqlite_vec_auto_extension();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str("sqlite::memory:")
                    .unwrap()
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let user = "u1";
        sqlx::query("INSERT INTO user_profiles (id, username, pin_hash) VALUES (?, ?, 'x')")
            .bind(user)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();

        // Two single-chunk files. The query below shares NO tokens with either
        // chunk, so the BM25 half contributes nothing and only the fabricated
        // vectors decide the order.
        for (fid, fname, text) in [
            ("f-a", "a.txt", "alpha bravo charlie"),
            ("f-b", "b.txt", "delta echo foxtrot"),
        ] {
            sqlx::query(
                "INSERT INTO corpus_files (id, user_id, filename, file_type, sha256, status) \
                 VALUES (?, ?, ?, 'txt', ?, 'ready')",
            )
            .bind(fid)
            .bind(user)
            .bind(fname)
            .bind(format!("sha-{fid}"))
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO corpus_chunks (file_id, user_id, seq, section_role, text) \
                 VALUES (?, ?, 0, 'argument', ?)",
            )
            .bind(fid)
            .bind(user)
            .bind(text)
            .execute(&pool)
            .await
            .unwrap();
        }

        let chunks: Vec<(i64, String)> =
            sqlx::query_as("SELECT id, file_id FROM corpus_chunks ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(chunks.len(), 2);
        let (id_a, file_a) = (chunks[0].0, chunks[0].1.clone());
        let (id_b, file_b) = (chunks[1].0, chunks[1].1.clone());

        // Orthogonal unit vectors: A = e0, B = e1; query = e0 (identical to A,
        // distance 0; far from B). Holds under both L2 and cosine.
        let mut va = vec![0.0_f32; 768];
        va[0] = 1.0;
        let mut vb = vec![0.0_f32; 768];
        vb[1] = 1.0;
        let qv = va.clone();

        for (cid, fid, v) in [(id_a, &file_a, &va), (id_b, &file_b, &vb)] {
            let blob = crate::embeddings::service::vec_to_blob(v);
            sqlx::query(
                "INSERT INTO corpus_chunks_vec (embedding, user_id, chunk_id, file_id) \
                 VALUES (?, ?, ?, ?)",
            )
            .bind(blob)
            .bind(user)
            .bind(cid)
            .bind(fid)
            .execute(&pool)
            .await
            .unwrap();
        }

        let args = json!({ "query": "zzz totally unrelated keywords" });
        let out = hybrid_search_core(&pool, &qv, user, &args).await;
        let v: Value = serde_json::from_str(&out).unwrap();
        let results = v["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "both chunks surface via the vector half: {out}");
        assert_eq!(
            results[0]["chunk_id"].as_i64(),
            Some(id_a),
            "the chunk whose vector matches the query ranks first"
        );
        assert_eq!(results[1]["chunk_id"].as_i64(), Some(id_b));
    }

    /// A freshly-ingested chunk becomes retrievable exactly when its file
    /// reaches `ready`: BM25 search returns the ready file's chunk and skips an
    /// identical chunk whose file is still `pending`. This is why a folder
    /// upload's docs surface in drafting only after ingest completes.
    #[cfg(feature = "rag")]
    #[tokio::test]
    async fn search_returns_ready_chunk_and_hides_pending() {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr;

        crate::embeddings::register_sqlite_vec_auto_extension();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str("sqlite::memory:")
                    .unwrap()
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let user = "u1";
        sqlx::query("INSERT INTO user_profiles (id, username, pin_hash) VALUES (?, ?, 'x')")
            .bind(user)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();

        // Same distinctive text in both files; only the status differs.
        for (fid, status) in [("f-ready", "ready"), ("f-pending", "pending")] {
            sqlx::query(
                "INSERT INTO corpus_files (id, user_id, filename, file_type, sha256, status) \
                 VALUES (?, ?, ?, 'txt', ?, ?)",
            )
            .bind(fid)
            .bind(user)
            .bind(format!("{fid}.txt"))
            .bind(format!("sha-{fid}"))
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO corpus_chunks (file_id, user_id, seq, section_role, text) \
                 VALUES (?, ?, 0, 'argument', 'A bespoke indemnity covenant for the lessor.')",
            )
            .bind(fid)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();
        }

        let out = bm25_search_firm_corpus(&pool, user, &json!({ "query": "indemnity covenant" })).await;
        let v: Value = serde_json::from_str(&out).unwrap();
        let results = v["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1, "only the ready file's chunk is returned: {out}");
        assert_eq!(results[0]["filename"].as_str(), Some("f-ready.txt"));
    }

    #[test]
    fn template_doc_type_maps_to_is_template_filter() {
        // doc_type="template" is translated to an is_template filter, not a
        // doomed f.doc_type='template' match (templates carry a CONTENT
        // doc_type + the separate is_template flag).
        let f = Filters::parse(&json!({"query": "x", "doc_type": "template"}));
        assert!(f.template_only, "template doc_type sets template_only");
        assert_eq!(f.doc_type, None, "template is not also matched as a content doc_type");

        // A real content doc_type still filters on doc_type, not is_template.
        let f = Filters::parse(&json!({"doc_type": "petition"}));
        assert!(!f.template_only);
        assert_eq!(f.doc_type.as_deref(), Some("petition"));

        // Capitalisation tolerated by whitelist().
        assert!(Filters::parse(&json!({"doc_type": "TEMPLATE"})).template_only);

        // No doc_type filter at all.
        let f = Filters::parse(&json!({"query": "x"}));
        assert!(!f.template_only);
        assert_eq!(f.doc_type, None);
    }

    /// Regression for the firm-knowledge retrieval defect: a file uploaded as
    /// a template (is_template = 1) carries a NULL/content doc_type, never
    /// doc_type = "template". A search the model naturally issues —
    /// doc_type = "template" — must still find it (via the is_template
    /// mapping), while a genuine mismatched content doc_type filter must not.
    #[cfg(feature = "rag")]
    #[tokio::test]
    async fn template_filter_finds_is_template_file_with_null_doc_type() {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        use std::str::FromStr;

        crate::embeddings::register_sqlite_vec_auto_extension();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::from_str("sqlite::memory:")
                    .unwrap()
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();

        let user = "u1";
        sqlx::query("INSERT INTO user_profiles (id, username, pin_hash) VALUES (?, ?, 'x')")
            .bind(user)
            .bind(user)
            .execute(&pool)
            .await
            .unwrap();

        // The real ingest shape for an uploaded template that the tagger did
        // not classify: doc_type NULL, is_template = 1, ready.
        sqlx::query(
            "INSERT INTO corpus_files (id, user_id, filename, file_type, sha256, is_template, status) \
             VALUES ('f1', ?, 'Condonation Template.docx', 'docx', 'sha1', 1, 'ready')",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO corpus_chunks (file_id, user_id, seq, heading, section_role, text) \
             VALUES ('f1', ?, 0, 'APPLICATION', 'other', \
                     'Application for condonation of delay in filing the appeal')",
        )
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();

        let count = |out: String| -> usize {
            serde_json::from_str::<Value>(&out).unwrap()["count"]
                .as_u64()
                .unwrap() as usize
        };

        // doc_type="template" now finds the is_template file (the fix).
        let out =
            bm25_search_firm_corpus(&pool, user, &json!({"query":"condonation delay","doc_type":"template"}))
                .await;
        assert_eq!(count(out), 1, "doc_type=template must find an is_template file");

        // A bare query still finds it (unchanged behaviour).
        let out = bm25_search_firm_corpus(&pool, user, &json!({"query":"condonation delay"})).await;
        assert_eq!(count(out), 1);

        // A genuine content-type filter the file lacks still excludes it — the
        // fix does not loosen real doc_type filtering.
        let out =
            bm25_search_firm_corpus(&pool, user, &json!({"query":"condonation delay","doc_type":"petition"}))
                .await;
        assert_eq!(count(out), 0, "a mismatched content doc_type still filters out");
    }
}
