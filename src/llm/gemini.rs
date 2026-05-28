/// Google Gemini — generateContent API with server-sent events streaming.
/// Supports function calling (tool-use) via `tools.function_declarations`.
use anyhow::{anyhow, Result};
use futures_util::{stream, StreamExt};
use serde_json::{json, Value};

use super::types::{Message, Role, StreamEvent, StreamParams, ToolCall};
use crate::llm::BoxStream;

fn api_key(params: &StreamParams) -> Result<String> {
    if let Some(k) = params.gemini_api_key.as_ref().filter(|s| !s.trim().is_empty()) {
        return Ok(k.clone());
    }
    if let Ok(env_key) = std::env::var("GEMINI_API_KEY") {
        if !env_key.trim().is_empty() {
            return Ok(env_key);
        }
    }
    Err(anyhow!("Gemini API key not configured: set it in Account → Models, or set GEMINI_API_KEY"))
}

/// Resolve the Gemini endpoint URL.
///
/// The Generative Language API (the API-key-authenticated endpoint at
/// `generativelanguage.googleapis.com`) is *only* served globally — there
/// are no regional subdomains for it. Regional Gemini access requires
/// **Vertex AI**, which is a separate API using OAuth / service-account
/// credentials and a different URL pattern
/// (`<region>-aiplatform.googleapis.com/v1/projects/<proj>/locations/...`).
///
/// We accept and persist the user's region preference via
/// `params.gemini_region` so the choice survives MikeRust restarts and
/// is ready for the future Vertex integration, but for now we route
/// every call to the global endpoint and log an info line when a
/// non-global region is requested.
fn base_url_with(model: &str, region: Option<&str>, suffix: &str) -> String {
    if let Some(r) = region.map(|s| s.trim()).filter(|r| !r.is_empty() && *r != "global") {
        tracing::info!(
            "[gemini] region '{r}' requested but Generative Language API is global-only — \
             routing to generativelanguage.googleapis.com (Vertex AI integration pending)"
        );
    }
    format!("https://generativelanguage.googleapis.com/v1beta/models/{model}:{suffix}")
}

fn base_url(params: &StreamParams) -> String {
    base_url_with(
        &params.model,
        params.gemini_region.as_deref(),
        "streamGenerateContent",
    )
}

/// Convert MikeRust messages into Gemini `contents` parts.
/// Roles: user → "user", assistant → "model".
/// Tool calls (assistant.tool_calls) → `model` part with `functionCall`.
/// Tool results (role=Tool) → `user` part with `functionResponse`.
fn to_wire_contents(messages: &[Message]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m.role {
            Role::System => continue, // system goes to systemInstruction, not contents
            Role::Tool => {
                // Gemini expects role:user with a functionResponse keyed by
                // the function NAME (not the call id). Prefer `tool_name`,
                // fall back to `tool_call_id` (which for OpenAI-compat models
                // happens to also be the function name in many cases).
                let name = m
                    .tool_name
                    .clone()
                    .or_else(|| m.tool_call_id.clone())
                    .unwrap_or_default();
                let response_value: Value = serde_json::from_str(&m.content)
                    .unwrap_or_else(|_| json!({ "result": m.content }));
                out.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": name,
                            "response": response_value
                        }
                    }]
                }));
            }
            Role::Assistant if !m.tool_calls.is_empty() => {
                let parts: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .map(|c| {
                        json!({
                            "functionCall": {
                                "name": c.name,
                                "args": c.input
                            }
                        })
                    })
                    .collect();
                out.push(json!({ "role": "model", "parts": parts }));
            }
            Role::User | Role::Assistant => {
                let role = if matches!(m.role, Role::Assistant) { "model" } else { "user" };
                out.push(json!({ "role": role, "parts": [{ "text": m.content }] }));
            }
        }
    }
    out
}

fn build_body(params: &StreamParams) -> Value {
    let mut body = json!({
        "contents": to_wire_contents(&params.messages),
        "generationConfig": {
            "maxOutputTokens": 8192,
            "temperature": 0.5,
        },
    });
    let system = params.full_system();
    if !system.is_empty() {
        body["systemInstruction"] = json!({
            "parts": [{ "text": system }]
        });
    }
    if !params.tools.is_empty() {
        let function_declarations: Vec<Value> = params
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.function.name,
                    "description": t.function.description,
                    "parameters": sanitize_schema_for_gemini(&t.function.parameters),
                })
            })
            .collect();
        body["tools"] = json!([{ "function_declarations": function_declarations }]);
    }
    body
}

