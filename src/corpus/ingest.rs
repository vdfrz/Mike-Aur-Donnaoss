//! Firm-corpus ingest pipeline.
//!
//! Drives one `corpus_files` row from `pending` to `ready`:
//!   pending → extracting → chunking → tagging → ready
//! (or `unsupported` when there's nothing to index, `failed` on hard IO/DB
//! errors which the caller stamps). Every LLM / parse failure is
//! non-fatal: we degrade to NULL metadata and still mark the file `ready`.
//!
//! Text extraction reuses `crate::sync::scanner::extract_text_dispatch`
//! (the same per-format dispatch the folder scanner and the document
//! upload handler use — pdf w/ OCR, docx, rtf, txt, xlsx). That fn keys
//! its format off the PATH EXTENSION, so we hand it a synthetic path built
//! from the stored filename.

use anyhow::{Context, Result};
use serde_json::Value;

use crate::corpus::chunker;

/// Cap on the document text fed to the distillation LLM call.
const DISTILL_TEXT_CAP: usize = 12_000;

/// Progress events emitted to an optional channel as ingest proceeds. The
/// route can forward these over SSE/websocket; pass `None` for fire-and-forget.
#[derive(Debug, Clone)]
pub enum IngestEvent {
    Stage {
        file_id: String,
        stage: String,
    },
    Done {
        file_id: String,
        chunk_count: usize,
        doc_type: Option<String>,
    },
    Error {
        file_id: String,
        message: String,
    },
}

