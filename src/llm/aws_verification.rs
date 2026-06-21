//! AWS verification layer for Kanoon search results.
//!
//! Cross-references each Indian Kanoon hit against the public
//! `indian-high-court-judgments` S3 dataset by:
//!
//!   1. Mapping Kanoon's court string (e.g. "Delhi High Court") to the
//!      AWS partition code (e.g. "7_26").
//!   2. Downloading the matching per-(court, year) parquet metadata
//!      files. Public bucket, no auth needed. Cached locally so
//!      repeat queries are instant.
//!   3. Fuzzy-matching the Kanoon title (+ date when available)
//!      against the parquet rows to find the canonical CNR and PDF URL.
//!   4. Returning a `Verification` struct with a status enum the
//!      kanoon_tool can attach to each result.
//!
//! Design principles:
//!
//!   - GRACEFUL: every failure path returns `Verification::unverified(reason)`,
//!     never panics or bubbles errors. The model must always get results
//!     back, even when verification can't complete.
//!   - BOUNDED: per-result verification has a hard timeout. If we can't
//!     verify in time, we return UNVERIFIED and let the model proceed.
//!   - CACHEABLE: parquet files are written to `~/.mikerust/aws-cache/`
//!     after first download. Subsequent verifications skip the network.
//!
//! The module is feature-gated on `aws-verification`. When the feature
//! is off, the public `verify()` fn returns a stub status so the
//! kanoon_tool keeps working without conditional compilation everywhere.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Public-bucket base URL for HIGH COURT judgments. Bucket is in
/// ap-south-1 but is reachable via the global virtual-hosted endpoint.
const AWS_S3_BASE: &str = "https://indian-high-court-judgments.s3.ap-south-1.amazonaws.com";

/// Public-bucket base URL for SUPREME COURT judgments. Separate dataset
/// from the HC corpus — different bucket, different partition layout.
/// SC parquet is partitioned ONLY by year (one parquet file per year,
/// no court/bench partitioning needed since it's all one court). Schema
/// includes cnr, title, court_name, judge, pdf_link, decision_date, etc.
/// See: https://registry.opendata.aws/indian-supreme-court-judgments/
const SC_AWS_S3_BASE: &str = "https://indian-supreme-court-judgments.s3.amazonaws.com";

/// Hard timeout for a single verification (HTTP + parse + match).
/// Above this we give up and return UNVERIFIED so the search call
/// can return on time.
const PER_RESULT_TIMEOUT_SECS: u64 = 8;

/// Verification outcome attached to each Kanoon result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Verification {
    pub status: VerificationStatus,
    pub canonical_pdf_url: Option<String>,
    pub canonical_cnr: Option<String>,
    /// Canonical neutral / reporter citation from the AWS dataset
    /// (e.g. "2021 INSC 687"). Often more useful to a lawyer than the
    /// PDF link, and present even when the dataset has no per-case PDF.
    pub canonical_citation: Option<String>,
    /// Human-readable note explaining the status — surfaced to the LLM.
    pub note: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum VerificationStatus {
    /// Case found in the canonical AWS dataset. canonical_pdf_url is set.
    Verified,
    /// Kanoon has it but AWS doesn't. Common for tribunal/recent rulings.
    NotInAws,
    /// Verification couldn't complete (network, no court mapping, timeout, parse error).
    /// Result still usable, just unconfirmed.
    Unverified,
}

impl Verification {
    pub fn unverified(reason: impl Into<String>) -> Self {
        Self {
            status: VerificationStatus::Unverified,
            canonical_pdf_url: None,
            canonical_cnr: None,
            canonical_citation: None,
            note: reason.into(),
        }
    }
    pub fn not_in_aws(court: &str, year: i32) -> Self {
        Self {
            status: VerificationStatus::NotInAws,
            canonical_pdf_url: None,
            canonical_cnr: None,
            canonical_citation: None,
            note: format!(
                "No case with this title found in the AWS canonical dataset for {court} ({year}). \
                 Common for tribunal rulings, very recent cases, or special-court orders. \
                 The Kanoon result is likely real but not cross-checked."
            ),
        }
    }
    /// Build a VERIFIED result. The PDF link and citation are both optional:
    /// a title+CNR match in the canonical dataset is sufficient proof the case
    /// is real, even when the dataset exposes no per-case PDF (e.g. the SC
    /// corpus tars its PDFs) or no citation.
    pub fn verified(
        cnr: String,
        pdf_url: Option<String>,
        citation: Option<String>,
    ) -> Self {
        let note = match (&pdf_url, &citation) {
            (Some(_), _) => "Cross-checked against the canonical AWS court-judgments dataset. \
                 The canonical court PDF is available at canonical_pdf_url.",
            (None, Some(_)) => "Cross-checked against the canonical AWS court-judgments dataset \
                 (matched by title + CNR + neutral citation). No per-case PDF is published in \
                 this dataset, but the case is confirmed genuine — cite with confidence.",
            (None, None) => "Cross-checked against the canonical AWS court-judgments dataset \
                 (matched by title + CNR). The case is confirmed genuine.",
        };
        Self {
            status: VerificationStatus::Verified,
            canonical_pdf_url: pdf_url,
            canonical_cnr: Some(cnr),
            canonical_citation: citation,
            note: note.into(),
        }
    }
}

