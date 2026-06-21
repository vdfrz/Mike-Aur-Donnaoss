/// Anthropic Claude — Messages API with streaming (text/event-stream)
use anyhow::{anyhow, Result};
use futures_util::{stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};

use super::types::{Message, Role, StreamEvent, StreamParams};
use crate::llm::BoxStream;

fn api_key(params: &StreamParams) -> Result<String> {
    if let Some(k) = params.claude_api_key.as_ref().filter(|s| !s.trim().is_empty()) {
        return Ok(k.clone());
    }
    std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| anyhow!("Anthropic API key not configured: set it in Account → Models, or set ANTHROPIC_API_KEY"))
}

fn to_wire_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter(|m| m.role != Role::System)
        .map(|m| {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "user",
                Role::System => "user",
            };
            json!({ "role": role, "content": m.content })
        })
        .collect()
}

pub async fn stream(params: StreamParams) -> Result<BoxStream> {
    let key = api_key(&params)?;
    let client = reqwest::Client::new();

    let wire_messages = to_wire_messages(&params.messages);
    let mut body = json!({
        "model": params.model,
        "max_tokens": 8192,
        "temperature": 0.5,
        "stream": true,
        "messages": wire_messages,
    });
    // Send `system` as content blocks so the stable prefix can carry a
    // `cache_control` breakpoint. Anthropic then caches that prefix for
    // ~5 min; follow-up turns of the same chat re-use it at ~10% of the
    // input-token cost and with a faster time-to-first-token. The
    // volatile tail (per-query KB retrieval) follows uncached so it
    // never invalidates the cached prefix. All Claude 3+ models support
    // this; below the cache minimum Anthropic just skips caching, no error.
    let mut system_blocks: Vec<Value> = Vec::new();
    if !params.system_prompt.is_empty() {
        system_blocks.push(json!({
            "type": "text",
            "text": params.system_prompt,
            "cache_control": { "type": "ephemeral" },
        }));
    }
    if !params.system_volatile.is_empty() {
        system_blocks.push(json!({ "type": "text", "text": params.system_volatile }));
    }
    if !system_blocks.is_empty() {
        body["system"] = json!(system_blocks);
    }

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Claude API error {status}: {text}"));
    }

    let byte_stream = resp.bytes_stream();
    let event_stream = stream::unfold(
        (byte_stream, String::new()),
        |(mut bs, mut buf)| async move {
            loop {
                match bs.next().await {
                    None => {
                        if buf.trim().is_empty() { return None; }
                        let line = buf.trim().to_string();
                        buf.clear();
                        return Some((parse_claude_sse(&line), (bs, buf)));
                    }
                    Some(Err(e)) => {
                        return Some((Err(anyhow!("stream error: {e}")), (bs, buf)));
                    }
                    Some(Ok(bytes)) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(pos) = buf.find('\n') {
                            let line = buf[..pos].trim().to_string();
                            buf.drain(..=pos);
                            if let Some(ev) = parse_claude_sse_opt(&line) {
                                return Some((Ok(ev), (bs, buf)));
                            }
                        }
                    }
                }
            }
        },
    );

    Ok(Box::pin(event_stream))
}

fn parse_claude_sse(line: &str) -> Result<StreamEvent> {
    parse_claude_sse_opt(line).ok_or_else(|| anyhow!("empty SSE line"))
}

fn parse_claude_sse_opt(line: &str) -> Option<StreamEvent> {
    if !line.starts_with("data: ") { return None; }
    let data = line[6..].trim();
    let v: Value = serde_json::from_str(data).ok()?;
    let event_type = v.get("type")?.as_str()?;
    match event_type {
        "content_block_delta" => {
            let delta = v.get("delta")?;
            if delta.get("type")?.as_str()? == "text_delta" {
                let text = delta.get("text")?.as_str()?.to_string();
                return Some(StreamEvent::ContentDelta(text));
            }
            None
        }
        "message_start" => {
            // Surface prompt-cache effectiveness: `cache_read` > 0 means a
            // follow-up turn re-used the cached system prefix instead of
            // re-billing it. Logged only when caching actually engaged.
            if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                let read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                let written = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(0);
                if read > 0 || written > 0 {
                    let fresh = usage
                        .get("input_tokens")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0);
                    tracing::info!(
                        "[claude] prompt cache: {read} tokens read from cache, \
                         {written} written, {fresh} fresh input tokens"
                    );
                }
            }
            None
        }
        "message_delta" => {
            // `message_delta` carries the final `stop_reason`. When the answer
            // was cut off at the token limit (`max_tokens`) the consumer would
            // otherwise see a clean `message_stop` → `Done` and treat the
            // partial draft as a finished deliverable. Surface it as a visible
            // truncation notice (and log) so a clipped affidavit/petition is
            // not handed over as final.
            let stop_reason = v
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|s| s.as_str());
            if stop_reason == Some("max_tokens") {
                tracing::warn!(
                    "[claude] response truncated: stop_reason=max_tokens (hit the {} token output cap)",
                    8192
                );
                return Some(StreamEvent::ContentDelta(
                    "\n\n⚠️ Response truncated at the model's token limit — \
                     ask me to continue to get the rest."
                        .to_string(),
                ));
            }
            None
        }
        "message_stop" => Some(StreamEvent::Done),
        _ => None,
    }
}