/// Ingest a single already-uploaded corpus file.
///
/// Preconditions: the `corpus_files` row exists and its bytes are stored
/// at `corpus/{user_id}/{file_id}` (the upload route's responsibility).
///
/// Returns `Ok(())` for every terminal outcome that isn't an
/// infrastructure failure — including `unsupported` and degraded `ready`.
/// Returns `Err` only on hard IO/DB errors; the caller marks the row
/// `status='failed'` in that case.
pub async fn ingest_file(
    state: &crate::AppState,
    user_id: &str,
    file_id: &str,
    progress: Option<&tokio::sync::mpsc::Sender<IngestEvent>>,
) -> Result<()> {
    // 1. Load the file row.
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT filename, file_type FROM corpus_files WHERE id = ? AND user_id = ?",
    )
    .bind(file_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .context("load corpus_files row")?;

    let Some((filename, _file_type)) = row else {
        anyhow::bail!("corpus file {file_id} not found for user {user_id}");
    };

    // Read the stored bytes. The upload route stores at this key.
    let storage_key = format!("corpus/{user_id}/{file_id}");
    let storage = crate::storage::make_storage().context("init storage")?;
    let bytes = storage
        .get(&storage_key)
        .await
        .with_context(|| format!("read stored bytes at {storage_key}"))?;

    // 2. Extract text. extract_text_dispatch dispatches on path EXTENSION,
    //    so build a path from the original filename (carries the extension).
    set_status(&state.db, file_id, "extracting").await?;
    emit(progress, IngestEvent::Stage { file_id: file_id.to_string(), stage: "extracting".into() }).await;

    let synthetic_path = std::path::PathBuf::from(&filename);
    let (text, skip_reason) = match safe_extract(&synthetic_path, &bytes) {
        Ok(v) => v,
        Err(e) => {
            // Extraction itself errored (e.g. corrupt docx) — non-fatal:
            // mark unsupported with the reason and return Ok.
            mark_unsupported(&state.db, file_id, &format!("{e}")).await?;
            emit(progress, IngestEvent::Error { file_id: file_id.to_string(), message: format!("{e}") }).await;
            return Ok(());
        }
    };

    if let Some(reason) = skip_reason {
        mark_unsupported(&state.db, file_id, &reason).await?;
        emit(progress, IngestEvent::Error { file_id: file_id.to_string(), message: reason }).await;
        return Ok(());
    }
    if text.trim().is_empty() {
        mark_unsupported(&state.db, file_id, "extraction yielded no text").await?;
        emit(progress, IngestEvent::Error { file_id: file_id.to_string(), message: "no text".into() }).await;
        return Ok(());
    }

    // 3. Chunk and insert.
    set_status(&state.db, file_id, "chunking").await?;
    emit(progress, IngestEvent::Stage { file_id: file_id.to_string(), stage: "chunking".into() }).await;

    let chunks = chunker::chunk_legal_text(&text);
    if chunks.is_empty() {
        mark_unsupported(&state.db, file_id, "no chunks produced").await?;
        emit(progress, IngestEvent::Error { file_id: file_id.to_string(), message: "no chunks".into() }).await;
        return Ok(());
    }

    // Replace any prior chunks for idempotent re-ingest, then insert.
    sqlx::query("DELETE FROM corpus_chunks WHERE file_id = ?")
        .bind(file_id)
        .execute(&state.db)
        .await
        .context("clear prior chunks")?;

    for c in &chunks {
        sqlx::query(
            "INSERT INTO corpus_chunks (file_id, user_id, seq, heading, section_role, page, text) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(file_id)
        .bind(user_id)
        .bind(c.seq)
        .bind(&c.heading)
        .bind(&c.section_role)
        .bind(c.page)
        .bind(&c.text)
        .execute(&state.db)
        .await
        .context("insert corpus_chunk")?;
    }

    let chunk_count = chunks.len();
    sqlx::query("UPDATE corpus_files SET chunk_count = ? WHERE id = ?")
        .bind(chunk_count as i64)
        .bind(file_id)
        .execute(&state.db)
        .await
        .context("update chunk_count")?;

    // 3b. Embed the freshly-stored chunks into the sqlite-vec store so
    //     search_firm_corpus can retrieve them semantically (hybrid with
    //     BM25). Best-effort: an embedding failure (model not yet
    //     downloaded, offline, OOM) must NOT fail ingest — the file stays
    //     `ready` and searchable via BM25 alone. Gated on the `rag`
    //     feature + an initialised embedding service.
    #[cfg(feature = "rag")]
    if let Some(emb) = state.embeddings.as_deref() {
        if let Err(e) = index_corpus_chunks(emb, &state.db, user_id, file_id).await {
            tracing::warn!(
                "[corpus] embedding chunks for file {file_id} failed; \
                 search falls back to BM25-only for this file: {e}"
            );
        }
    }

    // 4. LLM metadata tagging — fully best-effort. Any failure leaves the
    //    metadata NULL but still proceeds to `ready`.
    set_status(&state.db, file_id, "tagging").await?;
    emit(progress, IngestEvent::Stage { file_id: file_id.to_string(), stage: "tagging".into() }).await;

    let headings: Vec<String> = chunks
        .iter()
        .filter_map(|c| c.heading.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();

    let meta = tag_metadata(state, user_id, &text, &headings).await;

    let doc_type = meta.as_ref().and_then(|m| m.doc_type.clone());
    if let Some(m) = &meta {
        let is_template = i64::from(m.looks_like_template);
        sqlx::query(
            "UPDATE corpus_files SET doc_type = ?, case_type = ?, court = ?, doc_date = ?, \
             language = ?, is_template = ? WHERE id = ?",
        )
        .bind(&m.doc_type)
        .bind(&m.case_type)
        .bind(&m.court)
        .bind(&m.doc_date)
        .bind(&m.language)
        .bind(is_template)
        .bind(file_id)
        .execute(&state.db)
        .await
        .context("update corpus metadata")?;
    }

    // 4b. Online distillation — extract a reusable style/structure profile
    //     and store it. Fully best-effort + offline-safe: a missing model or
    //     failed call writes no profile and never fails ingest.
    emit(progress, IngestEvent::Stage { file_id: file_id.to_string(), stage: "distilling".into() }).await;
    if let Err(e) = distill_profile(state, user_id, file_id, &text, doc_type.as_deref()).await {
        tracing::warn!("[corpus] distillation for file {file_id} failed (non-fatal): {e}");
    }

    // 5. Templates: only flag is_template (above). The lead builds the
    //    workflow row + template_md later; we do not touch workflows here.

    // 6. Done.
    sqlx::query("UPDATE corpus_files SET status = 'ready', error = NULL WHERE id = ?")
        .bind(file_id)
        .execute(&state.db)
        .await
        .context("mark ready")?;

    emit(progress, IngestEvent::Done { file_id: file_id.to_string(), chunk_count, doc_type }).await;
    Ok(())
}

/// Embed every chunk of `file_id` and upsert the vectors into the
/// `corpus_chunks_vec` sqlite-vec table. Idempotent: clears this file's
/// prior vectors first, so re-ingest replaces rather than accumulates.
///
/// Returns the number of chunks embedded. Reads the chunk text straight
/// from `corpus_chunks` (already stored by `ingest_file`), so the embedded
/// text matches the BM25-indexed text exactly.
#[cfg(feature = "rag")]
pub async fn index_corpus_chunks(
    emb: &crate::embeddings::EmbeddingService,
    db: &sqlx::SqlitePool,
    user_id: &str,
    file_id: &str,
) -> Result<usize> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, text FROM corpus_chunks WHERE file_id = ? ORDER BY seq")
            .bind(file_id)
            .fetch_all(db)
            .await
            .context("load corpus chunks to embed")?;
    if rows.is_empty() {
        return Ok(0);
    }

    let (chunk_ids, texts): (Vec<i64>, Vec<String>) = rows.into_iter().unzip();
    let vectors = emb.embed_passages(&texts).await.context("embed corpus chunks")?;
    debug_assert_eq!(
        vectors.len(),
        chunk_ids.len(),
        "embed_passages must return one vector per input chunk"
    );

    let mut tx = db.begin().await?;
    // Clear any prior vectors for this file (idempotent re-ingest). vec0
    // virtual tables can't carry the ON DELETE CASCADE from corpus_files,
    // so we delete by the auxiliary file_id column explicitly.
    sqlx::query("DELETE FROM corpus_chunks_vec WHERE file_id = ?")
        .bind(file_id)
        .execute(&mut *tx)
        .await
        .context("clear prior corpus vectors")?;

    for (chunk_id, vec) in chunk_ids.iter().zip(vectors.iter()) {
        let blob = crate::embeddings::service::vec_to_blob(vec);
        sqlx::query(
            "INSERT INTO corpus_chunks_vec (embedding, user_id, chunk_id, file_id) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(&blob[..])
        .bind(user_id)
        .bind(chunk_id)
        .bind(file_id)
        .execute(&mut *tx)
        .await
        .context("insert corpus chunk vector")?;
    }

    tx.commit().await?;
    Ok(chunk_ids.len())
}

/// Thin wrapper over the shared per-format extractor so a malformed file
/// surfaces as an `Err` we can degrade on (mark `unsupported`) rather than
/// aborting the whole ingest.
fn safe_extract(
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<(String, Option<String>)> {
    crate::sync::scanner::extract_text_dispatch(path, bytes)
}

/// Light metadata produced by the tagging LLM call.
struct CorpusMeta {
    doc_type: Option<String>,
    case_type: Option<String>,
    court: Option<String>,
    doc_date: Option<String>,
    language: Option<String>,
    looks_like_template: bool,
}

/// One best-effort LLM call to classify the document. Returns `None` on any
/// configuration / call / parse failure — the caller leaves fields NULL.
async fn tag_metadata(
    state: &crate::AppState,
    user_id: &str,
    full_text: &str,
    headings: &[String],
) -> Option<CorpusMeta> {
    let settings = crate::routes::user::fetch_llm_settings(&state.db, user_id)
        .await
        .ok();
    let config = crate::llm::oneshot::config_from_settings(&settings);

    let head: String = full_text.chars().take(3000).collect();
    let heading_list = if headings.is_empty() {
        "(none detected)".to_string()
    } else {
        headings.join("\n- ")
    };

    let system = "You classify Indian legal documents for a law firm's internal knowledge base. \
Respond with ONLY a single JSON object, no prose, no code fences. Keys: \
doc_type (one of: petition, written_statement, judgment, skeleton_argument, template, deed, notice, other), \
case_type (one of: matrimonial, consumer, criminal, writ, service, ni_act, civil, other), \
court (short court name or null), \
doc_date (ISO date or the date as written, or null), \
language (e.g. en, hi, mixed), \
looks_like_template (boolean: true if the document is a reusable skeleton/precedent with blanks or {{placeholders}} rather than a real filed case). \
Use null for anything you cannot determine. Do not invent values.";

    let user_msg = format!(
        "First ~3000 characters of the document:\n\n{head}\n\n---\nSection headings detected:\n- {heading_list}\n\nReturn the JSON object now."
    );

    let raw = crate::llm::oneshot::complete(&config, system, &user_msg)
        .await
        .ok()?;

    parse_meta(&raw)
}

/// Defensively parse the tagging response. Tolerates code fences and
/// surrounding prose by extracting the first `{...}` span.
fn parse_meta(raw: &str) -> Option<CorpusMeta> {
    let json_slice = extract_json_object(raw)?;
    let v: Value = serde_json::from_str(json_slice).ok()?;

    let s = |key: &str| -> Option<String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty() && !x.eq_ignore_ascii_case("null") && !x.eq_ignore_ascii_case("unknown"))
    };

    Some(CorpusMeta {
        doc_type: s("doc_type"),
        case_type: s("case_type"),
        court: s("court"),
        doc_date: s("doc_date"),
        language: s("language"),
        looks_like_template: v
            .get("looks_like_template")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
    })
}

/// A distilled style/structure profile of one imported draft.
struct DistilledProfile {
    summary: Option<String>,
    structure: Option<String>,        // newline-joined section outline
    style_notes: Option<String>,
    reusable_phrases: Option<String>, // JSON array string, as returned
}

/// Online distillation pass: ask the configured cloud model to distill the
/// draft into a reusable style/structure profile, then store it. Fully
/// best-effort and OFFLINE-SAFE: if no model is configured or the call
/// fails, `oneshot::complete(...).await.ok()` is `None`, we write NO profile
/// row and return `Ok(false)` — ingest is never failed by this.
///
/// Returns Ok(true) iff a profile row was written.
pub async fn distill_profile(
    state: &crate::AppState,
    user_id: &str,
    file_id: &str,
    full_text: &str,
    doc_type: Option<&str>,
) -> Result<bool> {
    let settings = crate::routes::user::fetch_llm_settings(&state.db, user_id)
        .await
        .ok();
    let config = crate::llm::oneshot::config_from_settings(&settings);

    let head: String = full_text.chars().take(DISTILL_TEXT_CAP).collect();
    let dt = doc_type.unwrap_or("(unknown)");
    let system = "You distil a lawyer's own legal draft into a REUSABLE STYLE PROFILE so the \
firm's drafting assistant can later imitate this lawyer's structure and phrasing. \
Respond with ONLY a single JSON object, no prose, no code fences. Keys: \
summary (string: one or two sentences naming what kind of draft this is), \
structure (array of strings: the section/heading outline in order), \
style_notes (string: tone, register, formatting and drafting habits worth copying), \
reusable_phrases (array of strings: distinctive stock phrases/openers/closers this lawyer reuses). \
Use null or [] for anything you cannot determine. Do not invent content.";
    let user_msg = format!(
        "Document type (best guess): {dt}\n\nDocument text (truncated):\n\n{head}\n\nReturn the JSON object now."
    );

    let raw = crate::llm::oneshot::complete(&config, system, &user_msg)
        .await
        .ok();
    persist_profile_from_raw(&state.db, user_id, file_id, doc_type, config.model.as_str(), raw.as_deref()).await
}

/// Deterministic seam (mockable online path / offline fallback): given the
/// raw model output (`None` = offline or any call failure), parse and store a
/// profile. Returns Ok(true) iff a row was written. Never errors on a missing
/// or unparseable response — that is the offline-safe contract.
pub async fn persist_profile_from_raw(
    db: &sqlx::SqlitePool,
    user_id: &str,
    file_id: &str,
    doc_type: Option<&str>,
    model: &str,
    raw: Option<&str>,
) -> Result<bool> {
    let Some(raw) = raw else { return Ok(false) };
    let Some(p) = parse_profile(raw) else { return Ok(false) };
    // Idempotent: re-ingest replaces the prior profile (PK = file_id).
    sqlx::query(
        "INSERT INTO corpus_profiles \
            (file_id, user_id, doc_type, summary, structure, style_notes, reusable_phrases, model) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(file_id) DO UPDATE SET \
            user_id = excluded.user_id, doc_type = excluded.doc_type, summary = excluded.summary, \
            structure = excluded.structure, style_notes = excluded.style_notes, \
            reusable_phrases = excluded.reusable_phrases, model = excluded.model, \
            created_at = datetime('now')",
    )
    .bind(file_id)
    .bind(user_id)
    .bind(doc_type)
    .bind(&p.summary)
    .bind(&p.structure)
    .bind(&p.style_notes)
    .bind(&p.reusable_phrases)
    .bind(model)
    .execute(db)
    .await
    .context("upsert corpus_profile")?;
    Ok(true)
}

/// Parse the distillation response into a profile. Tolerates code fences /
/// surrounding prose via `extract_json_object`. Returns None when there is no
/// JSON object at all OR when every field is empty (nothing worth storing).
fn parse_profile(raw: &str) -> Option<DistilledProfile> {
    let json_slice = extract_json_object(raw)?;
    let v: Value = serde_json::from_str(json_slice).ok()?;

    // string field -> trimmed non-empty, treating "null"/"unknown"/"" as None
    let s = |key: &str| -> Option<String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty() && !x.eq_ignore_ascii_case("null") && !x.eq_ignore_ascii_case("unknown"))
    };
    // array-of-strings field -> newline-joined (structure) or compact JSON
    // array string (reusable_phrases). Accepts either an array or a string.
    let arr_lines = |key: &str| -> Option<String> {
        match v.get(key) {
            Some(Value::Array(items)) => {
                let lines: Vec<String> = items
                    .iter()
                    .filter_map(|x| x.as_str())
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect();
                if lines.is_empty() { None } else { Some(lines.join("\n")) }
            }
            Some(Value::String(st)) => {
                let st = st.trim();
                if st.is_empty() { None } else { Some(st.to_string()) }
            }
            _ => None,
        }
    };
    let arr_json = |key: &str| -> Option<String> {
        match v.get(key) {
            Some(Value::Array(items)) => {
                let kept: Vec<&str> = items
                    .iter()
                    .filter_map(|x| x.as_str())
                    .map(|x| x.trim())
                    .filter(|x| !x.is_empty())
                    .collect();
                if kept.is_empty() { None } else { serde_json::to_string(&kept).ok() }
            }
            // A bare string (model didn't return an array): wrap it as a
            // single-element JSON array so the column is always valid JSON.
            Some(Value::String(st)) => {
                let st = st.trim();
                if st.is_empty() { None } else { serde_json::to_string(&[st]).ok() }
            }
            _ => None,
        }
    };

    let p = DistilledProfile {
        summary: s("summary"),
        structure: arr_lines("structure"),
        style_notes: s("style_notes"),
        reusable_phrases: arr_json("reusable_phrases"),
    };
    // Nothing worth storing if every field is empty.
    if p.summary.is_none() && p.structure.is_none() && p.style_notes.is_none() && p.reusable_phrases.is_none() {
        return None;
    }
    Some(p)
}