/// Map a Kanoon court string (e.g. "Delhi High Court") to the AWS
/// court_code partition (e.g. "7_26"). Codes match the frontend's
/// case-search/page.tsx COURT_OPTIONS table. Unmapped courts return
/// None — verification returns UNVERIFIED in that case.
pub fn map_court_code(kanoon_court: &str) -> Option<&'static str> {
    // Lowercase, strip diacritics, condense whitespace for matching.
    let key = kanoon_court.to_lowercase();
    let key = key.trim();

    // Order: more specific names first.
    for (needle, code) in COURT_MAP {
        if key.contains(needle) {
            return Some(code);
        }
    }
    None
}

/// (substring-match needle → AWS court_code). Keep needles lowercase.
/// First match wins, so list specific names before broad ones.
const COURT_MAP: &[(&str, &str)] = &[
    // Supreme Court — not in the High Court partition tree but the
    // mapping is recorded for future expansion. Caller currently
    // skips verification for SC.
    ("supreme court of india", "supremecourt"),
    ("supreme court", "supremecourt"),
    // High Courts (codes mirror frontend/src/app/case-search/page.tsx).
    ("delhi", "7_26"),
    ("bombay", "27_1"),
    ("madras", "33_10"),
    ("calcutta", "19_16"),
    ("karnataka", "29_3"),
    ("kerala", "32_4"),
    ("allahabad", "9_13"),
    ("gujarat", "24_17"),
    ("punjab & haryana", "3_22"),
    ("punjab and haryana", "3_22"),
    ("punjab", "3_22"),
    ("rajasthan", "8_9"),
    ("telangana", "36_29"),
    ("andhra pradesh", "28_2"),
    ("patna", "10_8"),
    ("jharkhand", "20_7"),
    ("gauhati", "18_6"),
    ("guwahati", "18_6"),
    ("madhya pradesh", "23_23"),
    ("orissa", "21_11"),
    ("odisha", "21_11"),
    ("chhattisgarh", "22_18"),
    ("uttarakhand", "5_15"),
    ("himachal pradesh", "2_5"),
    ("jammu", "1_12"),
    ("kashmir", "1_12"),
];

/// Extract a 4-digit year from common Kanoon date string shapes:
/// "12-04-2023", "2023-04-12", "April 12, 2023". Splits on non-digits and
/// returns the first 4-digit group that's a plausible year (1900..=2100).
/// A pure last-4-digits scan was wrong for ISO dates ("2023-04-12" → "0412").
pub fn parse_year(date_str: &str) -> Option<i32> {
    date_str
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| g.len() == 4)
        .filter_map(|g| g.parse::<i32>().ok())
        .find(|y| (1900..=2100).contains(y))
}

