use anyhow::{Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use std::collections::HashMap;
use tokio::sync::mpsc;

use crate::llm::summarize::{context_window_tokens, estimate_tokens};
use crate::llm::{Message, StreamEvent, StreamParams};
use crate::storage::make_storage;

/// Progress events streamed back from the orchestrator while agents run.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    ExtractingDoc { filename: String, doc_index: usize, total_docs: usize },
    ExtractedDoc { filename: String, doc_index: usize, total_docs: usize, page_count: usize, needed_ocr: bool },
    Compressing { original_tokens: usize, target_tokens: usize },
    /// Compression failed; context was hard-truncated to the budget instead.
    /// Surfaced so the user knows document content was dropped, not condensed.
    Truncated { original_tokens: usize, target_tokens: usize, error: String },
    Estimate { total_pages: usize, estimated_seconds: u32, has_ocr: bool },
    AgentStarted { agent_name: &'static str },
    AgentThinking { agent_name: &'static str, snippet: String },
    AgentDone { finding: Finding },
    AgentError { agent_name: &'static str, error: String },
}

use super::{
    CASE_SUMMARY_AGENT, EVIDENCE_GAP_AGENT, OPPOSITION_PREDICTOR, PRECEDENT_FINDER,
    RISK_ASSESSOR, STRATEGY_RECOMMENDER, STRENGTHS_WEAKNESSES_AGENT,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub case_id: String,
    pub agent_name: String,
    pub finding_type: String,
    pub content_json: String,
    pub grounding_json: Option<String>,
    pub created_at: String,
}

const AGENTS: &[(&str, &str, &str)] = &[
    ("case_summary", "summary", CASE_SUMMARY_AGENT),
    ("strengths_weaknesses", "swot", STRENGTHS_WEAKNESSES_AGENT),
    ("evidence_gap", "evidence", EVIDENCE_GAP_AGENT),
    ("opposition_predictor", "opposition", OPPOSITION_PREDICTOR),
    ("strategy_recommender", "strategy", STRATEGY_RECOMMENDER),
    ("precedent_finder", "precedent", PRECEDENT_FINDER),
    ("risk_assessor", "risk", RISK_ASSESSOR),
];

const OUTPUT_RESERVE_TOKENS: usize = 8_000;

/// Chars-per-token heuristic, matching `summarize::estimate_tokens` (4 chars/token).
/// Used to convert a token budget to a char cap for the truncation fallback.
const CHARS_PER_TOKEN: usize = 4;

pub async fn analyze_case(
    case_id: &str,
    user_id: &str,
    db: &SqlitePool,
    llm_params: StreamParams,
    redact_pii: bool,
    progress_tx: Option<mpsc::Sender<ProgressEvent>>,
) -> Result<Vec<Finding>> {
    // 1. Load case documents from DB (include size_bytes + page_count for early estimate)
    let docs: Vec<(String, String, String, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT d.id, d.filename, d.file_type, d.storage_path, d.size_bytes, d.page_count \
         FROM case_documents cd \
         JOIN documents d ON d.id = cd.document_id \
         WHERE cd.case_id = ? AND d.storage_path IS NOT NULL",
    )
    .bind(case_id)
    .fetch_all(db)
    .await
    .context("loading case documents")?;

    if docs.is_empty() {
        anyhow::bail!("no documents attached to case {case_id}");
    }

    // Emit an early estimate BEFORE extraction starts, based on file sizes.
    // ~1 page per 50KB for PDFs is a rough heuristic when page_count is unknown.
    if let Some(tx) = &progress_tx {
        let has_pdf = docs.iter().any(|(_, _, ft, _, _, _)| ft == "pdf");
        let est_pages: usize = docs.iter().map(|(_, _, ft, _, sz, pc)| {
            if let Some(p) = pc { return *p as usize; }
            let bytes = sz.unwrap_or(0) as usize;
            if ft == "pdf" { bytes / 50_000 } else { bytes / (300 * 5) } // ~300 words/page, ~5 bytes/word
        }).sum();
        let estimated = estimate_analysis_seconds(est_pages.max(1), has_pdf);
        let _ = tx.send(ProgressEvent::Estimate {
            total_pages: est_pages,
            estimated_seconds: estimated,
            has_ocr: has_pdf,
        }).await;
    }

    // 2. Extract text from each document
    let storage = make_storage()?;
    let mut doc_texts: Vec<(String, String)> = Vec::with_capacity(docs.len());
    let mut total_pages: usize = 0;
    let mut any_ocr = false;
    for (idx, (_doc_id, filename, file_type, storage_path, _size, _pc)) in docs.iter().enumerate() {
        if let Some(tx) = &progress_tx {
            let _ = tx.send(ProgressEvent::ExtractingDoc {
                filename: filename.clone(),
                doc_index: idx,
                total_docs: docs.len(),
            }).await;
        }
        let bytes = storage
            .get(storage_path)
            .await
            .with_context(|| format!("reading {filename} from storage"))?;
        let (raw_text, page_count, needed_ocr) = extract_text_with_meta(file_type, &bytes);
        total_pages += page_count;
        if needed_ocr { any_ocr = true; }
        if let Some(tx) = &progress_tx {
            let _ = tx.send(ProgressEvent::ExtractedDoc {
                filename: filename.clone(),
                doc_index: idx,
                total_docs: docs.len(),
                page_count,
                needed_ocr,
            }).await;
        }
        if !raw_text.is_empty() {
            let text = if redact_pii {
                crate::pii::scrub_pii(&raw_text).scrubbed_text
            } else {
                raw_text
            };
            doc_texts.push((filename.clone(), text));
        }
    }

    // Emit a refined estimate now that we know actual page counts + OCR status
    if let Some(tx) = &progress_tx {
        let estimated_seconds = estimate_analysis_seconds(total_pages, any_ocr);
        let _ = tx.send(ProgressEvent::Estimate {
            total_pages,
            estimated_seconds,
            has_ocr: any_ocr,
        }).await;
    }

    // 3. Build case context with document labels
    let mut case_context = String::new();
    for (i, (filename, text)) in doc_texts.iter().enumerate() {
        case_context.push_str(&format!("[doc-{i}: {filename}]\n{text}\n\n"));
    }

    // Original (uncompressed) per-doc text, keyed by doc-N, for quote verification.
    let doc_text_map: HashMap<String, String> = doc_texts
        .iter()
        .enumerate()
        .map(|(i, (_, text))| (format!("doc-{i}"), text.clone()))
        .collect();

    // 4. Token budget — compress if documents exceed 75% of context window
    let window = context_window_tokens(&llm_params.model);
    let budget = (window as f64 * 0.75) as usize;
    let context_tokens = estimate_tokens(&case_context);

    let final_context = if context_tokens + OUTPUT_RESERVE_TOKENS > budget {
        tracing::info!(
            "[case_prep] context ({context_tokens} tok) exceeds budget ({budget}), compressing"
        );
        let target = budget.saturating_sub(OUTPUT_RESERVE_TOKENS);
        if let Some(tx) = &progress_tx {
            let _ = tx.send(ProgressEvent::Compressing {
                original_tokens: context_tokens,
                target_tokens: target,
            }).await;
        }
        match compress_context(&case_context, target, &llm_params).await {
            Ok(compressed) => compressed,
            Err(e) => {
                // Compression failed — do NOT fan the over-budget context out to
                // the 7 agents (each would 400 or be provider-truncated, silently
                // dropping the decisive tail). Hard-truncate to the budget and make
                // the dropped content visible to the user.
                tracing::warn!(
                    "[case_prep] compression failed ({e}); hard-truncating context \
                     from {context_tokens} tok to ~{target} tok"
                );
                if let Some(tx) = &progress_tx {
                    let _ = tx.send(ProgressEvent::Truncated {
                        original_tokens: context_tokens,
                        target_tokens: target,
                        error: e.to_string(),
                    }).await;
                }
                hard_truncate_to_tokens(&case_context, target)
            }
        }
    } else {
        case_context
    };

    // 5. Fetch structured preferences + tone for injection
    let effective_prefs = crate::preferences::load_effective_preferences(
        db, user_id, Some(case_id), crate::preferences::PreferenceContext::CasePrep,
    ).await;
    let prefs_prompt = crate::preferences::format_preferences_prompt(&effective_prefs);
    let tone = crate::routes::chat::TONE_RULES.trim();

    // 6. Spawn all 7 agents in parallel
    let mut handles = Vec::with_capacity(AGENTS.len());
    for &(agent_name, finding_type, system_prompt) in AGENTS {
        let ctx = final_context.clone();
        let cid = case_id.to_string();
        let pool = db.clone();
        let doc_map = doc_text_map.clone();
        let tx = progress_tx.clone();
        let mut full_prompt = format!("{}\n\n{}", super::INDIAN_LEGAL_CONTEXT, system_prompt);
        // Layer the litigation-risk rubric onto the risk + strategy agents so they
        // screen against the same HIGH/MED/LOW + side taxonomy as the chat
        // assistant. Re-assert the JSON-only contract so the prose rubric never
        // tips the model into emitting a triage table instead of the schema.
        if matches!(agent_name, "risk_assessor" | "strategy_recommender") {
            full_prompt = format!(
                "{full_prompt}\n\n---\n\n{}\n\nUSE THE RUBRIC ABOVE to broaden and prioritise what you flag, and record each item's severity (HIGH/MED/LOW) and the side it helps vs hurts inside the JSON fields (e.g. begin the description with \"[HIGH | hurts petitioner] ...\"). Still output ONLY the JSON object specified earlier — no prose, no tables, no markdown fences.",
                super::LITIGATION_RISK_RUBRIC_BLOCK
            );
        }
        if !prefs_prompt.is_empty() {
            full_prompt = format!("{full_prompt}\n\n---\n\n{prefs_prompt}");
        }
        full_prompt = format!("{full_prompt}\n\n---\n\n{tone}");
        let params = StreamParams {
            model: llm_params.model.clone(),
            system_prompt: full_prompt,
            system_volatile: String::new(),
            messages: vec![Message::user(format!("{ctx}\n\nAnalyze this case."))],
            tools: vec![],
            max_iterations: 1,
            enable_thinking: false,
            local_config: llm_params.local_config.clone(),
            claude_api_key: llm_params.claude_api_key.clone(),
            gemini_api_key: llm_params.gemini_api_key.clone(),
            gemini_region: llm_params.gemini_region.clone(),
        };

        if let Some(tx) = &tx {
            let _ = tx.send(ProgressEvent::AgentStarted { agent_name }).await;
        }

        handles.push(tokio::spawn(async move {
            let finding = run_agent(&cid, agent_name, finding_type, params, &doc_map, &pool, tx.clone()).await;
            if let Some(tx) = &tx {
                let _ = tx.send(ProgressEvent::AgentDone { finding: finding.clone() }).await;
            }
            finding
        }));
    }

    // 6. Collect results — failed agents still produce error findings
    let results = futures_util::future::join_all(handles).await;
    let mut findings = Vec::with_capacity(AGENTS.len());
    for result in results {
        match result {
            Ok(finding) => findings.push(finding),
            Err(e) => tracing::error!("[case_prep] agent task panicked: {e}"),
        }
    }

    Ok(findings)
}

async fn run_agent(
    case_id: &str,
    agent_name: &'static str,
    finding_type: &str,
    params: StreamParams,
    doc_texts: &HashMap<String, String>,
    db: &SqlitePool,
    progress_tx: Option<mpsc::Sender<ProgressEvent>>,
) -> Finding {
    let now = chrono::Utc::now().to_rfc3339();
    let id = uuid::Uuid::new_v4().to_string();

    let (content_json, grounding_json) = match call_llm(params, agent_name, progress_tx.clone()).await {
        Ok(raw) => {
            let parsed = parse_json_response(&raw);
            let grounding = validate_grounding(&parsed, doc_texts);
            (parsed.to_string(), Some(grounding.to_string()))
        }
        Err(e) => {
            tracing::warn!("[case_prep] agent {agent_name} failed: {e}");
            if let Some(tx) = &progress_tx {
                let _ = tx.send(ProgressEvent::AgentError {
                    agent_name,
                    error: e.to_string(),
                }).await;
            }
            (json!({"error": e.to_string()}).to_string(), None)
        }
    };

    let finding = Finding {
        id: id.clone(),
        case_id: case_id.to_string(),
        agent_name: agent_name.to_string(),
        finding_type: finding_type.to_string(),
        content_json: content_json.clone(),
        grounding_json: grounding_json.clone(),
        created_at: now.clone(),
    };

    if let Err(e) = sqlx::query(
        "INSERT INTO case_findings (id, case_id, agent_name, finding_type, \
         content_json, grounding_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(case_id)
    .bind(agent_name)
    .bind(finding_type)
    .bind(&content_json)
    .bind(&grounding_json)
    .bind(&now)
    .execute(db)
    .await
    {
        tracing::error!("[case_prep] DB insert failed for {agent_name}: {e}");
    }

    finding
}

async fn call_llm(
    params: StreamParams,
    agent_name: &'static str,
    progress_tx: Option<mpsc::Sender<ProgressEvent>>,
) -> Result<String> {
    tokio::time::timeout(std::time::Duration::from_secs(180), async {
        let mut stream = crate::llm::stream_chat(params).await?;
        let mut response = String::new();
        let mut reasoning = String::new();
        let mut last_snippet_at = std::time::Instant::now();
        let mut snippet_buf = String::new();
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::ContentDelta(delta) => {
                    response.push_str(&delta);
                    snippet_buf.push_str(&delta);
                }
                StreamEvent::ReasoningDelta(delta) => {
                    reasoning.push_str(&delta);
                    snippet_buf.push_str(&delta);
                }
                StreamEvent::Done => break,
                _ => {}
            }
            // Flush a thinking snippet every ~1s so the UI shows activity.
            if let Some(tx) = &progress_tx {
                if last_snippet_at.elapsed() >= std::time::Duration::from_millis(1000)
                    && !snippet_buf.trim().is_empty()
                {
                    let snippet = std::mem::take(&mut snippet_buf);
                    let _ = tx.send(ProgressEvent::AgentThinking {
                        agent_name,
                        snippet,
                    }).await;
                    last_snippet_at = std::time::Instant::now();
                }
            }
        }
        if response.is_empty() && !reasoning.is_empty() {
            response = reasoning;
        }
        if response.is_empty() {
            anyhow::bail!("empty response from LLM");
        }
        Ok(response)
    })
    .await
    .map_err(|_| anyhow::anyhow!("LLM call timed out after 180s"))?
}

fn parse_json_response(raw: &str) -> Value {
    let trimmed = raw.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v;
    }

    let stripped = strip_markdown_fences(trimmed);
    if let Ok(v) = serde_json::from_str::<Value>(&stripped) {
        return v;
    }

    if let (Some(start), Some(end)) = (stripped.find('{'), stripped.rfind('}')) {
        if start < end {
            if let Ok(v) = serde_json::from_str::<Value>(&stripped[start..=end]) {
                return v;
            }
        }
    }

    json!({"raw_text": raw})
}

fn strip_markdown_fences(s: &str) -> String {
    let mut out = s;
    if out.starts_with("```") {
        if let Some(nl) = out.find('\n') {
            out = &out[nl + 1..];
        }
    }
    if let Some(rest) = out.strip_suffix("```") {
        out = rest;
    }
    out.trim().to_string()
}

const QUOTE_MATCH_THRESHOLD: f64 = 0.95;

fn validate_grounding(parsed: &Value, doc_texts: &HashMap<String, String>) -> Value {
    let mut refs = Vec::new();
    collect_quote_refs(parsed, &mut refs);

    let mut verified = 0usize;
    let mut unverified = Vec::new();
    for (doc_id, quote) in &refs {
        match doc_texts.get(doc_id) {
            Some(text) if verify_quote(quote, text) >= QUOTE_MATCH_THRESHOLD => verified += 1,
            Some(_) => unverified.push(json!({
                "doc_id": doc_id, "quote": quote, "reason": "quote_not_in_doc",
            })),
            None => unverified.push(json!({
                "doc_id": doc_id, "quote": quote, "reason": "invalid_doc_id",
            })),
        }
    }

    json!({
        "total_references": refs.len(),
        "verified": verified,
        "unverified": unverified,
    })
}

/// Collect (source_doc_id, quote) pairs from agent JSON. Handles both field
/// conventions: {source_doc_id, exact_quote} and {supporting_doc, supporting_text}.
fn collect_quote_refs(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let (Some(Value::String(id)), Some(Value::String(q))) =
                (map.get("source_doc_id"), map.get("exact_quote"))
            {
                out.push((id.clone(), q.clone()));
            }
            if let (Some(Value::String(id)), Some(Value::String(q))) =
                (map.get("supporting_doc"), map.get("supporting_text"))
            {
                out.push((id.clone(), q.clone()));
            }
            for v in map.values() {
                collect_quote_refs(v, out);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_quote_refs(v, out);
            }
        }
        _ => {}
    }
}

