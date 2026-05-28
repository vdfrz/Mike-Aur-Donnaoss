//! Case-prep output generators: case brief, strategy memo, hearing prep.
//!
//! Each generator builds a single-shot LLM call from case findings,
//! converts the Markdown result to .docx, and persists both to the database.

use anyhow::{anyhow, Result};
use sqlx::SqlitePool;

use crate::llm::{self, types::{LocalConfig, Message, StreamParams}};
use crate::pdf::docx_writer::markdown_to_docx;

const NO_PROCESS_TEXT: &str = "\
Your output goes directly to a client as-is. Every word is the deliverable. \
Do NOT include any preamble, commentary, meta-text, or sign-off. \
Do NOT say things like 'Here is your document' or 'I have prepared'. \
Output ONLY the requested Markdown content, starting with the first heading.";

pub struct OutputConfig {
    pub model: String,
    pub local_config: Option<LocalConfig>,
    pub claude_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_region: Option<String>,
}

struct GeneratedOutput {
    content_md: String,
    docx_bytes: Vec<u8>,
}

async fn call_llm(config: &OutputConfig, system: &str, user_msg: &str) -> Result<String> {
    let params = StreamParams {
        model: config.model.clone(),
        system_prompt: system.to_string(),
        system_volatile: String::new(),
        messages: vec![Message::user(user_msg.to_string())],
        tools: vec![],
        max_iterations: 1,
        enable_thinking: false,
        local_config: config.local_config.clone(),
        claude_api_key: config.claude_api_key.clone(),
        gemini_api_key: config.gemini_api_key.clone(),
        gemini_region: config.gemini_region.clone(),
    };

    match llm::provider_for_model(&config.model) {
        llm::Provider::Claude => llm::claude::complete(params).await,
        llm::Provider::OpenAI => llm::local::complete(params).await,
        llm::Provider::Gemini => llm::gemini::complete(params).await,
    }
}

/// Strip markdown code fences and preamble that some models wrap around output.
fn clean_markdown(md: &str) -> String {
    let mut text = md.trim().to_string();
    // Strip ```markdown ... ``` or ``` ... ``` wrappers
    if text.starts_with("```") {
        // Remove opening fence line
        if let Some(idx) = text.find('\n') {
            text = text[idx + 1..].to_string();
        }
        // Remove closing fence
        if text.ends_with("```") {
            text = text[..text.len() - 3].trim_end().to_string();
        }
    }
    // Strip preamble lines before the first heading
    if let Some(heading_pos) = text.find('#') {
        let before = &text[..heading_pos];
        // If there's only whitespace or short preamble before the heading, strip it
        if before.trim().split_whitespace().count() < 20 {
            text = text[heading_pos..].to_string();
        }
    }
    text.trim().to_string()
}

fn validate_markdown(md: &str) -> bool {
    let trimmed = md.trim();
    if trimmed.is_empty() { return false; }
    // After cleaning, just check it has some substance
    let cleaned = clean_markdown(trimmed);
    if cleaned.is_empty() { return false; }
    // Must have at least 50 chars of content
    cleaned.len() >= 50
}

