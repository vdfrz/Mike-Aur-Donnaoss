//! Builtin tools that ship with Mike's legal-assistant identity.
//!
//! Mirror the OpenAI/Anthropic tool schemas declared by upstream Mike
//! (`backend/src/lib/chatTools.ts`):
//!
//! * `read_document` — fetch full text of a chat-attached document by `doc-N` label
//! * `find_in_document` — case-insensitive search within a document
//! * `read_workflow` — load the Markdown body of a saved workflow by id
//! * `generate_docx` — produce a downloadable .docx (stub for now)
//! * `edit_document` — modify an existing .docx (stub for now)
//!
//! The model is expected to call these tools to ground its answers. The
//! dispatch fn returns plain-string results that get fed back as `tool`
//! messages in the next iteration, exactly like MCP tool results.

use crate::llm::types::{ToolFunction, ToolSchema};
use crate::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;

const READ_DOCUMENT: &str = "read_document";
const FIND_IN_DOCUMENT: &str = "find_in_document";
const READ_WORKFLOW: &str = "read_workflow";
const GENERATE_DOCX: &str = "generate_docx";
const EDIT_DOCUMENT: &str = "edit_document";

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        READ_DOCUMENT | FIND_IN_DOCUMENT | READ_WORKFLOW | GENERATE_DOCX | EDIT_DOCUMENT
    )
}

pub fn schemas() -> Vec<ToolSchema> {
    fn fun(name: &str, description: &str, parameters: Value) -> ToolSchema {
        ToolSchema {
            kind: "function".to_string(),
            function: ToolFunction {
                name: name.to_string(),
                description: description.to_string(),
                parameters,
            },
        }
    }

    vec![
        fun(
            READ_DOCUMENT,
            "Read the full text content of a document attached by the user. Always call this before answering questions about, summarising, or citing from a document.",
            json!({
                "type": "object",
                "properties": {
                    "doc_id": {
                        "type": "string",
                        "description": "The document ID to read (e.g. 'doc-0', 'doc-1')"
                    }
                },
                "required": ["doc_id"]
            }),
        ),
        fun(
            FIND_IN_DOCUMENT,
            "Search for specific strings inside a document — a Ctrl+F equivalent. Returns each match with surrounding context. Matching is case-insensitive and whitespace-tolerant.",
            json!({
                "type": "object",
                "properties": {
                    "doc_id": { "type": "string", "description": "The document ID to search (e.g. 'doc-0')." },
                    "query":  { "type": "string", "description": "The string to search for (case-insensitive)." },
                    "max_results": { "type": "integer", "description": "Maximum matches to return (default 20).", "minimum": 1, "maximum": 200 }
                },
                "required": ["doc_id", "query"]
            }),
        ),
        fun(
            READ_WORKFLOW,
            "Read the full instructions (prompt) of a workflow by its ID. Call this after a workflow marker has been mentioned.",
            json!({
                "type": "object",
                "properties": {
                    "workflow_id": { "type": "string", "description": "The workflow ID to read." }
                },
                "required": ["workflow_id"]
            }),
        ),
        fun(
            GENERATE_DOCX,
            "Produce a downloadable .docx document. Pass `title` (file label) and `body` (Markdown). Returns the new document id and filename.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Document title / base filename (no extension)." },
                    "body":  { "type": "string", "description": "Document content in Markdown. Headings (#, ##, ###), bullet lists and bold/italic are honored." }
                },
                "required": ["title", "body"]
            }),
        ),
        fun(
            EDIT_DOCUMENT,
            "Apply minimal substitutions to an existing .docx document attached to the chat. Pass `doc_id` (e.g. 'doc-0') and an array of `edits`, each with `find` and `replace` strings. The find string MUST appear verbatim in the document.",
            json!({
                "type": "object",
                "properties": {
                    "doc_id": { "type": "string", "description": "The document ID to edit (e.g. 'doc-0')." },
                    "edits": {
                        "type": "array",
                        "description": "List of substitutions to apply atomically.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "find":    { "type": "string" },
                                "replace": { "type": "string" }
                            },
                            "required": ["find", "replace"]
                        }
                    }
                },
                "required": ["doc_id", "edits"]
            }),
        ),
    ]
}

