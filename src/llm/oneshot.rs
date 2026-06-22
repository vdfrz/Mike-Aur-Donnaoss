//! Shared single-shot LLM helper.
//!
//! A thin wrapper around `provider_for_model` + the per-provider `complete`
//! calls, plus the model-resolution logic that turns a user's saved LLM
//! settings into a concrete (model, local_config) pair. Used by case-prep
//! output generators and firm-corpus metadata tagging so none of them depend
//! on each other.

use anyhow::Result;

use crate::llm::{self, types::{LocalConfig, Message, StreamParams}};
use crate::routes::user::LlmSettings;

/// Provider-agnostic config for a one-shot (non-streaming, no-tools) LLM call.
#[derive(Clone)]
pub struct OneshotConfig {
    pub model: String,
    pub local_config: Option<LocalConfig>,
    pub claude_api_key: Option<String>,
    pub gemini_api_key: Option<String>,
    pub gemini_region: Option<String>,
}

/// Run a single user+system turn and return the model's text.
pub async fn complete(config: &OneshotConfig, system: &str, user_msg: &str) -> Result<String> {
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

/// Like [`complete`], but raises every provider's output-token cap to
/// `max_tokens`. The default one-shot cap is 512 for the local and Claude paths
/// (Gemini defaults to 8192), which silently truncates large structured replies
/// (case briefs, statute section extraction). Each provider's `complete_with_max`
/// appends a `[…truncated at token limit]` marker if the model still stops on
/// the cap, so a clipped result is never shipped silently. Mirrors the
/// per-provider routing in `summarize.rs`.
pub async fn complete_with_max(
    config: &OneshotConfig,
    system: &str,
    user_msg: &str,
    max_tokens: u32,
) -> Result<String> {
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
        llm::Provider::Claude => llm::claude::complete_with_max(params, max_tokens).await,
        llm::Provider::OpenAI => llm::local::complete_with_max(params, max_tokens).await,
        llm::Provider::Gemini => llm::gemini::complete_with_max(params, max_tokens).await,
    }
}

/// Build a `OneshotConfig` from a user's saved LLM settings.
pub fn config_from_settings(settings: &Option<LlmSettings>) -> OneshotConfig {
    let (model, local_config) = resolve_analysis_model(settings);
    OneshotConfig {
        model,
        local_config,
        claude_api_key: settings
            .as_ref()
            .and_then(|s| s.claude_api_key.clone())
            .filter(|k| !k.trim().is_empty()),
        gemini_api_key: settings
            .as_ref()
            .and_then(|s| s.gemini_api_key.clone())
            .filter(|k| !k.trim().is_empty()),
        gemini_region: settings.as_ref().and_then(|s| s.gemini_region.clone()),
    }
}

/// Resolve which model + local config to use for background analysis-style
/// work, honouring the user's saved provider first and falling back to
/// environment-configured providers. Moved here from `routes::cases` so any
/// module can reuse it.
pub fn resolve_analysis_model(settings: &Option<LlmSettings>) -> (String, Option<LocalConfig>) {
    // 1. Try user-configured provider from DB settings
    if let Some(s) = settings {
        if s.active_provider.as_deref() == Some("claude") {
            if s.claude_api_key.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false) {
                let m = s.main_model.clone().unwrap_or_else(|| "claude-sonnet-4-6".to_string());
                return (m, None);
            }
        }

        if s.active_provider.as_deref() == Some("gemini") {
            if s.gemini_api_key.as_deref().map(|k| !k.trim().is_empty()).unwrap_or(false) {
                return ("gemini-2.0-flash".to_string(), None);
            }
        }

        if s.active_provider.as_deref() == Some("openai") {
            if let (Some(m), Some(k)) = (&s.openai_model, &s.openai_api_key) {
                if !k.trim().is_empty() {
                    let cfg = LocalConfig {
                        base_url: "https://api.openai.com/v1".to_string(),
                        api_key: Some(k.clone()),
                        model: m.clone(),
                    };
                    return (format!("openai:{m}"), Some(cfg));
                }
            }
        }

        if s.active_provider.as_deref() == Some("deepseek") {
            if let Some(ref m) = s.local_model {
                let cfg = LocalConfig {
                    base_url: "https://api.deepseek.com/v1".to_string(),
                    api_key: s.local_api_key.clone().filter(|k| !k.trim().is_empty()),
                    model: m.clone(),
                };
                return (format!("local:{m}"), Some(cfg));
            }
        }

        if let Some(ref m) = s.local_model {
            if let Some(ref b) = s.local_base_url {
                if !b.trim().is_empty() {
                    let cfg = LocalConfig {
                        base_url: b.clone(),
                        api_key: s.local_api_key.clone(),
                        model: m.clone(),
                    };
                    return (format!("local:{m}"), Some(cfg));
                }
            }
        }
    }

    // 2. Fallback: check env vars for an available provider
    if let Ok(key) = std::env::var("GEMINI_API_KEY") {
        if !key.trim().is_empty() {
            return ("gemini-2.0-flash".to_string(), None);
        }
    }

    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        if !key.trim().is_empty() {
            let model = "deepseek-chat".to_string();
            let cfg = LocalConfig {
                base_url: "https://api.deepseek.com/v1".to_string(),
                api_key: Some(key),
                model: model.clone(),
            };
            return (format!("local:{model}"), Some(cfg));
        }
    }

    if let Ok(base) = std::env::var("VLLM_BASE_URL") {
        if !base.trim().is_empty() {
            let model = std::env::var("VLLM_MAIN_MODEL").unwrap_or_else(|_| "default".to_string());
            let cfg = LocalConfig {
                base_url: base,
                api_key: std::env::var("VLLM_API_KEY").ok(),
                model: model.clone(),
            };
            return (format!("local:{model}"), Some(cfg));
        }
    }

    ("gemini-2.0-flash".to_string(), None)
}