pub async fn complete(params: StreamParams) -> Result<String> {
    complete_with_max(params, 512).await
}

/// Like `complete`, but with a caller-supplied output-token budget and a
/// visible `[truncated]` marker when the model stops on `max_tokens`. Used by
/// callers (e.g. the history summarizer) that need a larger budget than the
/// 512-token default and must not silently ship a clipped result.
pub async fn complete_with_max(params: StreamParams, max_tokens: u32) -> Result<String> {
    let key = api_key(&params)?;
    let client = reqwest::Client::new();

    let wire_messages = to_wire_messages(&params.messages);
    let mut body = json!({
        "model": params.model,
        "max_tokens": max_tokens,
        "temperature": 0.5,
        "messages": wire_messages,
    });
    let full_system = params.full_system();
    if !full_system.is_empty() {
        body["system"] = json!(full_system);
    }

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Claude API error {status}: {text}"));
    }

    #[derive(Deserialize)]
    struct Resp { content: Vec<ContentBlock>, stop_reason: Option<String> }
    #[derive(Deserialize)]
    struct ContentBlock { #[serde(rename = "type")] kind: String, text: Option<String> }

    let data: Resp = resp.json().await?;
    let truncated = data.stop_reason.as_deref() == Some("max_tokens");
    let mut text = data.content.into_iter()
        .filter(|b| b.kind == "text")
        .filter_map(|b| b.text)
        .collect::<Vec<_>>()
        .join("");
    if truncated {
        tracing::warn!("[claude] complete truncated at max_tokens={max_tokens}");
        text.push_str(" […truncated at token limit]");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::StreamEvent;

    #[test]
    fn parses_text_delta() {
        let line = r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#;
        match parse_claude_sse_opt(line) {
            Some(StreamEvent::ContentDelta(s)) => assert_eq!(s, "hello"),
            other => panic!("expected ContentDelta, got {other:?}"),
        }
    }

    #[test]
    fn parses_message_stop() {
        let line = r#"data: {"type":"message_stop"}"#;
        matches!(parse_claude_sse_opt(line), Some(StreamEvent::Done));
    }

    #[test]
    fn ignores_non_data_lines() {
        assert!(parse_claude_sse_opt("event: message_start").is_none());
        assert!(parse_claude_sse_opt("").is_none());
    }

    #[test]
    fn ignores_unknown_event_types() {
        let line = r#"data: {"type":"message_start","message":{}}"#;
        assert!(parse_claude_sse_opt(line).is_none());
    }

    #[test]
    fn ignores_non_text_delta() {
        let line = r#"data: {"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{}"}}"#;
        assert!(parse_claude_sse_opt(line).is_none());
    }

    #[test]
    fn surfaces_max_tokens_truncation_as_visible_notice() {
        // A message_delta carrying stop_reason "max_tokens" means the answer
        // was cut off at the token limit. The parser must surface this as a
        // visible content delta rather than dropping it (which would make a
        // truncated draft look complete).
        let line = r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens","stop_sequence":null},"usage":{"output_tokens":8192}}"#;
        match parse_claude_sse_opt(line) {
            Some(StreamEvent::ContentDelta(s)) => {
                assert!(
                    s.to_lowercase().contains("truncat"),
                    "expected a truncation notice, got {s:?}"
                );
            }
            other => panic!("expected a visible truncation ContentDelta, got {other:?}"),
        }
    }

    #[test]
    fn normal_end_turn_message_delta_is_silent() {
        // A normal end_turn must NOT emit a spurious truncation notice.
        let line = r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":42}}"#;
        assert!(parse_claude_sse_opt(line).is_none());
    }
}
