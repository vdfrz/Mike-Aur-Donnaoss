//! Case-prep output generators: case brief, strategy memo, hearing prep.
//!
//! Each generator builds a single-shot LLM call from case findings,
//! converts the Markdown result to .docx, and persists both to the database.

use anyhow::{anyhow, Result};
use sqlx::SqlitePool;

use crate::pdf::docx_writer::markdown_to_docx;

/// Config for the single-shot output-generation calls. Shared with corpus
/// tagging via `crate::llm::oneshot`.
pub use crate::llm::oneshot::OneshotConfig as OutputConfig;

const NO_PROCESS_TEXT: &str = "\
Your output goes directly to a client as-is. Every word is the deliverable. \
Do NOT include any preamble, commentary, meta-text, or sign-off. \
Do NOT say things like 'Here is your document' or 'I have prepared'. \
Output ONLY the requested Markdown content, starting with the first heading.";

struct GeneratedOutput {
    content_md: String,
    docx_bytes: Vec<u8>,
}

async fn call_llm(config: &OutputConfig, system: &str, user_msg: &str) -> Result<String> {
    crate::llm::oneshot::complete(config, system, user_msg).await
}

/// Score how strongly each candidate case actually supports a point of law.
/// Returns one (confidence 0-100, reason) per case, in input order. Never fails:
/// on any LLM/parse error it returns zeros so precedent resolution still succeeds.
pub async fn score_precedent_cases(
    config: &OutputConfig,
    point_of_law: &str,
    cases: &[serde_json::Value],
) -> Vec<(i64, String)> {
    if cases.is_empty() {
        return Vec::new();
    }

    let mut list = String::new();
    for (i, c) in cases.iter().enumerate() {
        let title = c.get("title").and_then(|v| v.as_str()).unwrap_or("(untitled)");
        let court = c.get("court").and_then(|v| v.as_str()).unwrap_or("");
        let text = c
            .get("relevant_paragraphs")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| c.get("snippet").and_then(|v| v.as_str()))
            .unwrap_or("");
        let snippet: String = text.chars().take(600).collect();
        list.push_str(&format!("[{i}] {title} ({court})\n{snippet}\n\n"));
    }

    let system = "You are a skeptical legal-research verifier. For each candidate case, \
judge how strongly it ACTUALLY supports the given point of law — a keyword match is NOT support. \
Return ONLY a JSON array (no prose, no code fences), one object per case in the same order: \
[{\"index\":0,\"confidence\":85,\"reason\":\"one short sentence\"}]. confidence is an integer 0-100.";
    let user = format!("POINT OF LAW:\n{point_of_law}\n\nCANDIDATE CASES:\n{list}");

    let raw = match call_llm(config, system, &user).await {
        Ok(r) => r,
        Err(_) => return cases.iter().map(|_| (0i64, String::new())).collect(),
    };

    let cleaned = raw
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: serde_json::Value =
        serde_json::from_str(cleaned).unwrap_or(serde_json::Value::Null);

    let mut out: Vec<(i64, String)> = vec![(0i64, String::new()); cases.len()];
    if let Some(arr) = parsed.as_array() {
        for item in arr {
            let idx = item.get("index").and_then(|v| v.as_i64()).unwrap_or(-1);
            if idx >= 0 && (idx as usize) < out.len() {
                let conf = item
                    .get("confidence")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    .clamp(0, 100);
                let reason = item
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out[idx as usize] = (conf, reason);
            }
        }
    }
    out
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

