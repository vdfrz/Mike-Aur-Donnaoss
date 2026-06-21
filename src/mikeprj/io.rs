//! `.mikeprj` build / parse pipeline.
//!
//! Splits the work in two layers so the route handlers stay small:
//!
//!  - `build_payload(...)` queries the DB and storage, assembles a
//!    `Payload` struct (project + documents + reviews + workflows +
//!    optional chats), and serialises it as a ZIP.
//!  - `unpack_payload(...)` does the inverse: ZIP bytes → `Payload`.
//!
//! The actual encryption / file-format envelope lives in `crypto.rs`;
//! this module is format-agnostic about transport.

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;
use std::io::{Cursor, Read, Write};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use super::manifest::{
    ChatRecord, DocumentRecord, Manifest, ManifestContents, ProjectRecord,
    TabularReviewRecord, WorkflowRecord, SCHEMA_VERSION,
};

#[derive(Debug)]
pub struct Payload {
    pub manifest: Manifest,
    pub project: ProjectRecord,
    pub documents: Vec<(DocumentRecord, Vec<u8>)>,
    pub tabular_reviews: Vec<TabularReviewRecord>,
    pub workflows: Vec<WorkflowRecord>,
    pub chats: Vec<ChatRecord>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ExportOptions {
    pub include_chats: bool,
}

/// Read everything that belongs to `project_id` for `user_id` from the
/// DB + storage and assemble it into a `Payload`. The caller owns the
/// SqlitePool and a storage handle (passed as a closure so we don't
/// take a hard dep on the storage trait here — easier to mock in
/// tests, and lets the route handler decide between local / S3).
pub async fn build_payload(
    db: &SqlitePool,
    user_id: &str,
    project_id: &str,
    options: ExportOptions,
    read_storage: impl Fn(&str) -> futures_util::future::BoxFuture<'_, Result<Vec<u8>>>,
) -> Result<Payload> {
    // ---------- project ----------
    let p_row: Option<(String, String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT id, name, cm_number, created_at, isolation_mode \
         FROM projects WHERE id = ? AND user_id = ?",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_optional(db)
    .await
    .context("read project")?;
    let (pid, name, cm_number, created_at, _iso) =
        p_row.ok_or_else(|| anyhow!("project not found"))?;
    let project = ProjectRecord {
        id: pid.clone(),
        name,
        cm_number,
        created_at,
        original_creator_email: None,
    };

    // ---------- documents ----------
    let doc_rows: Vec<(
        String, String, String, i64, Option<String>, String,
    )> = sqlx::query_as(
        "SELECT id, filename, file_type, size_bytes, storage_path, created_at \
         FROM documents WHERE user_id = ? AND project_id = ?",
    )
    .bind(user_id)
    .bind(&pid)
    .fetch_all(db)
    .await
    .context("read documents")?;

    let mut documents: Vec<(DocumentRecord, Vec<u8>)> = Vec::with_capacity(doc_rows.len());
    for (id, filename, file_type, size_bytes, storage_path, created_at) in doc_rows {
        let bytes = if let Some(key) = storage_path.as_deref() {
            read_storage(key)
                .await
                .with_context(|| format!("read document content {id}"))?
        } else {
            Vec::new()
        };
        let sha = sha256_hex(&bytes);
        documents.push((
            DocumentRecord {
                id,
                filename,
                file_type: Some(file_type),
                mime_type: None,
                size_bytes: Some(size_bytes as u64),
                sha256: sha,
                created_at,
            },
            bytes,
        ));
    }

    // ---------- tabular reviews (config only, no cells) ----------
    let tr_rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, title, columns_config, created_at \
         FROM tabular_reviews WHERE user_id = ? AND project_id = ?",
    )
    .bind(user_id)
    .bind(&pid)
    .fetch_all(db)
    .await
    .context("read tabular_reviews")?;
    let tabular_reviews: Vec<TabularReviewRecord> = tr_rows
        .into_iter()
        .map(|(id, title, cfg, created_at)| TabularReviewRecord {
            id,
            title: Some(title),
            columns_config: serde_json::from_str(&cfg).unwrap_or(Value::Array(Vec::new())),
            document_ids: Vec::new(), // only configuration travels
            created_at,
        })
        .collect();

    // ---------- workflows (custom only — no built-ins, they're recreated by id) ----------
    let wf_rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, title, prompt_md FROM workflows WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
    .context("read workflows")?;
    let workflows: Vec<WorkflowRecord> = wf_rows
        .into_iter()
        .map(|(id, title, prompt_md)| WorkflowRecord {
            id,
            title,
            r#type: "assistant".to_string(),
            prompt_md: Some(prompt_md),
            columns_config: None,
            practice: None,
        })
        .collect();

    // ---------- chats (opt-in) ----------
    let chats = if options.include_chats {
        let chat_rows: Vec<(String, Option<String>, String)> = sqlx::query_as(
            "SELECT id, title, created_at FROM chats \
             WHERE user_id = ? AND project_id = ?",
        )
        .bind(user_id)
        .bind(&pid)
        .fetch_all(db)
        .await
        .context("read chats")?;
        let mut out = Vec::with_capacity(chat_rows.len());
        for (cid, title, created_at) in chat_rows {
            let msg_rows: Vec<(String, String, String)> = sqlx::query_as(
                "SELECT role, content, created_at FROM messages \
                 WHERE chat_id = ? ORDER BY created_at ASC",
            )
            .bind(&cid)
            .fetch_all(db)
            .await
            .unwrap_or_default();
            let messages = msg_rows
                .into_iter()
                .map(|(role, content, created_at)| {
                    serde_json::json!({
                        "role": role, "content": content, "created_at": created_at,
                    })
                })
                .collect();
            out.push(ChatRecord {
                id: cid,
                title,
                created_at,
                messages,
            });
        }
        out
    } else {
        Vec::new()
    };

    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        exporter: format!("MikeRust {}", env!("CARGO_PKG_VERSION")),
        exported_at: Utc::now().to_rfc3339(),
        exported_by_display_name: None,
        contents: ManifestContents {
            project: true,
            document_count: documents.len() as u32,
            tabular_review_count: tabular_reviews.len() as u32,
            workflow_count: workflows.len() as u32,
            chat_count: chats.len() as u32,
            includes_chats: options.include_chats,
        },
    };