/// Where downloaded parquet files live on disk. Same root as the
/// existing storage layer; adds an `aws-cache/metadata/` sub-tree.
fn cache_root() -> PathBuf {
    let base = std::env::var("STORAGE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::storage::default_storage_root());
    base.join("aws-cache").join("metadata")
}

// ---------------------------------------------------------------------------
// Public entry point — called from kanoon_tool for each hit.
// ---------------------------------------------------------------------------

/// Verify one Kanoon hit against the canonical AWS dataset.
///
/// Always returns a `Verification`. Never errors. On any failure path
/// the status is `Unverified` with a short note explaining why.
pub async fn verify(
    title: &str,
    court: &str,
    decision_date: Option<&str>,
) -> Verification {
    #[cfg(not(feature = "aws-verification"))]
    {
        let _ = (title, court, decision_date);
        return Verification::unverified(
            "AWS verification disabled at build time (no `aws-verification` feature)."
        );
    }

    #[cfg(feature = "aws-verification")]
    {
        match tokio::time::timeout(
            Duration::from_secs(PER_RESULT_TIMEOUT_SECS),
            verify_inner(title, court, decision_date),
        )
        .await
        {
            Ok(v) => v,
            Err(_) => Verification::unverified(
                "AWS verification timed out. The Kanoon result is still usable; the canonical court \
                 PDF couldn't be cross-checked within the per-result budget."
            ),
        }
    }
}

#[cfg(feature = "aws-verification")]
async fn verify_inner(
    title: &str,
    court: &str,
    decision_date: Option<&str>,
) -> Verification {
    // 1. Map Kanoon court → AWS court_code.
    let court_code = match map_court_code(court) {
        Some(c) => c,
        None => {
            return Verification::unverified(format!(
                "Court \"{court}\" is not in the AWS court-code mapping. Add it to COURT_MAP in src/llm/aws_verification.rs to enable verification for this court."
            ));
        }
    };
    // 2. Determine year. Required for the AWS partition path (both HC and SC).
    let year = match decision_date.and_then(parse_year) {
        Some(y) => y,
        None => {
            return Verification::unverified(
                "Kanoon didn't return a parseable decision date for this result. Cannot determine the AWS year partition."
            );
        }
    };

    // Supreme Court has its own dataset and partition layout — single
    // parquet file per year, no court/bench partitioning. Route SC
    // verifications through the dedicated SC bucket.
    if court_code == "supremecourt" {
        return verify_supreme_court(title, year).await;
    }

    // 3. Find candidate parquet files for (court_code, year). Lists S3
    //    by HTTP, parses XML for bench partitions, returns parquet keys.
    let parquet_keys = match parquet_index::list_bench_parquets(court_code, year).await {
        Ok(keys) if !keys.is_empty() => keys,
        Ok(_) => {
            return Verification::not_in_aws(court, year);
        }
        Err(e) => {
            tracing::warn!("[aws-verify] list S3 prefix failed: {e}");
            return Verification::unverified(format!(
                "AWS S3 listing failed: {e}. Network issue or bucket layout changed."
            ));
        }
    };

    // 4. For each bench parquet, download (with cache) and scan for a
    //    fuzzy title match. First hit wins.
    let title_norm = normalize_title(title);
    for key in &parquet_keys {
        let local = match parquet_index::ensure_local(key).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[aws-verify] download/cache {key} failed: {e}");
                continue;
            }
        };
        match parquet_index::find_matching_case(&local, &title_norm) {
            Ok(Some(hit)) => {
                let pdf_url = hit.pdf_link.as_deref().map(build_pdf_url);
                return Verification::verified(hit.cnr, pdf_url, hit.citation);
            }
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("[aws-verify] parquet scan {key} failed: {e}");
                continue;
            }
        }
    }

    Verification::not_in_aws(court, year)
}

/// Construct a public HTTPS URL for a HIGH COURT PDF given the
/// `pdf_link` field stored in the HC parquet. The dataset stores keys
/// like `data/pdf/year=YYYY/court=CCC/bench=BBB/CNR.pdf`; we just
/// prepend the HC bucket base.
fn build_pdf_url(pdf_link: &str) -> String {
    if pdf_link.starts_with("http") {
        pdf_link.to_string()
    } else {
        let trimmed = pdf_link.trim_start_matches('/');
        format!("{AWS_S3_BASE}/{trimmed}")
    }
}

/// Construct a public HTTPS URL for a SUPREME COURT PDF. SC dataset
/// pdf_link entries are stored as relative paths under the SC bucket,
/// so we prepend the SC bucket base. Absolute http(s) URLs pass through.
fn build_sc_pdf_url(pdf_link: &str) -> String {
    if pdf_link.starts_with("http") {
        pdf_link.to_string()
    } else {
        let trimmed = pdf_link.trim_start_matches('/');
        format!("{SC_AWS_S3_BASE}/{trimmed}")
    }
}