fn format_documents_block(documents: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for (i, d) in documents.iter().enumerate() {
        let filename = d.get("filename").and_then(|v| v.as_str()).unwrap_or("(unnamed)");
        let dtype = d.get("document_type").and_then(|v| v.as_str()).unwrap_or("");
        let pages = d.get("page_count").and_then(|v| v.as_i64());
        let pages_str = pages.map(|p| format!(" | {p} page(s)")).unwrap_or_default();
        out.push_str(&format!("{}. {filename} [{dtype}]{pages_str}\n", i + 1));
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

    let resolved = crate::drafting::crossrefs::resolve_crossrefs(db, case_id, &content_md).await;
    let docx_bytes = markdown_to_docx("Case Brief", &resolved.markdown)?;
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

    let resolved = crate::drafting::crossrefs::resolve_crossrefs(db, case_id, &content_md).await;
    let docx_bytes = markdown_to_docx("Strategy Memo", &resolved.markdown)?;
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

    let resolved = crate::drafting::crossrefs::resolve_crossrefs(db, case_id, &content_md).await;
    let docx_bytes = markdown_to_docx("Hearing Preparation Brief", &resolved.markdown)?;
    let generated = GeneratedOutput { content_md, docx_bytes };

    persist_output(db, case_id, user_id, "hearing_prep", "Hearing_Prep.docx", &generated).await
}

// ---------------------------------------------------------------------------
// List of Dates (Synopsis)
// ---------------------------------------------------------------------------

pub async fn generate_list_of_dates(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
    findings: &[serde_json::Value],
    config: &OutputConfig,
) -> Result<String> {
    let system = format!(
        "You are a litigation associate preparing a \"List of Dates\" in Markdown, \
         in the exact format used in Indian court pleadings (writ petitions, tribunal applications).\n\n\
         {NO_PROCESS_TEXT}\n\n\
         FORMAT — output ONLY this table, with no synopsis, key points, or arguments:\n\
         # List of Dates\n\
         | Dates | Events |\n\
         |---|---|\n\
         One row per dated event, strictly chronological with the earliest first, Dates in DD.MM.YYYY format.\n\
         - Begin with the statutory or contextual origin (when the governing Act, Rule or scheme came into force), \
         then the specific events and impugned actions, ending with the filing of the present matter \
         (e.g. \"Hence, the present Writ Petition / Original Application.\").\n\
         - Write each event as a formal, factual full sentence in the third person, naming the relevant document \
         by its exact identifier (Order No., Notification No., letter No. and date). State facts only — no argument.\n\
         - Use the parties' roles as in the record (Applicant, Respondent No. 1, etc.), \
         with an inline citation [Agent: quote] identifying the finding each event is drawn from.\n\
         Do NOT invent dates or events. If the findings conflict on a date, note both."
    );

    let user_msg = format!(
        "Produce a Synopsis and List of Dates for case {case_id} using these agent findings:\n{}",
        format_findings_block(findings)
    );

    let raw = call_llm(config, &system, &user_msg).await?;
    let md = clean_markdown(&raw);

    let content_md = if validate_markdown(&md) {
        md
    } else {
        let retry_system = format!(
            "{system}\n\nCRITICAL: Your previous output was rejected. \
             Start DIRECTLY with '# List of Dates'. No preamble."
        );
        let raw_retry = call_llm(config, &retry_system, &user_msg).await?;
        let retry = clean_markdown(&raw_retry);
        if validate_markdown(&retry) { retry } else {
            return Err(anyhow!("LLM failed to produce valid Markdown after retry"));
        }
    };

    let resolved = crate::drafting::crossrefs::resolve_crossrefs(db, case_id, &content_md).await;
    let docx_bytes = markdown_to_docx("List of Dates", &resolved.markdown)?;
    let generated = GeneratedOutput { content_md, docx_bytes };

    persist_output(db, case_id, user_id, "list_of_dates", "List_of_Dates.docx", &generated).await
}

// ---------------------------------------------------------------------------
// Annexure Index
// ---------------------------------------------------------------------------

pub async fn generate_annexure_index(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
    documents: &[serde_json::Value],
    config: &OutputConfig,
) -> Result<String> {
    let system = format!(
        "You are a litigation associate preparing an Annexure Index in Markdown — the numbered list of \
         annexures as they would be filed with a pleading or petition before an Indian court.\n\n\
         {NO_PROCESS_TEXT}\n\n\
         FORMAT:\n\
         # Annexure Index\n\
         | Annexure No. | Description of Document | Date of Document |\n\
         |---|---|---|\n\
         Number annexures sequentially as Annexure A-1, A-2, A-3 ... in the order listed. \
         Write each description as \"A true copy of <document> dated <date>\". \
         Use DD.MM.YYYY for dates; leave the Date cell blank if the document gives no date.\n\
         ## Cross-references\n\
         For each annexure, give the exact sentence to paste into the body of the pleading, e.g. \
         \"A true copy of the ... dated 10.01.2024 is annexed hereto and marked as **Annexure A-1**.\"\n\n\
         Use ONLY the documents listed below. Do NOT invent documents. \
         Base each description on the document's file name and type; if the date is not evident, leave it blank."
    );

    let user_msg = format!(
        "Prepare an Annexure Index for case {case_id} from these attached documents (in order):\n{}",
        format_documents_block(documents)
    );

    let raw = call_llm(config, &system, &user_msg).await?;
    let md = clean_markdown(&raw);

    let content_md = if validate_markdown(&md) {
        md
    } else {
        let retry_system = format!(
            "{system}\n\nCRITICAL: Your previous output was rejected. \
             Start DIRECTLY with '# Annexure Index'. No preamble."
        );
        let raw_retry = call_llm(config, &retry_system, &user_msg).await?;
        let retry = clean_markdown(&raw_retry);
        if validate_markdown(&retry) { retry } else {
            return Err(anyhow!("LLM failed to produce valid Markdown after retry"));
        }
    };

    let resolved = crate::drafting::crossrefs::resolve_crossrefs(db, case_id, &content_md).await;
    let docx_bytes = markdown_to_docx("Annexure Index", &resolved.markdown)?;
    let generated = GeneratedOutput { content_md, docx_bytes };

    persist_output(db, case_id, user_id, "annexure_index", "Annexure_Index.docx", &generated).await
}

// ---------------------------------------------------------------------------
// List of Cases Referred (deterministic — no LLM)
// ---------------------------------------------------------------------------

pub async fn generate_cases_referred(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
) -> Result<String> {
    let title = "List of Cases Referred";
    let md = crate::drafting::citations::render_cases_referred(db, case_id).await?;
    let docx_bytes = markdown_to_docx(title, &md)?;
    let generated = GeneratedOutput { content_md: md, docx_bytes };

    persist_output(
        db,
        case_id,
        user_id,
        "cases_referred",
        &format!("{title}.docx"),
        &generated,
    )
    .await
}

// ---------------------------------------------------------------------------
// List of Authorities (deterministic — no LLM)
// ---------------------------------------------------------------------------

pub async fn generate_authorities(
    db: &SqlitePool,
    case_id: &str,
    user_id: &str,
) -> Result<String> {
    let title = "List of Authorities";
    let md = crate::drafting::citations::render_authorities(db, case_id).await?;
    let docx_bytes = markdown_to_docx(title, &md)?;
    let generated = GeneratedOutput { content_md: md, docx_bytes };

    persist_output(
        db,
        case_id,
        user_id,
        "list_of_authorities",
        &format!("{title}.docx"),
        &generated,
    )
    .await
}