    Ok(Payload {
        manifest,
        project,
        documents,
        tabular_reviews,
        workflows,
        chats,
    })
}

/// Serialise a `Payload` as a ZIP archive (the bytes that go into
/// `crypto::seal`). Layout matches the spec in `mikeprj/mod.rs`.
pub fn zip_payload(payload: &Payload) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::with_capacity(64 * 1024));
    {
        let mut z = ZipWriter::new(&mut buf);
        // Compression: deflate is good for JSON; documents are mostly
        // already-compressed (PDF/DOCX) so deflate is mostly a no-op
        // there but doesn't hurt.
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);

        write_json(&mut z, "manifest.json", &payload.manifest, opts)?;
        write_json(&mut z, "project.json", &payload.project, opts)?;

        for (doc, bytes) in &payload.documents {
            let dir = format!("documents/{}/", doc.id);
            write_json(&mut z, &format!("{dir}meta.json"), doc, opts)?;
            z.start_file(format!("{dir}content.bin"), opts)?;
            z.write_all(bytes)?;
        }
        for tr in &payload.tabular_reviews {
            write_json(
                &mut z,
                &format!("tabular_reviews/{}.json", tr.id),
                tr,
                opts,
            )?;
        }
        for wf in &payload.workflows {
            write_json(&mut z, &format!("workflows/{}.json", wf.id), wf, opts)?;
        }
        for c in &payload.chats {
            write_json(&mut z, &format!("chats/{}.json", c.id), c, opts)?;
        }

        // Friendly README so the file isn't completely opaque to anyone
        // who unzips it manually (e.g. forensic recovery).
        z.start_file("README.txt", opts)?;
        z.write_all(b"This is a MikeRust project archive (.mikeprj).\n")?;
        z.write_all(b"It is meant to be imported via the MikeRust UI.\n")?;
        z.write_all(b"Manual extraction is supported but you'll lose the citation links.\n")?;

        z.finish()?;
    }
    Ok(buf.into_inner())
}

fn write_json<W: Write + std::io::Seek, T: serde::Serialize>(
    z: &mut ZipWriter<W>,
    name: &str,
    value: &T,
    opts: SimpleFileOptions,
) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value)?;
    z.start_file(name, opts)?;
    z.write_all(&bytes)?;
    Ok(())
}