/// Normalize for OCR-tolerant comparison: lowercase, ASCII-ize smart quotes,
/// collapse whitespace (also flattens injected "[Page N]" line breaks), and fold
/// the classic "rn"->"m" OCR misread mid-word. The fold is applied symmetrically
/// to both quote and source, so legitimate text matches unchanged while a quote
/// the model silently "corrected" still aligns with its garbled source.
fn normalize_for_match(s: &str) -> String {
    let s = s
        .replace(['\u{2018}', '\u{2019}'], "'")
        .replace(['\u{201C}', '\u{201D}'], "\"");
    let mut lowered = String::with_capacity(s.len());
    let mut last_was_ws = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_was_ws {
                lowered.push(' ');
                last_was_ws = true;
            }
        } else {
            lowered.extend(ch.to_lowercase());
            last_was_ws = false;
        }
    }

    let chars: Vec<char> = lowered.trim().chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut prev: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == 'r'
            && chars.get(i + 1) == Some(&'n')
            && prev.is_some_and(|p| p.is_alphanumeric())
        {
            out.push('m');
            prev = Some('m');
            i += 2;
            continue;
        }
        out.push(c);
        prev = Some(c);
        i += 1;
    }
    out
}

/// Verify a quote against its source doc: exact substring first (free), then a
/// sliding fuzzy match for OCR garbling. Returns match confidence (0.0 = miss).
fn verify_quote(quote: &str, doc_text: &str) -> f64 {
    let q = normalize_for_match(quote);
    if q.chars().count() < 8 {
        return 0.0; // too short to verify meaningfully
    }
    let doc = normalize_for_match(doc_text);
    if doc.contains(&q) {
        return 1.0;
    }
    // Fuzzy fallback for OCR garbling. Slide a word-aligned window (word count =
    // quote's) across the doc: OCR corrupts a word's characters but rarely its
    // boundaries, so word alignment keeps the levenshtein comparison meaningful.
    let qwords: Vec<&str> = q.split(' ').collect();
    let dwords: Vec<&str> = doc.split(' ').collect();
    if dwords.len() < qwords.len() {
        return strsim::normalized_levenshtein(&q, &doc);
    }
    let n = qwords.len();
    let mut best = 0.0_f64;
    for start in 0..=(dwords.len() - n) {
        let window = dwords[start..start + n].join(" ");
        let ratio = strsim::normalized_levenshtein(&q, &window);
        if ratio > best {
            best = ratio;
            if best >= 0.99 {
                break;
            }
        }
    }
    best
}