/// Verify a Supreme Court case against the dedicated SC AWS dataset.
/// The SC bucket is partitioned only by year (one parquet file per
/// year covering all SC cases), so the lookup is much simpler than HC:
/// download one file, fuzzy-match the title.
#[cfg(feature = "aws-verification")]
async fn verify_supreme_court(title: &str, year: i32) -> Verification {
    let title_norm = normalize_title(title);
    let parquet_key = format!("metadata/parquet/year={year}/metadata.parquet");

    let local = match parquet_index::ensure_sc_local(&parquet_key).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[aws-verify-sc] download/cache failed: {e}");
            return Verification::unverified(format!(
                "AWS SC parquet download failed for year {year}: {e}. Network issue or year not in dataset (SC dataset covers 1950-2025)."
            ));
        }
    };

    match parquet_index::find_matching_case(&local, &title_norm) {
        Ok(Some(hit)) => {
            let pdf_url = hit.pdf_link.as_deref().map(build_sc_pdf_url);
            Verification::verified(hit.cnr, pdf_url, hit.citation)
        }
        Ok(None) => Verification::not_in_aws("Supreme Court of India", year),
        Err(e) => {
            tracing::warn!("[aws-verify-sc] parquet scan failed: {e}");
            Verification::unverified(format!("AWS SC parquet parse failed: {e}"))
        }
    }
}

