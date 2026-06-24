//! Single-folder scan job.
//!
//! Driven by the `/sync-folder/scan` endpoint. The scan runs in a
//! `tokio::task` and reports progress via a shared `ScanProgress`
//! handle that the GET status endpoint reads. The job is idempotent:
//! re-running it is cheap because we hash files only when their
//! `mtime` differs from what's recorded in `synced_files`.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use super::extension_is_supported;
use crate::embeddings::EmbeddingService;

/// In-memory progress tracker. The route handler reads this to render
/// the progress bar; the scan task writes to it.
#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
    pub total: u32,
    pub processed: u32,
    pub indexed: u32,
    pub skipped: u32,
    pub failed: u32,
    pub status: ScanStatus,
    pub current_file: Option<String>,
    /// Coarse stage tag for the *current_file* — surfaced so the
    /// frontend can show "estraendo testo" vs "embedding" without
    /// inferring from log lines. One of:
    /// `extracting`, `extracting page N/M`, `embedding`, `loading-model`.
    pub current_step: Option<String>,
    /// The most recent per-file failure. Kept for back-compat; the full
    /// list is in `failures` so earlier failures aren't lost.
    pub last_error: Option<String>,
    /// Every per-file failure ("path: reason"), so a partial-failure scan
    /// can show *all* the files that failed — not just the last one.
    /// Capped at `MAX_TRACKED_FAILURES`; once full, further failures are
    /// counted (`failed`) but their messages are dropped to bound memory.
    pub failures: Vec<String>,
}

/// Upper bound on the number of failure messages we retain in
/// `ScanProgress::failures`. A scan over a large tree could otherwise
/// accumulate one string per file; the `failed` counter still reflects
/// the true total even after this cap is hit.
const MAX_TRACKED_FAILURES: usize = 500;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScanStatus {
    #[default]
    Idle,
    Running,
    Done,
    /// The scan finished, but one or more files failed to index. The
    /// caller must inspect `failed` / `failures` — this is NOT a success.
    CompletedWithErrors,
    Failed,
}

pub type ScanProgressHandle = Arc<RwLock<ScanProgress>>;

/// Aggregated scan result, returned at the end. Mirrors the fields of
/// `ScanProgress` for easy serialisation.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ScanReport {
    pub total: u32,
    pub indexed: u32,
    pub skipped: u32,
    pub failed: u32,
    pub duration_secs: f64,
}

/// Run a full scan of a folder. Updates `progress` as it goes, then
/// commits a final `Done`/`Failed` status. `project_id = None` →
/// global pool; `Some(_)` → project-scoped pool. The same value is
/// stamped on every chunk so retrieval can filter without joins.
pub async fn scan_folder(
    db: SqlitePool,
    embeddings: Arc<EmbeddingService>,
    user_id: String,
    folder_id: String,
    project_id: Option<String>,
    folder_path: PathBuf,
    recursive: bool,
    progress: ScanProgressHandle,
) -> Result<ScanReport> {
    let started = std::time::Instant::now();

    {
        let mut p = progress.write().await;
        *p = ScanProgress {
            status: ScanStatus::Running,
            ..Default::default()
        };
    }

    // Build the file list first so we can show "x of N". We honour
    // `.gitignore` and a project-local `.mikesyncignore` so the user
    // can exclude folders without renaming/moving files.
    let walker = WalkBuilder::new(&folder_path)
        .max_depth(if recursive { None } else { Some(1) })
        .standard_filters(true)
        .add_custom_ignore_filename(".mikesyncignore")
        .build();

    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in walker.flatten() {
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            && extension_is_supported(&entry.file_name().to_string_lossy())
        {
            candidates.push(entry.into_path());
        }
    }

    {
        let mut p = progress.write().await;
        p.total = candidates.len() as u32;
    }

    let mut indexed = 0u32;
    let mut skipped = 0u32;
    let mut failed = 0u32;

    for path in candidates {
        {
            let mut p = progress.write().await;
            p.current_file = Some(path.to_string_lossy().to_string());
            p.current_step = Some("starting".to_string());
        }

        match process_one(
            &db,
            &embeddings,
            &user_id,
            &folder_id,
            project_id.as_deref(),
            &path,
            &progress,
        )
        .await
        {
            Ok(ProcessOutcome::Indexed { .. }) => indexed += 1,
            Ok(ProcessOutcome::Unchanged) => {
                // Already up-to-date; counted as indexed for the user's
                // purposes (the chunks remain in the vector store).
                indexed += 1;
            }
            Ok(ProcessOutcome::Skipped { .. }) => skipped += 1,
            Err(e) => {
                tracing::warn!("[sync] {} failed: {e}", path.display());
                failed += 1;
                let msg = format!("{}: {e}", path.display());
                let mut p = progress.write().await;
                if p.failures.len() < MAX_TRACKED_FAILURES {
                    p.failures.push(msg.clone());
                }
                p.last_error = Some(msg);
            }
        }

        let mut p = progress.write().await;
        p.processed += 1;
        p.indexed = indexed;
        p.skipped = skipped;
        p.failed = failed;
    }

    sqlx::query("UPDATE sync_folders SET last_scan_at = datetime('now') WHERE id = ?")
        .bind(&folder_id)
        .execute(&db)
        .await
        .ok();

    let report = ScanReport {
        total: indexed + skipped + failed,
        indexed,
        skipped,
        failed,
        duration_secs: started.elapsed().as_secs_f64(),
    };
    {
        let mut p = progress.write().await;
        // Only report a clean `Done` when nothing failed. A partial
        // failure must surface as its own terminal status so a UI keying
        // off status alone can't render success while files silently
        // failed to index.
        p.status = if failed > 0 {
            ScanStatus::CompletedWithErrors
        } else {
            ScanStatus::Done
        };
        p.current_file = None;
        p.current_step = None;
    }
    Ok(report)
}