/// `doc_label_map` maps the chat-local label (`doc-0`, `doc-1`, …) to the
/// real `documents.id` UUID stored in SQLite. Built by the chat dispatcher
/// from the message's attached files.
pub async fn dispatch(
    state: &AppState,
    user_id: &str,
    doc_label_map: &HashMap<String, String>,
    name: &str,
    arguments: &Value,
) -> String {
    match name {
        READ_DOCUMENT => exec_read_document(state, user_id, doc_label_map, arguments).await,
        FIND_IN_DOCUMENT => exec_find_in_document(state, user_id, doc_label_map, arguments).await,
        READ_WORKFLOW => exec_read_workflow(state, user_id, arguments).await,
        GENERATE_DOCX => exec_generate_docx(state, user_id, arguments).await,
        EDIT_DOCUMENT => exec_edit_document(state, user_id, doc_label_map, arguments).await,
        other => json!({"error": format!("unknown builtin tool: {other}")}).to_string(),
    }
}

async fn resolve_doc(
    state: &AppState,
    user_id: &str,
    doc_label_map: &HashMap<String, String>,
    label_or_id: &str,
) -> Option<(String, String, Option<String>)> {
    let real_id = doc_label_map
        .get(label_or_id)
        .cloned()
        .unwrap_or_else(|| label_or_id.to_string());
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT filename, file_type, storage_path FROM documents WHERE id = ? AND user_id = ?",
    )
    .bind(&real_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    row
}

async fn exec_read_document(
    state: &AppState,
    user_id: &str,
    doc_label_map: &HashMap<String, String>,
    arguments: &Value,
) -> String {
    let doc_label = arguments.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
    if doc_label.is_empty() {
        return json!({"error": "doc_id is required"}).to_string();
    }
    let Some((filename, file_type, Some(storage_path))) =
        resolve_doc(state, user_id, doc_label_map, doc_label).await
    else {
        return json!({"error": format!("document {doc_label} not found")}).to_string();
    };
    let bytes = match crate::storage::make_storage()
        .ok()
        .and_then(|s| Some(s))
    {
        Some(s) => match s.get(&storage_path).await {
            Ok(b) => b,
            Err(e) => return json!({"error": format!("storage read: {e}")}).to_string(),
        },
        None => return json!({"error": "storage backend unavailable"}).to_string(),
    };
    let text = extract_text(&file_type, &filename, &bytes);
    json!({
        "doc_id": doc_label,
        "filename": filename,
        "file_type": file_type,
        "text": text,
    })
    .to_string()
}

async fn exec_find_in_document(
    state: &AppState,
    user_id: &str,
    doc_label_map: &HashMap<String, String>,
    arguments: &Value,
) -> String {
    let doc_label = arguments.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
    let query = arguments.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let max_results = arguments
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .min(200) as usize;
    if doc_label.is_empty() || query.is_empty() {
        return json!({"error": "doc_id and query are required"}).to_string();
    }
    let Some((filename, file_type, Some(storage_path))) =
        resolve_doc(state, user_id, doc_label_map, doc_label).await
    else {
        return json!({"error": format!("document {doc_label} not found")}).to_string();
    };
    let bytes = match crate::storage::make_storage()
        .ok()
        .and_then(|s| Some(s))
    {
        Some(s) => match s.get(&storage_path).await {
            Ok(b) => b,
            Err(e) => return json!({"error": format!("storage read: {e}")}).to_string(),
        },
        None => return json!({"error": "storage backend unavailable"}).to_string(),
    };
    let text = extract_text(&file_type, &filename, &bytes);

    // Case-insensitive, whitespace-tolerant search.
    let needle: String = query.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let haystack_norm: String = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();

    let mut matches = Vec::new();
    let mut start = 0usize;
    while let Some(idx) = haystack_norm[start..].find(&needle) {
        let abs = start + idx;
        let ctx_lo = abs.saturating_sub(60);
        let ctx_hi = (abs + needle.len() + 60).min(haystack_norm.len());
        let snippet = &haystack_norm[ctx_lo..ctx_hi];
        matches.push(json!({
            "offset": abs,
            "snippet": snippet,
        }));
        if matches.len() >= max_results { break; }
        start = abs + needle.len();
    }
    json!({
        "doc_id": doc_label,
        "query": query,
        "match_count": matches.len(),
        "matches": matches,
    })
    .to_string()
}

