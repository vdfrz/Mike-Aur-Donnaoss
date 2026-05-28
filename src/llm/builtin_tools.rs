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
const VANGA_SEARCH: &str = "vanga_search";
const KANOON_SEARCH: &str = "kanoon_search";
const KANOON_GET_FRAGMENT: &str = "kanoon_get_fragment";
const KANOON_VERIFY_CASE: &str = "kanoon_verify_case";

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        READ_DOCUMENT
            | FIND_IN_DOCUMENT
            | READ_WORKFLOW
            | GENERATE_DOCX
            | EDIT_DOCUMENT
            | KANOON_SEARCH
            | KANOON_GET_FRAGMENT
            | KANOON_VERIFY_CASE
    )
}

pub fn is_client_tool(name: &str) -> bool {
    name == VANGA_SEARCH
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
        fun(
            VANGA_SEARCH,
            "Search Indian High Court judgments by court, year range, and keyword. Queries metadata (case title, description, judge, court, date) from the public Vanga dataset. Returns matching cases with snippets. The search runs in the user's browser — results may take a few seconds on first use. IMPORTANT: Do NOT call this tool more than 2-3 times per user query. After 2 searches, present whatever results you have found to the user — do not keep refining the search endlessly. If results are not specific enough, summarize what you found and suggest the user refine their query.",
            json!({
                "type": "object",
                "properties": {
                    "court_code": {
                        "type": "string",
                        "description": "Court code, e.g. '7_26' for Delhi HC, '27_1' for Bombay HC. Omit for all courts."
                    },
                    "year_start": {
                        "type": "integer",
                        "description": "Start year (inclusive), e.g. 2020. Omit for no lower bound."
                    },
                    "year_end": {
                        "type": "integer",
                        "description": "End year (inclusive), e.g. 2024. Omit for no upper bound."
                    },
                    "query": {
                        "type": "string",
                        "description": "Keyword query to match against case title and description, e.g. 'Section 138 NI Act security cheque'"
                    }
                },
                "required": ["query"]
            }),
        ),
        fun(
            KANOON_SEARCH,
            "Search Indian case law on Indian Kanoon. Use this whenever the user asks a question that depends on Indian statutes, court rulings, or legal precedent. Returns matching judgments with the actual query-relevant paragraphs extracted from each case (not just a one-line headline), plus a clickable URL. Prefer narrow, well-formed queries with field operators (court, fromdate, todate, doctypes, cites) over bare keywords — these are dramatically more accurate than vague searches. Cite results as Markdown links: [Case Title](kanoon_url). If `relevant_paragraphs` is present, the text inside is authoritative judgment text — quote it directly when supporting a legal argument. IMPORTANT: call this tool with a focused query, not the user's entire question. Do not call more than 3 times per user turn; if you can't find what you need, tell the user.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search terms — legal concepts, statute references, doctrine names. Examples: 'section 138 NI Act dishonour security cheque', 'adverse possession against state', 'anticipatory bail proviso 438'. Avoid full natural-language questions."
                    },
                    "phrase": {
                        "type": "boolean",
                        "description": "If true, the query is wrapped in quotes for exact phrase matching. Use for multi-word doctrines like 'doctrine of merger' or 'colourable exercise of power'. Default false."
                    },
                    "court": {
                        "type": "string",
                        "description": "Filter by court. Common values: 'supremecourt', 'delhi', 'bombay', 'madras', 'calcutta', 'karnataka', 'kerala', 'punjab', 'allahabad', 'gujarat'. Omit for all courts."
                    },
                    "doctypes": {
                        "type": "string",
                        "description": "Document type filter. Common values: 'supremecourt', 'highcourts', 'laws' (for statutes), 'tribunals'. Omit for all types."
                    },
                    "fromdate": {
                        "type": "string",
                        "description": "Start date, format DD-MM-YYYY (e.g. '01-01-2020'). Omit for no lower bound."
                    },
                    "todate": {
                        "type": "string",
                        "description": "End date, format DD-MM-YYYY (e.g. '31-12-2024'). Omit for no upper bound."
                    },
                    "cites": {
                        "type": "integer",
                        "description": "Find cases that cite this Kanoon document ID (tid). Useful for finding cases that follow or distinguish a specific precedent."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Number of results to return (1-10, default 5). Each result includes extracted relevant paragraphs.",
                        "minimum": 1,
                        "maximum": 10
                    },
                    "include_fragments": {
                        "type": "boolean",
                        "description": "If true (default), fetch query-relevant paragraphs for each result so you can quote actual judgment text. Set false ONLY for fast title-level scans where you'll narrow down with a second call."
                    }
                },
                "required": ["query"]
            }),
        ),
        fun(
            KANOON_GET_FRAGMENT,
            "Fetch query-relevant paragraphs from a single Indian Kanoon judgment by its document ID (tid). Use this after kanoon_search when you want more detail on one specific case — for example, to extract paragraphs about a different sub-topic in the same judgment. Returns the matching paragraphs in plain text plus the canonical Kanoon URL.",
            json!({
                "type": "object",
                "properties": {
                    "tid": {
                        "type": "integer",
                        "description": "The Kanoon document ID (tid) from a previous kanoon_search result."
                    },
                    "query": {
                        "type": "string",
                        "description": "Topic or terms to find paragraphs about within this judgment."
                    }
                },
                "required": ["tid", "query"]
            }),
        ),
        fun(
            KANOON_VERIFY_CASE,
            "Cross-check a single Indian Kanoon case against the canonical AWS indian-high-court-judgments dataset. Call this for each case you are going to actually cite in your final answer (not every search result — only the 2-3 you commit to). Takes ~3-5 seconds: looks up the case by court + decision_date in the AWS metadata parquet, fuzzy-matches the title, and returns the canonical court PDF URL when found. Returns status VERIFIED (with canonical_pdf_url), NOT_IN_AWS (case isn't in the AWS corpus — common for tribunals or very recent rulings), or UNVERIFIED (court not in mapping, network issue, or fuzzy-match failed). Always cite the case regardless of status, but add a brief '(unverified)' caveat in your prose for non-VERIFIED results.",
            json!({
                "type": "object",
                "properties": {
                    "tid": {
                        "type": "integer",
                        "description": "The Kanoon document ID from a previous kanoon_search result. Optional but recommended for traceability."
                    },
                    "title": {
                        "type": "string",
                        "description": "The case title exactly as returned by kanoon_search (e.g. 'Sripati Singh vs The State of Jharkhand'). Used for fuzzy matching against the AWS parquet."
                    },
                    "court": {
                        "type": "string",
                        "description": "The court string exactly as returned by kanoon_search (e.g. 'Supreme Court of India', 'Delhi High Court')."
                    },
                    "decision_date": {
                        "type": "string",
                        "description": "Decision date in any common format (DD-MM-YYYY, YYYY-MM-DD, or 'April 12, 2023'). The verifier extracts the 4-digit year from it. Required to determine the AWS partition."
                    }
                },
                "required": ["title", "court"]
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
        KANOON_SEARCH => crate::llm::kanoon_tool::exec_kanoon_search(state, user_id, arguments).await,
        KANOON_GET_FRAGMENT => {
            crate::llm::kanoon_tool::exec_kanoon_get_fragment(state, user_id, arguments).await
        }
        KANOON_VERIFY_CASE => {
            crate::llm::kanoon_tool::exec_kanoon_verify_case(state, user_id, arguments).await
        }
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

/// Placeholder for RAG-aware name scrubbing.
///
/// TODO: Once RAG chunk metadata is passed into tool execution, scan `body`
/// for real names that appear in the source chunks but were NOT part of the
/// user query.  Replace them with `[Name]` or `[Party]` placeholders so that
/// generated legal drafts don't accidentally leak identities pulled from
/// retrieval context.  For now this is a no-op passthrough.
#[allow(unused_variables)]
fn scrub_rag_names(body: &str, _user_query: &str) -> String {
    body.to_string()
}

/// Structural validation for legal drafts.  Returns a list of human-readable
/// warnings — these are advisory and must NOT block document generation.
fn validate_legal_draft(body: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let lower = body.to_lowercase();

    // Check for verification clause (required for affidavits)
    let is_affidavit = lower.contains("affidavit") || lower.contains("solemnly affirm");
    if is_affidavit && !lower.contains("verified at") && !lower.contains("verification") {
        warnings.push(
            "Missing verification clause — affidavits require a 'Verified at [City]' block."
                .to_string(),
        );
    }

    // Check for court header (required for court filings)
    let court_indicators = ["petition", "application", "complaint", "reply"];
    let is_court_filing = court_indicators.iter().any(|w| lower.contains(w));
    if is_court_filing && !lower.contains("in the court of") && !lower.contains("hon'ble") {
        warnings.push(
            "Missing court header — filings should begin with 'IN THE COURT OF'.".to_string(),
        );
    }

    // Check for unfilled brackets (model failed to fill placeholders)
    let bracket_count = body.matches('[').count();
    let expected_brackets =
        body.matches("[Page").count() + body.matches("[removed").count();
    let suspicious_brackets = bracket_count.saturating_sub(expected_brackets);
    if suspicious_brackets > 3 {
        warnings.push(format!(
            "{suspicious_brackets} unfilled [bracket] placeholders detected — review before use."
        ));
    }

    warnings
}

async fn exec_generate_docx(state: &AppState, user_id: &str, arguments: &Value) -> String {
    let raw_title = arguments.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled").trim().to_string();
    let raw_body = arguments.get("body").and_then(|v| v.as_str()).unwrap_or("");
    if raw_body.is_empty() {
        return json!({"error": "body (Markdown) is required"}).to_string();
    }

    // Strip citation JSON that confused models sometimes embed in tool calls.
    let body = strip_citation_noise(raw_body);
    let title = clean_title(&raw_title, &body);

    let draft_warnings = validate_legal_draft(&body);
    let bytes = match crate::pdf::docx_writer::markdown_to_docx(&title, &body) {
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

    let mut result = json!({
        "doc_id": doc_id,
        "filename": filename,
        "size_bytes": size,
        "note": "Document persisted as a standalone document. Call read_document with this doc_id to verify content before describing it to the user."
    });
    if !draft_warnings.is_empty() {
        result["warnings"] = json!(draft_warnings);
    }
    result.to_string()
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
            let format = e.get("format").and_then(|v| v.as_str()).map(|s| s.to_string());
            Some(crate::pdf::docx_writer::DocxEdit { find, replace, format })
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

    // Use tracked changes so the frontend can show redlines with accept/deny
    let result = match crate::pdf::docx_writer::apply_tracked_edits(&bytes, &edits) {
        Ok(x) => x,
        Err(e) => return json!({"error": format!("docx edit: {e}")}).to_string(),
    };

    // Store the edited version
    let real_id = doc_label_map
        .get(label)
        .cloned()
        .unwrap_or_else(|| label.to_string());

    let version_id = uuid::Uuid::new_v4().to_string();
    let version_path = format!("{}/v/{}", storage_path.trim_end_matches("/docx"), &version_id);
    if let Err(e) = storage
        .put(
            &version_path,
            &result.bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await
    {
        return json!({"error": format!("storage write: {e}")}).to_string();
    }

    // Update the main document to point to the new version
    let new_size = result.bytes.len() as i64;
    let _ = sqlx::query(
        "UPDATE documents SET size_bytes = ?, storage_path = ? WHERE id = ? AND user_id = ?"
    )
        .bind(new_size)
        .bind(&version_path)
        .bind(&real_id)
        .bind(user_id)
        .execute(&state.db)
        .await;

    // Insert version record
    let next_version: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(version_number), 0) + 1 FROM document_versions WHERE document_id = ?"
    )
        .bind(&real_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(1);

    let _ = sqlx::query(
        "INSERT INTO document_versions (id, document_id, version_number, storage_path) VALUES (?, ?, ?, ?)"
    )
        .bind(&version_id)
        .bind(&real_id)
        .bind(next_version)
        .bind(&version_path)
        .execute(&state.db)
        .await;

    // Build annotations for the frontend
    let annotations: Vec<Value> = result.changes.iter().enumerate().map(|(i, c)| {
        let edit_id = uuid::Uuid::new_v4().to_string();
        let change_id = format!("change-{}", i);

        // Store each edit record
        let db = state.db.clone();
        let edit_id_c = edit_id.clone();
        let real_id_c = real_id.clone();
        let version_id_c = version_id.clone();
        let change_id_c = change_id.clone();
        let del = c.del_w_id.clone().unwrap_or_default();
        let ins = c.ins_w_id.clone().unwrap_or_default();
        let del_text = c.deleted_text.clone();
        let ins_text = c.inserted_text.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO document_edits \
                 (id, document_id, version_id, change_id, del_w_id, ins_w_id, \
                  deleted_text, inserted_text, status) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending')"
            )
                .bind(&edit_id_c)
                .bind(&real_id_c)
                .bind(&version_id_c)
                .bind(&change_id_c)
                .bind(&del)
                .bind(&ins)
                .bind(&del_text)
                .bind(&ins_text)
                .execute(&db)
                .await;
        });

        json!({
            "type": "edit_data",
            "kind": "edit",
            "edit_id": edit_id,
            "document_id": real_id,
            "version_id": version_id,
            "version_number": next_version,
            "change_id": change_id,
            "del_w_id": c.del_w_id,
            "ins_w_id": c.ins_w_id,
            "deleted_text": c.deleted_text,
            "inserted_text": c.inserted_text,
            "status": "pending",
        })
    }).collect();

    let summary: Vec<Value> = edits
        .iter()
        .zip(result.changes.iter())
        .map(|(e, _c)| json!({"find": e.find, "replace": e.replace, "hits": 1}))
        .collect();
    json!({
        "doc_id": label,
        "filename": filename,
        "version_id": version_id,
        "version_number": next_version,
        "edits_applied": summary,
        "annotations": annotations,
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

/// Remove citation JSON blocks that models sometimes embed in document body.
/// Strips lines like `{"ref": N, "doc_id": "doc-0", ...}` and `<CITATIONS>...</CITATIONS>`.
fn strip_citation_noise(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_citations_block = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("<citations>") || trimmed.eq_ignore_ascii_case("<citations>") {
            in_citations_block = true;
            continue;
        }
        if trimmed.eq_ignore_ascii_case("</citations>") {
            in_citations_block = false;
            continue;
        }
        if in_citations_block { continue; }
        // Skip lines that are citation ref objects: {"ref": N, "doc_id": ...}
        if trimmed.starts_with("{\"ref\"") && trimmed.contains("doc_id") {
            continue;
        }
        // Also skip lines that are just `[` or `]` (citation array delimiters)
        if trimmed == "[" || trimmed == "]" {
            // Only skip if surrounded by citation lines — but being conservative
            // and keeping them is fine; they're harmless in markdown.
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// If the title looks like JSON garbage (e.g. model passed a citation ref as title),
/// extract a real title from the first heading in the body instead.
fn clean_title(raw_title: &str, body: &str) -> String {
    let t = raw_title.trim();
    // If title contains "ref" + "doc_id" or is mostly non-alphanumeric, it's garbage
    let is_garbage = (t.contains("ref") && t.contains("doc_id"))
        || t.starts_with('{')
        || t.starts_with('[')
        || {
            let alpha_count = t.chars().filter(|c| c.is_alphanumeric() || *c == ' ').count();
            t.len() > 5 && alpha_count < t.len() / 2
        };
    if !is_garbage { return t.to_string(); }

    // Try to pull title from first markdown heading
    for line in body.lines() {
        let trimmed = line.trim().trim_start_matches('#').trim();
        if !trimmed.is_empty() && trimmed.len() > 2 {
            return trimmed.chars().take(60).collect();
        }
    }
    "Legal Document".to_string()
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
        for name in [
            "read_document",
            "find_in_document",
            "read_workflow",
            "generate_docx",
            "edit_document",
            "kanoon_search",
            "kanoon_get_fragment",
            "kanoon_verify_case",
        ] {
            assert!(is_builtin(name), "{name} should be builtin");
        }
        assert!(!is_builtin("unknown_tool"));
        assert!(!is_builtin(""));
        // vanga_search is a client tool, not a builtin.
        assert!(!is_builtin("vanga_search"));
        assert!(is_client_tool("vanga_search"));
        assert!(!is_client_tool("read_document"));
        assert!(!is_client_tool("kanoon_search"));
        assert!(!is_client_tool("kanoon_verify_case"));
    }

    #[test]
    fn schemas_have_required_fields() {
        let s = schemas();
        // 5 doc tools + vanga_search (client) + kanoon_search + kanoon_get_fragment + kanoon_verify_case = 9
        assert_eq!(s.len(), 9);
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
        assert!(names.contains(&"vanga_search"));
        assert!(names.contains(&"kanoon_search"));
        assert!(names.contains(&"kanoon_get_fragment"));
        assert!(names.contains(&"kanoon_verify_case"));
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