/// Parse a ZIP payload (already decrypted by `crypto::open`) back into
/// a `Payload`. Used by the import endpoint.
pub fn unzip_payload(zip_bytes: &[u8]) -> Result<Payload> {
    let mut zip = ZipArchive::new(Cursor::new(zip_bytes))?;

    let manifest: Manifest = read_json(&mut zip, "manifest.json")?;
    if manifest.schema_version != SCHEMA_VERSION {
        return Err(anyhow!(
            "unsupported .mikeprj schema_version {}; this build expects {}",
            manifest.schema_version,
            SCHEMA_VERSION
        ));
    }
    let project: ProjectRecord = read_json(&mut zip, "project.json")?;

    let mut documents: Vec<(DocumentRecord, Vec<u8>)> = Vec::new();
    let mut tabular_reviews: Vec<TabularReviewRecord> = Vec::new();
    let mut workflows: Vec<WorkflowRecord> = Vec::new();
    let mut chats: Vec<ChatRecord> = Vec::new();

    // First pass: list filenames so we can iterate index-by-name without
    // borrowing `zip` while iterating (zip's API doesn't expose a
    // borrow-friendly iterator for both name + content).
    let names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();

    for name in &names {
        if let Some(rest) = name.strip_prefix("documents/") {
            // pattern: documents/<doc_id>/meta.json   or  content.bin
            if let Some((id, tail)) = rest.split_once('/') {
                if tail == "meta.json" {
                    let meta: DocumentRecord = read_json(&mut zip, name)?;
                    let content_path = format!("documents/{id}/content.bin");
                    let bytes = read_bytes(&mut zip, &content_path)
                        .with_context(|| format!("read content for document {id}"))?;
                    // Verify the stored integrity hash: reject truncated,
                    // swapped, or corrupted content instead of importing it.
                    let actual = sha256_hex(&bytes);
                    if actual != meta.sha256 {
                        return Err(anyhow!(
                            "integrity check failed for document {id}: sha256 mismatch \
                             (expected {}, got {actual})",
                            meta.sha256
                        ));
                    }
                    documents.push((meta, bytes));
                }
            }
        } else if let Some(_) = name.strip_prefix("tabular_reviews/") {
            if name.ends_with(".json") {
                let tr: TabularReviewRecord = read_json(&mut zip, name)?;
                tabular_reviews.push(tr);
            }
        } else if let Some(_) = name.strip_prefix("workflows/") {
            if name.ends_with(".json") {
                let wf: WorkflowRecord = read_json(&mut zip, name)?;
                workflows.push(wf);
            }
        } else if let Some(_) = name.strip_prefix("chats/") {
            if name.ends_with(".json") {
                let c: ChatRecord = read_json(&mut zip, name)?;
                chats.push(c);
            }
        }
    }

    Ok(Payload {
        manifest,
        project,
        documents,
        tabular_reviews,
        workflows,
        chats,
    })
}