async fn exec_read_workflow(state: &AppState, user_id: &str, arguments: &Value) -> String {
    let id = arguments.get("workflow_id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() {
        return json!({"error": "workflow_id is required"}).to_string();
    }
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT title, prompt_md FROM workflows WHERE id = ? AND user_id = ?")
            .bind(id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    let Some((title, prompt_md)) = row else {
        return json!({"error": format!("workflow {id} not found")}).to_string();
    };
    json!({ "workflow_id": id, "title": title, "prompt_md": prompt_md }).to_string()
}

async fn exec_generate_docx(state: &AppState, user_id: &str, arguments: &Value) -> String {
    let title = arguments.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled").trim().to_string();
    let body = arguments.get("body").and_then(|v| v.as_str()).unwrap_or("");
    if body.is_empty() {
        return json!({"error": "body (Markdown) is required"}).to_string();
    }
    let bytes = match crate::pdf::docx_writer::markdown_to_docx(&title, body) {
        Ok(b) => b,
        Err(e) => return json!({"error": format!("docx build: {e}")}).to_string(),
    };
    let safe_title = sanitize_filename(&title);
    let filename = format!("{safe_title}.docx");
    let doc_id = uuid::Uuid::new_v4().to_string();
    let storage_path = format!("documents/{user_id}/{doc_id}");

    let storage = match crate::storage::make_storage() {
        Ok(s) => s,
        Err(e) => return json!({"error": format!("storage: {e}")}).to_string(),
    };
    if let Err(e) = storage
        .put(
            &storage_path,
            &bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await
    {
        return json!({"error": format!("storage write: {e}")}).to_string();
    }

    let size = bytes.len() as i64;
    if let Err(e) = sqlx::query(
        "INSERT INTO documents (id, user_id, project_id, filename, file_type, size_bytes, storage_path, status) \
         VALUES (?, ?, NULL, ?, 'docx', ?, ?, 'ready')",
    )
    .bind(&doc_id)
    .bind(user_id)
    .bind(&filename)
    .bind(size)
    .bind(&storage_path)
    .execute(&state.db)
    .await
    {
        return json!({"error": format!("db: {e}")}).to_string();
    }

    json!({
        "doc_id": doc_id,
        "filename": filename,
        "size_bytes": size,
        "note": "Document persisted as a standalone document. Call read_document with this doc_id to verify content before describing it to the user."
    })
    .to_string()
}

async fn exec_edit_document(
    state: &AppState,
    user_id: &str,
    doc_label_map: &HashMap<String, String>,
    arguments: &Value,
) -> String {
    let label = arguments.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
    let edits_val = arguments.get("edits").and_then(|v| v.as_array());
    let Some(edits_val) = edits_val else {
        return json!({"error": "edits array is required"}).to_string();
    };
    let edits: Vec<crate::pdf::docx_writer::DocxEdit> = edits_val
        .iter()
        .filter_map(|e| {
            let find = e.get("find").and_then(|v| v.as_str())?.to_string();
            let replace = e.get("replace").and_then(|v| v.as_str())?.to_string();
            Some(crate::pdf::docx_writer::DocxEdit { find, replace })
        })
        .collect();
    if edits.is_empty() {
        return json!({"error": "no valid edit entries"}).to_string();
    }

    let Some((filename, file_type, Some(storage_path))) =
        resolve_doc(state, user_id, doc_label_map, label).await
    else {
        return json!({"error": format!("document {label} not found")}).to_string();
    };
    if file_type != "docx" {
        return json!({"error": format!("edit_document only supports .docx files (got {file_type})")}).to_string();
    }

    let storage = match crate::storage::make_storage() {
        Ok(s) => s,
        Err(e) => return json!({"error": format!("storage: {e}")}).to_string(),
    };
    let bytes = match storage.get(&storage_path).await {
        Ok(b) => b,
        Err(e) => return json!({"error": format!("storage read: {e}")}).to_string(),
    };

    let (new_bytes, hits) = match crate::pdf::docx_writer::apply_text_edits(&bytes, &edits) {
        Ok(x) => x,
        Err(e) => return json!({"error": format!("docx edit: {e}")}).to_string(),
    };

    if let Err(e) = storage
        .put(
            &storage_path,
            &new_bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await
    {
        return json!({"error": format!("storage write: {e}")}).to_string();
    }
    let new_size = new_bytes.len() as i64;
    let real_id = doc_label_map
        .get(label)
        .cloned()
        .unwrap_or_else(|| label.to_string());
    let _ = sqlx::query("UPDATE documents SET size_bytes = ? WHERE id = ? AND user_id = ?")
        .bind(new_size)
        .bind(&real_id)
        .bind(user_id)
        .execute(&state.db)
        .await;

    let summary: Vec<Value> = edits
        .iter()
        .zip(hits.iter())
        .map(|(e, h)| json!({"find": e.find, "replace": e.replace, "hits": h}))
        .collect();
    json!({
        "doc_id": label,
        "filename": filename,
        "edits_applied": summary,
    })
    .to_string()
}

fn sanitize_filename(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() { return "Untitled".to_string(); }
    let cleaned: String = trimmed
        .chars()
        .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
        .collect();
    cleaned.chars().take(60).collect::<String>().trim().to_string()
}

fn extract_text(file_type: &str, filename: &str, bytes: &[u8]) -> String {
    match file_type {
        "docx" => crate::pdf::extract_docx_text(bytes).unwrap_or_default(),
        "rtf" => {
            // Same path the sync scanner uses — RtfDocument::get_text()
            // returns the body without control words / fonts / pictures.
            let raw = String::from_utf8_lossy(bytes);
            rtf_parser::RtfDocument::try_from(raw.as_ref())
                .map(|d| d.get_text())
                .unwrap_or_default()
        }
        "xlsx" | "xls" | "xlsb" | "ods" => {
            crate::pdf::extract_xlsx_text(bytes).unwrap_or_default()
        }
        "txt" | "md" | "csv" => String::from_utf8_lossy(bytes).to_string(),
        "pdf" => {
            #[cfg(feature = "pdf")]
            {
                let tmp = std::env::temp_dir().join(format!("mike-builtin-{filename}"));
                if std::fs::write(&tmp, bytes).is_ok() {
                    let out = crate::pdf::extract_full_text(&tmp).unwrap_or_default();
                    let _ = std::fs::remove_file(&tmp);
                    out
                } else {
                    String::new()
                }
            }
            #[cfg(not(feature = "pdf"))]
            {
                let _ = filename;
                String::new()
            }
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_builtin_recognises_each_tool() {
        for name in ["read_document", "find_in_document", "read_workflow",
                     "generate_docx", "edit_document"] {
            assert!(is_builtin(name), "{name} should be builtin");
        }
        assert!(!is_builtin("unknown_tool"));
        assert!(!is_builtin(""));
    }

    #[test]
    fn schemas_have_required_fields() {
        let s = schemas();
        assert_eq!(s.len(), 5);
        for sch in &s {
            assert_eq!(sch.kind, "function");
            assert!(!sch.function.name.is_empty());
            assert!(!sch.function.description.is_empty());
            assert_eq!(sch.function.parameters["type"], "object");
        }
        let names: Vec<&str> = s.iter().map(|t| t.function.name.as_str()).collect();
        assert!(names.contains(&"read_document"));
        assert!(names.contains(&"find_in_document"));
        assert!(names.contains(&"read_workflow"));
        assert!(names.contains(&"generate_docx"));
        assert!(names.contains(&"edit_document"));
    }

    #[test]
    fn schema_required_arrays_are_consistent() {
        let s = schemas();
        for sch in &s {
            let p = &sch.function.parameters;
            let required = p["required"].as_array().expect("required must be array");
            let props = p["properties"].as_object().expect("properties must be object");
            for r in required {
                let key = r.as_str().unwrap();
                assert!(props.contains_key(key), "{} requires {key} but property not declared", sch.function.name);
            }
        }
    }

    #[test]
    fn sanitize_filename_default_when_empty() {
        assert_eq!(sanitize_filename(""), "Untitled");
        assert_eq!(sanitize_filename("    "), "Untitled");
    }

    #[test]
    fn sanitize_filename_replaces_unsafe_chars() {
        let s = sanitize_filename("foo/bar:baz?\\<>|*\"");
        assert!(!s.contains('/'));
        assert!(!s.contains('\\'));
        assert!(!s.contains(':'));
        assert!(!s.contains('?'));
        assert!(!s.contains('*'));
        assert!(!s.contains('"'));
        assert!(!s.contains('<'));
        assert!(!s.contains('>'));
        assert!(!s.contains('|'));
    }

    #[test]
    fn sanitize_filename_truncates_to_60_chars() {
        let long = "a".repeat(120);
        let out = sanitize_filename(&long);
        // 60-char max via `take(60)`. The trim() at the end may yield ≤60.
        assert!(out.chars().count() <= 60);
    }

    #[test]
    fn sanitize_filename_keeps_safe_chars() {
        assert_eq!(sanitize_filename("Contract Draft 2025-Q1"), "Contract Draft 2025-Q1");
        assert_eq!(sanitize_filename("invoice_#42"), "invoice_#42".replace('#', "_"));
    }

    #[test]
    fn extract_text_handles_text_formats() {
        assert_eq!(extract_text("txt", "x.txt", b"hello"), "hello");
        assert_eq!(extract_text("md", "x.md", b"# title"), "# title");
        assert_eq!(extract_text("csv", "x.csv", b"a,b,c\n1,2,3"), "a,b,c\n1,2,3");
    }

    #[test]
    fn extract_text_unknown_format_returns_empty() {
        assert_eq!(extract_text("zip", "x.zip", b"PK\x03\x04"), "");
        assert_eq!(extract_text("", "x", b"data"), "");
    }
}