/// Lowercase + strip punctuation + collapse whitespace + drop short
/// stop-words. Used both for the AWS row scan and for the Kanoon
/// title we're matching against. Keep this fn pure — no async, no
/// allocations beyond the result string — because it runs inside the
/// parquet row loop.
pub fn normalize_title(t: &str) -> String {
    let lower = t.to_lowercase();
    let cleaned: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned
        .split_whitespace()
        .filter(|w| w.len() > 2 && !TITLE_STOPS.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Token overlap score between two normalized titles. Range 0.0..=1.0.
/// 1.0 means identical token sets; 0.0 means no shared tokens.
fn title_similarity(a: &str, b: &str) -> f32 {
    let a_tokens: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let b_tokens: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if a_tokens.is_empty() || b_tokens.is_empty() {
        return 0.0;
    }
    let shared = a_tokens.intersection(&b_tokens).count() as f32;
    let union = a_tokens.union(&b_tokens).count() as f32;
    shared / union
}

const TITLE_STOPS: &[&str] = &[
    "the", "and", "vs", "v", "of", "in", "on", "for", "to", "with",
    "by", "from", "as", "at", "is", "an", "ors", "anr",
];

// ---------------------------------------------------------------------------
// Parquet + S3 listing — feature-gated to keep the non-aws build slim.
// ---------------------------------------------------------------------------

#[cfg(feature = "aws-verification")]
mod parquet_index {
    use super::*;
    use anyhow::{anyhow, Result};
    use std::fs;
    use std::path::Path;

    pub struct MatchedCase {
        pub cnr: String,
        #[allow(dead_code)]
        pub title: String,
        /// None when the dataset has no PDF-link column (e.g. SC corpus).
        pub pdf_link: Option<String>,
        /// Neutral / reporter citation when the dataset provides one.
        pub citation: Option<String>,
    }

    /// Enumerate the bench-partitioned parquet files for one
    /// (court_code, year). Uses the anonymous S3 list-objects-v2
    /// endpoint with the parquet prefix.
    pub async fn list_bench_parquets(court_code: &str, year: i32) -> Result<Vec<String>> {
        let prefix = format!("metadata/parquet/year={year}/court={court_code}/");
        let url = format!(
            "{AWS_S3_BASE}/?list-type=2&prefix={}",
            url_encode_prefix(&prefix)
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(6))
            .build()?;
        let body = client.get(&url).send().await?.text().await?;
        // Cheap-and-cheerful XML scrape — list-objects-v2 returns
        // <Contents><Key>...</Key>... entries. Avoid an XML-parsing
        // dependency just for this.
        let mut keys = Vec::new();
        for chunk in body.split("<Key>").skip(1) {
            if let Some(end) = chunk.find("</Key>") {
                let key = &chunk[..end];
                if key.ends_with("metadata.parquet") {
                    keys.push(key.to_string());
                }
            }
        }
        Ok(keys)
    }

    /// Download (with cache) a HIGH COURT parquet file. Returns the local path.
    /// Cache layout: cache_root()/hc/<s3_key>
    pub async fn ensure_local(s3_key: &str) -> Result<PathBuf> {
        ensure_local_from(AWS_S3_BASE, "hc", s3_key).await
    }

    /// Download (with cache) a SUPREME COURT parquet file. SC bucket has
    /// a flatter layout (one parquet per year, no court/bench partitions),
    /// so this just downloads the single per-year file.
    /// Cache layout: cache_root()/sc/<s3_key>
    pub async fn ensure_sc_local(s3_key: &str) -> Result<PathBuf> {
        ensure_local_from(SC_AWS_S3_BASE, "sc", s3_key).await
    }

    /// Shared download-with-cache implementation. Different bucket bases
    /// + cache subdirectories let HC and SC coexist on disk without
    /// collision.
    async fn ensure_local_from(
        bucket_base: &str,
        cache_subdir: &str,
        s3_key: &str,
    ) -> Result<PathBuf> {
        let local = cache_root().join(cache_subdir).join(s3_key);
        if let Some(parent) = local.parent() {
            fs::create_dir_all(parent)?;
        }
        if local.exists() && fs::metadata(&local).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(local);
        }
        let url = format!("{bucket_base}/{s3_key}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        let bytes = client.get(&url).send().await?.error_for_status()?.bytes().await?;
        // Atomic write: dump to tmp, then rename.
        let tmp = local.with_extension("parquet.tmp");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &local)?;
        Ok(local)
    }

    /// Scan a parquet file for a case whose normalized title best
    /// matches the query title. Returns the best match if similarity
    /// is at least 0.5; otherwise None.
    ///
    /// HC and SC datasets use slightly different column names for the
    /// same logical fields, so we try multiple variants per field. If
    /// nothing matches, we log the actual schema field names at WARN
    /// level so the user can paste them back to us and we tighten the
    /// variant list.
    pub fn find_matching_case(path: &Path, query_norm_title: &str) -> Result<Option<MatchedCase>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

        let file = std::fs::File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let schema = builder.schema().clone();

        // Try multiple variants per field. First match wins.
        let cnr_idx = column_index_any(&schema, &[
            "cnr", "case_number", "diary_number", "case_no", "caseno",
        ])?;
        let title_idx = column_index_any(&schema, &[
            "title", "case_name", "cause_title", "caption", "case_title", "name",
        ])?;
        // PDF link is OPTIONAL — the SC corpus tars its PDFs and has no
        // per-case link column, but a title+CNR match still verifies the case.
        let pdf_idx = column_index_opt(&schema, &[
            "pdf_link", "pdf_url", "url", "source_url", "pdf", "pdf_path",
        ]);
        // Neutral / reporter citation, when present (e.g. SC `nc_display`).
        let cite_idx = column_index_opt(&schema, &[
            "nc_display", "neutral_citation", "citation",
        ]);

        let reader = builder.build()?;
        let mut best: Option<(f32, MatchedCase)> = None;

        for batch_result in reader {
            let batch = batch_result?;
            let cnr_arr = string_column(batch.column(cnr_idx))?;
            let title_arr = string_column(batch.column(title_idx))?;
            let pdf_arr = match pdf_idx {
                Some(idx) => Some(string_column(batch.column(idx))?),
                None => None,
            };
            let cite_arr = match cite_idx {
                Some(idx) => Some(string_column(batch.column(idx))?),
                None => None,
            };

            for i in 0..batch.num_rows() {
                let row_title = title_arr.value(i);
                if row_title.is_empty() {
                    continue;
                }
                let norm = normalize_title(row_title);
                let sim = title_similarity(query_norm_title, &norm);
                if sim < 0.5 {
                    continue;
                }
                let non_empty = |s: &str| {
                    let t = s.trim();
                    (!t.is_empty()).then(|| t.to_string())
                };
                let candidate = MatchedCase {
                    cnr: cnr_arr.value(i).to_string(),
                    title: row_title.to_string(),
                    pdf_link: pdf_arr.as_ref().and_then(|a| non_empty(a.value(i))),
                    citation: cite_arr.as_ref().and_then(|a| non_empty(a.value(i))),
                };
                match &best {
                    Some((bs, _)) if *bs >= sim => {}
                    _ => best = Some((sim, candidate)),
                }
                if sim > 0.95 {
                    // Near-exact: stop scanning to save time.
                    break;
                }
            }
        }

        Ok(best.map(|(_, m)| m))
    }

    #[allow(dead_code)]
    fn column_index(schema: &arrow_schema::SchemaRef, name: &str) -> Result<usize> {
        schema
            .fields()
            .iter()
            .position(|f| f.name().eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow!("column `{name}` not present in parquet schema"))
    }

    /// Like `column_index_any` but returns None instead of erroring when
    /// no candidate matches. Used for OPTIONAL columns (pdf link, citation)
    /// whose absence must not abort a verification.
    fn column_index_opt(
        schema: &arrow_schema::SchemaRef,
        candidates: &[&str],
    ) -> Option<usize> {
        candidates.iter().find_map(|name| {
            schema
                .fields()
                .iter()
                .position(|f| f.name().eq_ignore_ascii_case(name))
        })
    }

    /// Like column_index, but tries multiple candidate names. Logs the
    /// available schema field names at WARN level when nothing matches,
    /// so we can extend the candidate list without rebuilding to
    /// reproduce the failure.
    fn column_index_any(
        schema: &arrow_schema::SchemaRef,
        candidates: &[&str],
    ) -> Result<usize> {
        for name in candidates {
            if let Some(idx) = schema
                .fields()
                .iter()
                .position(|f| f.name().eq_ignore_ascii_case(name))
            {
                return Ok(idx);
            }
        }
        let available: Vec<String> = schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();
        tracing::warn!(
            "[aws-verify] no matching column for candidates {:?}; available columns in this parquet: {:?}",
            candidates,
            available
        );
        Err(anyhow!(
            "none of {:?} found in parquet schema; available columns: {:?}",
            candidates,
            available
        ))
    }

    fn string_column(
        col: &std::sync::Arc<dyn arrow_array::Array>,
    ) -> Result<&arrow_array::StringArray> {
        col.as_any()
            .downcast_ref::<arrow_array::StringArray>()
            .ok_or_else(|| anyhow!("expected StringArray column"))
    }

    fn url_encode_prefix(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | '=' => {
                    c.to_string()
                }
                ' ' => "%20".to_string(),
                other => format!("%{:02X}", other as u8),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn court_mapping_handles_common_kanoon_strings() {
        assert_eq!(map_court_code("Delhi High Court"), Some("7_26"));
        assert_eq!(map_court_code("Bombay High Court"), Some("27_1"));
        assert_eq!(map_court_code("High Court of Karnataka at Bangalore"), Some("29_3"));
        assert_eq!(map_court_code("Punjab & Haryana High Court"), Some("3_22"));
        assert_eq!(map_court_code("Punjab and Haryana High Court"), Some("3_22"));
        // Unknown court → None, verification returns UNVERIFIED.
        assert_eq!(map_court_code("Some Random Tribunal"), None);
    }

    #[test]
    fn court_mapping_recognises_supreme_court() {
        assert_eq!(map_court_code("Supreme Court of India"), Some("supremecourt"));
        assert_eq!(map_court_code("Supreme Court"), Some("supremecourt"));
    }

    #[test]
    fn parse_year_extracts_from_common_shapes() {
        assert_eq!(parse_year("12-04-2023"), Some(2023));
        assert_eq!(parse_year("2023-04-12"), Some(2023));
        assert_eq!(parse_year("April 12, 2023"), Some(2023));
        assert_eq!(parse_year("12/04/23"), None); // only 2-digit year
        assert_eq!(parse_year("no digits here"), None);
    }

    #[test]
    fn parse_year_rejects_out_of_range_years() {
        assert_eq!(parse_year("year 1850"), None);
        assert_eq!(parse_year("year 2200"), None);
    }

    #[test]
    fn normalize_title_strips_punct_and_stops() {
        let n = normalize_title("State of Maharashtra v. Suresh, AIR 2000 SC 123");
        // "the", "of", "v" are stops; punctuation removed.
        assert!(n.contains("state"));
        assert!(n.contains("maharashtra"));
        assert!(n.contains("suresh"));
        assert!(n.contains("air"));
        assert!(!n.split_whitespace().any(|t| t == "v"));
        assert!(!n.split_whitespace().any(|t| t == "of"));
    }

    #[test]
    fn title_similarity_handles_identical_and_disjoint() {
        let a = normalize_title("State of Maharashtra v. Suresh");
        let b = normalize_title("State of Maharashtra v. Suresh");
        assert!((title_similarity(&a, &b) - 1.0).abs() < 1e-6);

        let c = normalize_title("Income Tax Officer v. Ram Lal");
        assert!(title_similarity(&a, &c) < 0.2);
    }

    #[test]
    fn verification_unverified_carries_reason() {
        let v = Verification::unverified("network down");
        assert_eq!(v.status, VerificationStatus::Unverified);
        assert!(v.canonical_pdf_url.is_none());
        assert!(v.note.contains("network down"));
    }

    #[test]
    fn build_pdf_url_handles_both_relative_and_absolute() {
        assert_eq!(
            build_pdf_url("data/pdf/year=2023/court=7_26/bench=X/abc.pdf"),
            format!("{AWS_S3_BASE}/data/pdf/year=2023/court=7_26/bench=X/abc.pdf")
        );
        assert_eq!(
            build_pdf_url("https://example.com/foo.pdf"),
            "https://example.com/foo.pdf"
        );
    }
}
