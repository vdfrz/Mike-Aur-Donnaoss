//! Builtin tools that ship with Mike's legal-assistant identity.
//!
//! Mirror the OpenAI/Anthropic tool schemas declared by upstream Mike
//! (`backend/src/lib/chatTools.ts`):
//!
//! * `read_document` — fetch full text of a chat-attached document by `doc-N` label
//! * `find_in_document` — case-insensitive search within a document
//! * `read_workflow` — load the Markdown body of a saved workflow by id
//! * `draft_document` — persist a Markdown draft (no .docx until rendered)
//! * `render_word` — render a Markdown draft to a downloadable .docx
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
const DRAFT_DOCUMENT: &str = "draft_document";
const RENDER_WORD: &str = "render_word";
const EDIT_DOCUMENT: &str = "edit_document";
const VANGA_SEARCH: &str = "vanga_search";
const KANOON_SEARCH: &str = "kanoon_search";
const KANOON_GET_FRAGMENT: &str = "kanoon_get_fragment";
const KANOON_VERIFY_CASE: &str = "kanoon_verify_case";
const STATUTE_SEARCH: &str = "statute_search";
const SEARCH_FIRM_CORPUS: &str = "search_firm_corpus";
const EXPAND_CHUNK: &str = "expand_chunk";
const ASK_CLARIFYING: &str = "ask_clarifying_questions";

pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        READ_DOCUMENT
            | FIND_IN_DOCUMENT
            | READ_WORKFLOW
            | DRAFT_DOCUMENT
            | RENDER_WORD
            | EDIT_DOCUMENT
            | KANOON_SEARCH
            | KANOON_GET_FRAGMENT
            | KANOON_VERIFY_CASE
            | STATUTE_SEARCH
            | SEARCH_FIRM_CORPUS
            | EXPAND_CHUNK
    )
}