/// Find the first balanced `{...}` object in a string (handles ```json fences
/// and chatty wrappers).
fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let bytes = raw.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for i in start..raw.len() {
        let c = bytes[i] as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&raw[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

async fn set_status(db: &sqlx::SqlitePool, file_id: &str, status: &str) -> Result<()> {
    sqlx::query("UPDATE corpus_files SET status = ? WHERE id = ?")
        .bind(status)
        .bind(file_id)
        .execute(db)
        .await
        .with_context(|| format!("set status={status}"))?;
    Ok(())
}

async fn mark_unsupported(db: &sqlx::SqlitePool, file_id: &str, reason: &str) -> Result<()> {
    sqlx::query("UPDATE corpus_files SET status = 'unsupported', error = ? WHERE id = ?")
        .bind(reason)
        .bind(file_id)
        .execute(db)
        .await
        .context("mark unsupported")?;
    Ok(())
}

async fn emit(progress: Option<&tokio::sync::mpsc::Sender<IngestEvent>>, ev: IngestEvent) {
    if let Some(tx) = progress {
        let _ = tx.send(ev).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_json() {
        let raw = r#"{"doc_type":"petition","case_type":"writ","court":"Delhi HC","doc_date":"2023-04-01","language":"en","looks_like_template":false}"#;
        let m = parse_meta(raw).expect("should parse");
        assert_eq!(m.doc_type.as_deref(), Some("petition"));
        assert_eq!(m.case_type.as_deref(), Some("writ"));
        assert_eq!(m.court.as_deref(), Some("Delhi HC"));
        assert!(!m.looks_like_template);
    }

    #[test]
    fn parses_json_in_code_fence_with_prose() {
        let raw = "Here is the classification:\n```json\n{\"doc_type\":\"template\",\"looks_like_template\":true,\"court\":null}\n```\nHope that helps!";
        let m = parse_meta(raw).expect("should parse despite fence/prose");
        assert_eq!(m.doc_type.as_deref(), Some("template"));
        assert!(m.looks_like_template);
        assert_eq!(m.court, None);
    }

    #[test]
    fn null_and_unknown_strings_become_none() {
        let raw = r#"{"doc_type":"other","court":"null","doc_date":"unknown","language":""}"#;
        let m = parse_meta(raw).expect("should parse");
        assert_eq!(m.court, None);
        assert_eq!(m.doc_date, None);
        assert_eq!(m.language, None);
    }

    #[test]
    fn garbage_returns_none() {
        assert!(parse_meta("not json at all").is_none());
        assert!(parse_meta("").is_none());
    }

    #[test]
    fn extracts_nested_object_span() {
        let raw = "prefix {\"a\": {\"b\": 1}, \"c\": 2} suffix";
        let obj = extract_json_object(raw).unwrap();
        assert_eq!(obj, "{\"a\": {\"b\": 1}, \"c\": 2}");
    }

    #[test]
    fn distill_parse_handles_fence_and_arrays() {
        let raw = "```json\n{\"summary\":\"A consumer complaint.\",\"structure\":[\"Facts\",\"Grounds\",\"Relief sought\"],\"style_notes\":\"Formal register.\",\"reusable_phrases\":[\"MOST RESPECTFULLY SHEWETH\"]}\n```";
        let p = parse_profile(raw).expect("parses");
        assert!(p.summary.is_some());
        assert!(p.structure.as_deref().unwrap().contains('\n')); // newline-joined outline
        assert!(p.reusable_phrases.as_deref().unwrap().starts_with('[')); // JSON array
    }

    #[test]
    fn distill_parse_empty_object_is_none() {
        assert!(parse_profile("{}").is_none());
        assert!(parse_profile("no json").is_none());
    }
}