fn read_json<R: Read + std::io::Seek, T: serde::de::DeserializeOwned>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<T> {
    let mut f = zip.by_name(name).context(format!("missing entry: {name}"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    serde_json::from_slice(&buf).context(format!("parse {name}"))
}

fn read_bytes<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<Vec<u8>> {
    let mut f = zip.by_name(name).context(format!("missing entry: {name}"))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    fn doc_record(id: &str, bytes: &[u8], sha: &str) -> DocumentRecord {
        DocumentRecord {
            id: id.to_string(),
            filename: format!("{id}.bin"),
            file_type: Some("bin".to_string()),
            mime_type: None,
            size_bytes: Some(bytes.len() as u64),
            sha256: sha.to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn minimal_payload(documents: Vec<(DocumentRecord, Vec<u8>)>) -> Payload {
        Payload {
            manifest: Manifest {
                schema_version: SCHEMA_VERSION,
                exporter: "test".into(),
                exported_at: "2026-01-01T00:00:00Z".into(),
                exported_by_display_name: None,
                contents: ManifestContents {
                    project: true,
                    document_count: documents.len() as u32,
                    tabular_review_count: 0,
                    workflow_count: 0,
                    chat_count: 0,
                    includes_chats: false,
                },
            },
            project: ProjectRecord {
                id: "proj-1".into(),
                name: "Proj".into(),
                cm_number: None,
                created_at: "2026-01-01T00:00:00Z".into(),
                original_creator_email: None,
            },
            documents,
            tabular_reviews: Vec::new(),
            workflows: Vec::new(),
            chats: Vec::new(),
        }
    }

    #[test]
    fn unzip_round_trip_with_correct_sha_succeeds() {
        let bytes = b"genuine document content".to_vec();
        let sha = sha256_hex(&bytes);
        let payload = minimal_payload(vec![(doc_record("d1", &bytes, &sha), bytes.clone())]);
        let zip = zip_payload(&payload).unwrap();
        let parsed = unzip_payload(&zip).unwrap();
        assert_eq!(parsed.documents.len(), 1);
        assert_eq!(parsed.documents[0].1, bytes);
    }

    #[test]
    fn unzip_rejects_sha256_mismatch() {
        // content.bin bytes do not match the sha256 recorded in meta.json
        // (simulating truncated / swapped / tampered content).
        let bytes = b"genuine document content".to_vec();
        let wrong_sha = sha256_hex(b"different bytes entirely");
        let payload = minimal_payload(vec![(doc_record("d1", &bytes, &wrong_sha), bytes)]);
        let zip = zip_payload(&payload).unwrap();
        let res = unzip_payload(&zip);
        assert!(res.is_err(), "sha256 mismatch must be rejected on import");
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("sha256") || msg.contains("integrity") || msg.contains("d1"),
            "error should name the integrity failure, got: {msg}"
        );
    }

    #[test]
    fn unzip_rejects_corrupted_content_bytes() {
        // Build a valid zip, then flip a byte inside the stored content.bin
        // and confirm the stored hash no longer matches.
        let bytes = b"the prayer and relief clause".to_vec();
        let sha = sha256_hex(&bytes);
        let payload = minimal_payload(vec![(doc_record("d1", &bytes, &sha), bytes)]);
        let zip = zip_payload(&payload).unwrap();

        // Re-pack with corrupted content but the original (now wrong) hash
        // by constructing a payload whose bytes differ from meta.sha256.
        let corrupted = b"the prayer and relief clauseX".to_vec();
        let payload2 = minimal_payload(vec![(doc_record("d1", &corrupted, &sha), corrupted)]);
        let zip2 = zip_payload(&payload2).unwrap();
        assert!(unzip_payload(&zip).is_ok());
        assert!(
            unzip_payload(&zip2).is_err(),
            "corrupted content vs stored hash must be rejected"
        );
    }

    async fn mem_db_with_one_document(storage_path: Option<&str>) -> SqlitePool {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE projects (id TEXT, user_id TEXT, name TEXT, cm_number TEXT, \
             created_at TEXT, isolation_mode TEXT)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE documents (id TEXT, user_id TEXT, project_id TEXT, filename TEXT, \
             file_type TEXT, size_bytes INTEGER, storage_path TEXT, created_at TEXT)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("CREATE TABLE tabular_reviews (id TEXT, user_id TEXT, project_id TEXT, title TEXT, columns_config TEXT, created_at TEXT)")
            .execute(&db).await.unwrap();
        sqlx::query("CREATE TABLE workflows (id TEXT, user_id TEXT, title TEXT, prompt_md TEXT)")
            .execute(&db).await.unwrap();
        sqlx::query(
            "INSERT INTO projects (id, user_id, name, cm_number, created_at, isolation_mode) \
             VALUES ('proj-1', 'user-1', 'Proj', NULL, '2026-01-01T00:00:00Z', NULL)",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO documents (id, user_id, project_id, filename, file_type, size_bytes, storage_path, created_at) \
             VALUES ('doc-1', 'user-1', 'proj-1', 'a.bin', 'bin', 10, ?, '2026-01-01T00:00:00Z')",
        )
        .bind(storage_path)
        .execute(&db)
        .await
        .unwrap();
        db
    }

    #[tokio::test]
    async fn build_payload_propagates_storage_read_error() {
        let db = mem_db_with_one_document(Some("documents/user-1/doc-1")).await;
        // Storage handle that always fails (missing object / S3 outage).
        let res = build_payload(
            &db,
            "user-1",
            "proj-1",
            ExportOptions::default(),
            |key| {
                let key = key.to_string();
                Box::pin(async move { Err(anyhow!("storage unavailable for {key}")) })
            },
        )
        .await;
        assert!(
            res.is_err(),
            "a storage read failure must abort the export, not ship a 0-byte document"
        );
        let msg = res.unwrap_err().to_string();
        assert!(
            msg.contains("doc-1") || msg.contains("storage") || msg.contains("content"),
            "error should identify the failed document read, got: {msg}"
        );
    }

    #[tokio::test]
    async fn build_payload_succeeds_when_storage_reads_ok() {
        let db = mem_db_with_one_document(Some("documents/user-1/doc-1")).await;
        let content = b"hello bytes".to_vec();
        let payload = build_payload(&db, "user-1", "proj-1", ExportOptions::default(), {
            let content = content.clone();
            move |_key| {
                let content = content.clone();
                Box::pin(async move { Ok(content) })
            }
        })
        .await
        .unwrap();
        assert_eq!(payload.documents.len(), 1);
        assert_eq!(payload.documents[0].1, content);
        assert_eq!(payload.documents[0].0.sha256, sha256_hex(&content));
    }
}