enum ProcessOutcome {
    Indexed { chunks: usize },
    Unchanged,
    Skipped { reason: String },
}

/// Process a single file. Returns:
///  - Indexed if we extracted text and pushed chunks,
///  - Unchanged if mtime+sha256 match the prior record,
///  - Skipped with a human reason for non-text content.
async fn process_one(
    db: &SqlitePool,
    embeddings: &EmbeddingService,
    user_id: &str,
    folder_id: &str,
    project_id: Option<&str>,
    path: &Path,
    progress: &ScanProgressHandle,
) -> Result<ProcessOutcome> {
    // `metadata` is a blocking syscall; run it off the async worker so a
    // scan can't park a runtime thread that also serves HTTP.
    let metadata = {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || std::fs::metadata(&path))
            .await
            .context("stat task")?
            .context("stat")?
    };
    let mtime: DateTime<Utc> = metadata
        .modified()
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());
    let size_bytes = metadata.len() as i64;

    // Check the prior record. If mtime matches what we last saw, we can
    // skip both hashing and extraction: nothing has changed.
    let path_str = path.to_string_lossy().to_string();
    let prior: Option<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id, document_id, sha256, mtime, status FROM synced_files \
         WHERE user_id = ? AND path = ?",
    )
    .bind(user_id)
    .bind(&path_str)
    .fetch_optional(db)
    .await?;

    let mtime_str = mtime.to_rfc3339();

    if let Some((_id, _doc, _sha, prev_mtime, _status)) = &prior {
        if prev_mtime == &mtime_str {
            return Ok(ProcessOutcome::Unchanged);
        }
    }

    // mtime changed (or no prior record) — read + hash to decide if
    // content really changed. Cheaper than re-embedding when the file
    // was just touched. The whole-file read and the CPU-bound sha256 are
    // both blocking, so run them on a blocking thread to keep the async
    // runtime responsive for concurrent HTTP requests during a scan.
    let (bytes, sha) = {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, String)> {
            let bytes = std::fs::read(&path).context("read")?;
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let sha = format!("{:x}", hasher.finalize());
            Ok((bytes, sha))
        })
        .await
        .context("read task")??
    };

    if let Some((id, document_id, prev_sha, _, _)) = &prior {
        if prev_sha == &sha {
            // Same content, just touched. Update mtime and exit.
            sqlx::query("UPDATE synced_files SET mtime = ? WHERE id = ?")
                .bind(&mtime_str)
                .bind(id)
                .execute(db)
                .await?;
            return Ok(ProcessOutcome::Unchanged);
        }
        // Content actually changed — re-embed. Drop old chunks first.
        embeddings.delete_document(user_id, document_id).await.ok();
    }

    // Stable document_id for this file lifetime — MUST be unique even when
    // the file is skipped (the UNIQUE constraint on synced_files.document_id
    // would fail on the second skipped file if we passed empty string).
    let file_document_id = prior
        .as_ref()
        .map(|p| p.1.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    {
        let mut p = progress.write().await;
        p.current_step = Some("extracting".to_string());
    }
    let t_extract = std::time::Instant::now();
    tracing::info!(
        "[sync] {}: extracting text ({} bytes)",
        path.display(),
        bytes.len()
    );
    let (text, skip_reason) = extract_text_dispatch(path, &bytes)?;
    let extract_ms = t_extract.elapsed().as_millis();
    if let Some(reason) = skip_reason {
        tracing::info!(
            "[sync] {}: skipped after {}ms — {}",
            path.display(),
            extract_ms,
            reason
        );
        upsert_synced_file(
            db,
            prior.as_ref().map(|p| p.0.as_str()),
            user_id,
            folder_id,
            project_id,
            &path_str,
            &sha,
            size_bytes,
            &mtime_str,
            Some(&file_document_id),
            "skipped",
            Some(&reason),
            0,
        )
        .await?;
        return Ok(ProcessOutcome::Skipped { reason });
    }

    tracing::info!(
        "[sync] {}: text {} chars (extract {}ms) → embedding+indexing",
        path.display(),
        text.len(),
        extract_ms
    );
    {
        let mut p = progress.write().await;
        p.current_step = Some("embedding".to_string());
    }
    let t_embed = std::time::Instant::now();
    let chunk_count = embeddings
        .index_document(user_id, project_id, &file_document_id, &path_str, &text)
        .await
        .context("embed+index")?;
    tracing::info!(
        "[sync] {}: indexed {} chunks ({}ms)",
        path.display(),
        chunk_count,
        t_embed.elapsed().as_millis()
    );

    upsert_synced_file(
        db,
        prior.as_ref().map(|p| p.0.as_str()),
        user_id,
        folder_id,
        project_id,
        &path_str,
        &sha,
        size_bytes,
        &mtime_str,
        Some(&file_document_id),
        "ready",
        None,
        chunk_count as i64,
    )
    .await?;

    Ok(ProcessOutcome::Indexed { chunks: chunk_count })
}