pub fn is_client_tool(name: &str) -> bool {
    name == VANGA_SEARCH || name == ASK_CLARIFYING
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
            "Read the full text content of a document attached by the user. Always call this before answering questions about, summarising, or citing from a document. When you are about to redline or edit a .docx, pass format:\"markdown\" to get a structured view (clause numbering, tables, and any existing tracked changes) instead of flat text.",
            json!({
                "type": "object",
                "properties": {
                    "doc_id": {
                        "type": "string",
                        "description": "The document ID to read (e.g. 'doc-0', 'doc-1')"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "markdown"],
                        "description": "Output format. 'text' (default) returns plain extracted text. 'markdown' returns a structured Markdown view of a .docx preserving clause numbers, tables, and tracked changes; use it when redlining or editing a Word document. Ignored for non-.docx files."
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
            DRAFT_DOCUMENT,
            "Draft a legal document as Markdown. This is the persistent working copy — it renders FORMATTED in the side panel; it does NOT produce a Word file (the user renders Word later via render_word). Pass `title` (file label) and `body` (full Markdown). To EDIT an existing draft, rewrite the FULL Markdown and pass the same `document_id` — it upserts the draft and keeps a version history. Returns the document id and filename.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Document title / base filename (no extension)." },
                    "body":  { "type": "string", "description": "Full document content in Markdown. Headings (#, ##, ###), bullet lists and bold/italic are honored." },
                    "document_id": { "type": "string", "description": "Optional. To re-draft / edit an existing draft, pass its document_id (from a prior draft_document result) and the full rewritten Markdown." }
                },
                "required": ["title", "body"]
            }),
        ),
        fun(
            RENDER_WORD,
            "Render an existing Markdown draft to a downloadable Word (.docx) file. Call this ONLY after the user confirms they want a Word file. Pass the `document_id` of a draft created by draft_document.",
            json!({
                "type": "object",
                "properties": {
                    "document_id": { "type": "string", "description": "The document_id of the draft to render to .docx." }
                },
                "required": ["document_id"]
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
            "Search Indian case law on Indian Kanoon. Use this whenever the user asks a question that depends on Indian statutes, court rulings, or legal precedent. Returns matching judgments (title, court, date, snippet, clickable URL) FAST — full paragraph text is NOT fetched here. For the 2-3 cases you will actually CITE, call kanoon_get_fragment(tid, query) to pull authoritative judgment paragraphs to quote. Prefer narrow, well-formed queries with field operators (court, fromdate, todate, doctypes, cites) over bare keywords. IMPORTANT — be economical so the user isn't left waiting: run ONE broad search that combines all the legal issues in the question into a single query (e.g. 'court martial bias proportionality non-speaking order defence witnesses'), NOT a separate search per issue. Call at most 3 times per turn, ideally once; if you can't find what you need, tell the user. Cite results as Markdown links: [Case Title](kanoon_url).",
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
                        "description": "Number of results to return (1-10, default 5).",
                        "minimum": 1,
                        "maximum": 10
                    },
                    "include_fragments": {
                        "type": "boolean",
                        "description": "Default FALSE — search returns fast (titles + snippets + URLs) and you pull paragraph text on demand with kanoon_get_fragment for the 2-3 cases you cite. Set true ONLY when you genuinely need query-relevant paragraphs for every result inline (slower: one extra fetch per result)."
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
        fun(
            STATUTE_SEARCH,
            "Search the local Indian statute database (BNS, IPC, CrPC, BNSS, Evidence Act, BSA). Use this for questions about which statutory section applies or how the 2023 codes renumber the old ones — distinct from kanoon_search, which is for case law. Returns two things: `results` (matching section text, ordered by relevance) and `mappings` (authoritative old↔new section correspondences, e.g. IPC s.420 → BNS s.318(4)). Mapping lookups work whenever the query mentions a section number, so this answers 'what replaced IPC 420' even when section text isn't available. Query by legal concept or section number (e.g. 'cheating', 'dishonour cheque', '420'); do NOT include act abbreviations like 'BNS' as search words. Cite results as '<statute> s.<section>'.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Legal concept or section number to search section titles and text. Examples: 'cheating', 'criminal breach of trust', 'anticipatory bail', '138'. Avoid act names and full natural-language questions."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Number of sections to return (1-15, default 8).",
                        "minimum": 1,
                        "maximum": 15
                    }
                },
                "required": ["query"]
            }),
        ),
        fun(
            SEARCH_FIRM_CORPUS,
            "Search the firm's own uploaded knowledge base — its past petitions, written statements, judgments, skeleton arguments and templates — for language and arguments to reuse. Call this BEFORE drafting grounds, prayers, clauses or arguments, and PREFER the firm's own settled phrasing over generic boilerplate. Search broad, then narrow with filters; expand the best hit with expand_chunk before quoting. Returns matching chunks with their source file and metadata. Use at most 4 searches per turn.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Legal concept / phrasing to find in the firm's past work, e.g. 'ground for condonation medical', 'prayer interim maintenance', 'security cheque defence'." },
                    "doc_type": { "type": "string", "description": "Optional filter: petition|written_statement|judgment|skeleton_argument|template|deed|notice|other" },
                    "case_type": { "type": "string", "description": "Optional filter: matrimonial|consumer|criminal|writ|service|ni_act|civil|other" },
                    "section_role": { "type": "string", "description": "Optional filter: ground|prayer|argument|clause|facts|verification" },
                    "max_results": { "type": "integer", "description": "Results to return (1-20, default 8).", "minimum": 1, "maximum": 20 }
                },
                "required": ["query"]
            }),
        ),
        fun(
            EXPAND_CHUNK,
            "Fetch the surrounding context of a chunk returned by search_firm_corpus — the neighbouring paragraphs from the same firm document — so you can read a passage in full before relying on it.",
            json!({
                "type": "object",
                "properties": {
                    "chunk_id": { "type": "integer", "description": "The chunk_id from a search_firm_corpus result." },
                    "before": { "type": "integer", "description": "Preceding chunks to include (0-5, default 2).", "minimum": 0, "maximum": 5 },
                    "after": { "type": "integer", "description": "Following chunks to include (0-5, default 2).", "minimum": 0, "maximum": 5 }
                },
                "required": ["chunk_id"]
            }),
        ),
        fun(
            ASK_CLARIFYING,
            "Ask the user 1-4 structured clarifying questions in a SINGLE call before drafting or searching case law. Fire this ONLY for a choice that changes the document's shape or strategy (the forum / type of proceeding; for a criminal complaint, the cognizance-vs-FIR track; the enabling provision or principal relief; the governing code when only the offence date is missing) AND only when two or more options are each genuinely viable and the answer cannot be inferred from context. If one option clearly dominates on the facts, do NOT ask — assume it, state it in one line, and proceed. NEVER ask for a missing fact (names, dates, amounts, addresses, case/FIR numbers, the exact section) — those are \"________\" placeholders, not questions. MAKE EACH QUESTION SMART: ground it in the SPECIFIC facts the user gave — name the detail the choice turns on, never boilerplate you could ask of any case — and make every option a genuinely distinct, viable path with a one-line why, recommended first. (Bad: \"What kind of complaint?\" with generic options. Good: \"The accused has reportedly absconded and ₹40L is untraced — which track?\" → \"Police/EOW complaint to register an FIR (best for tracing & arrest)\" vs \"Private complaint to the Magistrate\".) Each question has a short header, the question text, whether multiple options may be selected, and 2-5 options (put the recommended option first).",
            json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "1-4 clarifying questions to ask the user.",
                        "minItems": 1,
                        "maxItems": 4,
                        "items": {
                            "type": "object",
                            "properties": {
                                "header": { "type": "string", "description": "Very short label for the question (<=12 chars)." },
                                "question": { "type": "string", "description": "The question text. Ends with '?'." },
                                "multiSelect": { "type": "boolean", "description": "Whether multiple options may be selected." },
                                "options": {
                                    "type": "array",
                                    "description": "2-5 answer options. Put the recommended option first.",
                                    "minItems": 2,
                                    "maxItems": 5,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string", "description": "Short option label (1-5 words)." },
                                            "description": { "type": "string", "description": "Optional one-line explanation." }
                                        },
                                        "required": ["label"]
                                    }
                                }
                            },
                            "required": ["header", "question", "options"]
                        }
                    }
                },
                "required": ["questions"]
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
    chat_id: &str,
    doc_label_map: &HashMap<String, String>,
    case_id: Option<&str>,
    name: &str,
    arguments: &Value,
) -> String {
    // `case_id` is Some only inside a case-scoped chat. Consumed by the arms
    // that need case context (draft_document cross-ref resolution, corpus
    // tools); the no-op keeps it live until every such arm is wired in.
    let _ = case_id;
    match name {
        READ_DOCUMENT => exec_read_document(state, user_id, doc_label_map, arguments).await,
        FIND_IN_DOCUMENT => exec_find_in_document(state, user_id, doc_label_map, arguments).await,
        READ_WORKFLOW => exec_read_workflow(state, user_id, arguments).await,
        DRAFT_DOCUMENT => exec_draft_document(state, user_id, chat_id, doc_label_map, arguments).await,
        RENDER_WORD => exec_render_word(state, user_id, doc_label_map, arguments).await,
        EDIT_DOCUMENT => exec_edit_document(state, user_id, doc_label_map, arguments).await,
        KANOON_SEARCH => crate::llm::kanoon_tool::exec_kanoon_search(state, user_id, arguments).await,
        KANOON_GET_FRAGMENT => {
            crate::llm::kanoon_tool::exec_kanoon_get_fragment(state, user_id, arguments).await
        }
        KANOON_VERIFY_CASE => {
            crate::llm::kanoon_tool::exec_kanoon_verify_case(state, user_id, arguments).await
        }
        STATUTE_SEARCH => crate::llm::statute_tool::exec_statute_search(state, arguments).await,
        SEARCH_FIRM_CORPUS => {
            crate::corpus::tools::exec_search_firm_corpus(state, user_id, arguments).await
        }
        EXPAND_CHUNK => crate::corpus::tools::exec_expand_chunk(state, user_id, arguments).await,
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
    if let Some(row) = fetch_doc_row(state, user_id, &real_id).await {
        return Some(row);
    }
    // Models occasionally mistype one character of a UUID copied from an
    // earlier tool result, then conclude the document vanished. Accept a
    // unique near-miss (edit distance ≤ 2) among this user's documents —
    // for UUIDs a false positive is practically impossible.
    let ids: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM documents WHERE user_id = ? ORDER BY created_at DESC LIMIT 200",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    let close: Vec<&str> = ids
        .iter()
        .map(|(id,)| id.as_str())
        .filter(|id| edit_distance_at_most(id, &real_id, 2))
        .collect();
    if let [only] = close[..] {
        tracing::warn!("[tools] doc id near-miss accepted: {real_id} -> {only}");
        return fetch_doc_row(state, user_id, only).await;
    }
    None
}

async fn fetch_doc_row(
    state: &AppState,
    user_id: &str,
    doc_id: &str,
) -> Option<(String, String, Option<String>)> {
    sqlx::query_as(
        "SELECT filename, file_type, storage_path FROM documents WHERE id = ? AND user_id = ?",
    )
    .bind(doc_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
}

/// Whether the Levenshtein distance between `a` and `b` is ≤ `max`.
fn edit_distance_at_most(a: &str, b: &str, max: usize) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len().abs_diff(b.len()) > max {
        return false;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, &ca) in a.iter().enumerate() {
        let mut cur = Vec::with_capacity(b.len() + 1);
        cur.push(i + 1);
        let mut row_min = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let v = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
            row_min = row_min.min(v);
            cur.push(v);
        }
        if row_min > max {
            return false;
        }
        prev = cur;
    }
    prev[b.len()] <= max
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
    // Optional structured-Markdown view for the redline path. Only meaningful
    // for .docx; the default ("text") and every other file type use the
    // pure-Rust hot extractor unchanged, so behavior is byte-identical when
    // `format` is absent.
    let want_markdown = arguments.get("format").and_then(|v| v.as_str()) == Some("markdown");
    let text = if want_markdown && file_type == "docx" {
        docx_markdown_view(&bytes)
    } else {
        extract_text(&file_type, &filename, &bytes)
    };
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
        let snippet = context_snippet(&haystack_norm, ctx_lo, ctx_hi);
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

/// Slice a context window out of `haystack`, snapping the raw ±byte offsets
/// to char boundaries first. The ±60-byte context padding in
/// `exec_find_in_document` can land mid-codepoint on multibyte UTF-8
/// (Devanagari, the ₹ sign, en-dashes, curly quotes — all routine in Indian
/// legal text), which would panic a raw `&haystack[lo..hi]` slice. Walk `lo`
/// down and `hi` up to the nearest boundary so the slice is always valid.
fn context_snippet(haystack: &str, mut lo: usize, mut hi: usize) -> &str {
    lo = lo.min(haystack.len());
    hi = hi.min(haystack.len());
    while lo > 0 && !haystack.is_char_boundary(lo) {
        lo -= 1;
    }
    while hi < haystack.len() && !haystack.is_char_boundary(hi) {
        hi += 1;
    }
    if lo > hi {
        return "";
    }
    &haystack[lo..hi]
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

/// Draft-only: persist the Markdown working copy of a document. Does NOT render
/// or store a .docx — the user renders Word later via `render_word`. On re-draft
/// (an owned `document_id` is supplied) it upserts the same row and appends a new
/// markdown version snapshot so prior drafts stay recoverable.
async fn exec_draft_document(
    state: &AppState,
    user_id: &str,
    chat_id: &str,
    doc_label_map: &HashMap<String, String>,
    arguments: &Value,
) -> String {
    let raw_title = arguments.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled").trim().to_string();
    let raw_body = arguments.get("body").and_then(|v| v.as_str()).unwrap_or("");
    // Resolve a chat-local label ("doc-0") back to the real UUID; a raw UUID
    // (not in the map) passes through unchanged.
    let existing_id = arguments
        .get("document_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| doc_label_map.get(s).map(|u| u.as_str()).unwrap_or(s));
    if raw_body.is_empty() {
        return json!({"error": "body (Markdown) is required"}).to_string();
    }

    // Strip citation JSON that confused models sometimes embed in tool calls.
    let body = strip_citation_noise(raw_body);
    let title = clean_title(&raw_title, &body);
    let safe_title = sanitize_filename(&title);
    let filename = format!("{safe_title}.docx");
    let draft_warnings = validate_legal_draft(&body);

    // Resolve the target row: re-draft an owned doc, else create a fresh one.
    let doc_id = match existing_id {
        Some(id) => {
            let owns: Option<(String,)> = sqlx::query_as(
                "SELECT id FROM documents WHERE id = ? AND user_id = ?",
            )
            .bind(id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            if owns.is_none() {
                return json!({"error": format!("document {id} not found")}).to_string();
            }
            // Upsert: keep storage_path as-is (a previously-rendered .docx is now
            // stale, but render_word re-renders from markdown_source on demand).
            // `chat_id = COALESCE(chat_id, ?)` links older NULL-chat drafts to
            // this chat on their next edit so the cross-turn restore SELECT
            // (which keys on documents.chat_id) can find them.
            if let Err(e) = sqlx::query(
                "UPDATE documents SET filename = ?, markdown_source = ?, status = 'draft', \
                 chat_id = COALESCE(chat_id, ?) \
                 WHERE id = ? AND user_id = ?",
            )
            .bind(&filename)
            .bind(&body)
            .bind(chat_id)
            .bind(id)
            .bind(user_id)
            .execute(&state.db)
            .await
            {
                return json!({"error": format!("db: {e}")}).to_string();
            }
            id.to_string()
        }
        None => {
            let new_id = uuid::Uuid::new_v4().to_string();
            if let Err(e) = sqlx::query(
                "INSERT INTO documents (id, user_id, project_id, chat_id, filename, file_type, size_bytes, storage_path, status, markdown_source) \
                 VALUES (?, ?, NULL, ?, ?, 'docx', 0, NULL, 'draft', ?)",
            )
            .bind(&new_id)
            .bind(user_id)
            .bind(chat_id)
            .bind(&filename)
            .bind(&body)
            .execute(&state.db)
            .await
            {
                return json!({"error": format!("db: {e}")}).to_string();
            }
            new_id
        }
    };

    // Append-only markdown snapshot (version_no = max + 1).
    let next_version: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(version_no), 0) + 1 FROM document_markdown_versions WHERE document_id = ?",
    )
    .bind(&doc_id)
    .fetch_one(&state.db)
    .await
    .unwrap_or(1);
    if let Err(e) = sqlx::query(
        "INSERT INTO document_markdown_versions (id, document_id, version_no, markdown) VALUES (?, ?, ?, ?)",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(&doc_id)
    .bind(next_version)
    .bind(&body)
    .execute(&state.db)
    .await
    {
        return json!({"error": format!("version snapshot: {e}")}).to_string();
    }

    let mut result = json!({
        "doc_id": doc_id,
        "document_id": doc_id,
        "filename": filename,
        "note": "Markdown draft persisted (no Word file yet — it renders formatted in the side panel). To edit, call draft_document again with the SAME document_id and the full rewritten Markdown. To produce a Word file, call render_word with this document_id."
    });
    if !draft_warnings.is_empty() {
        result["warnings"] = json!(draft_warnings);
    }
    result.to_string()
}

/// Render an existing Markdown draft to a stored .docx via the shared core in
/// `routes::documents`. Returns `{document_id, filename, download_url}` so
/// chat.rs can emit a rendered `doc_created` card.
async fn exec_render_word(
    state: &AppState,
    user_id: &str,
    doc_label_map: &HashMap<String, String>,
    arguments: &Value,
) -> String {
    let raw_id = arguments.get("document_id").and_then(|v| v.as_str()).unwrap_or("");
    if raw_id.is_empty() {
        return json!({"error": "document_id is required"}).to_string();
    }
    // Resolve a chat-local label ("doc-0") back to the real UUID.
    let doc_id = doc_label_map.get(raw_id).map(|u| u.as_str()).unwrap_or(raw_id);
    match crate::routes::documents::render_document_to_docx(state, user_id, doc_id).await {
        Ok((filename, download_url)) => json!({
            "document_id": doc_id,
            "filename": filename,
            "download_url": download_url,
            "note": "Word document rendered and ready for download."
        })
        .to_string(),
        Err(msg) => json!({"error": msg}).to_string(),
    }
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
    // Versions live under their own prefix: nesting "<storage_path>/v/<id>"
    // breaks when storage_path is itself a file (ENOTDIR on local storage).
    let version_path = format!("versions/{real_id}/{version_id}");
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
    let mut annotations: Vec<Value> = Vec::with_capacity(result.changes.len());
    for (i, c) in result.changes.iter().enumerate() {
        let edit_id = uuid::Uuid::new_v4().to_string();
        let change_id = format!("change-{}", i);

        // Store each edit record inline (await + propagate) rather than in a
        // detached spawn: a fire-and-forget insert can race the next reload or
        // hit SQLITE_BUSY, leaving the version row present but the edit rows
        // missing so the reload's INNER JOIN drops the doc_edited card.
        let del = c.del_w_id.clone().unwrap_or_default();
        let ins = c.ins_w_id.clone().unwrap_or_default();
        if let Err(e) = sqlx::query(
            "INSERT OR IGNORE INTO document_edits \
             (id, document_id, version_id, change_id, del_w_id, ins_w_id, \
              deleted_text, inserted_text, status) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending')"
        )
            .bind(&edit_id)
            .bind(&real_id)
            .bind(&version_id)
            .bind(&change_id)
            .bind(&del)
            .bind(&ins)
            .bind(&c.deleted_text)
            .bind(&c.inserted_text)
            .execute(&state.db)
            .await
        {
            return json!({"error": format!("edit record write: {e}")}).to_string();
        }

        annotations.push(json!({
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
        }));
    }

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

/// Structured-Markdown view of a `.docx` for the redline path: pandoc when it
/// is available, otherwise the pure-Rust `extract_docx_text` with a one-line
/// note prepended so the model knows it is looking at the flat fallback, not
/// structured Markdown. Never errors — a missing or failing pandoc degrades to
/// plain text rather than failing the read_document tool.
fn docx_markdown_view(bytes: &[u8]) -> String {
    // One pandoc probe, not two: docx_to_markdown resolves the binary itself
    // and returns Err if pandoc is missing or the conversion fails. On any
    // error we degrade to the pure-Rust extractor, so a missing or broken
    // pandoc never fails the read_document tool.
    match crate::pdf::pandoc::docx_to_markdown(bytes) {
        Ok(md) => md,
        Err(e) => {
            tracing::warn!(
                "[read_document] structured markdown unavailable, plain-text fallback: {e}"
            );
            let plain = crate::pdf::extract_docx_text(bytes).unwrap_or_default();
            format!("(pandoc unavailable - plain-text view)\n{plain}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal `.docx` (a ZIP carrying only `word/document.xml`) — what the
    /// pure-Rust `extract_docx_text` reads. Used to exercise the markdown-view
    /// fallback without packaging a full Word document.
    fn minimal_docx(text: &str) -> Vec<u8> {
        use std::io::Write;
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zipw = zip::ZipWriter::new(&mut cursor);
            zipw.start_file("word/document.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
                 <w:body><w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p></w:body>\
                 </w:document>"
            );
            zipw.write_all(xml.as_bytes()).unwrap();
            zipw.finish().unwrap();
        }
        cursor.into_inner()
    }

    #[test]
    fn docx_markdown_view_falls_back_to_plain_text_without_pandoc() {
        // When pandoc is unavailable the markdown view must NOT error: it
        // returns the pure-Rust extraction with the "(pandoc unavailable ...)"
        // note prepended. Deterministic when pandoc is absent (this env / CI);
        // skips when pandoc is installed (the round-trip test covers that).
        if crate::pdf::pandoc::pandoc_available() {
            eprintln!("skipping fallback assertion: pandoc is present");
            return;
        }
        let docx = minimal_docx("Clause 1. The fee is 50000 rupees.");
        let view = docx_markdown_view(&docx);
        assert!(
            view.starts_with("(pandoc unavailable - plain-text view)"),
            "expected plain-text fallback note, got: {view}"
        );
        assert!(
            view.contains("Clause 1."),
            "fallback must include the pure-Rust extracted text, got: {view}"
        );
    }

    #[test]
    fn context_snippet_does_not_panic_on_multibyte_boundaries() {
        // Build a haystack where the ±60-byte window offsets land mid-codepoint.
        // ₹ (3 bytes), Devanagari धारा (12 bytes), em-dash — (3 bytes), curly
        // quotes are all multibyte and routine in Indian legal text.
        let haystack = "₹ धारा 138 — “notice” of dishonour परक्राम्य लिखत अधिनियम बैंक खाता संख्या";
        // Walk every possible (lo, hi) byte pair; none may panic, all must
        // return a valid substring of the haystack.
        for lo in 0..=haystack.len() {
            for hi in lo..=haystack.len() {
                let snip = context_snippet(haystack, lo, hi);
                assert!(haystack.contains(snip) || snip.is_empty());
            }
        }
    }

    #[test]
    fn context_snippet_returns_expected_window_on_ascii() {
        let haystack = "the quick brown fox";
        assert_eq!(context_snippet(haystack, 4, 9), "quick");
        // Out-of-range hi is clamped to len.
        assert_eq!(context_snippet(haystack, 16, 999), "fox");
    }

    #[test]
    fn is_builtin_recognises_each_tool() {
        for name in [
            "read_document",
            "find_in_document",
            "read_workflow",
            "draft_document",
            "render_word",
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
        assert!(is_client_tool("ask_clarifying_questions"));
        assert!(!is_client_tool("read_document"));
        assert!(!is_client_tool("kanoon_search"));
        assert!(!is_client_tool("kanoon_verify_case"));
    }

    #[test]
    fn schemas_have_required_fields() {
        let s = schemas();
        // 6 doc tools (read_document, find_in_document, read_workflow, draft_document, render_word, edit_document)
        // + vanga_search (client) + kanoon_search + kanoon_get_fragment + kanoon_verify_case + statute_search = 11,
        // + ask_clarifying_questions (client) = 12, + search_firm_corpus + expand_chunk = 14
        assert_eq!(s.len(), 14);
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
        assert!(names.contains(&"draft_document"));
        assert!(names.contains(&"render_word"));
        assert!(names.contains(&"edit_document"));
        assert!(names.contains(&"vanga_search"));
        assert!(names.contains(&"kanoon_search"));
        assert!(names.contains(&"kanoon_get_fragment"));
        assert!(names.contains(&"kanoon_verify_case"));
        assert!(names.contains(&"ask_clarifying_questions"));
        assert!(names.contains(&"search_firm_corpus"));
        assert!(names.contains(&"expand_chunk"));
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

    #[test]
    fn edit_distance_catches_uuid_typos() {
        // The real failure: one hex char flipped mid-UUID.
        assert!(edit_distance_at_most(
            "33c79981-a7da-461a-b8d9-835c4f600109",
            "33c79981-a7da-461b-b8d9-835c4f600109",
            2
        ));
        assert!(edit_distance_at_most("same", "same", 2));
        assert!(!edit_distance_at_most(
            "33c79981-a7da-461a-b8d9-835c4f600109",
            "b27eca62-7475-4941-ae08-6a8610bd2ea6",
            2
        ));
        // Length differs by more than the cap → early reject.
        assert!(!edit_distance_at_most("doc-1", "33c79981-a7da", 2));
    }
}