/// Gemini's function-declaration schema is *almost* JSON-Schema but rejects
/// fields like `$schema`, `additionalProperties`, `title`, `default` and the
/// `format` keyword on strings. It also rejects a `required` entry that
/// names a property not present in `properties`. Strip / filter so a
/// permissive MCP schema doesn't trigger a 400 from Gemini.
fn sanitize_schema_for_gemini(v: &Value) -> Value {
    fn walk(v: &Value) -> Value {
        match v {
            Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, val) in map {
                    if matches!(
                        k.as_str(),
                        "$schema"
                            | "$id"
                            | "$ref"
                            | "$defs"
                            | "definitions"
                            | "additionalProperties"
                            | "title"
                            | "default"
                            | "examples"
                            | "const"
                            | "format"
                    ) {
                        continue;
                    }
                    out.insert(k.clone(), walk(val));
                }

                // Filter `required` to only names that exist in `properties`.
                if let (Some(Value::Array(req)), Some(Value::Object(props))) =
                    (out.get("required").cloned(), out.get("properties"))
                {
                    let prop_names: std::collections::HashSet<&str> =
                        props.keys().map(|s| s.as_str()).collect();
                    let filtered: Vec<Value> = req
                        .into_iter()
                        .filter(|r| match r.as_str() {
                            Some(name) => prop_names.contains(name),
                            None => false,
                        })
                        .collect();
                    if filtered.is_empty() {
                        out.remove("required");
                    } else {
                        out.insert("required".to_string(), Value::Array(filtered));
                    }
                } else if matches!(out.get("required"), Some(Value::Array(_)))
                    && out.get("properties").is_none()
                {
                    // `required` without `properties` is meaningless.
                    out.remove("required");
                }

                Value::Object(out)
            }
            Value::Array(arr) => Value::Array(arr.iter().map(walk).collect()),
            other => other.clone(),
        }
    }
    walk(v)
}

pub async fn stream(params: StreamParams) -> Result<BoxStream> {
    let key = api_key(&params)?;
    let client = reqwest::Client::new();
    let url = format!("{}?key={}&alt=sse", base_url(&params), key);

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&build_body(&params))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Gemini API error {status}: {text}"));
    }

    let byte_stream = resp.bytes_stream();
    let event_stream = stream::unfold(
        (byte_stream, String::new(), 0u64),
        |(mut bs, mut buf, mut tc_counter)| async move {
            loop {
                match bs.next().await {
                    None => {
                        if buf.trim().is_empty() { return None; }
                        let line = buf.trim().to_string();
                        buf.clear();
                        return Some((parse_gemini_sse(&line, &mut tc_counter), (bs, buf, tc_counter)));
                    }
                    Some(Err(e)) => {
                        return Some((Err(anyhow!("stream error: {e}")), (bs, buf, tc_counter)));
                    }
                    Some(Ok(bytes)) => {
                        buf.push_str(&String::from_utf8_lossy(&bytes));
                        while let Some(pos) = buf.find('\n') {
                            let line = buf[..pos].trim().to_string();
                            buf.drain(..=pos);
                            if let Some(ev) = parse_gemini_sse_opt(&line, &mut tc_counter) {
                                return Some((Ok(ev), (bs, buf, tc_counter)));
                            }
                        }
                    }
                }
            }
        },
    );

    Ok(Box::pin(event_stream))
}

fn parse_gemini_sse(line: &str, tc_counter: &mut u64) -> Result<StreamEvent> {
    parse_gemini_sse_opt(line, tc_counter).ok_or_else(|| anyhow!("empty SSE line"))
}

fn parse_gemini_sse_opt(line: &str, tc_counter: &mut u64) -> Option<StreamEvent> {
    if !line.starts_with("data: ") { return None; }
    let data = line[6..].trim();
    let v: Value = serde_json::from_str(data).ok()?;
    let parts = v
        .get("candidates")?
        .get(0)?
        .get("content")?
        .get("parts")?
        .as_array()?;

    // Function calls take priority — emit the whole batch as a ToolCalls event.
    let calls: Vec<ToolCall> = parts
        .iter()
        .filter_map(|p| {
            let fc = p.get("functionCall")?;
            *tc_counter += 1;
            let id = format!("gemini-fc-{tc_counter}");
            Some(ToolCall {
                id,
                name: fc.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                input: fc.get("args").cloned().unwrap_or(json!({})),
            })
        })
        .collect();
    if !calls.is_empty() {
        return Some(StreamEvent::ToolCalls(calls));
    }

    // Fall back to text content delta.
    let text = parts
        .iter()
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("");
    if !text.is_empty() {
        return Some(StreamEvent::ContentDelta(text));
    }
    None
}