fn format_findings_block(findings_json: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for f in findings_json {
        let agent = f.get("agent_name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let ftype = f.get("finding_type").and_then(|v| v.as_str()).unwrap_or("");
        let content = f.get("content_json").and_then(|v| v.as_str()).unwrap_or("{}");
        out.push_str(&format!("\n--- Agent: {agent} | Type: {ftype} ---\n{content}\n"));
    }
    out
}

async fn persist_output(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
    output_type: &str,
    filename: &str,
    generated: &GeneratedOutput,
) -> Result<String> {
    let output_id = uuid::Uuid::new_v4().to_string();
    let doc_id = uuid::Uuid::new_v4().to_string();
    let storage_path = format!("documents/{user_id}/{doc_id}");

    let storage = crate::storage::make_storage()?;
    storage
        .put(
            &storage_path,
            &generated.docx_bytes,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .await?;

    let size = generated.docx_bytes.len() as i64;
    let now = chrono::Utc::now().to_rfc3339();

    sqlx::query(
        "INSERT INTO documents (id, user_id, project_id, filename, file_type, size_bytes, storage_path, status) \
         VALUES (?, ?, NULL, ?, 'docx', ?, ?, 'ready')",
    )
    .bind(&doc_id)
    .bind(user_id)
    .bind(filename)
    .bind(size)
    .bind(&storage_path)
    .execute(db)
    .await?;

    sqlx::query(
        "INSERT INTO case_outputs (id, case_id, output_type, content_md, docx_document_id, created_at) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&output_id)
    .bind(case_id)
    .bind(output_type)
    .bind(&generated.content_md)
    .bind(&doc_id)
    .bind(&now)
    .execute(db)
    .await?;

    Ok(doc_id)
}

// ---------------------------------------------------------------------------
// Case Brief
// ---------------------------------------------------------------------------

pub async fn generate_case_brief(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
    findings: &[serde_json::Value],
    config: &OutputConfig,
) -> Result<String> {
    let system = format!(
        "You are a senior litigation associate producing a comprehensive case brief in Markdown.\n\n\
         {NO_PROCESS_TEXT}\n\n\
         FORMAT:\n\
         # Case Brief\n\
         ## Title\n\
         ## Parties\n\
         ## Court & Case No.\n\
         ## Procedural History\n\
         ## Factual Background\n\
         ## Legal Issues\n\
         ## Strengths\n\
         ## Weaknesses\n\
         ## Evidence Status\n\
         ## Strategic Recommendations\n\
         ## Risk Assessment\n\n\
         Use **bold** for key terms. Use bullet lists for multi-item sections. \
         Include inline citations in the format [Agent: exact quote] when referencing agent findings. \
         Every section must have substantive content drawn from the provided findings."
    );

    let user_msg = format!(
        "Produce a case brief for case {case_id} using these agent findings:\n{}",
        format_findings_block(findings)
    );

    let raw = call_llm(config, &system, &user_msg).await?;
    let md = clean_markdown(&raw);

    let content_md = if validate_markdown(&md) {
        md
    } else {
        let retry_system = format!(
            "{system}\n\nCRITICAL: Your previous output was rejected. \
             Start DIRECTLY with '# Case Brief'. No preamble. No commentary."
        );
        let raw_retry = call_llm(config, &retry_system, &user_msg).await?;
        let retry = clean_markdown(&raw_retry);
        if validate_markdown(&retry) { retry } else {
            return Err(anyhow!("LLM failed to produce valid Markdown after retry"));
        }
    };

    let docx_bytes = markdown_to_docx("Case Brief", &content_md)?;
    let generated = GeneratedOutput { content_md, docx_bytes };

    persist_output(db, case_id, user_id, "case_brief", "Case_Brief.docx", &generated).await
}

// ---------------------------------------------------------------------------
// Strategy Memo
// ---------------------------------------------------------------------------

pub async fn generate_strategy_memo(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
    findings: &[serde_json::Value],
    config: &OutputConfig,
) -> Result<String> {
    let system = format!(
        "You are a litigation strategist producing an action-oriented strategy memo in Markdown.\n\n\
         {NO_PROCESS_TEXT}\n\n\
         FORMAT:\n\
         # Strategy Memo\n\
         ## Bottom Line Up Front\n\
         Three sentences maximum. State the core strategic posture, primary risk, and recommended path.\n\
         ## Immediate Actions\n\
         Numbered list. Each item: **Action** — deadline (cite source if from documents) — rationale.\n\
         ## Medium-Term Strategy\n\
         Bullet list of strategic moves over the next 1-6 months.\n\
         ## Anticipated Opposition & Counter-Moves\n\
         | Their Move | Our Counter |\n\
         |---|---|\n\
         Table format. One row per anticipated argument.\n\
         ## Required Research\n\
         Bullet list of precedents, statutes, or authorities still needed.\n\n\
         Use **bold** for action items. Include inline citations [Agent: quote] from findings."
    );

    let user_msg = format!(
        "Produce a strategy memo for case {case_id} using these agent findings:\n{}",
        format_findings_block(findings)
    );

    let raw = call_llm(config, &system, &user_msg).await?;
    let md = clean_markdown(&raw);

    let content_md = if validate_markdown(&md) {
        md
    } else {
        let retry_system = format!(
            "{system}\n\nCRITICAL: Your previous output was rejected. \
             Start DIRECTLY with '# Strategy Memo'. No preamble."
        );
        let raw_retry = call_llm(config, &retry_system, &user_msg).await?;
        let retry = clean_markdown(&raw_retry);
        if validate_markdown(&retry) { retry } else {
            return Err(anyhow!("LLM failed to produce valid Markdown after retry"));
        }
    };

    let docx_bytes = markdown_to_docx("Strategy Memo", &content_md)?;
    let generated = GeneratedOutput { content_md, docx_bytes };

    persist_output(db, case_id, user_id, "strategy_memo", "Strategy_Memo.docx", &generated).await
}

// ---------------------------------------------------------------------------
// Hearing Prep
// ---------------------------------------------------------------------------

pub async fn generate_hearing_prep(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
    findings: &[serde_json::Value],
    hearing_date: Option<&str>,
    config: &OutputConfig,
) -> Result<String> {
    let date_line = hearing_date
        .map(|d| format!("The next hearing is on **{d}**. Tailor urgency accordingly.\n"))
        .unwrap_or_default();

    let system = format!(
        "You are a senior advocate's junior preparing a bullet-point hearing briefing sheet.\n\n\
         {NO_PROCESS_TEXT}\n\n\
         {date_line}\
         FORMAT:\n\
         # Hearing Preparation Brief\n\
         ## Key Facts to Recall\n\
         Bullet list — each fact with inline citation [Agent: quote].\n\
         ## Likely Questions from Bench\n\
         Numbered list of questions the judge is likely to ask, with a one-line suggested answer.\n\
         ## Our Position on Each Issue\n\
         | Issue | Our Position | Supporting Authority |\n\
         |---|---|---|\n\
         Table format.\n\
         ## Authorities to Cite\n\
         Numbered list — case name, citation, key ratio.\n\
         ## Documents to Reference\n\
         Bullet list — document name, relevant page/section numbers.\n\n\
         Keep every item concise — this is a quick-reference sheet, not a narrative."
    );

    let user_msg = format!(
        "Produce a hearing prep brief for case {case_id} using these agent findings:\n{}",
        format_findings_block(findings)
    );

    let raw = call_llm(config, &system, &user_msg).await?;
    let md = clean_markdown(&raw);

    let content_md = if validate_markdown(&md) {
        md
    } else {
        let retry_system = format!(
            "{system}\n\nCRITICAL: Your previous output was rejected. \
             Start DIRECTLY with '# Hearing Preparation Brief'. No preamble."
        );
        let raw_retry = call_llm(config, &retry_system, &user_msg).await?;
        let retry = clean_markdown(&raw_retry);
        if validate_markdown(&retry) { retry } else {
            return Err(anyhow!("LLM failed to produce valid Markdown after retry"));
        }
    };

    let docx_bytes = markdown_to_docx("Hearing Preparation Brief", &content_md)?;
    let generated = GeneratedOutput { content_md, docx_bytes };

    persist_output(db, case_id, user_id, "hearing_prep", "Hearing_Prep.docx", &generated).await
}