/// Returns (text, page_count, needed_ocr) for richer progress reporting.
fn extract_text_with_meta(file_type: &str, bytes: &[u8]) -> (String, usize, bool) {
    match file_type {
        "pdf" => {
            #[cfg(feature = "pdf")]
            {
                let tmp = std::env::temp_dir().join(format!("mike-case-{}", uuid::Uuid::new_v4()));
                if std::fs::write(&tmp, bytes).is_ok() {
                    match crate::pdf::extract_text(&tmp) {
                        Ok(pages) => {
                            let page_count = pages.len();
                            let needed_ocr = pages.iter().any(|p| p.needs_ocr);
                            let text = pages
                                .into_iter()
                                .map(|p| format!("[Page {}]\n{}", p.page + 1, p.text))
                                .collect::<Vec<_>>()
                                .join("\n\n");
                            let _ = std::fs::remove_file(&tmp);
                            return (text, page_count, needed_ocr);
                        }
                        Err(_) => {
                            let _ = std::fs::remove_file(&tmp);
                            return (String::new(), 0, false);
                        }
                    }
                }
                (String::new(), 0, false)
            }
            #[cfg(not(feature = "pdf"))]
            (String::new(), 0, false)
        }
        "docx" => {
            let text = crate::pdf::extract_docx_text(bytes).unwrap_or_default();
            let pages = if text.is_empty() { 0 } else { (text.split_whitespace().count() / 300).max(1) };
            (text, pages, false)
        }
        "xlsx" | "xls" | "xlsb" | "ods" => {
            let text = crate::pdf::extract_xlsx_text(bytes).unwrap_or_default();
            let pages = if text.is_empty() { 0 } else { 1 };
            (text, pages, false)
        }
        "txt" | "md" | "csv" => {
            let text = String::from_utf8_lossy(bytes).to_string();
            let pages = if text.is_empty() { 0 } else { (text.split_whitespace().count() / 300).max(1) };
            (text, pages, false)
        }
        _ => (String::new(), 0, false),
    }
}

