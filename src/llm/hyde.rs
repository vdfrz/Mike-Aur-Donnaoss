//! HyDE — Hypothetical Document Embeddings for chat-time retrieval.
//!
//! When enabled (env `MIKE_HYDE_ENABLED=1`), `retrieve_kb_chunks` in
//! chat.rs invokes [`generate_hypothesis`] to ask the user's
//! currently-active LLM to draft a short pseudo-answer — a 3-4 sentence
//! paragraph that would plausibly answer the query *as if it had been
//! extracted from an authoritative document*. Both the original query
//! and the hypothesis are then embedded; the two KNN result sets are
//! merged via Reciprocal Rank Fusion before the usual top-K + distance
//! threshold runs.
//!
//! Why this helps: embedding similarities match passages whose surface
//! form looks like the embedding input. A user question like "kya
//! contract mein notice period ka koi clause hai?" is structurally
//! unlike the actual passages that answer it — cosine drift can be
//! 0.05-0.10 lower than a paraphrased "passage". The hypothesis closes
//! that gap by giving the embedder something shaped like a passage.

use anyhow::Result;

use super::types::{Message, StreamParams};

/// Maximum characters returned. The hypothesis is only used as a
/// retrieval probe — there is no value in long completions. ~600 chars
/// ≈ 150 tokens is enough for 3-4 sentences and keeps the call cheap.
const HYPOTHESIS_MAX_CHARS: usize = 600;

/// Credentials needed to drive the one-shot HyDE call.
#[derive(Debug, Clone, Default)]
pub struct HydeCreds {
    pub local_config: Option<super::types::LocalConfig>,
    pub claude_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_region: Option<String>,
}

/// Draft a pseudo-answer for `user_query` suitable for embedding.
/// Returns the raw model output (trimmed), hard-capped to
/// `HYPOTHESIS_MAX_CHARS`.
pub async fn generate_hypothesis(
    user_query: &str,
    target_model: &str,
    creds: &HydeCreds,
) -> Result<String> {
    let system = "\
You generate a SHORT hypothetical passage to seed semantic retrieval. \
Your output will NOT be shown to the user — it is only embedded and \
compared against a vector index of real documents. \
\n\nRules:\n\
- Write 3 to 4 short sentences (under 600 characters total).\n\
- Phrase the answer as if it were lifted from an authoritative legal \
document — same register and vocabulary a legal corpus would use.\n\
- Do NOT preface with \"the answer is\", \"in this case\", or any meta \
commentary. Output ONLY the hypothetical passage.\n\
- Do NOT invent specific names, dates, citations, statutes or articles. \
Use generic placeholders (\"the relevant section\", \"the contract\", \
\"the date specified\") if needed — the embedder cares about phrasing, \
not facts.\n\
- If the query is in Hindi, Hinglish, or a mix of Hindi and English, \
write the hypothesis in the same mixed register.\n"
        .to_string();

    let user_msg = format!(
        "User query (to expand into a hypothetical passage):\n\n{}",
        user_query.trim()
    );

    let params = StreamParams {
        model: target_model.to_string(),
        system_prompt: system,
        system_volatile: String::new(),
        messages: vec![Message::user(user_msg)],
        tools: vec![],
        max_iterations: 1,
        enable_thinking: false,
        local_config: creds.local_config.clone(),
        claude_api_key: creds.claude_api_key.clone(),
        gemini_api_key: creds.gemini_api_key.clone(),
        gemini_region: creds.gemini_region.clone(),
    };

    let raw = match super::provider_for_model(target_model) {
        super::Provider::Claude => super::claude::complete(params).await?,
        super::Provider::OpenAI => super::local::complete(params).await?,
        super::Provider::Gemini => super::gemini::complete(params).await?,
    };

    let mut trimmed = raw.trim().to_string();
    if trimmed.chars().count() > HYPOTHESIS_MAX_CHARS {
        trimmed = trimmed
            .chars()
            .take(HYPOTHESIS_MAX_CHARS)
            .collect::<String>();
    }

    Ok(trimmed)
}