pub async fn complete(params: StreamParams) -> Result<String> {
    let key = api_key(&params)?;
    let client = reqwest::Client::new();
    let url = format!(
        "{}?key={}",
        base_url_with(&params.model, params.gemini_region.as_deref(), "generateContent"),
        key,
    );

    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&build_body(&params))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Gemini API error {status}: {text}"));
    }

    let v: Value = resp.json().await?;
    let text = v
        .get("candidates")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::StreamEvent;
    use serde_json::json;

    #[test]
    fn sanitize_drops_unsupported_keys() {
        let raw = json!({
            "$schema": "http://json-schema.org/draft-07/schema",
            "title": "X",
            "default": null,
            "additionalProperties": false,
            "type": "object",
            "properties": {
                "name": { "type": "string", "format": "uri" }
            },
            "required": ["name"]
        });
        let cleaned = sanitize_schema_for_gemini(&raw);
        let map = cleaned.as_object().unwrap();
        assert!(!map.contains_key("$schema"));
        assert!(!map.contains_key("title"));
        assert!(!map.contains_key("default"));
        assert!(!map.contains_key("additionalProperties"));
        assert_eq!(map["type"], "object");
        // `format` should be stripped from inner properties.
        let name = &map["properties"]["name"];
        assert!(!name.as_object().unwrap().contains_key("format"));
        assert_eq!(name["type"], "string");
        // `required` should remain since `name` exists in properties.
        assert_eq!(map["required"], json!(["name"]));
    }

    #[test]
    fn sanitize_filters_required_to_existing_props() {
        let raw = json!({
            "type": "object",
            "properties": { "a": { "type": "string" } },
            "required": ["a", "b", "ghost"]
        });
        let cleaned = sanitize_schema_for_gemini(&raw);
        // Only `a` exists, so `b` and `ghost` must be dropped.
        assert_eq!(cleaned["required"], json!(["a"]));
    }

    #[test]
    fn sanitize_removes_empty_required() {
        let raw = json!({
            "type": "object",
            "properties": { "a": { "type": "string" } },
            "required": ["b"]
        });
        let cleaned = sanitize_schema_for_gemini(&raw);
        assert!(cleaned.as_object().unwrap().get("required").is_none());
    }

    #[test]
    fn sanitize_drops_required_without_properties() {
        let raw = json!({
            "type": "object",
            "required": ["a"]
        });
        let cleaned = sanitize_schema_for_gemini(&raw);
        assert!(cleaned.as_object().unwrap().get("required").is_none());
    }

    #[test]
    fn sanitize_recurses_into_arrays() {
        let raw = json!({
            "type": "array",
            "items": {
                "type": "object",
                "title": "ITEM",
                "properties": { "x": { "type": "number", "format": "float" } }
            }
        });
        let cleaned = sanitize_schema_for_gemini(&raw);
        let item = &cleaned["items"];
        assert!(!item.as_object().unwrap().contains_key("title"));
        assert!(!item["properties"]["x"].as_object().unwrap().contains_key("format"));
    }

    #[test]
    fn parse_sse_text_delta() {
        let mut counter = 0u64;
        let line = r#"data: {"candidates":[{"content":{"parts":[{"text":"ciao"}]}}]}"#;
        match parse_gemini_sse_opt(line, &mut counter) {
            Some(StreamEvent::ContentDelta(s)) => assert_eq!(s, "ciao"),
            other => panic!("expected ContentDelta, got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_function_call_increments_counter() {
        let mut counter = 0u64;
        let line = r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"read_document","args":{"doc_id":"doc-0"}}}]}}]}"#;
        match parse_gemini_sse_opt(line, &mut counter) {
            Some(StreamEvent::ToolCalls(calls)) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "read_document");
                assert_eq!(calls[0].input["doc_id"], "doc-0");
                assert_eq!(calls[0].id, "gemini-fc-1");
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
        // Subsequent call should produce id #2.
        let line2 = r#"data: {"candidates":[{"content":{"parts":[{"functionCall":{"name":"x","args":{}}}]}}]}"#;
        match parse_gemini_sse_opt(line2, &mut counter) {
            Some(StreamEvent::ToolCalls(calls)) => assert_eq!(calls[0].id, "gemini-fc-2"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn parse_sse_ignores_garbage() {
        let mut counter = 0u64;
        assert!(parse_gemini_sse_opt("data: {}", &mut counter).is_none());
        assert!(parse_gemini_sse_opt("data: not json", &mut counter).is_none());
        assert!(parse_gemini_sse_opt("event: keepalive", &mut counter).is_none());
    }
}