fn estimate_analysis_seconds(total_pages: usize, has_ocr: bool) -> u32 {
    let base: u32 = 30; // minimum overhead
    // OCR @ 150 DPI: ~1s/page for render+OCR; non-OCR: ~0.1s/page for pdfium text
    let extraction_time = if has_ocr {
        total_pages as u32
    } else {
        (total_pages as u32) / 10
    };
    let llm_time: u32 = 60; // 7 agents in parallel, ~60s for LLM
    base + extraction_time + llm_time
}

fn extract_text(file_type: &str, bytes: &[u8]) -> String {
    match file_type {
        "docx" => crate::pdf::extract_docx_text(bytes).unwrap_or_default(),
        "xlsx" | "xls" | "xlsb" | "ods" => crate::pdf::extract_xlsx_text(bytes).unwrap_or_default(),
        "txt" | "md" | "csv" => String::from_utf8_lossy(bytes).to_string(),
        "pdf" => {
            #[cfg(feature = "pdf")]
            {
                let tmp =
                    std::env::temp_dir().join(format!("mike-case-{}", uuid::Uuid::new_v4()));
                if std::fs::write(&tmp, bytes).is_ok() {
                    let out = crate::pdf::extract_full_text(&tmp).unwrap_or_default();
                    let _ = std::fs::remove_file(&tmp);
                    out
                } else {
                    String::new()
                }
            }
            #[cfg(not(feature = "pdf"))]
            String::new()
        }
        _ => String::new(),
    }
}