#[allow(clippy::too_many_arguments)]
async fn upsert_synced_file(
    db: &SqlitePool,
    existing_id: Option<&str>,
    user_id: &str,
    folder_id: &str,
    project_id: Option<&str>,
    path: &str,
    sha: &str,
    size_bytes: i64,
    mtime: &str,
    document_id: Option<&str>,
    status: &str,
    skip_reason: Option<&str>,
    chunk_count: i64,
) -> Result<()> {
    if let Some(id) = existing_id {
        sqlx::query(
            "UPDATE synced_files SET sha256=?, size_bytes=?, mtime=?, status=?, \
             skip_reason=?, chunk_count=?, project_id=?, indexed_at=datetime('now') WHERE id=?",
        )
        .bind(sha)
        .bind(size_bytes)
        .bind(mtime)
        .bind(status)
        .bind(skip_reason)
        .bind(chunk_count)
        .bind(project_id)
        .bind(id)
        .execute(db)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO synced_files \
             (id, user_id, folder_id, project_id, path, sha256, size_bytes, mtime, \
              document_id, status, skip_reason, chunk_count) \
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id)
        .bind(folder_id)
        .bind(project_id)
        .bind(path)
        .bind(sha)
        .bind(size_bytes)
        .bind(mtime)
        .bind(document_id.unwrap_or(""))
        .bind(status)
        .bind(skip_reason)
        .bind(chunk_count)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Extract plain text from a file. Returns `(text, Some(reason))` when
/// the file is intentionally skipped (scanned PDF, etc.) — `text` is
/// empty in that case. Returns `(text, None)` on success.
///
/// Public so the document-upload handler (`/single-documents` with
/// `cache=true`) can extract on the same code path the folder scanner
/// uses, instead of duplicating the per-format dispatch.
pub fn extract_text_dispatch(path: &Path, bytes: &[u8]) -> Result<(String, Option<String>)> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "txt" | "md" | "csv" => {
            let s = String::from_utf8_lossy(bytes).into_owned();
            Ok((s, None))
        }
        "docx" => {
            let s = crate::pdf::extract_docx_text(bytes)?;
            Ok((s, None))
        }
        "rtf" => {
            // rtf-parser gives us plain text after stripping control
            // words, font tables, color tables, picture data and field
            // instructions. We only feed the LLM/embedder the body, so
            // that's exactly what we want.
            //
            // RTF is ASCII-with-escapes by spec but real files routinely
            // smuggle UTF-8 inside braces — lossy decode upfront keeps
            // the parser happy on those edge cases.
            let raw = String::from_utf8_lossy(bytes);
            let s = match rtf_parser::RtfDocument::try_from(raw.as_ref()) {
                Ok(doc) => doc.get_text(),
                Err(e) => {
                    return Ok((
                        String::new(),
                        Some(format!("malformed RTF: {e}")),
                    ));
                }
            };
            Ok((s, None))
        }
        "xlsx" | "xls" | "xlsb" | "ods" => {
            let s = crate::pdf::extract_xlsx_text(bytes)?;
            Ok((s, None))
        }
        #[cfg(feature = "pdf")]
        "pdf" => {
            // Extract from the bytes we already hold, not by re-opening `path`
            // on disk. This makes the dispatch work for in-memory uploads
            // (e.g. "Mike listens" attachments) whose bytes were never written
            // to disk — the previous path-based load looked up a bare filename
            // in cwd and errored, silently dropping the attachment.
            let label = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let pages = crate::pdf::extract_text_from_bytes(bytes, &label)?;
            // Concatenate pages with markers so retrieval can keep
            // some locality info when chunks straddle pages.
            let mut out = String::new();
            for (i, p) in pages.iter().enumerate() {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(&format!("[Page {}]\n", i + 1));
                out.push_str(&p.text);
            }
            Ok((out, None))
        }
        #[cfg(not(feature = "pdf"))]
        "pdf" => Ok((
            String::new(),
            Some("PDF support not compiled in this build".to_string()),
        )),
        other => Ok((
            String::new(),
            Some(format!("format not supported: {other}")),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dispatch(name: &str, bytes: &[u8]) -> (String, Option<String>) {
        extract_text_dispatch(&PathBuf::from(name), bytes).unwrap()
    }

    #[test]
    fn extracts_rtf_text_body() {
        // Minimal valid RTF: header, font table, plain "Hello world."
        let rtf = b"{\\rtf1\\ansi\\deff0 {\\fonttbl {\\f0 Times New Roman;}} \
                    \\f0\\fs24 Hello world.}";
        let (text, skip) = dispatch("note.rtf", rtf);
        assert!(skip.is_none(), "valid RTF should not be skipped");
        assert!(text.contains("Hello world."), "got: {text:?}");
    }

    #[test]
    fn extracts_rtf_strips_control_words() {
        // RTF with bold/italic toggles, font size, paragraph break.
        let rtf = b"{\\rtf1\\ansi {\\b Bold} text and {\\i italic} text.\\par \
                    Second paragraph.}";
        let (text, skip) = dispatch("doc.rtf", rtf);
        assert!(skip.is_none());
        // Control words must be gone — only the human-readable body remains.
        assert!(!text.contains("\\b"));
        assert!(!text.contains("\\i"));
        assert!(!text.contains("\\par"));
        assert!(text.contains("Bold"));
        assert!(text.contains("italic"));
        assert!(text.contains("Second paragraph"));
    }

    #[test]
    fn malformed_rtf_skipped_with_reason() {
        // Header looks like RTF but body is garbage that won't parse.
        let bad = b"{\\rtf1 \\bad{{nested";
        let (text, skip) = dispatch("broken.rtf", bad);
        assert!(text.is_empty());
        // Either parsed leniently to "" or returned a skip reason —
        // both are acceptable outcomes; what matters is no panic.
        if let Some(reason) = skip {
            assert!(reason.contains("RTF") || reason.contains("malformed"));
        }
    }

    #[test]
    fn unknown_extension_returns_skip_reason() {
        let (text, skip) = dispatch("data.xyz", b"some content");
        assert!(text.is_empty());
        assert!(skip.unwrap().contains("not supported"));
    }

    #[test]
    fn txt_md_csv_pass_through_as_plain_text() {
        for ext in ["txt", "md", "csv"] {
            let (text, skip) = dispatch(&format!("file.{ext}"), b"hello\nworld");
            assert!(skip.is_none(), ".{ext} must not be skipped");
            assert_eq!(text, "hello\nworld");
        }
    }

    // --- partial-failure terminal status (scanner.rs:160-173) ------------

    /// In-memory pool with just the two tables `process_one` touches.
    /// `max_connections(1)` keeps the `:memory:` DB alive across queries.
    async fn test_pool() -> sqlx::SqlitePool {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE sync_folders (id TEXT PRIMARY KEY, user_id TEXT, \
             path TEXT, recursive INTEGER, enabled INTEGER, last_scan_at TEXT, \
             label TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE synced_files (id TEXT PRIMARY KEY, user_id TEXT, \
             folder_id TEXT, project_id TEXT, path TEXT, sha256 TEXT, \
             size_bytes INTEGER, mtime TEXT, document_id TEXT UNIQUE, \
             status TEXT, skip_reason TEXT, chunk_count INTEGER, indexed_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    /// A file with a supported extension that fails to extract must:
    ///  - be counted in `failed`,
    ///  - record its reason in `failures` (not just overwrite `last_error`),
    ///  - flip the terminal status to `CompletedWithErrors`, NOT `Done`,
    /// so a UI keying off status alone can't render the scan as a success.
    /// Uses multibyte (Devanagari + ₹) garbage bytes to also prove the
    /// failure path doesn't panic on non-ASCII content.
    #[tokio::test]
    async fn partial_failure_yields_completed_with_errors_status() {
        let db = test_pool().await;
        let embeddings = Arc::new(EmbeddingService::new(db.clone()));

        let dir = tempfile::tempdir().unwrap();
        // Two .docx files whose bytes are NOT a valid ZIP → extraction
        // errors before any embedding/model load is reached.
        for name in ["धारा-१३८.docx", "contract-₹.docx"] {
            std::fs::write(
                dir.path().join(name),
                "not a real docx — धारा ₹500 \u{2014} curly “quote”".as_bytes(),
            )
            .unwrap();
        }

        let progress: ScanProgressHandle =
            Arc::new(RwLock::new(ScanProgress::default()));

        let report = scan_folder(
            db,
            embeddings,
            "user-1".to_string(),
            "folder-1".to_string(),
            None,
            dir.path().to_path_buf(),
            false,
            progress.clone(),
        )
        .await
        .expect("scan task itself should not error on per-file failures");

        assert_eq!(report.failed, 2, "both garbage docx must be counted failed");
        assert_eq!(report.indexed, 0);

        let p = progress.read().await;
        assert_eq!(
            p.status,
            ScanStatus::CompletedWithErrors,
            "partial failure must NOT report a clean Done"
        );
        assert_eq!(p.failed, 2);
        assert_eq!(
            p.failures.len(),
            2,
            "every failure must be retained, not just the last one"
        );
        assert!(
            p.failures.iter().any(|f| f.contains("धारा")),
            "failure list should name the failing files: {:?}",
            p.failures
        );
    }

    /// The clean path must still terminate as `Done` (here: an empty
    /// folder, so no embedding model is loaded) — the new variant only
    /// fires when something actually failed.
    #[tokio::test]
    async fn clean_scan_yields_done_status() {
        let db = test_pool().await;
        let embeddings = Arc::new(EmbeddingService::new(db.clone()));
        let dir = tempfile::tempdir().unwrap();

        let progress: ScanProgressHandle =
            Arc::new(RwLock::new(ScanProgress::default()));

        let report = scan_folder(
            db,
            embeddings,
            "user-1".to_string(),
            "folder-1".to_string(),
            None,
            dir.path().to_path_buf(),
            false,
            progress.clone(),
        )
        .await
        .unwrap();

        assert_eq!(report.failed, 0);
        let p = progress.read().await;
        assert_eq!(p.status, ScanStatus::Done);
        assert!(p.failures.is_empty());
    }
}