/// Hard-truncate `text` to roughly `target_tokens` worth of characters on a UTF-8
/// char boundary. Used as the fallback when LLM compression fails so we never fan
/// the over-budget context out to the agents. Mirrors compress_context's
/// char-boundary-safe slice (it never panics on multibyte input). Appends a marker
/// so the dropped tail is visible in the prompt itself.
fn hard_truncate_to_tokens(text: &str, target_tokens: usize) -> String {
    let max_chars = target_tokens * CHARS_PER_TOKEN;
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    // max_chars is a char count; convert to a byte index on a char boundary.
    let end = text
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    format!("{}\n\n[... context truncated: compression failed]", &text[..end])
}

async fn compress_context(
    text: &str,
    target_tokens: usize,
    llm_params: &StreamParams,
) -> Result<String> {
    let window = context_window_tokens(&llm_params.model);
    let max_input_chars = (window as f64 * 0.85 * 4.0) as usize;
    let input = if text.len() > max_input_chars {
        let mut end = max_input_chars;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    } else {
        text
    };

    let target_chars = target_tokens * 4;
    let params = StreamParams {
        model: llm_params.model.clone(),
        system_prompt: "You are a legal document compressor. Condense the provided case \
            documents while preserving all legally significant facts, dates, names, arguments, \
            and holdings. Keep document labels (e.g. [doc-0: filename]) intact."
            .to_string(),
        system_volatile: String::new(),
        messages: vec![Message::user(format!(
            "Compress the following case documents to approximately {target_chars} characters. \
             Preserve all legally material content.\n\n{input}"
        ))],
        tools: vec![],
        max_iterations: 1,
        enable_thinking: false,
        local_config: llm_params.local_config.clone(),
        claude_api_key: llm_params.claude_api_key.clone(),
        gemini_api_key: llm_params.gemini_api_key.clone(),
        gemini_region: llm_params.gemini_region.clone(),
    };
    call_llm(params, "compressor", None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "[Page 1]\nThe respondent failed to pay maintenance of Rs. 5,000\nper month as ordered by the Family Court on 12.03.2021.";

    #[test]
    fn exact_quote_verifies() {
        assert_eq!(verify_quote("failed to pay maintenance of Rs. 5,000", DOC), 1.0);
    }

    #[test]
    fn quote_spanning_page_break_verifies() {
        // quote crosses the "[Page 1]" line break — whitespace normalization bridges it
        assert!(verify_quote("maintenance of Rs. 5,000 per month", DOC) >= QUOTE_MATCH_THRESHOLD);
    }

    #[test]
    fn smart_quotes_normalize() {
        let doc = "the court held \u{201C}res judicata applies here\u{201D} firmly";
        assert!(verify_quote("\"res judicata applies here\"", doc) >= QUOTE_MATCH_THRESHOLD);
    }

    #[test]
    fn ocr_garble_passes_fuzzy() {
        // OCR misread "form" as "forrn"; fuzzy match should still accept
        let doc = "the applicant must submit the prescribed forrn before the deadline";
        assert!(verify_quote("submit the prescribed form before", doc) >= QUOTE_MATCH_THRESHOLD);
    }

    #[test]
    fn fabricated_quote_fails() {
        assert!(verify_quote("the respondent admitted committing the fraud", DOC) < QUOTE_MATCH_THRESHOLD);
    }

    #[test]
    fn hard_truncate_bounds_over_budget_context() {
        // A context far over budget must be cut down so it is NOT dispatched whole.
        let big = "word ".repeat(10_000); // ~50k chars, ~12.5k tokens
        let target = 1_000; // tokens
        let out = hard_truncate_to_tokens(&big, target);
        // Truncated payload (excluding marker) must fit the token budget.
        assert!(estimate_tokens(&out) <= target + 50, "truncated context still over budget: {} tok", estimate_tokens(&out));
        assert!(out.len() < big.len(), "nothing was truncated");
        assert!(out.contains("context truncated"), "no visible truncation marker");
    }

    #[test]
    fn hard_truncate_multibyte_never_panics() {
        // Devanagari + ₹ + smart quotes straddling the byte cut must not panic
        // and must stay on a char boundary.
        let unit = "धारा ₹5,000 \u{201C}res judicata\u{201D} café — "; // multibyte
        let big = unit.repeat(2_000);
        for target in [1usize, 7, 50, 333, 1_000] {
            let out = hard_truncate_to_tokens(&big, target);
            // Slicing on a non-boundary would have panicked above; reaching here
            // and round-tripping through chars proves boundary safety.
            assert_eq!(out, out.chars().collect::<String>());
        }
    }

    #[test]
    fn hard_truncate_keeps_short_context_intact() {
        let small = "short [doc-0: a.pdf] body";
        let out = hard_truncate_to_tokens(small, 1_000);
        assert_eq!(out, small, "under-budget context should pass through untouched");
    }

    #[test]
    fn collects_both_field_conventions() {
        let v = json!({
            "items": [{"source_doc_id": "doc-0", "exact_quote": "alpha"}],
            "strength": {"supporting_doc": "doc-1", "supporting_text": "beta"}
        });
        let mut refs = Vec::new();
        collect_quote_refs(&v, &mut refs);
        assert!(refs.contains(&("doc-0".to_string(), "alpha".to_string())));
        assert!(refs.contains(&("doc-1".to_string(), "beta".to_string())));
    }
}
