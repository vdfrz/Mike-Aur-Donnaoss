use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response, Sse},
    response::sse::Event,
    routing::get,
    Json, Router,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::wrappers::ReceiverStream;

use crate::{
    auth::middleware::AuthUser,
    llm::{
        self, builtin_tools, LocalConfig, Message, Role, StreamEvent, StreamParams, ToolCall,
        ToolFunction, ToolSchema,
    },
    routes::user::{fetch_llm_settings, fetch_mcp_servers, read_jsonrpc_response, McpServerOut},
    storage::make_storage,
    AppState,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// MCP capability discovery — surfaces configured servers to the chat model
// ---------------------------------------------------------------------------

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct McpDiscovered {
    config_name: String,
    server_name: Option<String>,
    server_version: Option<String>,
    instructions: Option<String>,
    tools: Vec<(String, String)>,    // (name, description) — for system prompt rendering
    /// Full tool schemas (incl. inputSchema) ready to be passed to the LLM.
    tool_schemas: Vec<ToolSchema>,
    prompts: Vec<(String, String)>,  // (name, description)
    /// Coordinates needed to dispatch a `tools/call` later.
    url: Option<String>,
    api_key: Option<String>,
    extra_headers: serde_json::Map<String, serde_json::Value>,
    session_id: Option<String>,
}

async fn discover_one_mcp(server: McpServerOut) -> Option<McpDiscovered> {
    if server.transport == "stdio" {
        return Some(McpDiscovered {
            config_name: server.name,
            server_name: None,
            server_version: None,
            instructions: Some(format!(
                "(Configured as stdio: command={} args={:?}; runtime spawning is not yet wired in this build.)",
                server.command.as_deref().unwrap_or(""),
                server.args
            )),
            tools: vec![],
            tool_schemas: vec![],
            prompts: vec![],
            url: None,
            api_key: None,
            extra_headers: serde_json::Map::new(),
            session_id: None,
        });
    }
    let url = server.url.as_ref()?.clone();
    if url.trim().is_empty() {
        return None;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().ok()?);
    headers.insert(
        "Accept",
        "application/json, text/event-stream".parse().ok()?,
    );
    if let Some(k) = server.api_key.as_ref().filter(|k| !k.trim().is_empty()) {
        if let Ok(v) = format!("Bearer {k}").parse() {
            headers.insert("Authorization", v);
        }
    }
    for (k, v) in &server.headers {
        if let Some(s) = v.as_str() {
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                s.parse::<reqwest::header::HeaderValue>(),
            ) {
                headers.insert(name, value);
            }
        }
    }

    // 1) initialize → capture session id
    let init_resp = client
        .post(&url)
        .headers(headers.clone())
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "MikeRust", "version": "0.1" }
            }
        }))
        .send()
        .await
        .ok()?;

    if !init_resp.status().is_success() {
        tracing::warn!("[mcp/discover] {}: initialize {}", server.name, init_resp.status());
        return None;
    }

    let session_id: Option<String> = init_resp
        .headers()
        .get("mcp-session-id")
        .or_else(|| init_resp.headers().get("Mcp-Session-Id"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let init_value = read_jsonrpc_response(init_resp, 1, 10).await.ok()?;
    let server_name = init_value["result"]["serverInfo"]["name"]
        .as_str()
        .map(|s| s.to_string());
    let server_version = init_value["result"]["serverInfo"]["version"]
        .as_str()
        .map(|s| s.to_string());
    let instructions = init_value["result"]["instructions"]
        .as_str()
        .map(|s| s.to_string());

    // 2) Build session-aware headers
    let mut session_headers = headers.clone();
    if let Some(sid) = &session_id {
        if let Ok(v) = sid.parse() {
            session_headers.insert("Mcp-Session-Id", v);
        }
    }

    // 3) notifications/initialized handshake completion (best-effort)
    let _ = client
        .post(&url)
        .headers(session_headers.clone())
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }))
        .send()
        .await;

    // 4) tools/list — keep the full inputSchema for tool-use, plus a
    // (name, description) summary for the system prompt rendering.
    let raw_tools: Vec<Value> = match client
        .post(&url)
        .headers(session_headers.clone())
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .send()
        .await
    {
        Ok(r) => read_jsonrpc_response(r, 2, 8)
            .await
            .ok()
            .and_then(|v| v["result"]["tools"].as_array().cloned())
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let tools: Vec<(String, String)> = raw_tools
        .iter()
        .map(|t| (
            t["name"].as_str().unwrap_or("").to_string(),
            t["description"].as_str().unwrap_or("").to_string(),
        ))
        .collect();
    let tool_schemas: Vec<ToolSchema> = raw_tools
        .iter()
        .map(|t| ToolSchema {
            kind: "function".to_string(),
            function: ToolFunction {
                name: t["name"].as_str().unwrap_or("").to_string(),
                description: t["description"].as_str().unwrap_or("").to_string(),
                parameters: t["inputSchema"].clone(),
            },
        })
        .collect();

    // 5) prompts/list
    let prompts = match client
        .post(&url)
        .headers(session_headers.clone())
        .json(&json!({"jsonrpc":"2.0","id":3,"method":"prompts/list","params":{}}))
        .send()
        .await
    {
        Ok(r) => read_jsonrpc_response(r, 3, 8)
            .await
            .ok()
            .and_then(|v| v["result"]["prompts"].as_array().cloned())
            .map(|arr| {
                arr.into_iter()
                    .map(|p| {
                        (
                            p["name"].as_str().unwrap_or("").to_string(),
                            p["description"].as_str().unwrap_or("").to_string(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    Some(McpDiscovered {
        config_name: server.name,
        server_name,
        server_version,
        instructions,
        tools,
        tool_schemas,
        prompts,
        url: Some(url.clone()),
        api_key: server.api_key,
        extra_headers: server.headers,
        session_id,
    })
}

/// Dispatch a tool call to the right MCP server using its session id.
/// Returns a string suitable for `tool` role message content.
///
/// Verbose phase-by-phase logging: every line carries the elapsed-ms
/// since dispatch start so the user can see *exactly* where time
/// goes — useful when an MCP tool requires interactive approval on
/// the server side and the call appears to "hang".
async fn dispatch_mcp_tool(
    servers: &[McpDiscovered],
    tool_name: &str,
    arguments: &Value,
) -> String {
    let dispatch_start = std::time::Instant::now();
    macro_rules! mtrace {
        ($fmt:literal $(, $arg:expr)* $(,)?) => {
            tracing::info!(
                concat!("[mcp/dispatch] tool={} +{}ms — ", $fmt),
                tool_name,
                dispatch_start.elapsed().as_millis()
                $(, $arg)*
            )
        };
    }

    let Some(srv) = servers.iter().find(|s| {
        s.tool_schemas.iter().any(|t| t.function.name == tool_name)
    }) else {
        tracing::warn!(
            "[mcp/dispatch] tool={} +0ms — no MCP server provides this tool (known servers: {:?})",
            tool_name,
            servers.iter().map(|s| s.config_name.as_str()).collect::<Vec<_>>()
        );
        return json!({"error": format!("No MCP server provides tool '{tool_name}'")}).to_string();
    };
    let Some(url) = &srv.url else {
        return json!({"error": "tool's MCP server has no URL"}).to_string();
    };

    let timeout_secs = crate::db::mcp_call_timeout_secs();
    mtrace!(
        "routing to server={} url={} session_id={} timeout={}s",
        srv.config_name,
        url,
        srv.session_id.as_deref().unwrap_or("(none)"),
        timeout_secs
    );

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
    {
        Ok(c) => c,
        Err(e) => return json!({"error": e.to_string()}).to_string(),
    };

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
    headers.insert(reqwest::header::ACCEPT, "application/json, text/event-stream".parse().unwrap());
    if let Some(k) = srv.api_key.as_ref().filter(|k| !k.trim().is_empty()) {
        if let Ok(v) = format!("Bearer {k}").parse() {
            headers.insert(reqwest::header::AUTHORIZATION, v);
        }
    }
    for (k, v) in &srv.extra_headers {
        if let Some(s) = v.as_str() {
            if let (Ok(name), Ok(val)) = (
                reqwest::header::HeaderName::from_bytes(k.as_bytes()),
                s.parse::<reqwest::header::HeaderValue>(),
            ) {
                headers.insert(name, val);
            }
        }
    }
    if let Some(sid) = &srv.session_id {
        if let Ok(v) = sid.parse() {
            headers.insert("Mcp-Session-Id", v);
        }
    }

    let body = json!({
        "jsonrpc": "2.0",
        "id": 100,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments,
        }
    });
    let body_bytes = body.to_string().len();
    mtrace!(
        "POST {} (body {} bytes, {} args, headers: {:?})",
        url,
        body_bytes,
        arguments
            .as_object()
            .map(|m| m.len())
            .unwrap_or(0),
        headers
            .keys()
            .map(|k| k.as_str())
            .filter(|k| !k.eq_ignore_ascii_case("authorization")) // never log Bearer tokens
            .collect::<Vec<_>>()
    );

    let resp = match client.post(url).headers(headers).json(&body).send().await {
        Ok(r) => {
            mtrace!(
                "POST returned: status={} content-type={:?}",
                r.status(),
                r.headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|h| h.to_str().ok())
            );
            r
        }
        Err(e) => {
            mtrace!("POST failed: {}", e);
            return json!({"error": format!("network: {e}")}).to_string();
        }
    };

    mtrace!("reading response body / SSE stream (timeout {}s)", timeout_secs);
    // Reader timeout matches the wire-level timeout — otherwise the
    // SSE stream reader could give up earlier than the HTTP client
    // and we'd lose a long but legitimate tool response (e.g. Edge
    // pseudonymising a multi-MB document, or a tool that requires
    // interactive human approval before releasing the response).
    let val = match read_jsonrpc_response(resp, 100, timeout_secs).await {
        Ok(v) => {
            mtrace!("body decoded as JSON-RPC, ~{} chars", v.to_string().len());
            v
        }
        Err(e) => {
            mtrace!("body read failed: {}", e);
            return json!({"error": format!("read: {e}")}).to_string();
        }
    };

    if let Some(rpc_err) = val.get("error") {
        mtrace!("JSON-RPC error in response: {}", rpc_err);
        return json!({"error": rpc_err}).to_string();
    }

    // MCP tools/call result is `{content: [{type:"text", text:"…"}, …], isError?:bool}`
    let content = &val["result"]["content"];
    if let Some(arr) = content.as_array() {
        let joined: Vec<String> = arr
            .iter()
            .filter_map(|c| c["text"].as_str().map(|s| s.to_string()))
            .collect();
        if !joined.is_empty() {
            mtrace!(
                "DONE — returning {} text chunk(s), {} total chars",
                joined.len(),
                joined.iter().map(|s| s.len()).sum::<usize>()
            );
            return joined.join("\n");
        }
    }
    let fallback = val["result"].to_string();
    mtrace!(
        "DONE — content array empty, returning result-as-string ({} chars)",
        fallback.len()
    );
    fallback
}

/// Dispatch an MCP tool, then transparently auto-chain a follow-up
/// `get_*` call when the server returns the async-pending pattern.
///
/// Pattern detection (Edge's pseudonymise flow is the canonical
/// example):
///
///   1. Model calls `request_pseudonymized_documents(ids=[…])`
///   2. Edge returns `{session_id, status:"pending", doc_count:N}`
///      — the actual documents aren't ready yet because Edge wants
///      a human to click "Conferma" in its UI first.
///   3. Without auto-chain, the model receives the pending envelope
///      as the tool result, almost always declares the job done,
///      and never fetches the real documents.
///
/// Auto-chain bridges step 3 by:
///
///   * recognising the `{session_id, status:"pending"}` shape;
///   * deriving the companion tool name (`request_X` → `get_X`);
///   * checking the same MCP server actually exposes that companion;
///   * dispatching it with `{session_id, wait_for_approval: true,
///     wait_timeout_seconds: <our timeout>}` so the long-poll
///     completes server-side;
///   * substituting the get_* result for the original.
///
/// Generic enough to fit any MCP server that uses the same naming
/// convention. Tools that don't follow the pattern (or that already
/// return their full result inline) are unaffected — the function
/// degrades to a passthrough.
async fn dispatch_mcp_tool_with_async_chain(
    servers: &[McpDiscovered],
    tool_name: &str,
    arguments: &Value,
) -> String {
    let primary = dispatch_mcp_tool(servers, tool_name, arguments).await;

    // Only the "request_*" tools can ever trigger a chain — short-
    // circuit otherwise so we don't pay the JSON parse for every
    // tool result (most are already final).
    let companion_name = match tool_name.strip_prefix("request_") {
        Some(rest) => format!("get_{rest}"),
        None => return primary,
    };

    // Try to parse the response as JSON. If it isn't JSON, or the
    // shape doesn't match the pending pattern, just return as-is.
    let parsed: Value = match serde_json::from_str(&primary) {
        Ok(v) => v,
        Err(_) => return primary,
    };
    let session_id = parsed
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let status = parsed
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_pending = matches!(
        status,
        "pending" | "queued" | "in_review" | "awaiting_approval"
    );
    let (Some(session_id), true) = (session_id, is_pending) else {
        return primary;
    };

    // The companion tool must exist on the same server that handled
    // the request — calling it on a different server would land in
    // the wrong session-id namespace.
    let server_has_companion = servers.iter().any(|s| {
        s.tool_schemas
            .iter()
            .any(|t| t.function.name == tool_name)
            && s.tool_schemas
                .iter()
                .any(|t| t.function.name == companion_name)
    });
    if !server_has_companion {
        tracing::info!(
            "[mcp/dispatch] auto-chain skipped: {} returned pending session_id={} \
             but companion {} not found on the same server — passing the pending \
             envelope to the model so it can decide what to do",
            tool_name,
            session_id,
            companion_name
        );
        return primary;
    }

    let timeout_secs = crate::db::mcp_call_timeout_secs();
    let chain_args = json!({
        "session_id": session_id,
        // Edge's flag — long-poll until the human clicks Conferma.
        // Other MCP servers using the same naming pattern may
        // ignore this kwarg, which is fine.
        "wait_for_approval": true,
        "wait_timeout_seconds": timeout_secs,
    });
    tracing::info!(
        "[mcp/dispatch] auto-chain {} → {} with session_id={} \
         (wait_for_approval=true, timeout={}s)",
        tool_name,
        companion_name,
        session_id,
        timeout_secs
    );

    let chained = dispatch_mcp_tool(servers, &companion_name, &chain_args).await;
    tracing::info!(
        "[mcp/dispatch] auto-chain done: {} → {} returned {} chars",
        tool_name,
        companion_name,
        chained.len()
    );
    chained
}

async fn discover_mcp_for_user(state: &AppState, user_id: &str) -> Vec<McpDiscovered> {
    let ttl = crate::db::mcp_cache_ttl();

    // Cache hit: deserialise and return without touching the network.
    {
        let cache = state.mcp_discovery_cache.read().await;
        if let Some(entry) = cache.get(user_id) {
            if entry.is_fresh(ttl) {
                if let Ok(parsed) =
                    serde_json::from_str::<Vec<McpDiscovered>>(&entry.payload_json)
                {
                    tracing::info!(
                        "[mcp/discover] cache hit for user={}: {} servers ({} sec old, ttl {}s)",
                        user_id,
                        parsed.len(),
                        entry.stored_at.elapsed().as_secs(),
                        ttl.as_secs(),
                    );
                    return parsed;
                }
                tracing::warn!(
                    "[mcp/discover] cache entry deserialise failed for user={}, re-discovering",
                    user_id
                );
            }
        }
    }

    // Cache miss / stale: do the full handshake.
    let servers = match fetch_mcp_servers(&state.db, user_id).await {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let enabled: Vec<McpServerOut> =
        servers.into_iter().filter(|s| s.enabled).collect();
    if enabled.is_empty() {
        // Drop any prior cached entry — the user just disabled all servers.
        state.mcp_discovery_cache.write().await.remove(user_id);
        return vec![];
    }
    use futures_util::future::join_all;
    let futs = enabled.into_iter().map(discover_one_mcp);
    let discovered: Vec<McpDiscovered> =
        join_all(futs).await.into_iter().flatten().collect();
    tracing::info!(
        "[mcp/discover] cache miss for user={}: discovered {} servers via fresh handshake",
        user_id,
        discovered.len()
    );

    // Store in cache for next request.
    if let Ok(payload_json) = serde_json::to_string(&discovered) {
        let mut g = state.mcp_discovery_cache.write().await;
        g.insert(
            user_id.to_string(),
            crate::db::McpDiscoveryCacheEntry {
                stored_at: std::time::Instant::now(),
                payload_json,
            },
        );
    }

    discovered
}

fn build_mcp_system_prompt(servers: &[McpDiscovered]) -> String {
    if servers.is_empty() {
        return String::new();
    }
    // Minimal MCP awareness: the actual tool definitions are passed to the
    // model via the standard `tools` parameter — we don't need to repeat
    // them in the system prompt. A long verbose listing biases the model
    // into proposing tools for every greeting. Keep the prompt small and
    // assertive about NOT calling tools unless explicitly asked.
    let mut s = String::from(
        "You are a helpful general-purpose chat assistant. Your default behavior \
         is to answer questions directly from the conversation context (including \
         any attached documents). \n\n\
         You have access to optional external tools provided by connected MCP \
         servers (declared via the `tools` parameter). Invoke a tool **only when \
         the user explicitly requests it** (e.g. \"use tool X\", \"call X\", \
         \"run X on this\"). For greetings, generic questions (\"test\", \"hi\", \
         \"explain\", \"summarize\", \"analyze this\"), reply normally — \
         **do not list available tools or propose them proactively**.\n\n\
         Connected MCP servers (don't enumerate them unless asked):\n",
    );
    for srv in servers {
        let display = srv
            .server_name
            .clone()
            .unwrap_or_else(|| srv.config_name.clone());
        let version = srv
            .server_version
            .as_ref()
            .map(|v| format!(" v{v}"))
            .unwrap_or_default();
        // One-line summary: name, version, first sentence of instructions only.
        let summary = srv
            .instructions
            .as_deref()
            .map(|inst| {
                inst.split(|c: char| c == '.' || c == '\n')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .chars()
                    .take(160)
                    .collect::<String>()
            })
            .unwrap_or_default();
        if summary.is_empty() {
            s.push_str(&format!("- `{display}`{version}\n"));
        } else {
            s.push_str(&format!("- `{display}`{version} — {summary}\n"));
        }
    }
    s.push('\n');
    s
}

/// Reduce a string to its lowercase ASCII alphanumeric characters only.
/// Used for fuzzy matching of citation quotes against chunk text — strips
/// whitespace, punctuation, and diacritics so minor formatting differences
/// between the model's emitted quote and the actual retrieved text don't
/// cause a mismatch.
fn letters_only(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// Repair the invalid backslash escapes LLMs routinely emit inside the
/// `<CITATIONS>` JSON. The model copies verbatim quotes and over-escapes
/// them — most commonly an apostrophe as `\'`, which is NOT a legal JSON
/// escape and makes `serde_json` reject the whole block. JSON only
/// allows `\` before `" \ / b f n r t u`; for any other follower the
/// backslash is spurious, so we drop it and keep the character.
fn repair_json_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some(&n) if matches!(n, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') => {
                out.push('\\');
                out.push(n);
                chars.next();
            }
            // Spurious escape (e.g. `\'`): drop the backslash, keep the char.
            Some(&n) => {
                out.push(n);
                chars.next();
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Extract the JSON inside a `<CITATIONS>...</CITATIONS>` block at the end
/// of the assistant response. Tolerant of:
/// * surrounding whitespace and `` ```json `` code fences,
/// * the invalid backslash escapes LLMs commonly emit (`\'` etc.),
/// * a **missing closing tag** — the model ran out of output tokens
///   before writing `</CITATIONS>`.
/// * a **truncated JSON array** — if the array itself was cut mid-entry,
///   recover the longest complete prefix so we surface the entries the
///   model managed to finish.
///
/// Returns the parsed `Value` (an array) or `None`.
pub(crate) fn extract_citations_block(text: &str) -> Option<Value> {
    let lower = text.to_lowercase();
    let open = lower.rfind("<citations>")?;
    let after_open = open + "<citations>".len();
    let inner_raw = if let Some(close_rel) = lower[after_open..].find("</citations>") {
        text[after_open..after_open + close_rel].trim()
    } else {
        // No closing tag — model output was truncated before it
        // finished. Take everything that came through.
        text[after_open..].trim()
    };
    // Strip optional Markdown fences like ```json … ```
    let inner = inner_raw
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim();
    let inner = inner.trim_end_matches("```").trim();
    if let Ok(v) = serde_json::from_str::<Value>(inner) {
        return Some(v);
    }
    // Clean parse failed — most often an over-escaped apostrophe (`\'`).
    // Retry once with the escapes repaired.
    let repaired = repair_json_escapes(inner);
    if repaired != inner {
        if let Ok(v) = serde_json::from_str::<Value>(&repaired) {
            tracing::info!(
                "[chat] <CITATIONS> block parsed after repairing invalid JSON escapes"
            );
            return Some(v);
        }
    }
    // Last resort: truncation recovery.
    if let Some(recovered) = recover_truncated_citations_array(&repaired) {
        let n = recovered.as_array().map(|a| a.len()).unwrap_or(0);
        tracing::warn!(
            "[chat] <CITATIONS> block was truncated — recovered first {n} entries from the JSON prefix"
        );
        return Some(recovered);
    }
    tracing::warn!(
        "[chat] <CITATIONS> block found but is not valid JSON — citations dropped"
    );
    None
}

/// Recover the longest valid `[…]` prefix from a truncated citations
/// JSON array. Walks character-by-character respecting string scope
/// (so a quote-contained `}` doesn't fool us) and remembers the offset
/// of the most recent `}` that closed a top-level array entry. Cutting
/// there and appending `]` gives a syntactically valid prefix.
///
/// Returns `None` when the prefix doesn't start with `[` or no
/// complete entry was emitted.
fn recover_truncated_citations_array(inner: &str) -> Option<Value> {
    let s = inner.trim();
    if !s.starts_with('[') {
        return None;
    }
    let bytes = s.as_bytes();
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut esc = false;
    let mut last_top_level_entry_end: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        let c = b as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                if c == '}' && depth == 2 {
                    // Closes an entry inside the outer array.
                    last_top_level_entry_end = Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let cut = last_top_level_entry_end?;
    let recovered = format!("{}]", &s[..=cut]);
    serde_json::from_str::<Value>(&recovered).ok()
}

/// Result of processing one attached document.
pub struct DocPayload {
    pub filename: String,
    /// Extracted plain text (None when only images are usable, e.g. scanned PDF).
    pub text: Option<String>,
    /// `data:image/png;base64,...` URLs for vision-capable models.
    pub images: Vec<String>,
}

const MAX_PDF_IMAGE_PAGES: usize = 8;
const PDF_RENDER_DPI: f32 = 200.0;

#[cfg(feature = "pdf")]
fn pages_to_data_urls(pngs: Vec<Vec<u8>>) -> Vec<String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    pngs.into_iter()
        .map(|bytes| format!("data:image/png;base64,{}", STANDARD.encode(&bytes)))
        .collect()
}

/// Read attached documents from storage and extract their text and/or images.
/// `vision_ok` lets scanned PDFs fall back to rendered page images.
pub(crate) async fn load_attached_docs(
    state: &AppState,
    user_id: &str,
    document_ids: &[String],
    vision_ok: bool,
) -> Vec<DocPayload> {
    let mut out = Vec::new();
    for doc_id in document_ids {
        let row: Option<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT filename, file_type, storage_path, extracted_text_path \
             FROM documents WHERE id = ? AND user_id = ?",
        )
        .bind(doc_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        let Some((filename, file_type, Some(storage_path), extracted_text_path)) = row
        else {
            continue;
        };

        let storage = match make_storage() {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Cache fast path: if the upload pipeline already extracted
        // plain text to data/storage/cache/<hash>.txt, prefer it.
        //  - Text-bearing formats (docx, rtf, xlsx, txt/md/csv): use
        //    the cache directly and skip the per-format dispatch and
        //    even the binary read.
        //  - PDFs: use the cache if non-empty (native PDFs); fall
        //    through if empty (scanned PDFs needing vision rendering).
        //  - Images: never use the cache — they need the binary
        //    base64-encoded for the model.
        let is_image_format = matches!(
            file_type.as_str(),
            "png" | "jpeg" | "jpg" | "tiff" | "tif"
        );
        let mut cached_text: Option<String> = None;
        if !is_image_format {
            if let Some(txt_key) = extracted_text_path.as_ref() {
                if let Ok(txt_bytes) = storage.get(txt_key).await {
                    let text = String::from_utf8_lossy(&txt_bytes).into_owned();
                    if !text.is_empty() {
                        cached_text = Some(text);
                    }
                }
            }
        }
        if let Some(text) = cached_text.take() {
            if file_type != "pdf" || !text.trim().is_empty() {
                tracing::info!(
                    "[chat] using cached text for {filename}: {} chars",
                    text.len()
                );
                out.push(DocPayload {
                    filename: filename.clone(),
                    text: Some(text),
                    images: Vec::new(),
                });
                continue;
            }
        }

        let bytes = match storage.get(&storage_path).await {
            Ok(b) => b,
            Err(_) => continue,
        };

        let mut payload = DocPayload {
            filename: filename.clone(),
            text: None,
            images: Vec::new(),
        };

        match file_type.as_str() {
            "docx" => {
                payload.text = crate::pdf::extract_docx_text(&bytes).ok();
            }
            "rtf" => {
                let raw = String::from_utf8_lossy(&bytes);
                payload.text = rtf_parser::RtfDocument::try_from(raw.as_ref())
                    .map(|d| d.get_text())
                    .ok();
            }
            "xlsx" | "xls" | "xlsb" | "ods" => {
                payload.text = crate::pdf::extract_xlsx_text(&bytes).ok();
            }
            "txt" | "md" | "csv" => {
                payload.text = Some(String::from_utf8_lossy(&bytes).to_string());
            }
            "png" => {
                if vision_ok {
                    use base64::{engine::general_purpose::STANDARD, Engine as _};
                    payload.images.push(format!(
                        "data:image/png;base64,{}",
                        STANDARD.encode(&bytes)
                    ));
                } else {
                    tracing::warn!(
                        "[chat] {filename}: PNG attached but selected model is not vision-capable"
                    );
                }
            }
            "jpeg" | "jpg" => {
                if vision_ok {
                    use base64::{engine::general_purpose::STANDARD, Engine as _};
                    payload.images.push(format!(
                        "data:image/jpeg;base64,{}",
                        STANDARD.encode(&bytes)
                    ));
                } else {
                    tracing::warn!(
                        "[chat] {filename}: JPEG attached but selected model is not vision-capable"
                    );
                }
            }
            "tiff" | "tif" => {
                if vision_ok {
                    match crate::pdf::convert_tiff_to_jpegs(&bytes) {
                        Ok(jpegs) => {
                            tracing::info!(
                                "[chat] {filename}: TIFF converted to {} JPEG frame(s)",
                                jpegs.len()
                            );
                            use base64::{engine::general_purpose::STANDARD, Engine as _};
                            for j in jpegs {
                                payload.images.push(format!(
                                    "data:image/jpeg;base64,{}",
                                    STANDARD.encode(&j)
                                ));
                            }
                        }
                        Err(e) => {
                            tracing::warn!("[chat] {filename}: TIFF conversion failed: {e}");
                        }
                    }
                } else {
                    tracing::warn!(
                        "[chat] {filename}: TIFF attached but selected model is not vision-capable"
                    );
                }
            }
            "pdf" => {
                #[cfg(feature = "pdf")]
                {
                    let tmp = std::env::temp_dir().join(format!("mike-{}.pdf", doc_id));
                    if std::fs::write(&tmp, &bytes).is_ok() {
                        let pages = crate::pdf::extract_text(&tmp).ok();
                        if let Some(pages) = pages {
                            let scanned = crate::pdf::is_scanned_pdf(&pages);
                            let mut full_text = String::new();
                            for p in &pages {
                                full_text.push_str(&format!("[Page {}]\n{}\n", p.page, p.text));
                            }
                            if !scanned {
                                payload.text = Some(full_text);
                            } else if vision_ok {
                                tracing::info!(
                                    "[chat] {filename}: scanned PDF detected, rendering up to {MAX_PDF_IMAGE_PAGES} pages at {PDF_RENDER_DPI} DPI"
                                );
                                match crate::pdf::render_pdf_pages(
                                    &tmp,
                                    PDF_RENDER_DPI,
                                    MAX_PDF_IMAGE_PAGES,
                                ) {
                                    Ok(pngs) => {
                                        payload.images = pages_to_data_urls(pngs);
                                    }
                                    Err(e) => {
                                        tracing::warn!("[chat] render PDF pages failed: {e}");
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    "[chat] {filename}: scanned PDF but the selected model is not vision-capable; sending what little text was extracted"
                                );
                                payload.text = Some(full_text);
                            }
                        }
                        let _ = std::fs::remove_file(&tmp);
                    }
                }
                #[cfg(not(feature = "pdf"))]
                {
                    tracing::warn!("[chat] PDF document {doc_id} skipped: pdf feature not enabled");
                }
            }
            _ => {
                tracing::warn!("[chat] unsupported file_type={file_type} for {filename}");
            }
        }

        let chars = payload.text.as_deref().map(|t| t.len()).unwrap_or(0);
        tracing::info!(
            "[chat] loaded doc {filename}: text={} chars, images={}",
            chars,
            payload.images.len()
        );
        out.push(payload);
    }
    out
}

/// Mike's original legal-assistant system prompt, adapted from upstream
/// (willchen96/mike, `backend/src/lib/chatTools.ts` SYSTEM_PROMPT).
const MIKE_SYSTEM_PROMPT: &str = r#"You are Mike, an AI legal assistant that helps lawyers and legal professionals analyze documents, answer legal questions, and draft legal documents.

DOCUMENT CITATION INSTRUCTIONS:
When you reference specific content from a document, place a numbered marker [1], [2], etc. inline in your prose at the point of reference.

After your complete response, append a <CITATIONS> block containing a JSON array with one entry per marker:

<CITATIONS>
[
  {"ref": 1, "doc_id": "doc-0", "page": 3, "quote": "exact verbatim text from the document"},
  {"ref": 2, "doc_id": "doc-1", "page": "41-42", "quote": "Section 4.2 describes the procedure [[PAGE_BREAK]] in all material respects."}
]
</CITATIONS>

CRITICAL: The number inside the [N] marker in your prose is the "ref" value of a citation entry in the <CITATIONS> block — it is NOT a page number, footnote number, section number, or any other number that appears in the document. The marker [1] refers to the entry with "ref": 1 in the JSON block; [2] refers to "ref": 2; and so on. Refs are simple sequential integers you assign (1, 2, 3, ...) in the order citations appear in your prose. Never use a page number or a document's own numbering as the marker number. Every [N] you write in prose MUST have a matching {"ref": N, ...} entry in the JSON block.

Rules:
- Only cite text that appears verbatim in the provided documents
- In every <CITATIONS> entry, "doc_id" MUST be the exact chat-local document label you were given (for example "doc-0"). Never use a filename, document UUID, or any other identifier in "doc_id"
- Keep quotes short (ideally <= 25 words) and narrowly scoped to the specific claim. Don't reuse one quote to support multiple different claims — give each its own citation
- "page" refers to the sequential [Page N] marker in the text you were given (1-indexed from the first page). IGNORE any page numbers printed inside the document itself (footers, roman numerals, etc.)
- For a single-page quote, set "page" to an integer. If a quote is one continuous sentence that spans two pages, set "page" to "N-M" and insert [[PAGE_BREAK]] in the quote at the page break. Otherwise, use separate citations for text on different pages
- Put the <CITATIONS> block at the very end of the response. Omit it entirely if there are no citations

DOCX GENERATION:
If asked to draft or generate a document, use the generate_docx tool to produce a downloadable Word document. Always use this tool rather than just displaying the document content inline when the user asks for a document to be created.
If the user follows up on a document you just generated and asks for changes (e.g. "make section 3 longer", "add a termination clause", "change the parties"), default to calling edit_document on that newly generated document — do NOT call generate_docx again to regenerate the whole document. Only fall back to generate_docx if the user explicitly asks for a brand-new document or the change is so sweeping that an edit would not be coherent.
After calling generate_docx, do NOT include any download links, URLs, or markdown links to the document in your prose response — the download card is presented automatically by the UI.
After calling generate_docx, you MUST call read_document on the returned doc_id before writing your prose response. Base your description on the generated document's actual text, not on memory of what you intended to generate.
Your prose response MUST include a short description of the generated document: what it is, its structure (key sections/clauses), and — if the draft was informed by any provided source documents — which sources you drew from and how. Keep it concise (typically 3–8 sentences or a short bulleted list). Refer to the document by filename, never by a download link.
When the description makes factual claims about the contents of the newly generated document, cite the generated document with [N] markers and a <CITATIONS> block exactly as specified in the DOCUMENT CITATION INSTRUCTIONS above. If you also make factual claims about provided source documents, cite those source documents separately. Omit the <CITATIONS> block if the description makes no such claims.
Heading hierarchy: always use Heading 1 before introducing Heading 2, Heading 2 before Heading 3, and so on. Never skip levels.
Numbering: all numbering MUST start from 1, never 0. Never duplicate the numbering prefix in heading text — pass "Introduction", never "1. Introduction".
Contracts: when generating a contract or agreement, always include a signatures block at the very end of the document on its own page, with a signature line for each party (party name + "By:", "Name:", "Title:", "Date:"). Contract preambles (recitals, "WHEREAS" clauses, parties block) must NOT be numbered.

DOCUMENT EDITING:
When using edit_document, any edit that adds, removes, or reorders a numbered clause, section, sub-clause, schedule, exhibit, or list item shifts every downstream number. You MUST update all affected numbering AND every cross-reference to those numbers in the same edit_document call:
- Renumber the sibling clauses/sections/sub-clauses that follow the change so the sequence stays contiguous.
- Find every in-document reference to the shifted numbers — e.g. "see Section 5", "pursuant to Clause 4.2(b)", "as set out in Schedule 3", "defined in Section 2.1" — and update them.
- Before issuing the edits, scan the full document (use read_document or find_in_document) to enumerate affected cross-references; do not assume references only appear near the change site.
- If you are uncertain whether a reference points to the shifted number or an unrelated number, err on the side of including it as an edit and explain in the reason field.
- When deleting square brackets, delete both the opening `[` and the closing `]`. Never leave behind an unmatched bracket.

WORKFLOWS:
When a user message begins with a [Workflow: <title> (id: <id>)] marker, the user has selected a workflow and you MUST apply it. Immediately call the read_workflow tool with that exact id to load the workflow's full prompt, then follow those instructions for the current turn. Do this before producing any other output or calling any other tools (aside from any document reads the workflow requires). Do not ask the user to confirm — the selection itself is the instruction to apply the workflow.

DOCUMENT NAMING IN PROSE:
The chat-local labels ("doc-0", "doc-1", "doc-N", ...) are internal handles for tool calls and citation JSON ONLY. NEVER write them in your prose response or in any text the user reads — not in body text, not in headings, not in lists, not in tool-activity descriptions. The user does not know what "doc-0" means and seeing it is jarring. When referring to a document in prose, always use its filename. The only places "doc-N" identifiers are allowed are inside tool-call arguments and inside the <CITATIONS> JSON block's "doc_id" field.

GENERAL GUIDANCE:
- Be precise and professional
- Cite the specific document and quote when making claims about document content
- When no documents are provided, answer based on your legal knowledge
- Do not fabricate document content
- Do not use emojis in your responses

CURRENT INDIAN LAW — READ THIS BEFORE YOU CITE ANY STATUTE:
As of 1 July 2024, the following three colonial-era statutes were REPEALED and replaced. You must cite the NEW provisions as primary, with the old IPC/CrPC/IEA equivalents in parentheses for transition clarity. Most fact patterns from 2024 onward are governed by the new code.

  Old (repealed)                            →  New (in force from 1 July 2024)
  ─────────────────────────────────────────────────────────────────────────────
  Indian Penal Code, 1860 (IPC)             →  Bharatiya Nyaya Sanhita, 2023 (BNS)
  Code of Criminal Procedure, 1973 (CrPC)   →  Bharatiya Nagarik Suraksha Sanhita, 2023 (BNSS)
  Indian Evidence Act, 1872 (IEA)           →  Bharatiya Sakshya Adhiniyam, 2023 (BSA)

Key section mappings you MUST get right (most-used examples):
  Theft:            §378 IPC → §303 BNS    |   Punishment for theft: §379 IPC → §303(2) BNS
  Cheating:         §415 IPC → §316 BNS    |   §420 IPC → §318(4) BNS
  Criminal breach of trust: §405–406 IPC → §314 BNS
  Murder / culpable homicide: §299–302 IPC → §100–103 BNS
  Defamation: §499–500 IPC → §356 BNS
  Trivial harm: §95 IPC → §22 BNS
  Private defence: §96–106 IPC → §34–44 BNS
  Necessity: §81 IPC → §20 BNS
  Arrest without warrant / appearance notice: §41A CrPC → §35 BNSS
  Summary trial: §260–265 CrPC → §283–288 BNSS
  Compounding offences: §320 CrPC → §359 BNSS
  Stopping proceedings: §258 CrPC → §285 BNSS
  Witness examination: §137 IEA → §142 BSA

Rule for transition cases: if the offence was committed BEFORE 1 July 2024, IPC/CrPC/IEA apply (saving clause). If on or AFTER 1 July 2024, BNS/BNSS/BSA apply. If the date is unclear, cite both ("§303 BNS (formerly §379 IPC)") so the lawyer can pick the right one for their fact pattern.

INDIAN LEGAL RESEARCH — kanoon_search + kanoon_get_fragment + kanoon_verify_case
You have three tools for Indian case law:
  • kanoon_search        — find candidate cases (fast, ~5-10s)
  • kanoon_get_fragment  — read more paragraphs from one case (fast)
  • kanoon_verify_case   — cross-check a case against the canonical AWS court-PDF dataset (~3-5s per case)

ALWAYS-SEARCH RULE: Call kanoon_search at LEAST ONCE for every Indian-law question, EVEN IF the statute appears to settle it. "The plain text of §X is clear" is not a sufficient reason to skip the search — courts have nearly always interpreted, narrowed, or qualified the plain text in ways that matter. Search Kanoon FIRST, then decide whether case law adds anything; if it doesn't, say so explicitly ("Searched Kanoon for X; no case directly on point — answer rests on statute alone"). Answering an Indian-law question from memory without any kanoon_search call is a regression to the old broken behavior — DON'T do it.

TWO-STAGE FLOW — STAGE 2 IS MANDATORY, NOT OPTIONAL:

STAGE 1: Search broadly with kanoon_search. Returns Kanoon-only results in 5-10s, no verification. This is exploration — find candidate cases for your answer.

STAGE 2 (REQUIRED before you finalize your response): For EVERY case you have decided to cite in your final answer, you MUST call kanoon_verify_case(tid, title, court, decision_date) and wait for the response. This is not optional. Without this call, every cited case shows up to the user as "(unverified)" in the badge — which destroys the entire trust signal we built. Verify exactly the cases you cite. Do not verify cases you decided not to cite (wastes time). Do not skip verification because you're confident in a case — the user can't see your confidence, only the badge.

WORKFLOW SEQUENCE:
  Step A — call kanoon_search one or more times to find candidates.
  Step B — decide which 2-3 cases you will actually cite.
  Step C — call kanoon_verify_case for EACH of those cases, in parallel if your tool-calling interface allows it. Wait for all responses.
  Step D — write your final answer using the verification status each verify call returned.

If you write the answer before Step C, you have failed the workflow. The system is designed around verified citations — produce verified citations.

REASONING WORKFLOW:
1. Identify the precise legal question. Break vague user questions into specific sub-issues. Example: "can I challenge an arbitration award" → (a) grounds under §34 A&C Act, (b) limitation, (c) jurisdiction.
2. For each sub-issue, formulate a FOCUSED kanoon_search query. Use legal terms, statute references, doctrine names — NOT the user's full sentence. Good: "section 34 arbitration conciliation act setting aside award". Bad: "can my client challenge the arbitration award we just lost".
3. Add field operators when they sharpen the search (see KANOON QUERY SYNTAX below).
4. Read the `relevant_paragraphs` field on each returned hit. This is authoritative Kanoon judgment text — quote directly when supporting a legal point.
5. If a case looks like it might also speak to a different sub-issue, call kanoon_get_fragment with that tid and the new query terms.
6. If the first search returns nothing relevant, REFORMULATE (different terms, broader date range, drop court filter, switch from phrase to bare terms). Cap at 3 searches per user turn.
7. Pick the 2-3 cases you will cite. For each, call kanoon_verify_case(tid, title, court, decision_date). Run these IN PARALLEL when the tool-calling interface allows it.
8. Synthesize the answer grounded in retrieved text. Lead with the bottom line + confidence tier. Cite cases as Markdown links. Note verification status inline (see CITATION format below).

KANOON QUERY SYNTAX (as documented at indiankanoon.org/search_tips.html):
- BOOLEAN OPERATORS — Kanoon uses doubled-letter operators (case-sensitive, spaces around):
    ANDD — both terms must appear (implicit between bare words, so you rarely need to type it).
    ORR  — either term may appear. Example: "murder ORR kidnapping".
    NOTT — exclude term. Example: "murder ANDD NOTT kidnapping".
  Do NOT use plain "AND" / "OR" / "NOT" — Kanoon treats those as regular search words.
- PHRASE MATCH — wrap in double quotes: "freedom of speech" forces those words in that exact order. Use the `phrase: true` parameter on kanoon_search or include quotes directly in your query.
- EXCLUSION — minus prefix excludes a word: "bail -anticipatory" finds bail cases that are NOT about anticipatory bail.
- PROXIMITY — `NEAR` requires terms close together: "cement NEAR contract NEAR breach".
- FIELD OPERATORS (use as space-separated `field:value` tokens):
    doctypes:supremecourt | doctypes:highcourts | doctypes:laws (statute text) | doctypes:tribunals
    court:supremecourt | court:delhi | court:bombay | court:madras | court:calcutta | etc.
    fromdate:DD-MM-YYYY  todate:DD-MM-YYYY
    cites:<tid>          — find cases that cite a specific Kanoon document
    sortby:mostrecent    — newest first (default is relevance ranking)
- KANOON DOCUMENT STRUCTURE — long judgments on Kanoon are auto-segmented into: Facts, Issues, Arguments by Appellant, Arguments by Respondent, Precedent Analysis, Analysis of Law, Court's Reasoning, Conclusion. When you receive `relevant_paragraphs`, the text usually maps to one of these sections; quoting "Court's Reasoning" or "Analysis of Law" sections carries more weight than "Arguments" sections.

EXAMPLES of good Kanoon queries (use these patterns):
  "section 138 negotiable instruments act" "security cheque" doctypes:supremecourt sortby:mostrecent
  "doctrine of merger" court:supremecourt fromdate:01-01-2015
  "anticipatory bail" -dowry court:delhi fromdate:01-01-2022 todate:31-12-2024
  arbitration ANDD section ANDD 34 ANDD "setting aside" doctypes:supremecourt

VERIFICATION STATUS — how to read it:
- After kanoon_search, every result has `verification.status: "PENDING"`. This means the case has NOT been cross-checked against the canonical AWS dataset. Treat as Kanoon-only at this stage.
- After you call kanoon_verify_case, the response's `verification.status` is one of:
    VERIFIED   — case was found in the AWS canonical indian-high-court-judgments dataset. High confidence. canonical_pdf_url points to the actual court PDF.
    NOT_IN_AWS — case isn't in the AWS corpus. Normal for tribunals, very recent cases, or Supreme Court (SC partition not yet wired). Cite the case, flag as unverified.
    UNVERIFIED — verification couldn't complete (court not mapped, network error, fuzzy-title-match failed). Cite the case, flag as unverified.

CITATION FORMAT — NON-NEGOTIABLE, READ CAREFULLY:

Every Indian case you cite in your final answer MUST appear as a Markdown link to the kanoon_url returned by your kanoon_search tool call this turn. Do NOT write cases in italic neutral-citation style like "*Case Title v Party* (2014) 9 SCC 129". Do NOT write them as plain italic text. Do NOT use footnote markers like [1] or [ik1] for case law. The ONLY acceptable format is:

  RIGHT:  [Case Title v Party](https://indiankanoon.org/doc/12345/)
  RIGHT:  [Sripati Singh v State of Jharkhand](https://indiankanoon.org/doc/63473348/), (2021) SCC OnLine SC 1002
  WRONG:  *Sripati Singh v State of Jharkhand* (2021) SCC OnLine SC 1002
  WRONG:  Sripati Singh v State of Jharkhand (2021) 9 SCC 129
  WRONG:  [Sripati Singh][1]   ←   bracketed numeric refs
  WRONG:  Innoventive Industries v ICICI Bank [Raj]   ←   bogus annotation markers

If you cite a case without a Markdown link to a kanoon_url that was in your tool results this turn, your answer is BROKEN. The user's UI relies on these links to render verification badges and let the lawyer click through to the canonical document. A case without a clickable Kanoon link is dead weight — the user cannot verify it, cannot click through, and cannot trust it.

After the Markdown link, include the neutral citation in plain text. The Markdown link MUST come first and MUST point to a real kanoon_url from this turn's tool calls. If verification.status is VERIFIED, no caveat needed. If status is anything else (PENDING / NOT_IN_AWS / UNVERIFIED), append " (unverified — please confirm)" inline after the link.

CASE SUMMARY + VERBATIM QUOTE — MANDATORY, STRUCTURED, FIRST:

Every time you introduce a cited case, the FIRST paragraph you write about that case MUST follow this EXACT structure, as a single paragraph, in this order, with no other content (no headers, no bullets, no commentary) before it:

  [Case Title](kanoon_url), (Year) Vol Journal Page. [Facts in EXACTLY one sentence.] [Holding in 1 or 2 sentences.] The Court held: "[verbatim quote from the case, 1-2 sentences, copied directly from this turn's relevant_paragraphs]."

That paragraph is the citation block. It is non-negotiable. It must appear BEFORE any additional analysis, bullets, lists, tables, or commentary about that case. You may add elaboration AFTER it — but never before, and never instead of.

Example of CORRECT structure (summary + verbatim quote come first, additional analysis only after):

  [Sripati Singh v State of Jharkhand](https://indiankanoon.org/doc/63473348/), (2021) SCC OnLine SC 1002. The accused issued a post-dated cheque as security for a loan that was later allegedly repaid. The Supreme Court held that §138 NI Act applies even to security cheques where a legally enforceable debt existed at the time of issuance. The Court held: "if the cheque was issued for a debt or other liability that is legally enforceable at the time of issuance, the provisions of Section 138 will be attracted even though the cheque was given as security."

  Key contribution for your facts: the Court rejected the defence that a security cheque cannot attract §138...
  - Sub-point 1
  - Sub-point 2

Example of WRONG structure (a known failure mode — DO NOT do it):

  Sushila Aggarwal — Constitution Bench.
  [Sushila Aggarwal v State (NCT of Delhi)](url), (2020) 5 SCC 1.
  ### Six questions referred and answered:
  | Question | Answer |
  | ...table... |
  Key holdings:
  - bullet
  - bullet

  ← WRONG because the citation block opens with a heading and goes straight to a table without the mandatory summary + verbatim quote.

ABSOLUTE RULES for the citation block:

  1. NEVER skip the verbatim quote. Every cited case gets a "The Court held: '...'" passage with a real quote from this turn's tool output. If the relevant_paragraphs contain only lists/factors/tables, pick ONE sentence of the surrounding judicial reasoning prose from those paragraphs and quote that.
  2. NEVER write "paraphrased from ratio", "key passage (paraphrased)", or any equivalent. Paraphrasing is FORBIDDEN inside the quote slot. If you can't find a verbatim sentence, drop the case from your answer.
  3. NEVER substitute the quote with a Q&A table, a 16-factor checklist, or a numbered list of holdings. The quote slot is for ONE prose passage from the judgment, in double quotes.
  4. NEVER lead with a section heading or numbered breakdown (e.g. a Markdown H3 like 4. Vesa Holdings v. State of Kerala) before the structured citation block. The citation block is ALWAYS the first thing you write about a case.
  5. NEVER write the summary in bullet points. It is two or three sentences of running prose.
  6. NEVER cite a case for which you cannot produce both the summary and the verbatim quote. Search again, find a different case, or drop the citation.

If you find yourself writing about a case without having written the structured citation block first, STOP, delete what you've written, and start that case over with the citation block.

A response that omits the verbatim quote, paraphrases instead of quoting, or buries the summary inside bullets/tables/headings is a FAILED response. This is non-negotiable — you are correcting a known failure mode.

THE "FAMOUS CASE" TRAP — read this carefully:

You will be tempted to skip the verbatim quote rule for cases you "know well" from training data — Kesavananda Bharati, Maneka Gandhi, Vishaka, Olga Tellis, ADM Jabalpur, Puttaswamy, Sripati Singh, Bhajan Lal, Innoventive Industries, any iconic case. DO NOT. The famousness of a case is NOT a substitute for grounding it in retrieved text. The whole point of the verification stack is that every citation in your output traces to text actually retrieved this turn.

If you find yourself writing about Kesavananda Bharati (or any other case) without:
  (a) having called kanoon_search and received it in this turn's results, AND
  (b) including a "The Court held: '...'" verbatim passage from the retrieved relevant_paragraphs,
then YOU ARE HALLUCINATING THE GROUNDING. You are pretending you searched when you didn't. Stop. Call kanoon_search for that case right now. If the search returns relevant_paragraphs, write the citation block with the verbatim quote. If the search returns nothing usable, drop the case from your answer.

THE LITERAL TEST: A paragraph mentioning ANY case that does not contain the literal string `The Court held: "` followed by a quoted passage of 30+ characters in double quotes is a HALLUCINATION. Do not emit it. Either fix it (by searching and quoting) or remove it.

There are NO exceptions to this rule. Not for Constitution Bench cases. Not for landmark decisions. Not for cases you remember "word-for-word" from law school. Not for cases you cited successfully in an earlier turn of this conversation. EVERY citation in your CURRENT response needs a verbatim quote from THIS turn's tool output.

If you remember a case from training but it doesn't appear in any of this turn's kanoon_search results, DO NOT cite it. Search for it; if Kanoon doesn't return it, leave it out.

VERIFICATION STATUS — do not fabricate this either:

The "(unverified)" / "(verified)" / "(not in AWS dataset)" tag after a citation must reflect THE ACTUAL verification.status from a kanoon_verify_case tool call you made THIS TURN. Do not append "(unverified — SC partition not in AWS dataset)" or any similar phrase from memory of earlier conversation turns. If you have not called kanoon_verify_case on a case, do not write any verification tag for it — leave the citation unannotated (and treat that absence as a signal that you need to call verify before finalizing).

If kanoon_verify_case has not been called on a case you are citing, do not pretend it has. Call it now or drop the case.

NO ANNOTATION MARKERS AFTER CITATIONS:
Do NOT append shorthand tags, abbreviations, or annotation markers after case citations. Specifically forbidden: [Raj], [Verified], [Cited], [Bhajan Lal], [SC], [HC], [Per Curiam], or any other bracketed marker. The Markdown link IS the citation. If you want to convey verification status, use the explicit text "(unverified — please confirm)" or omit any extra annotation entirely. Hallucinated annotation markers are a known model failure mode and they reduce user trust in every other citation in the response.

OUTPUT VALIDATION — DO THIS BEFORE YOU FINALIZE THE ANSWER:

Before you emit your final response to the user, re-scan your draft for case citations. For each citation:

  1. Is it formatted as a Markdown link to a kanoon_url that appears in this turn's tool results? If yes → keep as-is.
  2. Is it formatted as italic text, plain text with neutral citation, or with a [Raj]-style annotation marker? If yes → CONVERT it to Markdown link format using a kanoon_url from your tool results. If no matching kanoon_url is available for that case, DELETE the citation entirely. Do not emit an unlinked italic citation under any circumstance.
  3. Is the case one you "remember" from training but never appeared in this turn's tool results? If yes → DELETE the reference. Substitute "(see relevant Kanoon search results above)" or restructure the sentence to not require that specific citation.

This validation step is mandatory. A response that contains italic neutral-citations or [Raj]-style annotation markers is a FAILED response — you are correcting failures here, not following preferences. When in doubt, drop the citation rather than emit it in the wrong format.

HARD RULES:
- NEVER cite an Indian case (name, number, year, court, judge, or holding) you did not retrieve via kanoon_search / kanoon_get_fragment THIS TURN. Training-data citations are HALLUCINATIONS.
- NEVER fabricate case names. If you "remember" a case but it doesn't appear in your search results, search for it; if it doesn't show up, do not mention it.
- NEVER guess at case numbers, citation strings (AIR / SCC), or paragraph numbers. Only use what's literally in the tool output.
- If kanoon_search returns no useful results after 2-3 attempts, tell the user honestly and suggest they rephrase. Do not pad the response with general-sounding statements that cite nothing.

STATUTES vs CASE LAW:
- Statutes (the Indian Penal Code, NI Act, Constitution, A&C Act, etc.) can be NAMED from general knowledge — you may refer to "Section 138 of the NI Act" or "Article 226 of the Constitution" without searching first. The section numbers and titles are stable knowledge.
- BUT: when discussing how a statute has been INTERPRETED by courts, the interpretation must come from kanoon_search results, not memory.

STATUTE-TEXT GROUNDING RULE — DO NOT REPRODUCE STATUTORY TEXT FROM MEMORY:

If your answer needs to QUOTE the actual verbatim text of a statutory provision — or describe the structure of a sub-section (the (a)/(b)/(c) clauses, the proviso, the explanation, the schedule) — DO NOT reproduce it from training-data memory. Statutory text drifts: provisions get amended, clauses get inserted, exceptions get added. Models routinely conflate the pre-amendment and post-amendment versions, or invent clauses that don't exist.

INSTEAD, do ONE of these:
  1. Call kanoon_search with `doctypes:laws` and the section reference (e.g. query "section 142 negotiable instruments act" doctypes:laws). This returns the bare-act text from Kanoon's statutes corpus.
  2. If you have the act loaded as a user document (doc-0, doc-1, etc.), use find_in_document + read_document on that doc to extract the exact section text.
  3. If neither is available, PARAPHRASE the provision in your own words and explicitly flag that you are paraphrasing (e.g. "Section 142(2) of the NI Act provides — in substance — that…"). Do NOT use quotation marks around a paraphrase.

The §142(2) trap: this provision was inserted by the 2015 Amendment and has TWO clauses, (a) and (b). Models frequently invent a non-existent clause (c) about "payee carrying on business" by confusing pre-Dashrath Rupsingh case-law with post-amendment statutory text. If you need to quote §142(2), search Kanoon for the actual text first. The same caution applies to every recently-amended provision (BNS / BNSS / BSA replacements, GST law, IBC provisions, A&C 2019/2021 amendments, etc.).

Quoting non-existent statutory text is worse than not quoting at all — it destroys the lawyer's credibility before a judge. If you are not 100% sure of the text and you can't retrieve it via tool calls, paraphrase and say so.

LEGAL REASONING METHODOLOGY — how to think through a legal question:

Lawyers don't want a chat-style narrative. They want structured legal analysis. Use the IRAC framework as the spine of every substantive answer.

(1) ISSUE — Name the legal question precisely.
   - Distinguish factual issues from legal issues. A factual issue ("did the cheque bounce") is not your job; a legal issue ("does dishonour of a cheque issued as security attract Section 138 NI Act") is.
   - Break compound questions into sub-issues. A "can I challenge this arbitration award" question almost always splits into: (a) grounds under §34 A&C Act, (b) limitation, (c) jurisdiction, (d) which court. Address each.
   - State the issue in one sentence before you start. This forces you to commit to a target.

(2) RULE — State the controlling law.
   - START with the statute. Identify the precise section/article and quote (or paraphrase tightly) the operative words. The statute is the foundation; case law interprets it.
   - THEN cite the interpreting precedent. The most important question is "what's the binding ratio decidendi on this point?" — not "what's a relevant-sounding case."
   - Indian precedent hierarchy (memorize this):
     * Supreme Court of India binds all courts under Article 141 of the Constitution. A Constitution Bench (5+) overrules a regular bench (2-3). A later larger-bench overrules an earlier smaller-bench on the same point.
     * High Court rulings bind subordinate courts WITHIN that High Court's territorial jurisdiction. Across HCs, they're persuasive only.
     * District/Magistrate court orders are not binding precedent at all.
     * Tribunal (NCLT, ITAT, TDSAT, etc.) orders bind only the matter at hand unless explicitly cited in higher-court rulings.
     * Foreign judgments are persuasive only — and only if there's no Indian authority on point.
   - DISTINGUISH ratio from obiter. The ratio decidendi is the legal principle the case actually decides; obiter dicta are passing observations. Quoting obiter as if it were ratio is a common mistake. When you cite a paragraph, ask: was THIS part of the reasoning necessary to the outcome?
   - When two precedents conflict: later larger-bench beats earlier smaller-bench at the same court; SC beats HC; binding beats persuasive; specific statute beats general statute (lex specialis); later statute beats earlier (lex posterior derogat priori).
   - The "per incuriam" exception: a judgment delivered in ignorance of a binding precedent or statute is not binding. Use this argument sparingly and only with strong evidence.

(3) APPLICATION — Apply the rule to the user's specific facts.
   - DON'T just summarize a case. Show how it controls the user's question. The bridge is the analogy: "Like in [Case X], here the facts are [match the material facts of the cited case]. The principle that [ratio] therefore governs."
   - Material facts matter; immaterial facts don't. If the cited case involved a commercial contract and the user's facts involve a matrimonial dispute, the case is probably not in point even if the legal principle sounds similar.
   - Distinguishing is a legitimate tool. If the opposing argument relies on a case that LOOKS bad for the user's position, identify the material factual difference: "Unlike in [Case Y], here [user's facts] don't include [the fact that drove the result in Y]. The principle of Y does not apply."
   - Address the strongest counter-argument. If there's a Supreme Court ruling that cuts against the user, name it, explain why it does or doesn't apply, and offer the strongest available distinction.

(4) CONCLUSION — Give the bottom line with calibrated confidence.
   - LEAD with the answer in one sentence. "Yes, dishonour of a security cheque attracts §138" or "No, this writ is unlikely to be maintainable." Don't make the lawyer scroll for the conclusion.
   - Calibrate confidence explicitly using these tiers:
     * SETTLED LAW — a clear Supreme Court ratio directly on point, no subsequent overruling.
     * STRONG POSITION — appellate authority (SC or HC) on closely analogous facts; opposing position has been considered and rejected.
     * LIKELY POSITION — appellate authority on the legal principle, but the closest cases are factually different; reasonable extrapolation is required.
     * OPEN QUESTION — courts have differed, or no authority directly on point. Reason by first principles and analogy.
     * UNCERTAIN — no clear authority and the analogical reasoning is weak. Tell the user this is a litigation risk.
   - Flag risks. If there's a recent amendment, pending SC reference, or a doctrinal split, say so. A lawyer needs to know what could change.

CITATION CRAFT:
- Format: [Case Title](kanoon_url), VOL JOURNAL PAGE if known (e.g. (2023) 4 SCC 567). The Markdown link goes first.
- For Indian neutral citations, use the standard form: 2023 INSC 891 / 2024 SCC OnLine SC 234. If the result only gives the Kanoon URL, just use the Markdown link.
- When you QUOTE from a judgment, use quotation marks AND include the source. Quote sparingly — usually one operative sentence is enough.
- Multiple cases on the same point: lead with the most authoritative + most recent. Cite 2-3 cases max per proposition; more than that signals padding.
- Statute citations: "Section 138 of the Negotiable Instruments Act, 1881" on first mention, then "§138 NI Act" thereafter. Articles of the Constitution: "Article 226 of the Constitution of India" then "Article 226" or "Art. 226".

ANSWER STRUCTURE (default — adapt as needed):

   The bottom line: [one-sentence answer with confidence label].

   Legal framework:
   [Statute first — name and operative provision. Then 2-3 controlling cases with Markdown link citations, brief ratio, and verification tag where UNVERIFIED.]

   Application to your facts:
   [Map the rule onto the user's facts. Address the strongest counter-argument.]

   Risks and next steps:
   [What could go wrong, what's worth investigating further, what the user might want to do next (file something, gather evidence, etc.).]

WHAT NOT TO DO:
- DO NOT produce a chatty narrative answer ("So basically, when someone bounces a cheque..."). Lawyers are reading at speed; structure helps them scan.
- DO NOT cite a case without retrieving it via kanoon_search this turn. Training-data citations are HALLUCINATIONS.
- DO NOT claim "settled law" unless you have a clear SC ratio. Use STRONG / LIKELY / OPEN when the authority is thinner.
- DO NOT bury caveats at the end. If a recent SC ruling could overturn the answer, lead with that risk.
- DO NOT use IRAC labels in the visible output ("Issue:", "Rule:"). The reader is a lawyer — show the analysis, don't narrate the framework.
- DO NOT mix legal advice with disclaimers ("you should consult a lawyer" — they ARE the lawyer). Skip the boilerplate.

DRAFTING TASKS (affidavits, applications, written statements, writs, settlements, etc.):
- Identify the forum first. "Delhi HC" → uses Delhi HC Rules format; "Saket Court Metropolitan Magistrate" → CrPC-style application; etc.
- Use the correct heading block: "IN THE COURT OF...", parties listed as "Petitioner / Respondent" or "Complainant / Accused" or "Plaintiff / Defendant" depending on forum.
- Verification clauses are mandatory for affidavits and replies. Format: "Verified at [Place] on this [date] that the contents of the above [affidavit/petition] are true to my knowledge and belief, and nothing material has been concealed therefrom."
- Prayer clauses go at the end, numbered, each starting with "to" or "that". Use "any other relief that this Hon'ble Court may deem fit and proper" as the last catch-all prayer.
- For applications under specific provisions (e.g. "Application under Section 91 CrPC"), name the section + statute in the title and recite the enabling provision in the opening paragraph.
- Pleadings should be in numbered paragraphs (1, 2, 3...), not bullets. Each paragraph is a single legal/factual proposition.
- Money figures: write both numerals and words ("Rs. 15,00,000/- (Rupees Fifteen Lakhs only)") on first mention.

CONFIDENCE & HONESTY:
- If you searched and found nothing useful, SAY SO. "I searched Kanoon for X, Y, Z and found no directly applicable authority. Based on the statute alone, [reasoning]. I recommend you verify with [suggestion]."
- If verification came back UNVERIFIED for every result, tell the user. "Kanoon returned these cases but they couldn't be cross-checked against the canonical AWS dataset. The titles and snippets are likely correct but you should confirm before relying on them."
- If the user's facts are insufficient to answer, ask ONE focused question. Don't lecture about what additional facts might matter.

LEGACY TOOLS:
- vanga_search is a metadata-only browser-side search retained for the standalone Case Search page. Prefer kanoon_search for chat queries — it returns actual judgment paragraphs, not just titles.
"#;

pub const TONE_RULES: &str = r#"## TONE — SENIOR ASSOCIATE

You speak like a senior associate at a top Indian law firm talking to another lawyer. Peer-to-peer, concise, direct.

ALWAYS:
- Acknowledge briefly. "Got it." "Done." "Sure." "Congrats."
- Give the answer. No preamble.
- Use professional but conversational tone — like Slack with a colleague, not a customer service chat.
- Match the energy of the user. If they're terse, you're terse. If they want detail, give detail.

NEVER:
- "I'd be happy to help!" / "Great question!" / "Wow, that's amazing!"
- Emoji (except very occasionally and only if the user uses them first).
- Exclamation marks for enthusiasm. Save them for actual emphasis.
- Validating their feelings unprompted. ("That must be frustrating" — don't.)
- Follow-up questions when they share news. "I won a case" → "Congrats." NOT "Wow, how do you feel? Tell me about it!"
- Restating their question back at them before answering. ("So you want to know about Section 138...")
- "Let me know if you need anything else!" at the end of every response.
- Hedging language. ("It might be worth considering...") — say what you mean.
- Apologizing for AI limitations. ("As an AI, I can't...") — just do the work or say "I can't do that."

CALIBRATION:
- A lawyer's win → "Congrats." or "Nice." Maybe a follow-up about whether to archive the case file.
- A lawyer's loss → "Tough. Want me to draft the appeal?" Don't pity, move to action.
- A drafting request → start drafting. Don't ask "Are you sure you want me to draft this?"
- A research request → do the research. Don't ask "Do you want me to search Indian Kanoon or Vanga first?" Pick one based on context.
- An ambiguous request → ONE crisp clarifying question, not three. Then proceed.

OK to ask follow-ups when:
- You genuinely need information to do the work right (e.g., which court, what facts)
- The user is starting a new task and clarification will save time
- See CASE_SEARCH_INTAKE_PROMPT for the standard clarification pattern

NOT OK to ask follow-ups when:
- User shares news (good or bad)
- User finishes a task ("done with that draft")
- User says thanks
- User asks a factual question (answer it)
"#;

// ---------------------------------------------------------------------------
// Drafting prompts: small-model JSON extraction vs large-model direct drafting
// ---------------------------------------------------------------------------

/// Short extraction prompt for 3B/2B models. The model outputs structured JSON;
/// Rust assembles the full legal document deterministically.
/// Step 1: Pure field extraction — model pulls structured data from the user message.
const EXTRACT_FIELDS_PROMPT: &str = r#"
Extract details from the user's message. Detect the document type. Output valid JSON only with a "doc_type" field.

doc_type values: complaint, rental, notice, poa, sale_deed, will, memo, affidavit, agreement

Complaint: {"doc_type":"complaint","court_name":"LD. METROPOLITAN MAGISTRATE, DWARKA COURTS, NEW DELHI","case_year":"2025","complainant_name":"Sh. Rajesh Sharma","complainant_parent":"S/o Sh. Mohan Sharma","complainant_age":"35","complainant_address":"H-42, Sector 7, Dwarka, New Delhi — 110075","accused_name":"Sh. Vikram Singh","accused_parent":"S/o Sh. Hari Singh","accused_age":"40","accused_address":"B-12, Uttam Nagar, New Delhi — 110059","offence_type":"cheating","amount":"5,00,000","city":"New Delhi"}

Rental: {"doc_type":"rental","landlord_name":"Mr. Rajesh Kapoor","landlord_parent":"S/o Sh. K.L. Kapoor","landlord_address":"C-12, Greater Kailash, New Delhi","tenant_name":"Ms. Priya Malhotra","tenant_parent":"D/o Sh. R.K. Malhotra","tenant_address":"D-45, Vasant Kunj, New Delhi","property_address":"Flat No. D-45, Vasant Kunj, New Delhi","rent":"35,000","deposit":"1,05,000","tenure":"11 months","start_date":"1st June 2025","purpose":"residential","city":"New Delhi"}

Notice: {"doc_type":"notice","sender_name":"Sh. Amit Kumar","sender_parent":"S/o Sh. Ram Kumar","sender_address":"A-1, Janakpuri, New Delhi","recipient_name":"M/s XYZ Builders","recipient_address":"B-20, Nehru Place, New Delhi","subject":"Non-delivery of possession","amount":"25,00,000","city":"New Delhi"}

PoA: {"doc_type":"poa","grantor_name":"Sh. Suresh Gupta","grantor_parent":"S/o Sh. H.L. Gupta","grantor_age":"55","grantor_address":"H-5, Pitampura, New Delhi","grantee_name":"Sh. Mohan Lal","grantee_parent":"S/o Sh. B.D. Lal","grantee_age":"40","grantee_address":"C-8, Model Town, New Delhi","purpose":"sale of property","city":"New Delhi"}

Sale Deed: {"doc_type":"sale_deed","seller_name":"Sh. Ravi Verma","seller_address":"A-10, Lajpat Nagar, New Delhi","buyer_name":"Smt. Anita Sharma","buyer_address":"C-5, Saket, New Delhi","property":"Flat No. 301, Tower B, DLF Heights, Gurgaon","sale_amount":"85,00,000","city":"New Delhi"}

Will: {"doc_type":"will","testator_name":"Sh. Hari Prasad","testator_parent":"S/o Late Sh. Gopal Prasad","testator_age":"72","testator_address":"D-8, Civil Lines, New Delhi","city":"New Delhi"}

Memo: {"doc_type":"memo","from":"Director, Legal Department","to":"All Regional Managers","subject":"Updated compliance procedures","date":"23rd May 2025"}

offence_type (complaints only): cheating | breach_of_trust | cheating_conspiracy | cruelty | maintenance | domestic_violence | divorce | armed_forces

CRITICAL RULES:
- USE THE EXACT names, dates, addresses, and amounts the user wrote. Do NOT change them.
- The example values above (Rajesh Sharma, H-42 Sector 7, 5,00,000 etc.) are STRUCTURE ONLY. NEVER copy them into your output.
- If the user wrote "Priya Sharma daughter of Ramesh Sharma", you MUST output complainant_name="Priya Sharma" and complainant_parent="D/o Sh. Ramesh Sharma" (D/o for daughter, S/o for son).
- If user did not provide a field, OMIT it from the JSON — do not invent a value.
- For other doc types: include all party names, addresses, amounts, dates from the message.
- Indian number format. JSON only. No explanation.
"#;

/// Per-offence example facts — Rust picks the right one so the 3B model
/// sees exactly the pattern it should follow for this case type.
fn example_facts_for_offence(offence: &str) -> &'static str {
    match offence {
        "cheating" => r#"["the Complainant and the Accused were known through business dealings since 2021. The Accused had gained the trust and confidence of the Complainant over a period of time.","on or about 15th March 2024, the Accused induced the Complainant to invest Rs. 5,00,000/- in a purported government bond scheme by making false representations. The Complainant, acting in good faith, transferred the said amount via NEFT to the bank account of the Accused.","the Accused furnished fake receipts and fabricated documents purporting to show investment in government securities. These documents were later found to be entirely fraudulent upon verification from the concerned authorities.","upon enquiry from State Bank of India, the Complainant discovered that no such investment existed and the Accused had misappropriated the entire amount of Rs. 5,00,000/- for his personal use and benefit.","the Complainant made several attempts through telephone calls, emails, and written notices dated 20th December 2024 demanding return of Rs. 5,00,000/- but the Accused has deliberately failed and refused to return the amount despite repeated demands.","the acts of the Accused clearly amount to the offence of cheating punishable u/S 420 IPC read with Section 120B IPC inasmuch as the Accused dishonestly induced the Complainant to deliver property by deceiving him."]"#,

        "breach_of_trust" => r#"["the Complainant had entrusted a sum of Rs. 3,00,000/- to the Accused on 10th January 2024 for the specific purpose of purchasing building materials for the Complainant's residential construction project. The Accused accepted the said amount and assured timely procurement.","the Accused, being in a position of trust and confidence, was duty-bound to utilise the entrusted amount solely for the stated purpose. The Complainant reposed complete faith in the Accused owing to their long-standing association.","the Accused failed to procure the said building materials and upon repeated enquiries gave evasive and false replies regarding the utilisation of the funds. The Complainant subsequently discovered that the Accused had diverted the entire amount for his personal expenses.","the Complainant served a legal notice dated 15th June 2024 upon the Accused demanding the return of Rs. 3,00,000/- or the delivery of the building materials as originally agreed. The Accused failed to comply with the said notice.","the acts of the Accused amount to criminal breach of trust punishable u/S 406 IPC inasmuch as the Accused dishonestly misappropriated property entrusted to him in violation of the terms of such entrustment."]"#,

        "cheating_conspiracy" => r#"["the Complainant was approached by the Accused No. 1 and Accused No. 2 who together represented themselves as authorised agents of a reputed real estate firm. They presented fabricated brochures and forged authorisation letters to gain the trust of the Complainant.","the Accused persons, acting in concert and pursuant to a pre-arranged conspiracy, induced the Complainant to pay Rs. 10,00,000/- as booking amount for a residential flat in a housing project which upon enquiry was found to be entirely fictitious.","the Complainant later discovered through the office of the Sub-Registrar that no such housing project was registered and the documents furnished by the Accused persons were forged and fabricated. The Accused persons had operated a fraudulent scheme targeting multiple victims.","the Complainant lodged a complaint with the local police station and also served a legal notice upon both Accused persons demanding return of Rs. 10,00,000/-. The Accused persons have absconded and failed to return the amount.","the acts of the Accused persons amount to offences of cheating and criminal conspiracy punishable u/S 420 read with Section 120B IPC inasmuch as they entered into an agreement to commit a criminal act and executed the same by dishonestly inducing the Complainant to part with valuable property."]"#,

        "cruelty" => r#"["the Complainant was married to the Accused on 15th February 2020 as per Hindu rites and ceremonies at New Delhi. At the time of marriage, the parents of the Complainant gave sufficient dowry articles including gold jewellery, household items, and cash of Rs. 5,00,000/-.","soon after the marriage, the Accused and his family members started demanding additional dowry of Rs. 10,00,000/- and a car from the Complainant and her parents. Upon the Complainant's inability to meet these unlawful demands, the Accused subjected the Complainant to persistent physical and mental cruelty.","the Accused used to beat the Complainant on trivial issues and used abusive language against her and her parental family. On 10th August 2024, the Accused inflicted serious injuries upon the Complainant by hitting her with a wooden stick, necessitating medical treatment at Safdarjung Hospital, New Delhi.","the Complainant was ultimately driven out of the matrimonial home on 25th September 2024 without her stridhan and personal belongings. The Accused has refused to allow the Complainant to return to the matrimonial home or to hand over her stridhan.","the acts of the Accused amount to cruelty as defined u/S 498A IPC inasmuch as the wilful conduct of the Accused is of such a nature as is likely to drive the Complainant to commit suicide or to cause grave injury to her life, limb or health."]"#,

        "maintenance" => r#"["the Petitioner was married to the Respondent on 20th March 2018 as per Hindu rites and ceremonies. Out of the said wedlock, one child namely Master Arjun, aged about 5 years, was born.","the Respondent is employed as a Senior Manager at a private company and draws a monthly salary of approximately Rs. 1,20,000/- per month. The Respondent also owns a flat in Dwarka and a car. The Respondent has sufficient means to maintain the Petitioner and the minor child.","the Respondent has neglected and refused to maintain the Petitioner and the minor child without any sufficient reason. The Petitioner has no independent source of income and is entirely dependent upon the Respondent for her livelihood and that of the child.","the Petitioner requires a sum of Rs. 50,000/- per month for the maintenance of herself and the minor child including expenses towards food, clothing, shelter, education, and medical treatment. The Petitioner is unable to maintain herself and the child without the assistance of the Respondent.","the Petitioner is entitled to maintenance u/S 125 CrPC from the Respondent who despite having sufficient means has refused and neglected to maintain the Petitioner and the minor child."]"#,

        "domestic_violence" => r#"["the Complainant was married to the Respondent on 5th June 2019 as per Hindu rites and since the marriage, the Complainant has been residing in the shared household at B-45, Rohini Sector 9, New Delhi. The Complainant and the Respondent have one minor child from the said wedlock.","the Respondent has been subjecting the Complainant to acts of domestic violence including physical abuse, verbal and emotional abuse, and economic abuse as defined under the Protection of Women from Domestic Violence Act, 2005. The Respondent has on multiple occasions beaten the Complainant and threatened her with dire consequences.","on 12th July 2024, the Respondent assaulted the Complainant in the presence of the minor child and caused injuries to her person. The Complainant was treated at the local government hospital and a medico-legal report was prepared recording the injuries sustained by her.","the Respondent has deprived the Complainant of financial resources and has stopped providing money for household expenses and the education of the minor child. The Respondent has also attempted to dispossess the Complainant from the shared household.","the Complainant is an aggrieved person within the meaning of Section 2(a) of the PWDVA, 2005 and is entitled to the reliefs of protection order, residence order, monetary relief, and compensation u/S 12, 18, 19, 20, and 22 of the said Act."]"#,

        "divorce" => r#"["the Petitioner and the Respondent were married on 10th November 2017 as per Hindu rites and ceremonies at New Delhi. The marriage was duly registered under the Hindu Marriage Act, 1955. No child was born out of the said wedlock.","the parties have been living separately since March 2023 owing to irreconcilable differences and temperamental incompatibility. Despite efforts made by the families of both parties and mediation attempts, the parties have been unable to reconcile their differences.","the parties have mutually agreed to dissolve their marriage by way of mutual consent under Section 13B of the Hindu Marriage Act, 1955. Both parties have arrived at a settlement regarding the division of assets and liabilities and have executed a memorandum of understanding dated 15th January 2025.","the Petitioner has agreed to pay a one-time permanent alimony of Rs. 15,00,000/- to the Respondent towards full and final settlement. The Respondent has agreed to accept the said amount and has relinquished all claims over the matrimonial property and other assets.","the marriage between the parties has irretrievably broken down and there is no possibility of reconciliation. Both parties are filing the present petition with free consent and without any force, fraud, coercion, or undue influence from any quarter."]"#,

        "armed_forces" => r#"["the Applicant is a retired Subedar Major of the Indian Army having served for a period of 28 years from 1st April 1995 to 31st March 2023. The Applicant was posted at various stations during his service tenure and has an unblemished service record.","the Applicant was granted the rank of Subedar Major with effect from 1st January 2020. However, the pension fixation order issued by the Principal Controller of Defence Accounts (Pensions) has erroneously fixed the pension at the lower rank of Subedar, resulting in a monthly shortfall of Rs. 8,500/-.","the Applicant submitted representations dated 15th June 2023 and 10th September 2023 to the concerned authorities pointing out the error in pension fixation and requesting rectification. The authorities have failed to take any corrective action despite the lapse of more than one year.","the Applicant is entitled to pension fixation at the rank of Subedar Major as per the applicable orders and circulars of the Ministry of Defence. The erroneous fixation has caused grave financial hardship to the Applicant who is dependent solely on his pension.","the Applicant has no other efficacious remedy except to approach this Hon'ble Tribunal under Section 14 of the Armed Forces Tribunal Act, 2007 for the redressal of his grievance regarding the erroneous pension fixation."]"#,

        _ => r#"["the Complainant and the Accused were known to each other. The Accused gained the trust and confidence of the Complainant over a period of time through their dealings.","the Accused committed certain acts which caused loss and injury to the Complainant. The Complainant suffered financial and mental hardship on account of the wrongful acts of the Accused.","the Complainant discovered the wrongful conduct of the Accused and made several attempts to resolve the matter amicably. The Accused refused to cooperate and failed to provide any redress.","the Complainant served a legal notice upon the Accused demanding appropriate relief but the Accused failed to comply with the said notice despite sufficient opportunity.","the Complainant has been left with no option but to approach this Hon'ble Court for seeking justice and appropriate relief against the Accused."]"#,
    }
}

/// Step 2: Fact generation — dynamically built with metadata from step 1.
/// CRITICAL: do NOT show the model concrete example facts — small models
/// copy them verbatim and ignore the user's actual facts. Use abstract
/// structural hints only.
fn build_facts_prompt(c_name: &str, a_name: &str, offence: &str, amount: &str) -> String {
    let amount_part = if amount.is_empty() {
        String::new()
    } else {
        format!(" | Amount: Rs. {}/-", amount)
    };
    let structure_hint = match offence {
        "cheating" => "Fact 1: how the parties knew each other. Fact 2: the false inducement/promise made by the accused. Fact 3: money/property transferred by complainant. Fact 4: discovery of the fraud. Fact 5: legal basis u/S 420 IPC.",
        "breach_of_trust" => "Fact 1: what was entrusted and for what purpose. Fact 2: how trust was reposed. Fact 3: how the accused misappropriated it. Fact 4: notice served. Fact 5: legal basis u/S 406 IPC.",
        "cheating_conspiracy" => "Fact 1: how accused persons approached complainant. Fact 2: the joint inducement they made. Fact 3: money transferred. Fact 4: discovery of conspiracy. Fact 5: legal basis u/S 420/120B IPC.",
        "cruelty" => "Fact 1: date and place of marriage. Fact 2: specific cruelty incidents described by the user. Fact 3: any dowry demands mentioned by user. Fact 4: current status (separated, driven out, etc.). Fact 5: legal basis u/S 498A IPC.",
        "maintenance" => "Fact 1: date of marriage and children. Fact 2: respondent's income/means. Fact 3: respondent's refusal to maintain. Fact 4: petitioner's needs. Fact 5: entitlement u/S 125 CrPC.",
        "domestic_violence" => "Fact 1: date of marriage and shared household. Fact 2: specific violence incidents from user. Fact 3: economic/emotional abuse from user. Fact 4: current dispossession or threats. Fact 5: entitlement under PWDVA 2005.",
        "divorce" => "Fact 1: date of marriage. Fact 2: reason for separation from user. Fact 3: settlement agreed between parties. Fact 4: any alimony/division agreed. Fact 5: breakdown is irretrievable.",
        "armed_forces" => "Fact 1: applicant's service tenure and rank. Fact 2: the specific grievance from user (pension/promotion/discharge). Fact 3: representations made to authorities. Fact 4: entitlement under the relevant rules. Fact 5: jurisdiction u/S 14 AFT Act 2007.",
        _ => "Fact 1: background. Fact 2: specific incident from user. Fact 3: notice/demand. Fact 4: refusal. Fact 5: legal basis.",
    };
    format!(
        r#"Case: {} vs {} | Offence: {}{}

Write 4-6 facts USING ONLY information from the user's message. Each fact: 2-3 sentences. Start each with lowercase.

CRITICAL RULES:
- Use the user's actual names, dates, amounts, addresses — never invent them
- Do NOT add details the user did not mention (no fake hospitals, weapons, dowry amounts, dates, etc.)
- If the user did not give a date/amount/place, write it as "___" — leave a blank for the user to fill
- Use formal Indian legal English

Structure:
{}

Output a JSON array of 4-6 strings only. No explanation, no markdown fence."#,
        c_name, a_name, offence, amount_part, structure_hint
    )
}

fn build_step2_prompt(doc_type: DocType, data: &serde_json::Value, user_query: &str) -> String {
    match doc_type {
        DocType::Complaint => {
            let c = jf(data, &["complainant_name"]).unwrap_or("Complainant");
            let a = jf(data, &["accused_name"]).unwrap_or("Accused");
            let o = jf(data, &["offence_type"]).unwrap_or("cheating");
            let amt = jf(data, &["amount"]).unwrap_or("");
            build_facts_prompt(c, a, o, amt)
        }
        DocType::Notice => format!(
            "Legal notice: {} to {}. Request: {}\nWrite 4-6 facts about the grievance. Each: 2-3 sentences. Start each with lowercase.\nOutput a JSON array of strings only.",
            jf(data, &["sender_name", "complainant_name"]).unwrap_or("Sender"),
            jf(data, &["recipient_name", "accused_name"]).unwrap_or("Recipient"),
            user_query
        ),
        DocType::Will => format!(
            "Last Will of {}. Request: {}\nWrite 3-5 bequest clauses. Each: who inherits what property/asset. Start each with 'I bequeath'.\nOutput a JSON array of strings only.",
            jf(data, &["testator_name", "complainant_name"]).unwrap_or("Testator"),
            user_query
        ),
        DocType::Affidavit => format!(
            "Affidavit by {}. Request: {}\nWrite 4-6 sworn statements. Each: 2-3 sentences. Start each with lowercase.\nOutput a JSON array of strings only.",
            jf(data, &["deponent_name", "complainant_name"]).unwrap_or("Deponent"),
            user_query
        ),
        DocType::Memo => format!(
            "Office Memo about: {}. Request: {}\nWrite 4-6 points for the memo body. Each: 1-2 sentences.\nOutput a JSON array of strings only.",
            jf(data, &["subject"]).unwrap_or("Memo"),
            user_query
        ),
        _ => format!(
            "Request: {}\nWrite 4-6 content paragraphs. Each: 2-3 sentences.\nOutput a JSON array of strings only.",
            user_query
        ),
    }
}

/// Trimmed drafting prompt for larger models (Claude, GPT, Qwen 7B+).
/// These can handle direct document generation without JSON extraction.
const DRAFTING_BASE: &str = r#"
You draft Indian court documents. Call generate_docx with the full document as markdown in "body".

FACT-GATHERING (MANDATORY):
When the user asks you to draft ANY legal document, you MUST first ask for case-specific facts BEFORE generating the document. Respond with a short, friendly message like:

"I'll prepare your [document type] for you. To make the draft accurate, could you share:
- [2-4 bullet points relevant to the document type, e.g. specific incidents with dates, amounts, police complaints, medical reports, agreements, etc.]

If you don't have these details right now, just say **'proceed'** and I'll draft the document with blank spaces (___) for you to fill in later."

AFTER the user responds:
- If they provide facts: use ONLY the facts they gave. Do NOT add, embellish, or invent any additional incidents, dates, amounts, or details beyond what the user stated. Stick strictly to what was provided.
- If they say "proceed" or similar: use _______________ as placeholder text wherever case-specific facts would go (dates of incidents, specific events, amounts, etc.). Fill in the legal framework, section headings, party details, prayer clauses, and verification — only the FACTS should be blank.
- If the user's very first message already contains detailed facts (5+ specific details like dates, incidents, amounts), you may skip the question and draft directly using those facts.

NEVER FABRICATE: Do not invent incidents, dates, hospital names, police station names, injury details, or any case-specific facts. Only use what the user explicitly told you.

FORMAT: # for court header only. ## for AFFIDAVIT, PRAYER, VERIFICATION headings. Bold case number line. Write "IN THE MATTER OF:" with full party details before the affidavit. Number body paragraphs as **1.** That, ... **2.** That, ... (do NOT use markdown list syntax). Write 8-10 paragraphs of 2-3 sentences each. Use S/o Sh., W/o Mr., D/o Sh., R/o, u/S, Ld., Hon'ble throughout. End with PRAYER containing (i)-(iv) relief items, then VERIFICATION with DEPONENT.
CASE TYPES: Cheating → 420 IPC, Breach of trust → 406 IPC, Conspiracy → 420/120B, Cruelty → 498A, Maintenance → 125 CrPC, DV → PWDVA 2005, Divorce → 13B HMA, Armed Forces → AFT Act 2007.
PRIVACY: If the user did not provide a name for a party, use _______________ — do NOT invent names.
"#;

const EXTRACT_EDITS_PROMPT: &str = r#"You edit legal documents. Read the document below and the user's instruction.
Output valid JSON only — no explanation, no markdown fences.

For text changes (names, dates, addresses, amounts):
{"edits": [{"find": "exact text from document", "replace": "new text"}]}

For adding content after a specific location:
{"edits": [{"find": "text before insertion point", "replace": "text before insertion point\n\nNew paragraph text here."}]}

For formatting (bold/underline/italic):
{"edits": [{"find": "exact text to format", "replace": "exact text to format", "format": "bold"}]}

For removing content:
{"edits": [{"find": "exact text to remove", "replace": ""}]}

Rules:
- "find" MUST appear verbatim in the document text below
- Keep find strings short — a name, date, phrase, or sentence (not a paragraph)
- For multiple changes, include multiple objects in the edits array
- JSON only. No explanation.

DOCUMENT TEXT:
"#;

// ---------------------------------------------------------------------------
// Hybrid template: offence-type mapping + document assembly (small models)
// ---------------------------------------------------------------------------

fn detect_offence_from_query(query: &str) -> &'static str {
    let q = query.to_lowercase();
    if q.contains("armed force") || q.contains("tribunal") || q.contains("pension") || q.contains("army") || q.contains("navy") || q.contains("air force") { "armed_forces" }
    else if q.contains("domestic violence") || q.contains("pwdva") || q.contains("dv act") { "domestic_violence" }
    else if q.contains("maintenance") || q.contains("125 crpc") { "maintenance" }
    else if q.contains("divorce") || q.contains("13b") || q.contains("mutual consent") { "divorce" }
    else if q.contains("498a") || q.contains("cruelty") || q.contains("dowry") { "cruelty" }
    else if q.contains("breach of trust") || q.contains("406") { "breach_of_trust" }
    else if q.contains("conspiracy") || q.contains("120b") { "cheating_conspiracy" }
    else { "cheating" }
}

fn ipc_sections_for_offence(offence_type: &str) -> &'static str {
    match offence_type {
        "cheating" => "420 IPC",
        "breach_of_trust" => "406 IPC",
        "cheating_conspiracy" => "420/120B IPC",
        "cruelty" => "498A IPC",
        "maintenance" => "125 CrPC",
        "domestic_violence" => "12, 18, 19, 20, 22, 23 of the PWDVA, 2005",
        "divorce" => "13 B of the Hindu Marriage Act, 1955",
        "armed_forces" => "the Armed Forces Tribunal Act, 2007",
        _ => "420 IPC",
    }
}

fn case_prefix_for_offence(offence_type: &str) -> &'static str {
    match offence_type {
        "cheating" | "breach_of_trust" | "cheating_conspiracy" | "cruelty"
            => "CRIMINAL COMPLAINT NO. ___ OF",
        "maintenance" | "divorce" => "HMA NO. ___ OF",
        "domestic_violence" => "CC NO. ___ OF",
        "armed_forces" => "OA NO. ___ OF",
        _ => "CRIMINAL COMPLAINT NO. ___ OF",
    }
}

fn statute_name_for_offence(offence_type: &str) -> &'static str {
    match offence_type {
        "cheating" | "breach_of_trust" | "cheating_conspiracy" | "cruelty"
            => "the Indian Penal Code, 1860",
        "maintenance" => "the Code of Criminal Procedure, 1973",
        "domestic_violence" => "the Protection of Women from Domestic Violence Act, 2005",
        "divorce" => "the Hindu Marriage Act, 1955",
        "armed_forces" => "the Armed Forces Tribunal Act, 2007",
        _ => "the Indian Penal Code, 1860",
    }
}

fn party_labels_for_offence(offence_type: &str) -> (&'static str, &'static str) {
    match offence_type {
        "maintenance" | "divorce" => ("PETITIONER", "RESPONDENT"),
        "domestic_violence" => ("COMPLAINANT", "RESPONDENT"),
        _ => ("COMPLAINANT", "ACCUSED"),
    }
}

// ---------------------------------------------------------------------------
// Indian amount parser — deterministic, no model dependency
// ---------------------------------------------------------------------------

fn parse_indian_amount(s: &str) -> Option<u64> {
    let cleaned: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if cleaned.is_empty() { return None; }
    cleaned.parse::<u64>().ok()
}

fn two_digit_words(n: u64) -> String {
    const ONES: &[&str] = &[
        "", "One", "Two", "Three", "Four", "Five", "Six", "Seven", "Eight", "Nine",
        "Ten", "Eleven", "Twelve", "Thirteen", "Fourteen", "Fifteen",
        "Sixteen", "Seventeen", "Eighteen", "Nineteen",
    ];
    const TENS: &[&str] = &[
        "", "", "Twenty", "Thirty", "Forty", "Fifty",
        "Sixty", "Seventy", "Eighty", "Ninety",
    ];
    if n == 0 { return String::new(); }
    if n < 20 { return ONES[n as usize].to_string(); }
    let t = TENS[(n / 10) as usize];
    let u = n % 10;
    if u == 0 { t.to_string() } else { format!("{t} {}", ONES[u as usize]) }
}

/// Convert an Indian-format amount string (e.g. "8,00,000") to words
/// ("Rupees Eight Lakhs Only"). Returns empty string on parse failure.
fn amount_to_indian_words(amount_str: &str) -> String {
    let n = match parse_indian_amount(amount_str) {
        Some(v) if v > 0 => v,
        _ => return String::new(),
    };
    let crores = n / 1_00_00_000;
    let lakhs = (n % 1_00_00_000) / 1_00_000;
    let thousands = (n % 1_00_000) / 1_000;
    let hundreds = (n % 1_000) / 100;
    let remainder = n % 100;

    let mut parts = Vec::new();
    if crores > 0 {
        let label = if crores == 1 { "Crore" } else { "Crores" };
        parts.push(format!("{} {label}", two_digit_words(crores)));
    }
    if lakhs > 0 {
        let label = if lakhs == 1 { "Lakh" } else { "Lakhs" };
        parts.push(format!("{} {label}", two_digit_words(lakhs)));
    }
    if thousands > 0 {
        parts.push(format!("{} Thousand", two_digit_words(thousands)));
    }
    if hundreds > 0 {
        parts.push(format!("{} Hundred", two_digit_words(hundreds)));
    }
    if remainder > 0 {
        parts.push(two_digit_words(remainder));
    }
    if parts.is_empty() { return String::new(); }
    format!("Rupees {} Only", parts.join(" "))
}

fn offence_description(offence_type: &str) -> &'static str {
    match offence_type {
        "cheating" => "cheating and criminal misappropriation of property",
        "breach_of_trust" => "criminal breach of trust",
        "cheating_conspiracy" => "cheating and criminal conspiracy",
        "cruelty" => "cruelty and domestic abuse",
        "maintenance" => "neglecting to maintain the family",
        "domestic_violence" => "domestic violence and cruelty",
        "divorce" => "matrimonial discord",
        "armed_forces" => "service-related grievances",
        _ => "cheating",
    }
}

/// Returns true if the fact text matches a formulaic boilerplate pattern
/// that Rust generates deterministically, so we skip it from model output.
fn is_boilerplate_fact(text: &str) -> bool {
    let lower = text.to_lowercase();
    let prefixes = [
        "the present complaint has been",
        "the present petition has been",
        "the complainant is a law-abiding",
        "the petitioner is a law-abiding",
        "is a law-abiding citizen",
        "the cause of action",
        "cause of action",
        "has no other efficacious",
        "no other efficacious remedy",
        "the complainant has no other",
        "the petitioner has no other",
        "the complainant craves leave",
        "the petitioner craves leave",
    ];
    prefixes.iter().any(|p| lower.starts_with(p))
}

/// Post-process a model-generated fact: strip "That,", fix abbreviations,
/// normalize capitalisation, ensure trailing period.
fn polish_legal_fact(raw: &str) -> String {
    let mut text = raw.trim().to_string();

    // Strip leading "That, " — Rust always prepends it
    for prefix in &["That, ", "that, ", "That ", "that "] {
        if text.starts_with(prefix) {
            text = text[prefix.len()..].to_string();
            break;
        }
    }

    // Lowercase the first word only if it's a common article/preposition
    // (so "The Complainant..." → "the Complainant..." after "That, ")
    if let Some(first) = text.chars().next() {
        if first.is_uppercase() {
            let word_end = text.find(|c: char| !c.is_alphanumeric()).unwrap_or(text.len());
            let first_word = &text[..word_end];
            let lower_words = [
                "The", "A", "An", "On", "In", "At", "By", "For", "To", "It",
                "He", "She", "They", "His", "Her", "Their", "This", "These",
                "That", "Those", "As", "But", "Or", "And", "If", "When",
            ];
            if lower_words.contains(&first_word) {
                let lower: String = first.to_lowercase().collect();
                text = format!("{lower}{}", &text[first.len_utf8()..]);
            }
        }
    }

    // Legal abbreviation fixes
    text = text.replace("Honorable", "Hon'ble");
    text = text.replace("Honourable", "Hon'ble");
    text = text.replace("honorable", "Hon'ble");
    text = text.replace("honourable", "Hon'ble");
    text = text.replace("First Information Report", "FIR");
    text = text.replace("first information report", "FIR");

    // Ensure ends with period
    let trimmed = text.trim_end();
    if !trimmed.ends_with('.') && !trimmed.ends_with(';') {
        text = format!("{trimmed}.");
    }

    text
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum DocType { Complaint, Rental, Notice, Poa, SaleDeed, Will, Memo, Affidavit, Generic }

fn detect_doc_type(query: &str) -> DocType {
    let q = query.to_lowercase();
    if q.contains("power of attorney") || (q.contains("poa") && q.contains("draft")) { return DocType::Poa; }
    if q.contains("sale deed") || q.contains("conveyance deed") { return DocType::SaleDeed; }
    if q.contains("legal notice") || q.contains("cease and desist") || q.contains("reply to notice") { return DocType::Notice; }
    if (q.contains("rent") || q.contains("lease") || q.contains("tenancy")) && (q.contains("agreement") || q.contains("deed")) { return DocType::Rental; }
    if q.contains("will") && (q.contains("testament") || q.contains("last will") || q.contains("bequeath")) { return DocType::Will; }
    if q.contains("memo") || q.contains("memorandum") { return DocType::Memo; }
    if q.contains("affidavit") && !q.contains("complaint") { return DocType::Affidavit; }
    if q.contains("complaint") || q.contains("fir") || q.contains("420") || q.contains("406") || q.contains("498")
       || q.contains("armed forces tribunal") || q.contains("aft ") || q.contains("petition") { return DocType::Complaint; }
    DocType::Generic
}

fn detect_doc_type_from_data(data: &serde_json::Value) -> DocType {
    if let Some(dt) = data.get("doc_type").and_then(|v| v.as_str()) {
        match dt {
            "complaint" => return DocType::Complaint,
            "rental" => return DocType::Rental,
            "notice" => return DocType::Notice,
            "poa" => return DocType::Poa,
            "sale_deed" => return DocType::SaleDeed,
            "will" => return DocType::Will,
            "memo" => return DocType::Memo,
            "affidavit" => return DocType::Affidavit,
            "agreement" => return DocType::Generic,
            _ => {}
        }
    }
    if data.get("court_name").is_some() || data.get("offence_type").is_some() { DocType::Complaint }
    else if data.get("landlord_name").is_some() || data.get("tenant_name").is_some() || data.get("rent").is_some() { DocType::Rental }
    else if data.get("sender_name").is_some() && data.get("recipient_name").is_some() { DocType::Notice }
    else if data.get("grantor_name").is_some() || data.get("grantee_name").is_some() { DocType::Poa }
    else if data.get("seller_name").is_some() || data.get("buyer_name").is_some() || data.get("sale_amount").is_some() { DocType::SaleDeed }
    else if data.get("testator_name").is_some() { DocType::Will }
    else { DocType::Generic }
}

fn jf<'a>(data: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().filter_map(|k| data.get(*k)?.as_str()).find(|v| !v.is_empty())
}

fn needs_step2(dt: DocType) -> bool {
    matches!(dt, DocType::Complaint | DocType::Notice | DocType::Will | DocType::Affidavit | DocType::Memo | DocType::Generic)
}

/// Extract facts from the user's own query when the model fails to do so.
/// Splits the query into sentences and keeps the substantive ones. This is
/// vastly better than injecting hardcoded fake facts (which the model used
/// to copy verbatim, hallucinating dates/names/places the user never said).
fn facts_from_user_query(query: &str) -> Vec<String> {
    // Strip any leading "draft a ... for" / "write a ... about" instruction
    let cleaned = {
        let lower = query.to_lowercase();
        let cut_after = ["for ", "about ", "regarding ", "against ", "in the matter of "];
        let mut start = 0;
        for marker in cut_after.iter() {
            if let Some(pos) = lower.find(marker) {
                start = pos + marker.len();
                break;
            }
        }
        query[start..].trim()
    };

    // Split on sentence boundaries
    let mut facts: Vec<String> = Vec::new();
    for raw in cleaned.split(|c: char| c == '.' || c == ';') {
        let s = raw.trim();
        // Skip too-short fragments
        if s.split_whitespace().count() < 4 { continue; }
        // Lowercase first letter for legal style
        let mut chars = s.chars();
        let first = chars.next().unwrap_or(' ').to_lowercase().next().unwrap_or(' ');
        let rest: String = chars.collect();
        facts.push(format!("{}{}", first, rest));
        if facts.len() >= 6 { break; }
    }
    facts
}

/// Find capitalised proper-name candidates in the user query (2+ words starting with Capital).
/// Used as a sanity check when the model returned junk values like "" or "COMPLAINANT".
fn proper_names_in_query(query: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].trim_matches(|c: char| !c.is_alphabetic());
        // Skip stop-words that may start with caps
        let stop = ["I","The","A","An","Draft","Write","Create","Generate","Prepare",
                    "He","She","They","His","Her","Their","It","We","Our","My",
                    "Mr","Mrs","Ms","Dr","Sh","Smt","Section","Sector","Court","India","Delhi","New"];
        if w.len() > 1 && w.chars().next().map_or(false, |c| c.is_uppercase()) && !stop.contains(&w) {
            // Try to extend with following capitalised words
            let mut full = w.to_string();
            let mut j = i + 1;
            while j < words.len() {
                let nw = words[j].trim_matches(|c: char| !c.is_alphabetic());
                if nw.len() > 1 && nw.chars().next().map_or(false, |c| c.is_uppercase()) && !stop.contains(&nw) {
                    full.push(' ');
                    full.push_str(nw);
                    j += 1;
                } else {
                    break;
                }
            }
            if full.split_whitespace().count() >= 2 {
                names.push(full);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    names
}

/// Patch obviously-broken name fields in `data` using names found in the user query.
/// Triggers when the model returned empty, literal "COMPLAINANT"/"ACCUSED" placeholders,
/// or the model field doesn't match anything the user actually wrote.
fn patch_names_from_query(data: &mut serde_json::Value, user_query: &str) {
    let names = proper_names_in_query(user_query);
    if names.is_empty() { return; }

    let broken = |v: Option<&str>| -> bool {
        match v {
            None => true,
            Some(s) => {
                let t = s.trim();
                t.is_empty()
                    || t.eq_ignore_ascii_case("complainant")
                    || t.eq_ignore_ascii_case("accused")
                    || t.eq_ignore_ascii_case("petitioner")
                    || t.eq_ignore_ascii_case("respondent")
                    || t.eq_ignore_ascii_case("applicant")
                    || t.eq_ignore_ascii_case("deponent")
                    || t.eq_ignore_ascii_case("sender")
                    || t.eq_ignore_ascii_case("recipient")
            }
        }
    };

    let primary_fields = ["complainant_name", "petitioner_name", "applicant_name",
                          "sender_name", "client_name", "landlord_name", "seller_name",
                          "grantor_name", "testator_name", "deponent_name"];
    let secondary_fields = ["accused_name", "respondent_name", "opposite_party",
                            "recipient_name", "tenant_name", "buyer_name", "grantee_name"];

    let cur_primary: Option<String> = primary_fields.iter()
        .find_map(|k| data.get(*k).and_then(|v| v.as_str()).map(String::from));
    let cur_secondary: Option<String> = secondary_fields.iter()
        .find_map(|k| data.get(*k).and_then(|v| v.as_str()).map(String::from));

    if broken(cur_primary.as_deref()) {
        if let Some(n) = names.get(0) {
            for k in primary_fields.iter() {
                if data.get(*k).is_some() {
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert(k.to_string(), serde_json::json!(n));
                    }
                    tracing::info!("[chat] patched {} from user query: {}", k, n);
                    break;
                }
            }
        }
    }
    if broken(cur_secondary.as_deref()) {
        if let Some(n) = names.get(1) {
            for k in secondary_fields.iter() {
                if data.get(*k).is_some() {
                    if let Some(obj) = data.as_object_mut() {
                        obj.insert(k.to_string(), serde_json::json!(n));
                    }
                    tracing::info!("[chat] patched {} from user query: {}", k, n);
                    break;
                }
            }
        }
    }
}

/// Inject fallback facts into `data` when step 2 failed to produce them.
/// Uses the user's actual query content (not hardcoded fake facts) so the
/// document reflects what the user actually asked for. Returns true if injected.
fn inject_fallback_facts(data: &mut serde_json::Value, doc_type: DocType, user_query: &str) -> bool {
    let has_facts = data.get("facts")
        .and_then(|f| f.as_array())
        .map_or(false, |a| !a.is_empty());
    if has_facts || !needs_step2(doc_type) { return false; }

    // For complaints, set offence_type from query if model gave nothing valid
    if matches!(doc_type, DocType::Complaint) {
        let raw_o = data.get("offence_type")
            .and_then(|v| v.as_str()).unwrap_or("").to_string();
        let valid = ["cheating","breach_of_trust","cheating_conspiracy",
            "cruelty","maintenance","domestic_violence","divorce","armed_forces"];
        let o_clean: &str = if valid.contains(&raw_o.as_str()) { &raw_o }
            else { detect_offence_from_query(user_query) };
        if let Some(obj) = data.as_object_mut() {
            obj.insert("offence_type".to_string(), serde_json::json!(o_clean));
        }
    }

    let user_facts = facts_from_user_query(user_query);
    if user_facts.is_empty() { return false; }

    let arr: serde_json::Value = serde_json::Value::Array(
        user_facts.into_iter().map(serde_json::Value::String).collect()
    );
    if let Some(obj) = data.as_object_mut() {
        obj.insert("facts".to_string(), arr);
        tracing::info!("[chat] injected fallback facts from user query for {:?}", doc_type);
        return true;
    }
    false
}

fn assemble_legal_document(data: &serde_json::Value, user_query: &str) -> Option<String> {
    // User query takes priority — the model often misclassifies doc types
    let dt = {
        let from_query = detect_doc_type(user_query);
        if from_query != DocType::Generic { from_query } else { detect_doc_type_from_data(data) }
    };
    match dt {
        DocType::Complaint => assemble_criminal_complaint(data),
        DocType::Rental => assemble_rental_agreement(data),
        DocType::Notice => assemble_legal_notice(data),
        DocType::Poa => assemble_poa(data),
        DocType::SaleDeed => assemble_sale_deed(data),
        DocType::Will => assemble_will(data),
        DocType::Memo => assemble_memo(data),
        DocType::Affidavit => assemble_generic_affidavit(data),
        DocType::Generic => assemble_generic_document(data),
    }
}

fn assemble_criminal_complaint(data: &serde_json::Value) -> Option<String> {
    let court_name = data.get("court_name")?.as_str()?;
    let case_year = data.get("case_year").and_then(|v| v.as_str()).unwrap_or("2025");
    let c_name = data.get("complainant_name")?.as_str()?;
    let c_parent = data.get("complainant_parent").and_then(|v| v.as_str()).unwrap_or("");
    let c_age = data.get("complainant_age").and_then(|v| v.as_str()).unwrap_or("35");
    let c_addr = data.get("complainant_address").and_then(|v| v.as_str()).unwrap_or("");
    let a_name = data.get("accused_name")?.as_str()?;
    let a_parent = data.get("accused_parent").and_then(|v| v.as_str()).unwrap_or("");
    let a_age = data.get("accused_age").and_then(|v| v.as_str()).unwrap_or("40");
    let a_addr = data.get("accused_address").and_then(|v| v.as_str()).unwrap_or("");
    let offence = data.get("offence_type").and_then(|v| v.as_str()).unwrap_or("cheating");
    let amount = data.get("amount").and_then(|v| v.as_str()).unwrap_or("");
    let amount_words = data.get("amount_words").and_then(|v| v.as_str()).unwrap_or("");
    let city = data.get("city").and_then(|v| v.as_str()).unwrap_or("New Delhi");
    let verif_date_default = format!("___ day of _____, {}", case_year);
    let verif_date = data.get("date_of_verification").and_then(|v| v.as_str())
        .unwrap_or(&verif_date_default);

    let facts = data.get("facts")?.as_array()?;
    if facts.is_empty() { return None; }

    let ipc = ipc_sections_for_offence(offence);
    let prefix = case_prefix_for_offence(offence);
    let statute = statute_name_for_offence(offence);
    let (label_c, label_a) = party_labels_for_offence(offence);
    let offence_desc = offence_description(offence);

    // Rust-computed amount in words — overrides model's (often wrong) conversion
    let amount_words_final = if !amount.is_empty() {
        let computed = amount_to_indian_words(amount);
        if computed.is_empty() { amount_words.to_string() } else { computed }
    } else {
        amount_words.to_string()
    };

    let mut doc = String::with_capacity(8192);

    // Court header
    doc.push_str(&format!("# IN THE COURT OF {court_name}\n\n"));

    // Case number (bold)
    doc.push_str(&format!("**{prefix} {case_year}**\n\n"));

    // Party block
    doc.push_str("IN THE MATTER OF:\n\n");
    let c_parent_fmt = if c_parent.is_empty() { String::new() } else { format!(" {c_parent}") };
    doc.push_str(&format!("{c_name}{c_parent_fmt}, aged about {c_age} years, R/o {c_addr}\n\n"));
    doc.push_str(&format!("... {label_c}\n\n"));
    doc.push_str("Versus\n\n");
    let a_parent_fmt = if a_parent.is_empty() { String::new() } else { format!(" {a_parent}") };
    doc.push_str(&format!("{a_name}{a_parent_fmt}, aged about {a_age} years, R/o {a_addr}\n\n"));
    doc.push_str(&format!("... {label_a}\n\n"));

    // Affidavit
    doc.push_str("## AFFIDAVIT\n\n");
    doc.push_str("MOST RESPECTFULLY SHOWETH:\n\n");

    let mut para = 1;

    // --- Para 1: Formal introduction (deterministic) ---
    doc.push_str(&format!(
        "**{para}.** That, the present complaint has been preferred by the {label_c} \
         {c_name}{c_parent_fmt}, aged about {c_age} years, R/o {c_addr}, who is an \
         aggrieved person seeking indulgence of this Hon'ble Court qua the acts of \
         {offence_desc} committed by the {label_a} {a_name}{a_parent_fmt}, aged about \
         {a_age} years, R/o {a_addr}.\n\n"
    ));
    para += 1;

    // --- Para 2: Competency (deterministic) ---
    doc.push_str(&format!(
        "**{para}.** That, the {label_c} is a law-abiding citizen of India, residing at \
         the above-mentioned address and is fully conversant with the facts and circumstances \
         of the present case. The {label_c} is competent to swear this affidavit.\n\n"
    ));
    para += 1;

    // --- Paras 3..N-2: Case-specific facts from model (skip any boilerplate) ---
    for fact in facts.iter() {
        if let Some(text) = fact.as_str() {
            if !is_boilerplate_fact(text) {
                let polished = polish_legal_fact(text);
                doc.push_str(&format!("**{para}.** That, {polished}\n\n"));
                para += 1;
            }
        }
    }

    // --- Para N-1: Cause of action (deterministic) ---
    doc.push_str(&format!(
        "**{para}.** That, the cause of action in the present matter arose at {city}, \
         within the territorial jurisdiction of this Hon'ble Court, when the {label_a} \
         committed acts of {offence_desc} against the {label_c} as described hereinabove. \
         The {label_c} is entitled to file the present complaint.\n\n"
    ));
    para += 1;

    // --- Para N: No other remedy (deterministic) ---
    doc.push_str(&format!(
        "**{para}.** That, the {label_c} has no other efficacious remedy available in law \
         except to approach this Hon'ble Court for seeking justice and redressal of grievances. \
         The {label_c} craves leave of this Hon'ble Court to add, alter, amend or vary the \
         grounds of the present complaint at the time of hearing.\n\n"
    ));

    // Prayer
    doc.push_str("## PRAYER\n\n");
    doc.push_str("It is, therefore, most respectfully prayed that this Hon'ble Court may kindly be pleased to:\n\n");
    doc.push_str(&format!("(i) Take cognizance of the offence u/S {ipc} of {statute};\n\n"));
    doc.push_str(&format!("(ii) Summon the {label_a} and try him in accordance with law;\n\n"));
    if !amount.is_empty() {
        doc.push_str(&format!(
            "(iii) Direct the {label_a} to return Rs. {amount}/- ({amount_words_final}) to the {label_c};\n\n"
        ));
    }
    doc.push_str("(iv) Pass such other and further order(s) as this Hon'ble Court may deem fit and proper in the interest of justice.\n\n");
    doc.push_str(&format!("And for this act of kindness, the {label_c} shall ever pray.\n\n"));
    doc.push_str(&format!("{label_c}\n\n"));
    doc.push_str("Through Counsel\n\n");

    // Verification
    doc.push_str("## VERIFICATION\n\n");
    doc.push_str(&format!(
        "Verified at {city} on this {verif_date} that the contents of the above affidavit are true and correct to the best of my knowledge and belief and nothing material has been concealed therefrom.\n\n"
    ));
    doc.push_str("Solemnly affirmed before me.\n\n");
    doc.push_str("DEPONENT\n");

    Some(doc)
}

fn assemble_rental_agreement(data: &serde_json::Value) -> Option<String> {
    let landlord = jf(data, &["landlord_name", "complainant_name", "owner_name", "first_party"]).unwrap_or("___");
    let landlord_parent = jf(data, &["landlord_parent", "complainant_parent"]).unwrap_or("");
    let landlord_addr = jf(data, &["landlord_address", "complainant_address"]).unwrap_or("___");
    let tenant = jf(data, &["tenant_name", "accused_name", "lessee_name", "second_party"]).unwrap_or("___");
    let tenant_parent = jf(data, &["tenant_parent", "accused_parent"]).unwrap_or("");
    let tenant_addr = jf(data, &["tenant_address", "accused_address"]).unwrap_or("___");
    let property = jf(data, &["property_address", "property", "premises", "flat_address"]).unwrap_or("___");
    let rent = jf(data, &["rent", "monthly_rent", "rent_amount", "amount"]).unwrap_or("___");
    let deposit = jf(data, &["deposit", "security_deposit"]).unwrap_or("___");
    let tenure = jf(data, &["tenure", "period", "duration", "lease_period"]).unwrap_or("11 months");
    let start_date = jf(data, &["start_date", "commencement_date", "effective_date", "date"]).unwrap_or("___");
    let purpose = jf(data, &["purpose", "use"]).unwrap_or("residential");
    let city = jf(data, &["city", "place"]).unwrap_or("New Delhi");

    let lp = if landlord_parent.is_empty() { String::new() } else { format!(", {landlord_parent}") };
    let tp = if tenant_parent.is_empty() { String::new() } else { format!(", {tenant_parent}") };
    let rw = { let w = amount_to_indian_words(rent); if w.is_empty() { String::new() } else { format!(" ({w})") } };
    let dw = { let w = amount_to_indian_words(deposit); if w.is_empty() { String::new() } else { format!(" ({w})") } };

    let mut d = String::with_capacity(8192);
    d.push_str("# RENTAL AGREEMENT\n\n");
    d.push_str(&format!("This Rental Agreement (\"Agreement\") is made and executed on **{start_date}** at **{city}**.\n\n"));
    d.push_str("## BETWEEN\n\n");
    d.push_str(&format!(
        "**{landlord}**{lp}, R/o {landlord_addr} (hereinafter referred to as the \"**LANDLORD**\", which expression shall, \
         unless repugnant to the context, include his/her heirs, executors, administrators and assigns) of the FIRST PART;\n\n"
    ));
    d.push_str("AND\n\n");
    d.push_str(&format!(
        "**{tenant}**{tp}, R/o {tenant_addr} (hereinafter referred to as the \"**TENANT**\", which expression shall, \
         unless repugnant to the context, include his/her heirs, executors, administrators and assigns) of the SECOND PART.\n\n"
    ));
    d.push_str(&format!(
        "WHEREAS the Landlord is the absolute owner of and in possession of the property bearing address \
         **{property}** (hereinafter referred to as the \"Demised Premises\");\n\n"
    ));
    d.push_str(
        "AND WHEREAS the Tenant has approached the Landlord for taking the said Demised Premises on rent \
         and the Landlord has agreed to let out the same on the following terms and conditions:\n\n"
    );
    d.push_str("## TERMS AND CONDITIONS\n\n");
    let mut c = 1;
    d.push_str(&format!(
        "**{c}.** That the Landlord hereby lets out and the Tenant hereby takes on rent the Demised Premises \
         for a period of **{tenure}** commencing from **{start_date}**.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant shall pay a monthly rent of **Rs. {rent}/-**{rw} to the Landlord, payable \
         on or before the 7th day of each English calendar month. Any delay beyond 15 days shall attract a \
         late fee of Rs. 500/- per day.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant has deposited a sum of **Rs. {deposit}/-**{dw} as interest-free security deposit \
         with the Landlord. The said deposit shall be refundable at the time of vacating the premises, after \
         deducting any outstanding dues, unpaid bills, or cost of damages caused to the premises, if any.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant shall use the Demised Premises solely for **{purpose}** purposes and shall not \
         use or permit the same to be used for any unlawful, immoral, or objectionable purpose.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant shall not make any structural alterations, additions, or modifications to the \
         Demised Premises without the prior written consent of the Landlord. Any approved alterations shall become \
         the property of the Landlord upon termination of this Agreement.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant shall maintain the Demised Premises in good and tenantable condition throughout \
         the tenancy period. Minor repairs (up to Rs. 2,000/- per instance) shall be at the Tenant's cost. Major \
         repairs and structural maintenance shall be the responsibility of the Landlord.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant shall not sublet, assign, or transfer the tenancy or any part thereof to any \
         third party without the prior written consent of the Landlord. Any subletting without consent shall \
         render this Agreement void.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Landlord or his/her authorised representative shall have the right to enter and inspect \
         the Demised Premises at any reasonable time after giving at least 24 hours prior notice to the Tenant.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the electricity, water, maintenance charges, and other utility bills shall be borne and \
         paid by the Tenant directly to the concerned authorities. The Tenant shall furnish proof of payment \
         upon request.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That either party may terminate this Agreement by giving **two months** prior written notice \
         to the other party. In the event of early termination by the Tenant without proper notice, the Landlord \
         shall be entitled to forfeit rent for the notice period from the security deposit.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That in the event the Tenant fails to pay rent for a period of two consecutive months, the \
         Landlord shall be entitled to terminate this Agreement and take possession of the Demised Premises \
         forthwith after giving 15 days written notice.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant shall not keep or store any hazardous, inflammable, or illegal materials in \
         the Demised Premises. The Tenant shall comply with all applicable laws, rules, and regulations.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Tenant shall peacefully vacate and hand over vacant possession of the Demised Premises \
         to the Landlord upon expiry or termination of this Agreement, in the same condition as received, subject \
         to normal wear and tear.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That any dispute arising out of or in connection with this Agreement shall be subject to the \
         exclusive jurisdiction of the courts at **{city}**.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That this Agreement constitutes the entire understanding between the parties. Any amendment \
         or modification shall be in writing and signed by both parties.\n\n"
    ));
    d.push_str(
        "IN WITNESS WHEREOF, the parties hereto have set and subscribed their respective hands on this \
         Agreement on the day, month, and year first above written.\n\n"
    );
    d.push_str(&format!("**LANDLORD**\n{landlord}\n\n"));
    d.push_str(&format!("**TENANT**\n{tenant}\n\n"));
    d.push_str("**WITNESSES:**\n\n");
    d.push_str("1. Name: _______________ Signature: _______________\n\n");
    d.push_str("2. Name: _______________ Signature: _______________\n");
    Some(d)
}

fn assemble_legal_notice(data: &serde_json::Value) -> Option<String> {
    let sender = jf(data, &["sender_name", "complainant_name", "client_name"]).unwrap_or("___");
    let sender_parent = jf(data, &["sender_parent", "complainant_parent"]).unwrap_or("");
    let sender_addr = jf(data, &["sender_address", "complainant_address"]).unwrap_or("___");
    let recipient = jf(data, &["recipient_name", "accused_name", "opposite_party"]).unwrap_or("___");
    let recipient_addr = jf(data, &["recipient_address", "accused_address"]).unwrap_or("___");
    let subject = jf(data, &["subject", "re", "regarding"]).unwrap_or("Legal Notice");
    let amount = jf(data, &["amount", "demand_amount"]).unwrap_or("");
    let city = jf(data, &["city", "place"]).unwrap_or("New Delhi");

    let facts = data.get("facts").and_then(|v| v.as_array());
    if facts.map_or(true, |f| f.is_empty()) { return None; }

    let sp = if sender_parent.is_empty() { String::new() } else { format!(", {sender_parent}") };
    let aw = amount_to_indian_words(amount);

    let mut d = String::with_capacity(4096);
    d.push_str("# LEGAL NOTICE\n\n");
    d.push_str("**Under Section 80 of the Code of Civil Procedure, 1908**\n\n");
    d.push_str(&format!("**Date:** ___/___/2025\n\n"));
    d.push_str(&format!("**To,**\n{recipient}\n{recipient_addr}\n\n"));
    d.push_str(&format!("**From,**\n{sender}{sp}\n{sender_addr}\n\n"));
    d.push_str(&format!("**Subject: {subject}**\n\n"));
    d.push_str("**Sir/Madam,**\n\n");
    d.push_str(&format!(
        "Under the instructions and on behalf of my client, **{sender}**{sp}, R/o {sender_addr}, \
         I do hereby serve upon you the following Legal Notice:\n\n"
    ));
    let mut p = 1;
    if let Some(facts) = facts {
        for fact in facts {
            if let Some(text) = fact.as_str() {
                if !is_boilerplate_fact(text) {
                    let polished = polish_legal_fact(text);
                    d.push_str(&format!("**{p}.** That, {polished}\n\n"));
                    p += 1;
                }
            }
        }
    }
    if !amount.is_empty() {
        let w = if aw.is_empty() { String::new() } else { format!(" ({aw})") };
        d.push_str(&format!(
            "**{p}.** That, my client hereby demands the payment of **Rs. {amount}/-**{w} within **15 days** \
             from the receipt of this notice, failing which my client shall be constrained to initiate appropriate \
             civil and/or criminal proceedings against you at your risk, cost, and consequences.\n\n"
        ));
        p += 1;
    }
    d.push_str(&format!(
        "**{p}.** That, a copy of this notice is being retained in my office for record and future reference.\n\n"
    ));
    d.push_str("Please treat this notice as final and comply within the stipulated time.\n\n");
    d.push_str("**Yours faithfully,**\n\n");
    d.push_str("**Advocate**\n");
    d.push_str(&format!("(On behalf of {sender})\n{city}\n"));
    Some(d)
}

fn assemble_poa(data: &serde_json::Value) -> Option<String> {
    let grantor = jf(data, &["grantor_name", "complainant_name", "principal_name"]).unwrap_or("___");
    let grantor_parent = jf(data, &["grantor_parent", "complainant_parent"]).unwrap_or("");
    let grantor_addr = jf(data, &["grantor_address", "complainant_address"]).unwrap_or("___");
    let grantor_age = jf(data, &["grantor_age", "complainant_age"]).unwrap_or("___");
    let grantee = jf(data, &["grantee_name", "accused_name", "agent_name", "attorney_name"]).unwrap_or("___");
    let grantee_parent = jf(data, &["grantee_parent", "accused_parent"]).unwrap_or("");
    let grantee_addr = jf(data, &["grantee_address", "accused_address"]).unwrap_or("___");
    let grantee_age = jf(data, &["grantee_age", "accused_age"]).unwrap_or("___");
    let purpose = jf(data, &["purpose", "subject"]).unwrap_or("general administration of affairs");
    let city = jf(data, &["city", "place"]).unwrap_or("New Delhi");

    let gp = if grantor_parent.is_empty() { String::new() } else { format!(", {grantor_parent}") };
    let ap = if grantee_parent.is_empty() { String::new() } else { format!(", {grantee_parent}") };

    let mut d = String::with_capacity(4096);
    d.push_str("# GENERAL POWER OF ATTORNEY\n\n");
    d.push_str(&format!(
        "**KNOW ALL MEN BY THESE PRESENTS** that I, **{grantor}**{gp}, aged about {grantor_age} years, \
         R/o {grantor_addr} (hereinafter called the \"Executant/Principal\") do hereby appoint, nominate, \
         constitute, and authorize **{grantee}**{ap}, aged about {grantee_age} years, R/o {grantee_addr} \
         (hereinafter called the \"Attorney/Agent\") as my true and lawful Attorney to act on my behalf:\n\n"
    ));
    d.push_str(&format!("**Purpose:** {purpose}\n\n"));
    d.push_str("## POWERS GRANTED\n\n");
    let mut c = 1;
    d.push_str(&format!(
        "**{c}.** To appear before all courts, tribunals, authorities, and government offices on my behalf and to \
         sign, execute, and present all applications, affidavits, petitions, and other documents.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** To negotiate, enter into, execute, and register all agreements, deeds, contracts, and documents \
         on my behalf, and to receive and give receipts for all monies payable to me.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** To operate my bank accounts, to deposit and withdraw money, to sign cheques and demand drafts, \
         and to deal with all banking and financial matters on my behalf.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** To manage, administer, let out, and look after my immovable and movable properties, to collect \
         rents and other dues, and to institute or defend legal proceedings in relation thereto.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** To sell, purchase, mortgage, lease, or otherwise transfer or acquire any property on my behalf, \
         and to execute all necessary documents in that regard.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** To appoint and engage advocates, chartered accountants, and other professionals on my behalf \
         and to pay their fees and charges.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** To do all such acts, deeds, and things as may be necessary or expedient for the purposes \
         aforesaid, as fully and effectually as I could do if personally present.\n\n"
    ));
    d.push_str(
        "I hereby declare that all acts lawfully done by my said Attorney by virtue of this Power of Attorney \
         shall be construed as acts done by me. This Power of Attorney shall remain in force until revoked \
         by me in writing.\n\n"
    );
    d.push_str(&format!(
        "IN WITNESS WHEREOF, I have executed this Power of Attorney on this ___ day of _____, 2025 at {city}.\n\n"
    ));
    d.push_str(&format!("**EXECUTANT/PRINCIPAL**\n{grantor}\n\n"));
    d.push_str("**WITNESSES:**\n\n");
    d.push_str("1. Name: _______________ Signature: _______________\n\n");
    d.push_str("2. Name: _______________ Signature: _______________\n");
    Some(d)
}

fn assemble_sale_deed(data: &serde_json::Value) -> Option<String> {
    let seller = jf(data, &["seller_name", "complainant_name", "vendor_name"]).unwrap_or("___");
    let seller_parent = jf(data, &["seller_parent", "complainant_parent"]).unwrap_or("");
    let seller_addr = jf(data, &["seller_address", "complainant_address"]).unwrap_or("___");
    let buyer = jf(data, &["buyer_name", "accused_name", "vendee_name", "purchaser_name"]).unwrap_or("___");
    let buyer_parent = jf(data, &["buyer_parent", "accused_parent"]).unwrap_or("");
    let buyer_addr = jf(data, &["buyer_address", "accused_address"]).unwrap_or("___");
    let property = jf(data, &["property", "property_description", "property_address"]).unwrap_or("___");
    let sale_amount = jf(data, &["sale_amount", "amount", "consideration", "sale_price"]).unwrap_or("___");
    let city = jf(data, &["city", "place"]).unwrap_or("New Delhi");

    let sp = if seller_parent.is_empty() { String::new() } else { format!(", {seller_parent}") };
    let bp = if buyer_parent.is_empty() { String::new() } else { format!(", {buyer_parent}") };
    let aw = { let w = amount_to_indian_words(sale_amount); if w.is_empty() { String::new() } else { format!(" ({w})") } };

    let mut d = String::with_capacity(6144);
    d.push_str("# SALE DEED\n\n");
    d.push_str(&format!("This Sale Deed is made and executed on this ___ day of _____, 2025 at **{city}**.\n\n"));
    d.push_str("## BETWEEN\n\n");
    d.push_str(&format!(
        "**{seller}**{sp}, R/o {seller_addr} (hereinafter called the \"**SELLER/VENDOR**\", which expression \
         shall include his/her heirs, executors, administrators and assigns) of the FIRST PART;\n\n"
    ));
    d.push_str("AND\n\n");
    d.push_str(&format!(
        "**{buyer}**{bp}, R/o {buyer_addr} (hereinafter called the \"**BUYER/VENDEE**\", which expression \
         shall include his/her heirs, executors, administrators and assigns) of the SECOND PART.\n\n"
    ));
    d.push_str(&format!(
        "WHEREAS the Seller is the sole and absolute owner of the property described herein, viz. **{property}** \
         (hereinafter referred to as the \"Scheduled Property\"), and is in lawful possession thereof;\n\n"
    ));
    d.push_str(&format!(
        "AND WHEREAS the Seller has agreed to sell and convey the Scheduled Property to the Buyer for a total \
         sale consideration of **Rs. {sale_amount}/-**{aw};\n\n"
    ));
    d.push_str("NOW THIS DEED WITNESSETH AS FOLLOWS:\n\n");
    let mut c = 1;
    d.push_str(&format!(
        "**{c}.** That, in consideration of Rs. {sale_amount}/-{aw} paid by the Buyer to the Seller (the receipt \
         whereof the Seller doth hereby acknowledge), the Seller doth hereby sell, convey, transfer, and assign \
         unto the Buyer ALL THAT the Scheduled Property, together with all rights, title, interest, and \
         appurtenances thereto.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Seller hereby declares and warrants that the Scheduled Property is free from all \
         encumbrances, liens, charges, mortgages, litigations, and claims of whatsoever nature, and the Seller \
         has the absolute right and authority to sell and convey the same.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Seller hereby covenants to indemnify and keep indemnified the Buyer against all claims, \
         demands, and proceedings arising out of any defect in the title or any encumbrance on the property.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Seller has delivered physical and legal possession of the Scheduled Property to the \
         Buyer on the date of execution of this Deed.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Buyer shall bear and pay all stamp duty, registration charges, and other expenses \
         in connection with the execution and registration of this Sale Deed.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That all taxes, rates, and outgoings up to the date of this Deed have been paid by the Seller. \
         From the date hereof, all such charges shall be borne by the Buyer.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** That the Seller shall execute all further deeds and documents as may be required to more fully \
         convey the Scheduled Property to the Buyer.\n\n"
    ));
    d.push_str("## SCHEDULE OF PROPERTY\n\n");
    d.push_str(&format!("{property}\n\n"));
    d.push_str(&format!(
        "IN WITNESS WHEREOF, the parties have set their hands on this Sale Deed at {city}.\n\n"
    ));
    d.push_str(&format!("**SELLER/VENDOR**\n{seller}\n\n"));
    d.push_str(&format!("**BUYER/VENDEE**\n{buyer}\n\n"));
    d.push_str("**WITNESSES:**\n\n");
    d.push_str("1. Name: _______________ Signature: _______________\n\n");
    d.push_str("2. Name: _______________ Signature: _______________\n");
    Some(d)
}

fn assemble_will(data: &serde_json::Value) -> Option<String> {
    let testator = jf(data, &["testator_name", "complainant_name", "name"]).unwrap_or("___");
    let testator_parent = jf(data, &["testator_parent", "complainant_parent"]).unwrap_or("");
    let testator_addr = jf(data, &["testator_address", "complainant_address"]).unwrap_or("___");
    let testator_age = jf(data, &["testator_age", "complainant_age"]).unwrap_or("___");
    let city = jf(data, &["city", "place"]).unwrap_or("New Delhi");

    let facts = data.get("facts").and_then(|v| v.as_array());
    if facts.map_or(true, |f| f.is_empty()) { return None; }

    let tp = if testator_parent.is_empty() { String::new() } else { format!(", {testator_parent}") };

    let mut d = String::with_capacity(4096);
    d.push_str("# LAST WILL AND TESTAMENT\n\n");
    d.push_str(&format!(
        "I, **{testator}**{tp}, aged about {testator_age} years, R/o {testator_addr}, being of sound mind and \
         disposing memory, do hereby declare this to be my Last Will and Testament, revoking all previous Wills \
         and Codicils made by me.\n\n"
    ));
    let mut c = 1;
    d.push_str(&format!(
        "**{c}.** I declare that I am making this Will of my own free will and volition, without any coercion, \
         undue influence, or persuasion from any person whatsoever.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** I declare that I am in good health and of sound and disposing mind at the time of execution \
         of this Will and fully understand its nature and consequences.\n\n"
    )); c += 1;
    if let Some(facts) = facts {
        for fact in facts {
            if let Some(text) = fact.as_str() {
                d.push_str(&format!("**{c}.** {text}\n\n"));
                c += 1;
            }
        }
    }
    d.push_str(&format!(
        "**{c}.** In the event any beneficiary named above predeceases me, the bequest made in his/her favour \
         shall lapse and the said property shall devolve upon the remaining beneficiaries in equal shares.\n\n"
    )); c += 1;
    d.push_str(&format!(
        "**{c}.** I appoint _______________ as the Executor of this Will to carry out the provisions contained herein.\n\n"
    ));
    d.push_str(&format!(
        "IN WITNESS WHEREOF, I have signed this Will on this ___ day of _____, 2025 at {city}, in the presence \
         of the following witnesses.\n\n"
    ));
    d.push_str(&format!("**TESTATOR**\n{testator}\n\n"));
    d.push_str("**WITNESSES:**\n\n");
    d.push_str("1. Name: _______________ Signature: _______________\n\n");
    d.push_str("2. Name: _______________ Signature: _______________\n");
    Some(d)
}

fn assemble_memo(data: &serde_json::Value) -> Option<String> {
    let from = jf(data, &["from", "sender_name", "complainant_name"]).unwrap_or("___");
    let to = jf(data, &["to", "recipient_name", "accused_name"]).unwrap_or("___");
    let subject = jf(data, &["subject", "re", "regarding"]).unwrap_or("___");
    let date = jf(data, &["date", "memo_date"]).unwrap_or("___/___/2025");
    let ref_no = jf(data, &["ref_no", "reference", "memo_no"]).unwrap_or("");

    let facts = data.get("facts").and_then(|v| v.as_array());
    if facts.map_or(true, |f| f.is_empty()) { return None; }

    let mut d = String::with_capacity(2048);
    d.push_str("# OFFICE MEMORANDUM\n\n");
    if !ref_no.is_empty() { d.push_str(&format!("**Ref. No.:** {ref_no}\n\n")); }
    d.push_str(&format!("**Date:** {date}\n\n"));
    d.push_str(&format!("**From:** {from}\n\n"));
    d.push_str(&format!("**To:** {to}\n\n"));
    d.push_str(&format!("**Subject:** {subject}\n\n"));
    d.push_str("---\n\n");
    if let Some(facts) = facts {
        let mut p = 1;
        for fact in facts {
            if let Some(text) = fact.as_str() {
                d.push_str(&format!("**{p}.** {text}\n\n"));
                p += 1;
            }
        }
    }
    d.push_str(&format!("\n{from}\n"));
    Some(d)
}

fn assemble_generic_affidavit(data: &serde_json::Value) -> Option<String> {
    let deponent = jf(data, &["deponent_name", "complainant_name", "name"]).unwrap_or("___");
    let deponent_parent = jf(data, &["deponent_parent", "complainant_parent"]).unwrap_or("");
    let deponent_addr = jf(data, &["deponent_address", "complainant_address"]).unwrap_or("___");
    let deponent_age = jf(data, &["deponent_age", "complainant_age"]).unwrap_or("___");
    let city = jf(data, &["city", "place"]).unwrap_or("New Delhi");

    let facts = data.get("facts").and_then(|v| v.as_array());
    if facts.map_or(true, |f| f.is_empty()) { return None; }

    let dp = if deponent_parent.is_empty() { String::new() } else { format!(", {deponent_parent}") };

    let mut d = String::with_capacity(4096);
    d.push_str("# AFFIDAVIT\n\n");
    d.push_str(&format!(
        "I, **{deponent}**{dp}, aged about {deponent_age} years, R/o {deponent_addr}, do hereby solemnly \
         affirm and declare as under:\n\n"
    ));
    let mut p = 1;
    if let Some(facts) = facts {
        for fact in facts {
            if let Some(text) = fact.as_str() {
                d.push_str(&format!("**{p}.** That, {text}\n\n"));
                p += 1;
            }
        }
    }
    d.push_str(&format!(
        "**{p}.** That, the contents of this Affidavit are true and correct to the best of my knowledge and \
         belief, and nothing material has been concealed therefrom.\n\n"
    ));
    d.push_str("## VERIFICATION\n\n");
    d.push_str(&format!(
        "Verified at {city} on this ___ day of _____, 2025 that the contents of the above Affidavit are true \
         and correct to the best of my knowledge and belief.\n\n"
    ));
    d.push_str("Solemnly affirmed before me.\n\n");
    d.push_str("DEPONENT\n");
    Some(d)
}

fn assemble_generic_document(data: &serde_json::Value) -> Option<String> {
    let facts = data.get("facts").and_then(|v| v.as_array());
    if facts.map_or(true, |f| f.is_empty()) { return None; }
    let obj = data.as_object()?;
    let mut d = String::with_capacity(4096);
    if let Some(title) = jf(data, &["title", "subject", "heading"]) {
        d.push_str(&format!("# {}\n\n", title.to_uppercase()));
    }
    for (k, v) in obj {
        if k == "facts" || k == "doc_type" { continue; }
        if let Some(s) = v.as_str() {
            if !s.is_empty() {
                let label = k.replace('_', " ");
                d.push_str(&format!("**{label}:** {s}\n\n"));
            }
        }
    }
    d.push_str("---\n\n");
    let mut p = 1;
    if let Some(facts) = facts {
        for fact in facts {
            if let Some(text) = fact.as_str() {
                d.push_str(&format!("**{p}.** {text}\n\n"));
                p += 1;
            }
        }
    }
    Some(d)
}

const TEMPLATE_MAINTENANCE: &str = r#"
INCOME / EXPENSE AFFIDAVIT (Maintenance matters — Rajnesh v Neha SC format)

EXACT HEADER (mirror Bushra v Juned Ali, Annu Pal v Jitendra filings):
IN THE COURT OF LD. PRINCIPAL JUDGE, FAMILY COURT,
[DISTRICT] DISTRICT, [CITY]
[CASE TYPE] NO.         OF [YEAR]

IN THE MATTER OF:
[Full Name], [W/o / S/o / D/o] [Spouse/Father's Name],
aged about [age] years, presently residing at [full address with PIN]
                                        ... COMPLAINANT/PETITIONER
VERSUS
[Full Name], [relation]
                                        ... RESPONDENT

INCOME AFFIDAVIT BY AND ON BEHALF OF THE [COMPLAINANT/RESPONDENT]

I, [Full Name], [W/o / S/o] [Name], aged about [age] years, presently residing at [address], do hereby solemnly declare and affirm as under:

A. Personal Information
1. Age/Sex: [age] years / [Male/Female]
2. Qualification (Educational and Professional): [qualification]
3. Occupation: [occupation / designation / employer]

B. Income Details
4. That I am employed as [designation] at [employer name], earning a monthly gross salary of Rs. [amount] per month.
5. That I have [no / the following] other sources of income: [list or state "Nil"]

C. Monthly Expenditure
6. That my monthly expenditure is as follows:
   (i) House Rent: Rs. [amount]
   (ii) Food/Household: Rs. [amount]
   (iii) Transport: Rs. [amount]
   (iv) Medical: Rs. [amount]
   Total: Rs. [total]

D. Assets and Liabilities (as per Rajnesh v. Neha (2021) 2 SCC 324)
7. Immovable/Movable assets: [details or Nil]
8. Liabilities/Loans: [details or Nil]

VERIFICATION:
Verified at [City] on this [Date]. I, [Name], do hereby verify that the contents of the above affidavit are true and correct to the best of my knowledge and belief. No part of it is false and nothing material has been concealed therefrom.
Solemnly affirmed before me.
                                        DEPONENT
"#;

const TEMPLATE_PWDVA: &str = r#"
PWDVA APPLICATION REPLY (mirror Annu Pal v Jitendra, CC No. 2019/2022)

HEADER:
IN THE COURT OF LD. METROPOLITAN MAGISTRATE,
[NORTH/SOUTH/WEST] DISTRICT, ROHINI COURTS, DELHI
CC NO.        OF [YEAR]

REPLY TO THE APPLICATION U/S [12/18/19/20/22] OF THE PROTECTION OF WOMEN FROM DOMESTIC VIOLENCE ACT, 2005

MOST RESPECTFULLY SHOWETH:
1. That the present reply is being filed by the Respondent to the application filed by the Complainant u/s [section] of the PWDVA, 2005.
2. That the allegations levelled by the Complainant in her application are false, frivolous and have been made with the sole intention of harassing the Respondent.
[Continue with numbered "That, " paragraphs]
"#;

// ---------------------------------------------------------------------------
// Vanga case-search intake prompt — asks clarifying questions before searching
// ---------------------------------------------------------------------------
const CASE_SEARCH_INTAKE_PROMPT: &str = r#"
You are Mike's case-research intake agent. Your job: BEFORE searching case law, ask the lawyer 2-3 sharp, targeted clarifying questions that will materially change which judgments matter for their query. You are not the researcher yet — you are the senior associate who scopes the brief before sending the junior to the library.

## When to trigger intake (vs. skip)

TRIGGER intake when the user's query is a case-law research request missing at least 2 of: jurisdiction, time period, procedural posture, or specific sub-issue.

SKIP intake (search immediately with vanga_search) when:
- User cites a specific case by name or citation
- User already specified jurisdiction + time period + sub-issue
- User explicitly says "just search", "skip questions", "no clarifications"
- User is replying to a previous clarification turn (don't loop)
- Query is a follow-up that builds on already-clarified context

## Rules

1. Maximum 3 questions per intake. Often 2 is enough. Never 4+.
2. Every question must materially change the search result.
3. Provide concrete option chips, not open-ended questions.
4. Use Indian legal terminology. Lawyers use Section numbers, Act names, court abbreviations.
5. Be concise. No preamble. Just: "Before I search, [questions]."
6. Skip questions where context already gives the answer.

## Dimensions to cover (pick 2-3 most relevant)

1. Jurisdiction & forum: SC, specific HC(s), all HCs, Tribunals
2. Time period: Last 2/5/10 years, post a specific landmark, all-time
3. Statutory framework (IPC vs BNS post-July 2024, CrPC vs BNSS, IEA vs BSA)
4. Procedural posture: Pre-litigation, drafting, defending, trial, appeal, revision, writ, execution
5. Whose side: Plaintiff/petitioner, defendant/respondent, neutral
6. Specific sub-issue or facts
7. Type of relief sought
8. Personal/special law applicable
9. State-specific variations

## Output format

When asking questions, emit this block that the frontend renders as quick-reply chips:

[INTAKE]
question_1: { text: "Court level?", chips: ["Supreme Court", "Specific HC", "Pan-India"] }
question_2: { text: "Sub-issue?", chips: ["Notice service", "Security cheque", "Post-dated", "Other"] }
[/INTAKE]

## After intake

Once the user answers, acknowledge briefly and immediately call vanga_search with refined parameters. Do NOT ask further clarifications unless the user introduces a new ambiguity.

## Tone

Crisp, professional, peer-to-peer. No "I'd be happy to" or "Great question!".

## ABSOLUTE RULE — NO HALLUCINATED CASE LAW CITATIONS

You are STRICTLY PROHIBITED from citing case law from your training data or general knowledge. Case law citations MUST come from the vanga_search tool's actual results, and ONLY from results tagged [VERIFIED — full judgment retrieved] (meaning the actual judgment PDF was fetched and parsed).

If vanga_search returns:
- Empty array → say: "I couldn't find matching cases in my search index. Try rephrasing your query, broadening the court or year filters, or check Indian Kanoon directly." Do NOT cite anything. Do NOT fall back to training data.
- Results tagged [METADATA ONLY] (no full judgment text) → say: "I found some case titles that might be relevant but couldn't retrieve the full judgments to verify they're on point. The titles are: [list them]. I'd need the full text before I can say with confidence whether they support your point. Want me to try again, or do you have specific citations to look up?"
- Results tagged [VERIFIED] with full judgment text → cite ONLY from these. Every citation must be traceable to a specific passage in the retrieved text.

NEVER do any of these for case law:
- Cite cases by name when vanga_search returned empty
- Cite case numbers, years, or judges from your training data
- Say "the established position is..." with case names you didn't retrieve in this turn
- Construct plausible-sounding citations that might or might not exist

Statutes and Acts (NI Act, IPC, CrPC, BNS, BNSS, BSA, Constitution articles, etc.) ARE OK to cite from general knowledge — those are stable, published, and verifiable. Case law specifically is NOT.

If the lawyer asks a doctrinal question and vanga returns nothing useful, the correct response is: "I don't have those cases in my search index. Here's what the statute says: [statutory provision]. For authoritative case law on this point, I'd recommend checking Indian Kanoon or SCC Online."
"#;

/// Format vanga_search results into a system-prompt block for the model.
fn build_vanga_system_prompt(results: &[Value]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "<VANGA — Indian High Court judgment search results>\n\
         These cases were found by searching the Vanga open dataset of Indian High Court judgments.\n\n",
    );
    for (i, r) in results.iter().enumerate() {
        let title = r["title"].as_str().unwrap_or("Unknown");
        let court = r["court_name"].as_str().unwrap_or("");
        let date = r["decision_date"].as_str().unwrap_or("");
        let snippet = r["snippet"].as_str().unwrap_or("");
        let pdf_url = r["pdf_url"].as_str().unwrap_or("");
        let has_full_text = r.get("has_full_text")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let tag = if has_full_text {
            "[VERIFIED — full judgment retrieved]"
        } else {
            "[METADATA ONLY — title/description, full judgment NOT retrieved]"
        };
        s.push_str(&format!(
            "[v{idx}] {tag} {title}\n  Court: {court}\n  Date: {date}\n  Snippet: {snippet}\n  PDF: {pdf_url}\n\n",
            idx = i + 1,
        ));
    }
    s.push_str(
        "CITING VANGA RESULTS:\n\
         · Use numbered references [v1], [v2] etc. in your prose.\n\
         · The frontend will render these as clickable citation pills.\n\
         · Always mention the case name and court when citing.\n\
         · You may ONLY cite cases tagged [VERIFIED]. Do NOT cite [METADATA ONLY] cases as authoritative.\n\
         · If none of the results are relevant or all are [METADATA ONLY], say so honestly.\n\
         · NEVER supplement with case law from your training data.\n\
         </VANGA>\n",
    );
    s
}

fn build_doc_system_prompt(docs: &[DocPayload]) -> String {
    let with_text: Vec<&DocPayload> = docs.iter().filter(|d| d.text.is_some()).collect();
    let with_imgs: Vec<&DocPayload> = docs.iter().filter(|d| !d.images.is_empty()).collect();
    if with_text.is_empty() && with_imgs.is_empty() { return String::new(); }

    // Use Mike's chat-local doc-N labels so the citation system works.
    let mut s = String::from(
        "The user has attached the following documents. Use them to answer the question. \
         Cite the document name when relevant. The 'doc-N' label is for use in <CITATIONS> JSON only — \
         in prose, refer to documents by their filename.\n\n",
    );
    for (idx, d) in with_text.iter().enumerate() {
        s.push_str(&format!(
            "=== {label} (filename: {fname}) ===\n{body}\n\n",
            label = format!("doc-{idx}"),
            fname = d.filename,
            body = d.text.as_deref().unwrap_or("")
        ));
    }
    let img_offset = with_text.len();
    for (i, d) in with_imgs.iter().enumerate() {
        s.push_str(&format!(
            "=== {label} (filename: {fname}, rendered as {n} page image(s) attached below) ===\n\n",
            label = format!("doc-{}", img_offset + i),
            fname = d.filename,
            n = d.images.len()
        ));
    }
    s
}

fn collect_images(docs: &[DocPayload]) -> Vec<String> {
    docs.iter().flat_map(|d| d.images.clone()).collect()
}

/// One retrieved KB chunk plus the citation tag it was rendered with so
/// the response post-processor can map the model's `[g1]`/`[p1]` text
/// references back to the source path + chunk index.
#[derive(Debug, Clone)]
pub struct RetrievedKbEntry {
    /// Tag used in the system prompt: "g1", "g2", "p1", ... — used by
    /// the citation parser to look the entry up.
    pub tag: String,
    /// "global" | "project". Surfaced in the prompt and copied into
    /// the citation JSON.
    pub scope_label: &'static str,
    pub source_path: String,
    pub document_id: String,
    pub chunk_index: i32,
    pub text: String,
    /// 1-based page number authoritative from the chunker (PDFs only).
    /// `None` for non-PDF formats. Forwarded into the citation JSON so
    /// the DocPanel can scroll directly to the right page instead of
    /// falling back to text-search.
    pub page: Option<i64>,
}

/// Maximum cosine distance accepted for a chunk to be included. Values
/// above this threshold are noise rather than relevant context — but
/// 0.6 was too aggressive for cross-lingual queries (e.g. asking in
/// English about an Italian-language GDPR), where multilingual-e5
/// similarities cluster ~0.05-0.10 lower than monolingual. With an
/// English question against an Italian corpus doc we observed valid
/// matches falling around 0.62-0.68 and getting culled, leading to
/// "no relevant passages found" answers despite the doc being
/// retrievable in principle. 0.75 still excludes cosine-distant
/// noise while admitting cross-lingual paraphrases.
#[cfg(feature = "rag")]
const KB_DISTANCE_THRESHOLD: f32 = 0.75;

/// Reciprocal Rank Fusion (k=60) — merge two ranked chunk lists into
/// one, deduplicated by `(document_id, chunk_index)`. Each survivor
/// keeps the *minimum* distance across the two source lists so the
/// downstream `KB_DISTANCE_THRESHOLD` filter still has a meaningful
/// value.
#[cfg(feature = "rag")]
fn reciprocal_rank_fuse(
    primary: Vec<crate::embeddings::service::RetrievedChunk>,
    secondary: Vec<crate::embeddings::service::RetrievedChunk>,
    target: usize,
) -> Vec<crate::embeddings::service::RetrievedChunk> {
    use crate::embeddings::service::RetrievedChunk;
    const RRF_K: f32 = 60.0;

    let mut by_key: std::collections::HashMap<(String, i32), (RetrievedChunk, f32)> =
        std::collections::HashMap::new();
    let mut score: std::collections::HashMap<(String, i32), f32> =
        std::collections::HashMap::new();

    for (rank, c) in primary.into_iter().enumerate() {
        let key = (c.document_id.clone(), c.chunk_index);
        let s = 1.0 / (RRF_K + (rank as f32) + 1.0);
        *score.entry(key.clone()).or_insert(0.0) += s;
        let entry = by_key
            .entry(key)
            .or_insert((c.clone(), c.distance));
        if c.distance < entry.1 {
            entry.1 = c.distance;
        }
    }
    for (rank, c) in secondary.into_iter().enumerate() {
        let key = (c.document_id.clone(), c.chunk_index);
        let s = 1.0 / (RRF_K + (rank as f32) + 1.0);
        *score.entry(key.clone()).or_insert(0.0) += s;
        let entry = by_key
            .entry(key)
            .or_insert((c.clone(), c.distance));
        if c.distance < entry.1 {
            entry.1 = c.distance;
        }
    }

    let mut merged: Vec<(f32, RetrievedChunk)> = by_key
        .into_iter()
        .map(|(key, (mut chunk, min_dist))| {
            chunk.distance = min_dist;
            let s = *score.get(&key).unwrap_or(&0.0);
            (s, chunk)
        })
        .collect();
    merged.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    merged.into_iter().take(target).map(|(_, c)| c).collect()
}

/// Run vector retrieval against the user's library and return the
/// chunks ready to be rendered into the system prompt. The scope is
/// inferred from the chat's project_id + the project's isolation_mode.
/// Returns an empty vec when:
///  - the rag feature isn't compiled in
///  - the embedding service isn't initialised
///  - the user has no indexed documents in the relevant pool
///  - all retrieved chunks are above the distance threshold
///
/// When HyDE is enabled (`MIKE_HYDE_ENABLED=1` env var), the function
/// also fires a one-shot LLM call to draft a hypothetical answer (see
/// `crate::llm::hyde`) and runs a second KNN against that hypothesis;
/// the two result sets are fused via Reciprocal Rank Fusion before the
/// distance threshold is applied.
#[cfg(feature = "rag")]
async fn retrieve_kb_chunks(
    state: &AppState,
    user_id: &str,
    chat_id: &str,
    user_query: &str,
    top_k_target: usize,
) -> Vec<RetrievedKbEntry> {
    let Some(svc) = state.embeddings.as_ref() else {
        return Vec::new();
    };
    if user_query.trim().is_empty() {
        return Vec::new();
    }

    let hyde_enabled = std::env::var("MIKE_HYDE_ENABLED")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);

    // Resolve scope: chat → project_id → isolation_mode.
    let project_row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT project_id FROM chats WHERE id = ?",
    )
    .bind(chat_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten();
    let project_id: Option<String> = project_row.and_then(|(p,)| p);

    use crate::embeddings::service::SearchScope;
    let is_strict_project = match project_id.as_deref() {
        None => false,
        Some(pid) => {
            let mode: Option<(String,)> = sqlx::query_as(
                "SELECT isolation_mode FROM projects WHERE id = ?",
            )
            .bind(pid)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            mode.as_ref().map(|(m,)| m.as_str()) == Some("strict")
        }
    };

    tracing::info!(
        "[rag] retrieve_kb_chunks user={user_id}: hyde={}, top_k={top_k_target}",
        if hyde_enabled { "ON" } else { "OFF" },
    );

    // Primary pass — vanilla embedding of the user's query.
    let scope_primary = match project_id.as_deref() {
        None => SearchScope::Global,
        Some(p) if is_strict_project => SearchScope::ProjectStrict(p),
        Some(p) => SearchScope::ProjectShared(p),
    };
    let primary_result = svc
        .search(user_id, scope_primary, user_query, top_k_target)
        .await;

    // Optional HyDE pass — draft a hypothetical answer, embed it, run
    // a second KNN. Errors are non-fatal: log and degrade to primary.
    let hyde_result = if hyde_enabled {
        let user_settings = fetch_llm_settings(&state.db, user_id).await.ok();
        let raw_model = user_settings
            .as_ref()
            .and_then(|s| s.main_model.clone())
            .unwrap_or_else(|| "gemini-2.5-flash".to_string());

        let local_config = if raw_model.starts_with("local:") || raw_model.starts_with("openai:") {
            user_settings.as_ref().and_then(|s| {
                let (base, key, mname) = if raw_model.starts_with("openai:") {
                    (
                        s.openai_api_key.as_ref().map(|_| "https://api.openai.com/v1".to_string()).unwrap_or_default(),
                        s.openai_api_key.clone(),
                        s.openai_model.clone().unwrap_or_default(),
                    )
                } else {
                    (
                        s.local_base_url.clone().unwrap_or_default(),
                        s.local_api_key.clone(),
                        s.local_model.clone().unwrap_or_default(),
                    )
                };
                if base.is_empty() { None } else {
                    Some(LocalConfig {
                        base_url: base,
                        api_key: key.filter(|k| !k.trim().is_empty()),
                        model: {
                            let stripped = llm::strip_model_prefix(&raw_model).to_string();
                            if stripped.is_empty() { mname } else { stripped }
                        },
                    })
                }
            })
        } else { None };

        let creds = crate::llm::hyde::HydeCreds {
            local_config,
            claude_api_key: user_settings.as_ref().and_then(|s| s.claude_api_key.clone()),
            gemini_api_key: user_settings.as_ref().and_then(|s| s.gemini_api_key.clone()),
            gemini_region: user_settings.as_ref().and_then(|s| s.gemini_region.clone()),
        };

        match crate::llm::hyde::generate_hypothesis(user_query, &raw_model, &creds).await {
            Ok(hypothesis) if !hypothesis.trim().is_empty() => {
                tracing::info!(
                    "[rag] HyDE hypothesis for user={user_id} → {} chars",
                    hypothesis.chars().count(),
                );
                let scope_hyde = match project_id.as_deref() {
                    None => SearchScope::Global,
                    Some(p) if is_strict_project => SearchScope::ProjectStrict(p),
                    Some(p) => SearchScope::ProjectShared(p),
                };
                Some(
                    svc.search(user_id, scope_hyde, &hypothesis, top_k_target)
                        .await,
                )
            }
            Ok(_) => {
                tracing::warn!("[rag] HyDE returned empty hypothesis user={user_id} — primary only");
                None
            }
            Err(e) => {
                tracing::warn!("[rag] HyDE call failed user={user_id}: {e:#} — primary only");
                None
            }
        }
    } else {
        None
    };

    // Fuse rankings: without HyDE just the primary list; with HyDE use
    // Reciprocal Rank Fusion (k=60).
    let chunks = match (primary_result, hyde_result) {
        (Ok(primary), None) => primary,
        (Err(e), None) => {
            tracing::warn!("[rag] retrieval failed: {e}");
            return Vec::new();
        }
        (Ok(primary), Some(Ok(hyde_chunks))) => {
            tracing::info!(
                "[rag] RRF merge: primary={} hyde={} chunks before fusion",
                primary.len(),
                hyde_chunks.len(),
            );
            reciprocal_rank_fuse(primary, hyde_chunks, top_k_target)
        }
        (Ok(primary), Some(Err(e))) => {
            tracing::warn!("[rag] HyDE KNN failed: {e} — using primary only");
            primary
        }
        (Err(e), Some(Ok(hyde_chunks))) => {
            tracing::warn!("[rag] primary KNN failed: {e} — using HyDE only");
            hyde_chunks
        }
        (Err(ep), Some(Err(eh))) => {
            tracing::warn!("[rag] both retrievals failed: primary={ep} hyde={eh}");
            return Vec::new();
        }
    };

    // Filter by distance + label per-chunk based on whether the row had
    // project_id NULL (global) or a value (project). We can't know the
    // raw project_id from the public RetrievedChunk; instead, we look
    // it up in synced_files via the document_id — cheap and accurate.
    let mut out: Vec<RetrievedKbEntry> = Vec::new();
    let mut g_idx = 0u32;
    let mut p_idx = 0u32;
    for c in chunks.into_iter().filter(|c| c.distance <= KB_DISTANCE_THRESHOLD) {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT project_id FROM synced_files WHERE document_id = ?",
        )
        .bind(&c.document_id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();
        let is_global = row.and_then(|(p,)| p).is_none();
        let (tag, scope_label) = if is_global {
            g_idx += 1;
            (format!("g{g_idx}"), "global")
        } else {
            p_idx += 1;
            (format!("p{p_idx}"), "project")
        };
        out.push(RetrievedKbEntry {
            tag,
            scope_label,
            source_path: c.source_path,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            page: c.page,
        });
    }
    out
}

#[cfg(not(feature = "rag"))]
async fn retrieve_kb_chunks(
    _state: &AppState,
    _user_id: &str,
    _chat_id: &str,
    _user_query: &str,
    _top_k_target: usize,
) -> Vec<RetrievedKbEntry> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Case-scoped chat context
// ---------------------------------------------------------------------------

/// Injected into `stream_chat_root` when the chat is case-scoped.
/// Carries pre-loaded case metadata, document text, and findings so the
/// streaming handler can prepend a case system prompt and scope RAG.
pub(crate) struct CaseContext {
    pub case_id: String,
    pub case_system_prompt: String,
    pub case_doc_ids: Vec<String>,
    pub case_doc_labels: HashMap<String, String>,
}

pub(crate) fn build_case_system_prompt(
    title: &str,
    court: Option<&str>,
    parties: Option<&Value>,
    status: &str,
    docs: &[(String, String, Option<String>)],
    findings: &[(String, String, String)],
) -> String {
    let mut s = String::from(
        "<CASE CONTEXT — you are advising on a specific case>\n",
    );
    s.push_str(&format!("- Title: {}\n", title));
    if let Some(c) = court {
        s.push_str(&format!("- Court: {}\n", c));
    }
    if let Some(p) = parties {
        if let Some(arr) = p.as_array() {
            let names: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
            if !names.is_empty() {
                s.push_str(&format!("- Parties: {}\n", names.join(", ")));
            }
        } else if let Some(obj) = p.as_object() {
            for (role, name) in obj {
                s.push_str(&format!("- {}: {}\n", role, name));
            }
        } else if let Some(text) = p.as_str() {
            s.push_str(&format!("- Parties: {}\n", text));
        }
    }
    s.push_str(&format!("- Current Status: {}\n", status));

    if !docs.is_empty() {
        s.push_str("\nAttached case documents:\n");
        for (i, (doc_id, filename, doc_type)) in docs.iter().enumerate() {
            let label = doc_type.as_deref().unwrap_or("document");
            s.push_str(&format!(
                "  [case-doc-{}] {}: {} (id: {})\n",
                i, label, filename, doc_id
            ));
        }
    }

    if !findings.is_empty() {
        s.push_str("\nPrior analysis findings (from senior counsel agents):\n");
        for (agent, _ftype, content_json) in findings {
            if let Ok(content) = serde_json::from_str::<Value>(content_json) {
                let agent_label = agent.replace('_', " ");
                if let Some(obj) = content.as_object() {
                    s.push_str(&format!("  {} —", agent_label));
                    for (key, val) in obj {
                        if key == "status" || key == "agent" || key == "case_id" || key == "document_count" {
                            continue;
                        }
                        let text = val.as_str().map(|s| s.to_string())
                            .unwrap_or_else(|| val.to_string());
                        s.push_str(&format!(" {}: {};", key, text));
                    }
                    s.push('\n');
                } else {
                    s.push_str(&format!("  {}: {}\n", agent_label, content_json));
                }
            } else {
                s.push_str(&format!("  {}: {}\n", agent, content_json));
            }
        }
    }

    s.push_str(
        "\nYou have full context on this case. Answer the lawyer's questions, \
         draft documents, or run analyses with this context in mind. Cite \
         specific case documents when referencing facts.\n\
         </CASE CONTEXT>\n",
    );
    s
}

#[cfg(feature = "rag")]
async fn retrieve_kb_chunks_for_case(
    state: &AppState,
    user_id: &str,
    case_doc_ids: &[String],
    user_query: &str,
    top_k_target: usize,
) -> Vec<RetrievedKbEntry> {
    let Some(svc) = state.embeddings.as_ref() else {
        return Vec::new();
    };
    if user_query.trim().is_empty() || case_doc_ids.is_empty() {
        return Vec::new();
    }

    use crate::embeddings::service::SearchScope;
    let chunks_result = svc
        .search(user_id, SearchScope::DocumentSet(case_doc_ids), user_query, top_k_target)
        .await;

    let chunks = match chunks_result {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[rag] case-scoped retrieval failed: {e}");
            return Vec::new();
        }
    };

    let mut out: Vec<RetrievedKbEntry> = Vec::new();
    let mut idx = 0u32;
    for c in chunks.into_iter().filter(|c| c.distance <= KB_DISTANCE_THRESHOLD) {
        idx += 1;
        out.push(RetrievedKbEntry {
            tag: format!("p{idx}"),
            scope_label: "case",
            source_path: c.source_path,
            document_id: c.document_id,
            chunk_index: c.chunk_index,
            text: c.text,
            page: c.page,
        });
    }
    out
}

#[cfg(not(feature = "rag"))]
async fn retrieve_kb_chunks_for_case(
    _state: &AppState,
    _user_id: &str,
    _case_doc_ids: &[String],
    _user_query: &str,
    _top_k_target: usize,
) -> Vec<RetrievedKbEntry> {
    Vec::new()
}

/// Lightweight description of a doc in the user's authoritative-corpus
/// library — enough to render the "you have these documents indexed"
/// section of the system prompt without dragging the full text in.
struct CorpusInventoryEntry {
    corpus_id: String,
    identifier: String,
    title: String,
    language: String,
    status: String,
}

/// Pull the list of corpus-sourced documents the user has indexed.
/// Used to seed the library-inventory section of the system prompt
/// so the model orients itself even when the user's question doesn't
/// trigger a semantic-retrieval hit on those documents.
async fn list_indexed_corpus_docs(
    state: &AppState,
    user_id: &str,
) -> Vec<CorpusInventoryEntry> {
    let rows: Vec<(String, String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT corpus_id, corpus_identifier, filename, corpus_language, status \
         FROM documents \
         WHERE user_id = ? AND corpus_id IS NOT NULL AND corpus_identifier IS NOT NULL \
         ORDER BY created_at DESC \
         LIMIT 50",
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|(corpus_id, identifier, title, language, status)| CorpusInventoryEntry {
            corpus_id,
            identifier,
            title,
            language: language.unwrap_or_default(),
            status,
        })
        .collect()
}

/// Render the library inventory as a system-prompt block. Only docs
/// that have been **fully indexed** (status = "ready") are listed as
/// retrievable; documents in "syncing" or "interrupted" state are
/// surfaced separately so the model can tell the user about them but
/// shouldn't pretend to have their text available.
fn build_library_inventory_prompt(entries: &[CorpusInventoryEntry]) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let mut ready: Vec<&CorpusInventoryEntry> = Vec::new();
    let mut other: Vec<&CorpusInventoryEntry> = Vec::new();
    for e in entries {
        if e.status == "ready" {
            ready.push(e);
        } else {
            other.push(e);
        }
    }

    let mut s = String::from(
        "<USER LIBRARY — authoritative corpus documents indexed for this user>\n\
         This is an awareness list ONLY. The documents below are indexed and \
         retrievable. When a question matches one of them, the relevant \
         passages appear in the <KNOWLEDGE BASE> block above tagged \
         [g1]/[g2]/[p1]/...\n\
         \n\
         IF <KNOWLEDGE BASE> CONTAINS [gN]/[pN] TAGS:\n\
           · Use them and cite via the rules in that section. The user's \
             documents are authoritative.\n\
         \n\
         IF <KNOWLEDGE BASE> IS EMPTY OR HAS NO RELEVANT MATCH:\n\
           · The semantic match was below threshold, NOT that the document \
             is missing. Do NOT say \"not currently loaded\" or \"not \
             available for direct querying\" — those phrasings are wrong \
             and confuse the user.\n\
           · You may answer from general knowledge if confident, BUT state \
             plainly that the answer isn't grounded in the user's library, \
             and suggest the user re-phrase or attach the doc directly if \
             they want a citation-backed answer.\n\
         \n\
         CITATION DOC_ID RULES (mandatory):\n\
           · NEVER use the inventory identifiers below (e.g. \"32016R0679\", \
             \"eurlex_32016R0679\") as `doc_id` in <CITATIONS>. Those are \
             corpus references, NOT citation handles.\n\
           · NEVER invent doc-N labels when no files are attached to this \
             chat — only use doc-N if the user actually attached a file.\n\
           · The ONLY valid `doc_id` values are: (a) the [gN]/[pN] tags from \
             <KNOWLEDGE BASE>, or (b) the doc-N labels of files actually \
             attached to this chat. Anything else gets dropped or mis-routed.\n\
         \n\
         If asked \"what do you have?\" or \"do you know X?\", answer based on \
         this list (no citation needed for the meta-answer).\n\n",
    );
    if !ready.is_empty() {
        s.push_str("Indexed and ready:\n");
        for e in &ready {
            s.push_str(&format!(
                "  · [{corpus}] {ident}: {title} ({lang})\n",
                corpus = e.corpus_id,
                ident = e.identifier,
                title = e.title,
                lang = e.language.to_uppercase(),
            ));
        }
    }
    if !other.is_empty() {
        s.push_str("\nIndexing in progress / interrupted (not yet retrievable):\n");
        for e in &other {
            s.push_str(&format!(
                "  · [{corpus}] {ident}: {title} — {status}\n",
                corpus = e.corpus_id,
                ident = e.identifier,
                title = e.title,
                status = e.status,
            ));
        }
    }
    s
}

/// Render retrieved chunks as a `<KNOWLEDGE BASE>` section. Empty
/// string when there are no chunks — the caller skips the section
/// entirely so we don't pollute the prompt with empty headers.
fn build_kb_system_prompt(chunks: &[RetrievedKbEntry]) -> String {
    if chunks.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "<KNOWLEDGE BASE — retrieved excerpts (not full documents)>\n\
         These are partial passages selected by similarity to the user's question. \
         They come from the user's indexed library; they are NOT authoritative full \
         documents. If you need full context for any of them, either call the \
         `search_kb` tool to fetch more passages from the same area, or ask the \
         user to attach the document via the paperclip.\n\n",
    );
    for c in chunks {
        let basename = std::path::Path::new(&c.source_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| c.source_path.clone());
        s.push_str(&format!(
            "[{tag}] {scope} · {fname} (chunk {idx}):\n«{text}»\n\n",
            tag = c.tag,
            scope = c.scope_label,
            fname = basename,
            idx = c.chunk_index,
            text = c.text,
        ));
    }
    s.push_str(
        "CITING THESE PASSAGES (mandatory — read carefully):\n\
         When you cite ANY of the passages above:\n\
           1. Write the [tag] VERBATIM in your prose at the point of \
              reference — for example: \"Articolo 35 GDPR [g1]\".\n\
           2. INCLUDE a matching entry in the <CITATIONS> JSON block at \
              the end of your response. The KB tag IS your citation \
              identifier — these passages count as document references \
              and the <CITATIONS> block applies to them exactly the same \
              way it applies to attached documents.\n\
           3. In the <CITATIONS> entry, set \"doc_id\" to the EXACT tag \
              you used inline (\"g1\", \"g2\", \"p1\", etc.) — NOT a \
              number, NOT \"doc-0\", NOT a filename.\n\
           4. The `quote` field MUST be a verbatim substring of the \
              passage text shown above between «…» — do NOT translate, \
              paraphrase, summarise, or correct typography. Copy the \
              exact characters (including the original language and \
              punctuation). The viewer text-searches the PDF for this \
              quote to highlight it; any deviation breaks the highlight.\n\
              If you want to discuss the passage in the user's language \
              (e.g. translate while answering), do that in your prose, \
              but keep the JSON `quote` in the original.\n\n\
         Example for KB tags only:\n\
         \n\
         Prose: \"L'articolo 35 GDPR richiede una DPIA [g1].\"\n\
         <CITATIONS>\n\
         [\n  {\"doc_id\": \"g1\", \"quote\": \"...\"}\n]\n\
         </CITATIONS>\n\n\
         Skipping the <CITATIONS> block when you used [gN]/[pN] tags is \
         a bug — the UI relies on it to render the clickable pill that \
         opens the source document. The block is REQUIRED whenever any \
         [tag] appears in your prose.\n\
         </KNOWLEDGE BASE>\n",
    );
    s
}

// ---------------------------------------------------------------------------
// Indian Kanoon live search — fallback when RAG has no relevant chunks
// ---------------------------------------------------------------------------

const IK_API_BASE: &str = "https://api.indiankanoon.org";

struct IKResult {
    tid: i64,
    title: String,
    court: String,
    snippet: String,
    source_url: String,
    docfragment_url: String,
}

async fn search_indian_kanoon(
    state: &AppState,
    user_id: &str,
    query: &str,
) -> Vec<IKResult> {
    let ik_key: Option<String> = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT ik_api_key FROM corpus_settings WHERE user_id = ? AND corpus_id = ?",
    )
    .bind(user_id)
    .bind("indian-kanoon")
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .and_then(|(k,)| k)
    .or_else(|| std::env::var("IK_API_KEY").ok());

    let ik_key = match ik_key {
        Some(k) => k,
        None => return Vec::new(),
    };

    let keywords = extract_ik_keywords(query);
    let client = match reqwest::Client::builder()
        .user_agent("MikeRust/0.1")
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let resp = match client
        .post(format!("{IK_API_BASE}/search/"))
        .header("Authorization", format!("Token {ik_key}"))
        .form(&[("formInput", keywords.as_str()), ("pagenum", "0")])
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[ik-chat] search failed: {e}");
            return Vec::new();
        }
    };

    let data: Value = match resp.json().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("[ik-chat] parse failed: {e}");
            return Vec::new();
        }
    };

    let docs = data["docs"].as_array().cloned().unwrap_or_default();
    docs.iter().take(5).map(|doc| {
        let tid = doc["tid"].as_i64().unwrap_or(0);
        IKResult {
            tid,
            title: doc["title"].as_str().unwrap_or("Unknown").to_string(),
            court: doc["docsource"].as_str().unwrap_or("").to_string(),
            snippet: doc["headline"].as_str().unwrap_or("").to_string(),
            source_url: format!("https://indiankanoon.org/doc/{tid}/"),
            docfragment_url: format!("/indian-kanoon/docfragment/{tid}?formInput={}",
                url_encode(&keywords)),
        }
    }).collect()
}

fn extract_ik_keywords(query: &str) -> String {
    let stop_words = [
        "what","is","the","a","an","in","of","for","on","to","and","or",
        "by","with","from","as","at","it","be","has","have","been","was",
        "were","are","does","do","did","can","will","shall","may","would",
        "could","should","about","tell","me","show","find","give","explain",
        "how","when","where","which","who","whom","whose","why","please",
        "according","section","under","provision","provisions","related",
        "relevant","case","cases","law","laws","legal","judgment","court",
        "supreme","high","india","indian","there","any","this","that",
        "these","those","i","you","we","they","he","she",
    ];
    let lower = query.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace()
        .filter(|w| {
            let clean = w.trim_matches(|c: char| !c.is_alphanumeric());
            !stop_words.contains(&clean) && clean.len() > 1
        })
        .take(5)
        .collect();
    if words.is_empty() { query.to_string() } else { words.join(" ") }
}

fn url_encode(s: &str) -> String {
    s.chars().map(|c| match c {
        'A'..='Z'|'a'..='z'|'0'..='9'|'-'|'_'|'.'|'~' => c.to_string(),
        ' ' => "%20".to_string(),
        other => format!("%{:02X}", other as u8),
    }).collect()
}

fn build_ik_system_prompt(results: &[IKResult]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut s = String::from(
        "<INDIAN KANOON — live search results>\n\
         These cases were found by searching Indian Kanoon for the user's query. \
         They are real judgments from Indian courts. Use them to ground your answer. \
         If none are relevant, say so and fall back to general knowledge (clearly labeled).\n\n",
    );
    for (i, r) in results.iter().enumerate() {
        s.push_str(&format!(
            "[ik{}] {title}\n  Court: {court}\n  Snippet: {snippet}\n  URL: {url}\n\n",
            i + 1,
            title = r.title,
            court = r.court,
            snippet = r.snippet,
            url = r.source_url,
        ));
    }
    s.push_str(
        "CITING IK RESULTS:\n\
         · ALWAYS cite Indian Kanoon cases as clickable Markdown links.\n\
         · Format: [Case Title](https://indiankanoon.org/doc/XXXXX/) — \
         use the exact URL provided above for each case.\n\
         · Example: [Visionaries Media vs Star India](https://indiankanoon.org/doc/84054/)\n\
         · Do NOT write bare [ik1] or [ik2] tags — always use the Markdown link format.\n\
         · If you use general knowledge instead, START that section with: \
         \"[Note: the following is based on general legal knowledge, not on \
         specific case law found in Indian Kanoon.]\"\n\
         </INDIAN KANOON>\n",
    );
    s
}

/// Scan a finalized assistant response for Indian Kanoon Markdown
/// citations that are NOT accompanied by a verbatim quote from the
/// cited judgment. The system prompt requires every case citation to
/// include a `The Court held: "..."` passage drawn from this turn's
/// tool output, but the model often skips this for famous cases it
/// "knows" from training data. This validator is the safety net.
///
/// Returns a list of cited case titles that appear to be missing the
/// verbatim quote so the caller can surface a warning to the user.
/// Returns an empty vec if every citation is properly quote-grounded
/// (or if there are no citations at all).
fn validate_kanoon_quotes(response: &str) -> Vec<String> {
    let mut missing = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Scan for Markdown links pointing at indiankanoon.org/doc/{tid}.
    // The model emits these as `[Case Title](https://indiankanoon.org/doc/12345/)`.
    let pattern = match regex::Regex::new(r"\[([^\]]+)\]\(https?://(?:www\.)?indiankanoon\.org/doc/(\d+)") {
        Ok(re) => re,
        Err(e) => {
            tracing::warn!("[chat] validate_kanoon_quotes regex compile failed: {e}");
            return missing;
        }
    };
    for cap in pattern.captures_iter(response) {
        let Some(title_match) = cap.get(1) else { continue };
        let title = title_match.as_str().trim().to_string();
        if title.is_empty() || !seen.insert(title.clone()) {
            // Already checked this case; treat first-occurrence verdict as canonical.
            continue;
        }
        let link_start = title_match.start();
        // Inspect a window around the citation — 200 chars before
        // (rarely matters) and 1500 chars after the link to cover both
        // inline `The Court held: "..."` and a Markdown blockquote that
        // immediately follows the citation paragraph.
        let win_start = link_start.saturating_sub(200);
        let win_end = (link_start + 1500).min(response.len());
        let window = &response[win_start..win_end];
        if window_has_verbatim_quote(window) {
            continue;
        }
        missing.push(title);
    }
    missing
}

/// Decide whether a slice of the assistant response contains a
/// verbatim quote suitable as grounding for a case citation. Two
/// recognised shapes:
///   1. Inline: `The Court held: "verbatim passage"` (case-insensitive,
///      30+ chars between the quotes).
///   2. Markdown blockquote: a line starting with `>` containing at
///      least 40 chars of text — the model often renders the verbatim
///      passage as a standalone blockquote after the citation
///      paragraph.
fn window_has_verbatim_quote(window: &str) -> bool {
    // Inline pattern: "The Court held:" / "The court held:" / "The SC held:" etc.,
    // immediately followed by a double-quoted passage of 30+ chars.
    let lc = window.to_ascii_lowercase();
    let cues: &[&str] = &[
        "the court held:",
        "the supreme court held:",
        "the sc held:",
        "the hc held:",
        "the bench held:",
        "the court observed:",
        "the court reasoned:",
        "the court ruled:",
    ];
    for cue in cues {
        if let Some(idx) = lc.find(cue) {
            // Look at the next ~600 chars after the cue for a quoted passage.
            let after_start = idx + cue.len();
            let after_end = (after_start + 600).min(window.len());
            if after_end > after_start {
                let after = &window[after_start..after_end];
                if has_quoted_passage_of_min_len(after, 30) {
                    return true;
                }
            }
        }
    }
    // Markdown blockquote pattern: a non-trivial `>` line in the window.
    for line in window.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('>') {
            let inner = rest.trim();
            if inner.len() >= 40 {
                return true;
            }
        }
    }
    false
}

/// Check whether `s` contains a double-quoted (curly or straight)
/// passage of at least `min_len` characters between the quotes.
fn has_quoted_passage_of_min_len(s: &str, min_len: usize) -> bool {
    // Straight double quotes.
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' || chars[i] == '\u{201C}' /* " */ {
            // Find the closing quote (matching style or any-double).
            let mut j = i + 1;
            while j < chars.len() {
                if chars[j] == '"' || chars[j] == '\u{201D}' /* " */ {
                    break;
                }
                j += 1;
            }
            if j > i + 1 && (j - i - 1) >= min_len {
                return true;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    false
}

/// Build the user-visible warning block that gets appended to a chat
/// response when one or more cited cases lack a verbatim quote.
fn build_quote_warning(missing: &[String]) -> String {
    let mut s = String::from(
        "\n\n---\n\n⚠ **Citations missing verbatim quote — verify before relying.**  \n",
    );
    s.push_str(
        "The system prompt requires every cited case to be accompanied by a verbatim quote from \
         the judgment. The following citation(s) in this response do not have that grounding:\n\n",
    );
    for title in missing {
        s.push_str(&format!("- *{title}*\n"));
    }
    s.push_str(
        "\nFor each, click the case link to read the canonical text on Indian Kanoon, or use the \
         Verify on eCourts pill to cross-check against the official court record. If the case \
         turns out to be misstated, please flag it — this warning is generated automatically by \
         a post-response check on Mike's backend.\n",
    );
    s
}

/// Remove the `[Page N]` markers our PDF scanner prepends to each
/// extracted page when it concatenates them. The model often copies
/// these markers verbatim into citation quotes (because they appear
/// inside the chunk text it was given), but they aren't actually
/// present in the underlying PDF — leaving them in breaks the
/// PDF.js text-search highlight in the DocPanel viewer.
///
/// Strategy: drop standalone `[Page N]` tokens (with surrounding
/// whitespace), then collapse any double-spaces / leading newlines
/// the removal might leave behind. Quotes that don't contain a marker
/// pass through unchanged.
fn strip_page_markers(quote: &str) -> String {
    let mut out = String::with_capacity(quote.len());
    let bytes = quote.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Detect `[Page <digits>]` at byte i.
        if bytes[i] == b'[' && bytes.get(i..i + 6) == Some(b"[Page ") {
            let num_start = i + 6;
            let mut j = num_start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > num_start && bytes.get(j) == Some(&b']') {
                // Skip the marker and a single trailing whitespace
                // character (newline or space) if present.
                i = j + 1;
                if i < bytes.len() && (bytes[i] == b'\n' || bytes[i] == b' ') {
                    i += 1;
                }
                continue;
            }
        }
        out.push(quote[i..].chars().next().unwrap());
        i += quote[i..].chars().next().unwrap().len_utf8();
    }
    // Trim and collapse the most common leftover artefact (leading
    // newline that remained when the marker was at the very start).
    out.trim_start().to_string()
}

/// Walk a citations JSON array and rewrite each entry's `quote` field
/// through `strip_page_markers`. Used by the chat-history loader so
/// citations persisted before the strip-on-write fix still render
/// without literal `[Page N]` contamination.
fn sanitise_annotations_quotes(value: Value) -> Value {
    let Value::Array(items) = value else {
        return value;
    };
    let cleaned = items
        .into_iter()
        .map(|item| {
            let Value::Object(mut obj) = item else {
                return item;
            };
            if let Some(q) = obj.get("quote").and_then(|v| v.as_str()) {
                let stripped = strip_page_markers(q);
                if stripped != q {
                    obj.insert("quote".into(), Value::String(stripped));
                }
            }
            Value::Object(obj)
        })
        .collect();
    Value::Array(cleaned)
}

/// Fallback path that synthesises citation entries from the inline
/// `[gN]`/`[pN]` markers in the assistant's response when the model
/// forgot to emit the trailing `<CITATIONS>` JSON block. Each unique
/// tag found in `text` that resolves to a `kb_by_tag` entry produces a
/// `{"doc_id": "<tag>", "quote": "..."}` shape that the downstream
/// resolver then enriches with `source: "kb"`, `path`, `page`, etc.
///
/// Returns `None` when `text` has no resolvable KB markers — caller
/// should treat that as "no citations" and ship an empty array.
fn synthesise_kb_citations_from_markers(
    text: &str,
    kb_by_tag: &HashMap<String, RetrievedKbEntry>,
) -> Option<Value> {
    use std::collections::BTreeSet;
    let re_iter = text.match_indices('[');
    let mut tags = BTreeSet::<String>::new();
    for (i, _) in re_iter {
        // Simple state machine: after `[` we accept `g|p` then digits then `]`.
        let bytes = text.as_bytes();
        if let Some(&b) = bytes.get(i + 1) {
            if b == b'g' || b == b'p' || b == b'G' || b == b'P' {
                let mut j = i + 2;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 2 && bytes.get(j) == Some(&b']') {
                    let tag = text[i + 1..j].to_ascii_lowercase();
                    if kb_by_tag.contains_key(&tag) {
                        tags.insert(tag);
                    }
                }
            }
        }
    }
    if tags.is_empty() {
        return None;
    }
    let arr: Vec<Value> = tags
        .into_iter()
        .map(|tag| {
            // Use a short prefix of the chunk text as the synthesized
            // quote so the DocPanel still has something to highlight.
            // The resolver further down stamps the authoritative page
            // and source path so the click-to-open path still works.
            let quote = kb_by_tag
                .get(&tag)
                .map(|e| {
                    let t = e.text.trim();
                    let cap = 200.min(t.len());
                    let mut end = cap;
                    while end < t.len() && !t.is_char_boundary(end) {
                        end -= 1;
                    }
                    t[..end].to_string()
                })
                .unwrap_or_default();
            json!({ "doc_id": tag, "quote": quote })
        })
        .collect();
    tracing::info!(
        "[chat] no <CITATIONS> block in response — synthesised {} citation(s) from inline KB markers",
        arr.len()
    );
    Some(Value::Array(arr))
}

type ApiResult = Result<Json<Value>, (StatusCode, Json<Value>)>;

fn err(status: StatusCode, msg: &str) -> (StatusCode, Json<Value>) {
    (status, Json(json!({"detail": msg})))
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_chats).post(post_chat_root))
        .route("/{id}", get(get_chat).patch(patch_chat).delete(delete_chat))
        .route("/{id}/messages", get(get_messages))
        .route("/{id}/message", axum::routing::post(post_message))
        .route("/{id}/generate-title", axum::routing::post(generate_title))
        .route("/client-tool-result", axum::routing::post(post_client_tool_result))
}

// ---------------------------------------------------------------------------
// POST /chat/client-tool-result — frontend returns results for client-side tools
// ---------------------------------------------------------------------------
#[derive(serde::Deserialize)]
struct ClientToolResult {
    request_id: String,
    result: String,
}

async fn post_client_tool_result(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Json(body): Json<ClientToolResult>,
) -> StatusCode {
    let sender = {
        state.client_tool_tx.lock().unwrap().remove(&body.request_id)
    };
    if let Some(tx) = sender {
        let _ = tx.send(body.result);
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ---------------------------------------------------------------------------
// GET /chat  — list chats for user
// ---------------------------------------------------------------------------
async fn list_chats(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> ApiResult {
    let rows: Vec<(String, String, Option<String>, Option<String>, String)> =
        sqlx::query_as(
            "SELECT id, user_id, project_id, title, updated_at \
             FROM chats WHERE user_id = ? AND case_id IS NULL ORDER BY updated_at DESC",
        )
        .bind(&auth.user_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let chats: Vec<Value> = rows
        .into_iter()
        .map(|(id, user_id, project_id, title, updated_at)| {
            json!({
                "id": id,
                "user_id": user_id,
                "project_id": project_id,
                "title": title,
                "updated_at": updated_at,
            })
        })
        .collect();

    Ok(Json(json!({ "chats": chats })))
}

// ---------------------------------------------------------------------------
// POST /chat — dispatched by body shape
//   - { messages: [...], chat_id?, model? }     → SSE streaming
//   - { project_id?, title? } (no messages)    → create chat record (JSON)
// ---------------------------------------------------------------------------
async fn post_chat_root(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Response {
    let has_messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    tracing::info!("[chat] POST / dispatch: has_messages={has_messages}, user={}", auth.username);

    if has_messages {
        return stream_chat_root(state, auth, body, None).await;
    }
    create_chat_record(state, auth, body).await
}

async fn create_chat_record(
    state: Arc<AppState>,
    auth: AuthUser,
    body: Value,
) -> Response {
    let project_id = body.get("project_id").and_then(|v| v.as_str()).map(|s| s.to_string());
    let title = body.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());

    let id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query(
        "INSERT INTO chats (id, user_id, project_id, title) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&auth.user_id)
    .bind(&project_id)
    .bind(&title)
    .execute(&state.db)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail": e.to_string()})),
        )
            .into_response();
    }

    (StatusCode::OK, Json(json!({ "id": id }))).into_response()
}

/// SSE handler for the upstream-Mike `POST /chat` shape.
/// Body: { messages: [{role, content}], chat_id?, model? }
/// Emits `data: {type: ...}` events that useAssistantChat parses.
///
/// When `case_ctx` is `Some`, the chat is case-scoped: the case system
/// prompt is prepended, KB retrieval is restricted to case documents,
/// and citations carry `source: "case_doc"`.
pub(crate) async fn stream_chat_root(
    state: Arc<AppState>,
    auth: AuthUser,
    body: Value,
    case_ctx: Option<CaseContext>,
) -> Response {
    let model_request = body.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
    let chat_id_in = body.get("chat_id").and_then(|v| v.as_str()).map(|s| s.to_string());

    // Resolve / create chat row
    let (chat_id, is_new_chat) = match chat_id_in.clone() {
        Some(id) => {
            let exists: Option<(String,)> = sqlx::query_as(
                "SELECT id FROM chats WHERE id = ? AND user_id = ?",
            )
            .bind(&id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
            if exists.is_none() {
                return (StatusCode::NOT_FOUND, Json(json!({"detail": "Chat not found"}))).into_response();
            }
            (id, false)
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let case_id_val = case_ctx.as_ref().map(|c| c.case_id.clone());
            if let Err(e) = sqlx::query(
                "INSERT INTO chats (id, user_id, project_id, title, case_id) VALUES (?, ?, NULL, NULL, ?)",
            )
            .bind(&id)
            .bind(&auth.user_id)
            .bind(&case_id_val)
            .execute(&state.db)
            .await
            {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"detail": e.to_string()}))).into_response();
            }
            (id, true)
        }
    };

    // Parse messages from the request body. The frontend sends the entire
    // running history; persist only the *last* user message.
    let messages_in: Vec<(String, String, Option<String>)> = body
        .get("messages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let role = m.get("role").and_then(|r| r.as_str())?.to_string();
                    let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("").to_string();
                    let reasoning_content = m.get("reasoning_content").and_then(|c| c.as_str()).map(|s| s.to_string());
                    Some((role, content, reasoning_content))
                })
                .collect()
        })
        .unwrap_or_default();

    // Collect document_ids from message-level attachments.
    let mut doc_ids: Vec<String> = Vec::new();
    if let Some(arr) = body.get("messages").and_then(|v| v.as_array()) {
        for m in arr {
            if let Some(files) = m.get("files").and_then(|v| v.as_array()) {
                for f in files {
                    if let Some(id) = f.get("document_id").and_then(|v| v.as_str()) {
                        if !doc_ids.iter().any(|x| x == id) {
                            doc_ids.push(id.to_string());
                        }
                    }
                }
            }
        }
    }

    // Stamp this chat onto any newly attached cache documents so
    // chat-deletion can sweep their on-disk files (see migration
    // 0013). Restrictions:
    //   - chat_id IS NULL  → don't reroute a doc already linked to
    //     another chat (its cleanup belongs there).
    //   - content_hash IS NOT NULL  → only true for cache uploads.
    //     Project-scoped or pre-cache docs must NOT inherit chat_id,
    //     otherwise deleting the chat would cascade them away even
    //     though they live in a project library.
    if !doc_ids.is_empty() {
        let placeholders = doc_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE documents SET chat_id = ? \
             WHERE user_id = ? \
               AND chat_id IS NULL \
               AND content_hash IS NOT NULL \
               AND id IN ({})",
            placeholders
        );
        let mut q = sqlx::query(&sql).bind(&chat_id).bind(&auth.user_id);
        for id in &doc_ids {
            q = q.bind(id);
        }
        match q.execute(&state.db).await {
            Ok(res) => tracing::info!(
                "[chat] linked {}/{} attached cache doc(s) to chat {}",
                res.rows_affected(),
                doc_ids.len(),
                chat_id
            ),
            Err(e) => tracing::warn!(
                "[chat] failed to link attached docs to chat {}: {}",
                chat_id,
                e
            ),
        }
    }

    if let Some((role, content, _)) = messages_in.last() {
        if role == "user" && !content.trim().is_empty() {
            let user_msg_id = uuid::Uuid::new_v4().to_string();
            let _ = sqlx::query(
                "INSERT INTO messages (id, chat_id, role, content) VALUES (?, ?, 'user', ?)",
            )
            .bind(&user_msg_id)
            .bind(&chat_id)
            .bind(content)
            .execute(&state.db)
            .await;
        }
    }

    let messages: Vec<Message> = messages_in
        .into_iter()
        .filter_map(|(role, content, reasoning_content)| {
            let r = match role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => return None,
            };
            Some(Message { role: r, content, images: vec![], tool_calls: vec![], tool_call_id: None, tool_name: None, reasoning_content })
        })
        .collect();

    // Resolve LLM config from the user's saved settings
    let user_settings = fetch_llm_settings(&state.db, &auth.user_id).await.ok();
    let raw_model = model_request
        .or_else(|| user_settings.as_ref().and_then(|s| s.main_model.clone()))
        .unwrap_or_else(|| "local:qwen2.5:3b".to_string());

    let local_config = if raw_model.starts_with("local:") || raw_model.starts_with("openai:") {
        user_settings.as_ref().and_then(|s| {
            let (base, key, mname) = if raw_model.starts_with("openai:") {
                (
                    s.openai_api_key.as_ref().map(|_| "https://api.openai.com/v1".to_string()).unwrap_or_default(),
                    s.openai_api_key.clone(),
                    s.openai_model.clone().unwrap_or_default(),
                )
            } else if s.active_provider.as_deref() == Some("deepseek") {
                (
                    "https://api.deepseek.com/v1".to_string(),
                    s.local_api_key.clone(),
                    s.local_model.clone().unwrap_or_default(),
                )
            } else {
                (
                    s.local_base_url.clone().unwrap_or_default(),
                    s.local_api_key.clone(),
                    s.local_model.clone().unwrap_or_default(),
                )
            };
            if base.is_empty() { None } else {
                Some(LocalConfig {
                    base_url: base,
                    api_key: key.filter(|s| !s.trim().is_empty()),
                    model: {
                        let stripped = llm::strip_model_prefix(&raw_model).to_string();
                        if stripped.is_empty() || stripped == "localllm-main" || stripped == "localllm-light" {
                            if mname.is_empty() { "llama3.2:3b".to_string() } else { mname }
                        } else {
                            stripped
                        }
                    },
                })
            }
        })
    } else { None };

    let vision_ok = llm::is_vision_capable(&raw_model);

    // Last user message is what we embed for retrieval. We deliberately
    // skip the conversation history because cosine on the running
    // history smears across topics; the latest turn captures intent
    // best. See the strategy doc for the rationale.
    let last_user_query: String = messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let kb_top_k = if doc_ids.is_empty() { 8 } else { 6 };

    // When case-scoped, merge the case document IDs into the attached
    // docs list so their full text is available, and scope KB retrieval.
    if let Some(ref ctx) = case_ctx {
        for cid in &ctx.case_doc_ids {
            if !doc_ids.iter().any(|x| x == cid) {
                doc_ids.push(cid.clone());
            }
        }
    }

    // Discover MCP, load attached docs, retrieve KB chunks, and pull
    // a library inventory in parallel. The inventory is what tells the
    // model "the user has the GDPR and AI Act in their indexed library"
    // even when the user's question doesn't surface those documents
    // via semantic match — without it, the model defaults to "I don't
    // have access to your synced documents."
    let case_doc_ids_for_rag: Vec<String> = case_ctx
        .as_ref()
        .map(|c| c.case_doc_ids.clone())
        .unwrap_or_default();
    let (attached_docs, mcp_servers, kb_chunks, library_inventory) = tokio::join!(
        load_attached_docs(&state, &auth.user_id, &doc_ids, vision_ok),
        discover_mcp_for_user(&state, &auth.user_id),
        async {
            if !case_doc_ids_for_rag.is_empty() {
                retrieve_kb_chunks_for_case(
                    &state, &auth.user_id, &case_doc_ids_for_rag,
                    &last_user_query, kb_top_k,
                ).await
            } else {
                retrieve_kb_chunks(
                    &state, &auth.user_id, &chat_id, &last_user_query, kb_top_k,
                ).await
            }
        },
        list_indexed_corpus_docs(&state, &auth.user_id),
    );

    // If RAG returned fewer than 3 relevant chunks, try Indian Kanoon
    // as a live fallback. IK results are real judgments from Indian courts.
    let ik_results = if kb_chunks.len() < 3 {
        search_indian_kanoon(&state, &auth.user_id, &last_user_query).await
    } else {
        Vec::new()
    };

    let pref_context = if case_ctx.is_some() {
        crate::preferences::PreferenceContext::CasePrep
    } else {
        crate::preferences::PreferenceContext::GeneralChat
    };
    let case_id_for_prefs = case_ctx.as_ref().map(|c| c.case_id.as_str());
    let effective_prefs = crate::preferences::load_effective_preferences(
        &state.db, &auth.user_id, case_id_for_prefs, pref_context,
    ).await;
    let personalization_prompt = crate::preferences::format_preferences_prompt(&effective_prefs);

    // Compose: Mike base + library inventory + KB excerpts + IK results
    // + attached full-text + MCP.
    let is_tiny = raw_model.contains("3b") || raw_model.contains("2b") || raw_model.contains("1.5b");
    let is_finetuned = raw_model.contains("mike-legal") || raw_model.contains("mike_legal");
    let is_deepseek = user_settings.as_ref()
        .map_or(false, |s| s.active_provider.as_deref() == Some("deepseek"));
    let is_local_model = local_config.is_some() && !is_deepseek;
    let is_small = (is_tiny || is_local_model) && !is_finetuned;

    let inventory_prompt = if is_small {
        String::new()
    } else {
        build_library_inventory_prompt(&library_inventory)
    };
    let mcp_prompt = build_mcp_system_prompt(&mcp_servers);
    let docs_prompt = build_doc_system_prompt(&attached_docs);
    let kb_prompt = if is_small {
        String::new()
    } else {
        build_kb_system_prompt(&kb_chunks)
    };
    let ik_prompt = if is_small {
        String::new()
    } else {
        build_ik_system_prompt(&ik_results)
    };
    let mut sections: Vec<String> = Vec::new();
    if is_local_model && is_tiny {
        sections.push(
            "You are Mike, an expert legal clerk. Your only job is to draft formal legal templates.\n\n\
             ABSOLUTE RULES:\n\
             1. DO NOT write any introduction, greeting, preamble, or conversational text (e.g., do NOT say 'Sure, here is...' or 'Here is a draft...'). Start the document immediately with its legal title.\n\
             2. If the user has not provided specific names, dates, addresses, or numbers, leave them blank as ________ for the user to fill in. Do NOT invent or fabricate details.\n\
             3. Write the entire document in full, in a professional legal tone with numbered paragraphs starting with 'That, '.\n\n\
             EXAMPLE OF CORRECT DRAFTING:\n\n\
             AFFIDAVIT\n\
             I, Rahul Verma, S/o Sh. Ramesh Verma, R/o H-45, Dwarka, Sector 11, New Delhi, do hereby solemnly affirm and declare as under:\n\
             1. That the deponent is a citizen of India and is fully conversant with the facts.\n\
             2. That on 12th April 2026, an unauthorized bank transaction of Rs. 50,000 occurred in my account.\n\n\
             VERIFICATION:\n\
             Verified at New Delhi on this 19th day of May 2026 that the contents of the above affidavit are true and correct.\n\
                                                     DEPONENT"
                .to_string(),
        );
    } else {
        sections.push(MIKE_SYSTEM_PROMPT.trim().to_string());
    }
    if !personalization_prompt.is_empty() {
        sections.push(personalization_prompt);
    }
    sections.push(TONE_RULES.trim().to_string());
    let last_msg_lower = messages.iter().rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.content.to_lowercase())
        .unwrap_or_default();

    let wants_maintenance = last_msg_lower.contains("maintenance")
        || last_msg_lower.contains("rajnesh")
        || last_msg_lower.contains("expense")
        || last_msg_lower.contains("income");
    let wants_pwdva = last_msg_lower.contains("domestic violence")
        || last_msg_lower.contains("pwdva")
        || last_msg_lower.contains("dv act")
        || last_msg_lower.contains("protection of women");
    let wants_divorce = last_msg_lower.contains("divorce");

    let wants_case_search = {
        let q = &last_msg_lower;
        let case_signals = [
            "cases", "judgment", "judgement", "ruling", "case law", "precedent",
            "v.", "vs.", "versus", "what does the law say", "find me",
            "is there a case", "case search", "search cases", "court ruled",
            "landmark", "ratio decidendi", "obiter", "held that",
        ];
        let section_pattern = q.contains("section") && (
            q.contains("act") || q.contains("ipc") || q.contains("crpc")
            || q.contains("bns") || q.contains("bnss") || q.contains("ni act")
        );
        let court_mentions = q.contains("supreme court") || q.contains("high court")
            || q.contains(" hc ") || q.contains(" sc ");
        case_signals.iter().any(|s| q.contains(s)) || section_pattern || court_mentions
    };

    if is_small {
        sections.push(EXTRACT_FIELDS_PROMPT.trim().to_string());
    } else {
        sections.push(DRAFTING_BASE.trim().to_string());
        if wants_maintenance {
            sections.push(TEMPLATE_MAINTENANCE.trim().to_string());
        }
        if wants_pwdva {
            sections.push(TEMPLATE_PWDVA.trim().to_string());
        }
    }
    if wants_case_search && !is_small {
        sections.push(CASE_SEARCH_INTAKE_PROMPT.trim().to_string());
    }
    if let Some(ref ctx) = case_ctx {
        sections.push(ctx.case_system_prompt.clone());
    }
    if !inventory_prompt.is_empty() {
        sections.push(inventory_prompt);
    }
    if !docs_prompt.is_empty() {
        sections.push(docs_prompt);
    }
    if !mcp_prompt.is_empty() {
        sections.push(mcp_prompt);
    }
    let system_prompt = sections.join("\n\n---\n\n");
    // Volatile tail — per-query knowledge-base hits. Kept out of the
    // cached system prefix so it never invalidates the prompt cache.
    let mut volatile_parts: Vec<&str> = Vec::new();
    if !kb_prompt.is_empty() {
        volatile_parts.push(&kb_prompt);
    }
    if !ik_prompt.is_empty() {
        volatile_parts.push(&ik_prompt);
    }
    let system_volatile = volatile_parts.join("\n\n---\n\n");
    let images = if vision_ok { collect_images(&attached_docs) } else { Vec::new() };

    let mut messages = messages;
    if !images.is_empty() {
        // Attach the rendered page images to the *last* user message, which is
        // the one the model is replying to. Falls through silently if there is
        // no user message in the history.
        if let Some(last_user) = messages.iter_mut().rev().find(|m| matches!(m.role, Role::User)) {
            last_user.images = images.clone();
        }
    }



    tracing::info!(
        "[chat] stream_chat_root: chat_id={chat_id}, model={raw_model}, vision_ok={vision_ok}, local_config_present={}, docs={}, mcp_servers={}, kb_chunks={}, ik_results={} (sys_prompt={} chars, images={})",
        local_config.is_some(),
        attached_docs.len(),
        mcp_servers.len(),
        kb_chunks.len(),
        ik_results.len(),
        system_prompt.len(),
        images.len()
    );

    // ─── Tools available to the model ────────────────────────────────
    // Builtin Mike tools first (read_document, find_in_document,
    // read_workflow, generate_docx stub, edit_document stub).
    let mut all_tools: Vec<ToolSchema> = builtin_tools::schemas();

    // MCP tools: injected ONLY for models that handle large tool
    // schemas reliably (see `llm::supports_mcp_tools` for the gate).
    // Smaller local models keep the previous behaviour — the MCP
    // servers stay visible via the system-prompt summary
    // (`build_mcp_system_prompt`) but their tool schemas don't go
    // into the schema list. The system prompt structure is unchanged
    // either way; the only thing this gate decides is whether the
    // model receives the additional `tools` schemas at the wire
    // protocol level.
    let mcp_tools_enabled = llm::supports_mcp_tools(&raw_model);
    let mcp_tool_count: usize = mcp_servers
        .iter()
        .map(|s| s.tool_schemas.len())
        .sum();
    if mcp_tools_enabled {
        for srv in &mcp_servers {
            all_tools.extend(srv.tool_schemas.iter().cloned());
        }
    }

    // Map chat-local labels (`doc-0`, `doc-1`, …) to real document UUIDs so
    // builtin tools (read_document, find_in_document) can resolve them.
    let mut doc_label_map: HashMap<String, String> = HashMap::new();
    for (idx, doc_id) in doc_ids.iter().enumerate() {
        doc_label_map.insert(format!("doc-{idx}"), doc_id.clone());
    }
    // Case-doc labels (case-doc-0, case-doc-1, …) for case-scoped chats.
    if let Some(ref ctx) = case_ctx {
        for (label, uuid) in &ctx.case_doc_labels {
            doc_label_map.insert(label.clone(), uuid.clone());
        }
    }

    tracing::info!(
        "[chat] tool-use: {} total tools (builtin + {} MCP, mcp_enabled={}), labels={:?}",
        all_tools.len(),
        mcp_tool_count,
        mcp_tools_enabled,
        doc_label_map.keys().collect::<Vec<_>>()
    );
    // Verbose dump of the MCP tool names actually being shipped in the
    // request — invaluable when a user reports "the model never calls
    // my MCP tool". If this log shows the tool name, the schema is on
    // the wire; if not, either the gate dropped it (model-not-supported)
    // or discovery never returned it (server-side handshake failure).
    if mcp_tools_enabled && mcp_tool_count > 0 {
        let mcp_tool_names: Vec<&str> = mcp_servers
            .iter()
            .flat_map(|s| s.tool_schemas.iter().map(|t| t.function.name.as_str()))
            .collect();
        tracing::info!(
            "[chat] MCP tools shipped to model: {:?}",
            mcp_tool_names
        );
    } else if mcp_tool_count > 0 {
        let server_names: Vec<&str> = mcp_servers
            .iter()
            .map(|s| s.config_name.as_str())
            .collect();
        tracing::info!(
            "[chat] MCP servers discovered ({} tools total) but NOT shipped — model {:?} not in supports_mcp_tools allowlist. Servers: {:?}. Set MIKE_FORCE_MCP_TOOLS=1 to override.",
            mcp_tool_count,
            raw_model,
            server_names
        );
    }

    let claude_key = user_settings.as_ref().and_then(|s| s.claude_api_key.clone());
    let gemini_key = user_settings.as_ref().and_then(|s| s.gemini_api_key.clone());
    let gemini_region = user_settings.as_ref().and_then(|s| s.gemini_region.clone());

    // Compress older turns when the running history starts to crowd the
    // model's context window. The threshold (70%) leaves room for the
    // system prompt + RAG block + attached docs + reply. Failing-open:
    // if the summarizer LLM call errors, the original messages are
    // returned and the dispatch continues unchanged.
    let summarizer_creds = llm::summarize::SummarizerCreds {
        local_config: local_config.clone(),
        claude_api_key: claude_key.clone(),
        gemini_api_key: gemini_key.clone(),
        gemini_region: gemini_region.clone(),
    };
    let messages =
        llm::summarize::maybe_compress_history(messages, &raw_model, &summarizer_creds).await;

    let is_drafting_request = {
        let q = last_user_query.to_lowercase();
        let action_verbs = ["draft", "write", "create", "generate", "prepare", "formulate", "make a", "redigere", "scrivere"];
        let doc_nouns = ["affidavit", "petition", "agreement", "contract", "deed", "power of attorney", "notice", "reply", "complaint", "will", "memo", "memorandum", "letter", "application", "suit", "plaint"];
        action_verbs.iter().any(|v| q.contains(v)) && doc_nouns.iter().any(|n| q.contains(n))
    };

    #[derive(PartialEq)]
    enum EditKind { None, Content, Formatting }

    let edit_kind = {
        let q = last_user_query.to_lowercase();
        let doc_refs = ["document", "affidavit", "petition", "draft", "complaint",
            "agreement", "contract", "deed", "notice", "letter"];
        let has_doc_ref = doc_refs.iter().any(|n| q.contains(n));
        let fmt = ["bold", "underline", "italic", "heading", "spacing",
            "font size", "center", "align"];
        let content = ["change", "replace", "update", "fix", "correct", "modify",
            "rename", "add clause", "add paragraph", "add point",
            "add section", "remove", "delete", "insert"];
        if has_doc_ref && fmt.iter().any(|v| q.contains(v)) {
            EditKind::Formatting
        } else if has_doc_ref && content.iter().any(|v| q.contains(v)) {
            EditKind::Content
        } else {
            EditKind::None
        }
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    let state_clone = state.clone();
    let chat_id_clone = chat_id.clone();
    // Move retrieved KB chunks into the spawned task so the post-stream
    // citation parser can map model-emitted [g1]/[p1] tags back to the
    // source path + chunk index.
    let kb_chunks_for_citations = kb_chunks.clone();
    let case_id_for_citations: Option<String> = case_ctx.as_ref().map(|c| c.case_id.clone());

    tokio::spawn(async move {
        if is_new_chat {
            let chat_id_event = json!({ "type": "chat_id", "chatId": &chat_id_clone });
            let _ = tx
                .send(Ok(Event::default().data(chat_id_event.to_string())))
                .await;
        }

        const MAX_TOOL_ITERATIONS: u32 = 100;
        let mut full_response = String::new();
        let mut current_messages = messages;
        let mut iteration: u32 = 0;
        let mut errored = false;
        let mut doc_start_sent = false;
        let mut doc_already_generated = false;
        let mut last_doc_uuid: Option<String> = None;
        // Some models (e.g. gemma3 on Ollama) refuse the `tools` parameter
        // entirely. We detect that on the first call and disable tool-use
        // for the rest of the conversation, falling back to the system-prompt
        // listing (the model still "knows" the servers exist, just can't call them).
        // Persisted in AppState so we don't pay the retry on every message.
        let already_known_unsupported = state_clone
            .no_tools_models
            .read()
            .await
            .contains(&raw_model);
        let is_small_local = is_small;
        let mut tools_supported = !all_tools.is_empty() && !already_known_unsupported && !is_small_local;

        // If we already know this model does not support tools but there ARE
        // MCP servers configured, prepend an explicit warning to the response
        // so the user sees it in chat (not just in the backend log).
        let mut tool_warning_emitted = false;
        if !all_tools.is_empty() && already_known_unsupported {
            let warning = format!(
                "> ⚠️ **Tool-use non supportato dal modello selezionato** (`{}`). I {} \
                 server MCP configurati sono visibili nel mio contesto, ma non posso \
                 invocare direttamente i loro tools. Per il tool-use reale usa un \
                 modello compatibile: Claude, Gemini, GPT-4o, Qwen 2.5, Llama 3.1+, \
                 Mistral Small.\n\n---\n\n",
                raw_model,
                mcp_servers.len()
            );
            full_response.push_str(&warning);
            let payload = json!({ "type": "content_delta", "text": warning });
            let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
            tool_warning_emitted = true;
        }

        loop {
            iteration += 1;
            let params = StreamParams {
                model: raw_model.clone(),
                system_prompt: system_prompt.clone(),
                system_volatile: system_volatile.clone(),
                messages: current_messages.clone(),
                tools: if tools_supported { all_tools.clone() } else { vec![] },
                max_iterations: 1,
                enable_thinking: false,
                local_config: local_config.clone(),
                claude_api_key: claude_key.clone(),
                gemini_api_key: gemini_key.clone(),
                gemini_region: gemini_region.clone(),
            };

            let stream = llm::stream_chat(params).await;
            match stream {
                Err(e) => {
                    let msg = e.to_string();
                    // Be precise: only treat as "model can't do tools" if the
                    // upstream explicitly says so. A generic 400 with "tool"
                    // in the body usually means a malformed schema, not a
                    // model limitation — surfacing the error is more useful.
                    let lower = msg.to_lowercase();
                    let unsupported = lower.contains("does not support tools")
                        || lower.contains("tools not supported")
                        || lower.contains("does not support tool use")
                        || lower.contains("tool use is not supported")
                        || lower.contains("functioncalling is not supported")
                        || lower.contains("function calling is not supported");
                    if tools_supported && unsupported {
                        tracing::warn!(
                            "[chat] model {raw_model}: tools rejected — \
                             retrying without tool-use. Original error: {}",
                            msg.chars().take(500).collect::<String>()
                        );
                        state_clone
                            .no_tools_models
                            .write()
                            .await
                            .insert(raw_model.clone());
                        tools_supported = false;
                        if !tool_warning_emitted && !all_tools.is_empty() {
                            let warning = format!(
                                "> ⚠️ **Tool-use non supportato dal modello selezionato** (`{}`). I {} \
                                 server MCP configurati sono visibili nel mio contesto, ma non posso \
                                 invocare direttamente i loro tools. Per il tool-use reale usa un \
                                 modello compatibile: Claude, Gemini, GPT-4o, Qwen 2.5, Llama 3.1+, \
                                 Mistral Small.\n\n---\n\n",
                                raw_model,
                                mcp_servers.len()
                            );
                            full_response.push_str(&warning);
                            let payload = json!({ "type": "content_delta", "text": warning });
                            let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
                            tool_warning_emitted = true;
                        }
                        iteration -= 1; // don't count this as a real iteration
                        continue;
                    }
                    tracing::error!("[chat] stream_chat error (iter {iteration}): {e}");
                    let payload = json!({ "type": "error", "message": e.to_string() });
                    let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
                    errored = true;
                    break;
                }
                Ok(mut s) => {
                    let mut iter_text = String::new();
                    let mut iter_reasoning = String::new();
                    let mut iter_tool_calls: Vec<ToolCall> = Vec::new();
                    let mut got_done = false;
                    let mut got_err: Option<String> = None;
                    while let Some(event) = s.next().await {
                        match event {
                            Ok(StreamEvent::ContentDelta(text)) => {
                                iter_text.push_str(&text);
                                full_response.push_str(&text);
                                if is_drafting_request && iteration == 1 {
                                    if !doc_start_sent {
                                        let start_payload = serde_json::json!({
                                            "type": "doc_created_start",
                                            "filename": "document.docx",
                                        });
                                        let _ = tx.send(Ok(Event::default().data(start_payload.to_string()))).await;
                                        doc_start_sent = true;
                                    }
                                } else {
                                    let payload = json!({ "type": "content_delta", "text": text });
                                    if tx
                                        .send(Ok(Event::default().data(payload.to_string())))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                            Ok(StreamEvent::ToolCalls(calls)) => {
                                iter_tool_calls = calls;
                            }
                            Ok(StreamEvent::ReasoningDelta(text)) => {
                                iter_reasoning.push_str(&text);
                                let payload = json!({ "type": "reasoning_delta", "text": text });
                                let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
                            }
                            Ok(StreamEvent::ReasoningEnd) => {
                                let payload = json!({ "type": "reasoning_block_end" });
                                let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
                            }
                            Ok(StreamEvent::Done) => { got_done = true; break; }
                            Err(e) => { got_err = Some(e.to_string()); break; }
                            _ => {}
                        }
                    }
                    tracing::info!(
                        "[chat] iter {iteration}: text={}, tool_calls={}, done={}, err={:?}",
                        iter_text.len(),
                        iter_tool_calls.len(),
                        got_done,
                        got_err
                    );

                    if iter_tool_calls.is_empty() {
                        let lower = full_response.to_lowercase();
                        let keywords = ["in the court of", "affidavit", "petition",
                                        "solemnly affirm", "most respectfully showeth", "prayer:",
                                        "power of attorney", "agreement", "contract", "deed", "know all men"];

                        // Case 0: Two-step hybrid template.
                        // Step 1 (already done): model extracted party/case metadata.
                        // Step 2 (below): if no facts, make a focused second LLM call.
                        // Then Rust assembles the full legal document deterministically.
                        let mut hybrid_handled = false;
                        if (is_drafting_request || full_response.contains("_name\"") || full_response.contains("\"doc_type\"")) && !doc_already_generated && edit_kind == EditKind::None {
                            let trimmed = full_response.trim();
                            if let Some(json_start) = trimmed.find('{') {
                                if let Some(json_end) = trimmed.rfind('}') {
                                    if let Ok(mut data) = serde_json::from_str::<serde_json::Value>(
                                        &trimmed[json_start..=json_end],
                                    ) {
                                        let has_metadata = data.as_object().map_or(false, |obj| {
                                            obj.len() >= 2 && obj.keys().any(|k|
                                                k.ends_with("_name") || k.ends_with("_address")
                                                || k == "doc_type" || k == "rent" || k == "subject"
                                                || k == "court_name"
                                            )
                                        });

                                        if has_metadata && data.get("body").is_none() {
                                            let doc_type = {
                                                let from_query = detect_doc_type(&last_user_query);
                                                if from_query != DocType::Generic { from_query } else { detect_doc_type_from_data(&data) }
                                            };

                                            // Sanity check: model often emits empty/literal "COMPLAINANT" for names —
                                            // patch from the user's actual query before we assemble.
                                            patch_names_from_query(&mut data, &last_user_query);

                                            let has_facts = data
                                                .get("facts")
                                                .and_then(|f| f.as_array())
                                                .map_or(false, |a| !a.is_empty());

                                            if needs_step2(doc_type) && !has_facts {
                                                tracing::info!(
                                                    "[chat] hybrid step 2: generating content for {:?}",
                                                    data.get("doc_type")
                                                );
                                                let status_ev = json!({
                                                    "type": "content_delta",
                                                    "text": "\n\n> ✍️ Generating document content...\n\n"
                                                });
                                                let _ = tx.send(Ok(Event::default()
                                                    .data(status_ev.to_string()))).await;

                                                let facts_prompt = build_step2_prompt(
                                                    doc_type, &data, &last_user_query,
                                                );
                                                let facts_params = StreamParams {
                                                    model: raw_model.clone(),
                                                    system_prompt: facts_prompt,
                                                    system_volatile: String::new(),
                                                    messages: vec![Message {
                                                        role: Role::User,
                                                        content: last_user_query.clone(),
                                                        images: vec![],
                                                        tool_calls: vec![],
                                                        tool_call_id: None,
                                                        tool_name: None,
                                                        reasoning_content: None,
                                                    }],
                                                    tools: vec![],
                                                    max_iterations: 1,
                                                    enable_thinking: false,
                                                    local_config: local_config.clone(),
                                                    claude_api_key: claude_key.clone(),
                                                    gemini_api_key: gemini_key.clone(),
                                                    gemini_region: gemini_region.clone(),
                                                };

                                                match llm::local::complete(facts_params).await {
                                                    Ok(facts_resp) => {
                                                        let ft = facts_resp.trim();
                                                        if let Some(a_s) = ft.find('[') {
                                                            if let Some(a_e) = ft.rfind(']') {
                                                                if let Ok(arr) = serde_json::from_str::<serde_json::Value>(
                                                                    &ft[a_s..=a_e]
                                                                ) {
                                                                    if let Some(obj) = data.as_object_mut() {
                                                                        obj.insert("facts".to_string(), arr);
                                                                    }
                                                                    tracing::info!(
                                                                        "[chat] hybrid step 2: parsed facts array"
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(
                                                            "[chat] hybrid step 2 failed: {e}"
                                                        );
                                                    }
                                                }
                                            }

                                            // Fallback: if step 2 failed or was skipped but
                                            // assembly needs facts, inject example/default facts
                                            inject_fallback_facts(&mut data, doc_type, &last_user_query);

                                            if let Some(assembled) = assemble_legal_document(&data, &last_user_query) {
                                                tracing::info!(
                                                    "[chat] hybrid template: assembled {} chars, doc_type={:?}",
                                                    assembled.len(), data.get("doc_type")
                                                );
                                                full_response = String::new();
                                                iter_text = String::new();
                                                let title = jf(&data, &[
                                                    "court_name", "landlord_name", "seller_name",
                                                    "grantor_name", "testator_name", "sender_name", "from",
                                                ]).unwrap_or("Legal Draft")
                                                    .chars().take(40).collect::<String>();
                                                iter_tool_calls.push(ToolCall {
                                                    id: uuid::Uuid::new_v4().to_string(),
                                                    name: "generate_docx".to_string(),
                                                    input: serde_json::json!({
                                                        "title": title,
                                                        "body": assembled
                                                    }),
                                                });
                                                hybrid_handled = true;
                                            }
                                        }
                                    }
                                }
                            }
                            if !hybrid_handled && is_drafting_request {
                                // Retry: model didn't output valid JSON on first try.
                                // Make a focused call with ONLY the extraction prompt.
                                tracing::info!(
                                    "[chat] hybrid template: first pass failed, retrying extraction"
                                );
                                let retry_ev = json!({
                                    "type": "content_delta",
                                    "text": "\n\n> ✍️ Extracting document details...\n\n"
                                });
                                let _ = tx.send(Ok(Event::default()
                                    .data(retry_ev.to_string()))).await;

                                let retry_params = StreamParams {
                                    model: raw_model.clone(),
                                    system_prompt: EXTRACT_FIELDS_PROMPT.trim().to_string(),
                                    system_volatile: String::new(),
                                    messages: vec![Message {
                                        role: Role::User,
                                        content: last_user_query.clone(),
                                        images: vec![],
                                        tool_calls: vec![],
                                        tool_call_id: None,
                                        tool_name: None,
                                        reasoning_content: None,
                                    }],
                                    tools: vec![],
                                    max_iterations: 1,
                                    enable_thinking: false,
                                    local_config: local_config.clone(),
                                    claude_api_key: claude_key.clone(),
                                    gemini_api_key: gemini_key.clone(),
                                    gemini_region: gemini_region.clone(),
                                };

                                if let Ok(retry_resp) = llm::local::complete(retry_params).await {
                                    let rt = retry_resp.trim();
                                    if let Some(js) = rt.find('{') {
                                        if let Some(je) = rt.rfind('}') {
                                            if let Ok(mut data) = serde_json::from_str::<serde_json::Value>(
                                                &rt[js..=je]
                                            ) {
                                                tracing::info!("[chat] hybrid retry: parsed JSON on second try");
                                                let doc_type = {
                                                    let from_query = detect_doc_type(&last_user_query);
                                                    if from_query != DocType::Generic { from_query } else { detect_doc_type_from_data(&data) }
                                                };
                                                patch_names_from_query(&mut data, &last_user_query);
                                                inject_fallback_facts(&mut data, doc_type, &last_user_query);
                                                if let Some(assembled) = assemble_legal_document(&data, &last_user_query) {
                                                    tracing::info!(
                                                        "[chat] hybrid retry: assembled {} chars",
                                                        assembled.len()
                                                    );
                                                    full_response = String::new();
                                                    iter_text = String::new();
                                                    let title = jf(&data, &[
                                                        "court_name", "landlord_name", "seller_name",
                                                        "grantor_name", "testator_name", "sender_name", "from",
                                                    ]).unwrap_or("Legal Draft")
                                                        .chars().take(40).collect::<String>();
                                                    iter_tool_calls.push(ToolCall {
                                                        id: uuid::Uuid::new_v4().to_string(),
                                                        name: "generate_docx".to_string(),
                                                        input: serde_json::json!({
                                                            "title": title,
                                                            "body": assembled
                                                        }),
                                                    });
                                                    hybrid_handled = true;
                                                }
                                            }
                                        }
                                    }
                                }
                                if !hybrid_handled {
                                    tracing::info!("[chat] hybrid template: retry also failed");
                                }
                            }
                        }

                        // Case 0.5: Hybrid edit for small/non-tool models
                        if last_doc_uuid.is_none() && doc_label_map.is_empty() {
                            let row: Option<(String,)> = sqlx::query_as(
                                "SELECT id FROM documents WHERE chat_id = ? AND user_id = ? ORDER BY created_at DESC LIMIT 1"
                            )
                                .bind(&chat_id_clone)
                                .bind(&auth.user_id)
                                .fetch_optional(&state_clone.db)
                                .await
                                .ok()
                                .flatten();
                            if let Some((doc_id,)) = row {
                                last_doc_uuid = Some(doc_id.clone());
                                doc_label_map.insert("doc-0".to_string(), doc_id);
                            }
                        }

                        if !hybrid_handled
                            && edit_kind != EditKind::None
                        {
                            let target_uuid = last_doc_uuid.clone()
                                .or_else(|| doc_label_map.values().last().cloned());

                            if let Some(uuid) = target_uuid {
                                let target_label = doc_label_map.iter()
                                    .find(|(_, v)| **v == uuid)
                                    .map(|(k, _)| k.clone())
                                    .unwrap_or_else(|| "doc-0".to_string());

                                // Fetch document text for the edit prompt
                                let doc_text = {
                                    let row: Option<(Option<String>,)> = sqlx::query_as(
                                        "SELECT storage_path FROM documents WHERE id = ? AND user_id = ?"
                                    )
                                        .bind(&uuid)
                                        .bind(&auth.user_id)
                                        .fetch_optional(&state_clone.db)
                                        .await
                                        .ok()
                                        .flatten();
                                    if let Some((Some(path),)) = row {
                                        if let Ok(storage) = crate::storage::make_storage() {
                                            if let Ok(bytes) = storage.get(&path).await {
                                                crate::pdf::extract_docx_text(&bytes).ok()
                                            } else { None }
                                        } else { None }
                                    } else { None }
                                };

                                if let Some(text) = doc_text {
                                    let truncated = if text.len() > 3000 {
                                        format!("{}...(truncated)", &text[..3000])
                                    } else {
                                        text
                                    };

                                    let status_ev = json!({
                                        "type": "content_delta",
                                        "text": "\n\n> ✏️ Editing document...\n\n"
                                    });
                                    let _ = tx.send(Ok(Event::default()
                                        .data(status_ev.to_string()))).await;

                                    let edit_prompt = format!(
                                        "{}{}", EXTRACT_EDITS_PROMPT, truncated
                                    );

                                    let edit_params = StreamParams {
                                        model: raw_model.clone(),
                                        system_prompt: edit_prompt,
                                        system_volatile: String::new(),
                                        messages: vec![Message {
                                            role: Role::User,
                                            content: last_user_query.clone(),
                                            images: vec![],
                                            tool_calls: vec![],
                                            tool_call_id: None,
                                            tool_name: None,
                                            reasoning_content: None,
                                        }],
                                        tools: vec![],
                                        max_iterations: 1,
                                        enable_thinking: false,
                                        local_config: local_config.clone(),
                                        claude_api_key: claude_key.clone(),
                                        gemini_api_key: gemini_key.clone(),
                                        gemini_region: gemini_region.clone(),
                                    };

                                    match llm::local::complete(edit_params).await {
                                        Ok(edit_resp) => {
                                            let trimmed = edit_resp.trim();
                                            let json_start = trimmed.find('{');
                                            let json_end = trimmed.rfind('}');
                                            if let (Some(s), Some(e)) = (json_start, json_end) {
                                                if let Ok(data) = serde_json::from_str::<serde_json::Value>(
                                                    &trimmed[s..=e]
                                                ) {
                                                    if let Some(edits_arr) = data.get("edits").and_then(|v| v.as_array()) {
                                                        let edits_json: Vec<serde_json::Value> = edits_arr.iter()
                                                            .filter_map(|e| {
                                                                let find = e.get("find")?.as_str()?;
                                                                let replace = e.get("replace")?.as_str()?;
                                                                let mut obj = json!({
                                                                    "find": find,
                                                                    "replace": replace,
                                                                });
                                                                if let Some(fmt) = e.get("format").and_then(|v| v.as_str()) {
                                                                    obj["format"] = json!(fmt);
                                                                }
                                                                Some(obj)
                                                            })
                                                            .collect();

                                                        if !edits_json.is_empty() {
                                                            tracing::info!(
                                                                "[chat] hybrid edit: parsed {} edits",
                                                                edits_json.len()
                                                            );
                                                            full_response = String::new();
                                                            iter_text = String::new();
                                                            iter_tool_calls.push(ToolCall {
                                                                id: uuid::Uuid::new_v4().to_string(),
                                                                name: "edit_document".to_string(),
                                                                input: json!({
                                                                    "doc_id": target_label,
                                                                    "edits": edits_json,
                                                                }),
                                                            });
                                                            hybrid_handled = true;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::warn!("[chat] hybrid edit failed: {e}");
                                        }
                                    }
                                }
                            }
                        }

                        if !hybrid_handled {
                            // Case 1: weak local model echoed generate_docx JSON as plain text.
                            // Parse it and forge a real ToolCall, then suppress the raw text.
                            let echoed_tool = if lower.contains("generate_docx") && lower.contains("\"body\"") {
                                let trimmed = full_response.trim();
                                let start = trimmed.find('{').unwrap_or(0);
                                serde_json::from_str::<serde_json::Value>(&trimmed[start..]).ok()
                                    .and_then(|v| {
                                        let params = v.get("parameters").or_else(|| v.get("input"));
                                        params.and_then(|p| p.get("body"))
                                              .and_then(|b| b.as_str())
                                              .map(|body| (
                                                  v.get("parameters")
                                                   .or_else(|| v.get("input"))
                                                   .and_then(|p| p.get("title"))
                                                   .and_then(|t| t.as_str())
                                                   .unwrap_or("Legal Draft")
                                                   .to_string(),
                                                  body.to_string()
                                              ))
                                    })
                            } else {
                                None
                            };

                            if let Some((title, body)) = echoed_tool {
                                full_response = String::new();
                                iter_text = String::new();
                                iter_tool_calls.push(ToolCall {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    name: "generate_docx".to_string(),
                                    input: serde_json::json!({
                                        "title": title,
                                        "body": body
                                    }),
                                });
                            // Case 2: model produced real legal content but forgot to call the tool.
                            } else if !doc_already_generated && (is_drafting_request || (is_small_local && keywords.iter().any(|k| lower.contains(k)) && !full_response.contains("doc_id"))) {
                                let cleaned_body = clean_draft_text(&full_response);
                                let title = cleaned_body
                                    .lines()
                                    .next()
                                    .unwrap_or("Legal Draft")
                                    .trim_matches(|c: char| !c.is_alphanumeric() && c != ' ')
                                    .chars()
                                    .take(40)
                                    .collect::<String>();
                                iter_tool_calls.push(ToolCall {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    name: "generate_docx".to_string(),
                                    input: serde_json::json!({
                                        "title": if title.is_empty() { "Legal Draft" } else { &title },
                                        "body": cleaned_body
                                    }),
                                });
                            } else {
                                break;
                            }
                        }
                    }
                    if iteration >= MAX_TOOL_ITERATIONS {
                        tracing::warn!("[chat] hit MAX_TOOL_ITERATIONS, stopping");
                        let payload = json!({
                            "type": "content_delta",
                            "text": "\n\n_(stopped: too many tool iterations)_"
                        });
                        let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
                        break;
                    }

                    // Replay the assistant's tool_calls in the next round, then
                    // dispatch each call and append its result as a `tool` message.
                    let mut tc_msg = Message::assistant_tool_calls(iter_tool_calls.clone());
                    if !iter_reasoning.is_empty() {
                        tc_msg.reasoning_content = Some(std::mem::take(&mut iter_reasoning));
                    }
                    current_messages.push(tc_msg);
                    for call in &iter_tool_calls {
                        let payload = json!({ "type": "tool_call_start", "name": call.name });
                        let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;

                        // Race the dispatch against a 5-s ticker that
                        // emits `tool_call_progress` SSE events to the
                        // browser. Without this, slow MCP tools (e.g.
                        // Edge's pseudonymise-with-human-approval flow
                        // that can hold the connection for minutes
                        // while a user clicks Conferma in the Edge UI)
                        // looked silent in the chat — the user thought
                        // Mike had died. Now the chat shows
                        // "Sto eseguendo X (37s)…" so the wait is
                        // visibly progressing.
                        let dispatch_start_ts = std::time::Instant::now();
                        let tool_name_for_progress = call.name.clone();
                        let tx_progress = tx.clone();
                        let progress_task = tokio::spawn(async move {
                            // First tick at 5 s, then every 5 s after.
                            let mut ticker = tokio::time::interval(
                                std::time::Duration::from_secs(5),
                            );
                            // Skip the immediate first tick that
                            // tokio::interval fires.
                            ticker.tick().await;
                            loop {
                                ticker.tick().await;
                                let elapsed_secs =
                                    dispatch_start_ts.elapsed().as_secs();
                                let payload = json!({
                                    "type": "tool_call_progress",
                                    "name": tool_name_for_progress,
                                    "elapsed_secs": elapsed_secs,
                                });
                                if tx_progress
                                    .send(Ok(Event::default()
                                        .data(payload.to_string())))
                                    .await
                                    .is_err()
                                {
                                    // Receiver gone — stop ticking.
                                    return;
                                }
                            }
                        });

                        // Emit doc_created_start/doc_edited_start so the frontend
                        // shows a document card placeholder immediately.
                        if call.name == "generate_docx" {
                            let start_payload = serde_json::json!({
                                "type": "doc_created_start",
                                "filename": "document.docx",
                            });
                            let _ = tx.send(Ok(axum::response::sse::Event::default()
                                .data(start_payload.to_string()))).await;
                        } else if call.name == "edit_document" {
                            let start_payload = serde_json::json!({
                                "type": "doc_edited_start",
                                "filename": "edited.docx",
                            });
                            let _ = tx.send(Ok(axum::response::sse::Event::default()
                                .data(start_payload.to_string()))).await;
                        }

                        let result = if builtin_tools::is_builtin(&call.name) {
                            tracing::info!("[chat] dispatching builtin tool: {}", call.name);
                            builtin_tools::dispatch(
                                &state_clone,
                                &auth.user_id,
                                &doc_label_map,
                                &call.name,
                                &call.input,
                            )
                            .await
                        } else if builtin_tools::is_client_tool(&call.name) {
                            tracing::info!("[chat] dispatching client-side tool: {}", call.name);
                            let request_id = uuid::Uuid::new_v4().to_string();
                            let (otx, orx) = tokio::sync::oneshot::channel::<String>();
                            {
                                state_clone.client_tool_tx.lock().unwrap()
                                    .insert(request_id.clone(), otx);
                            }
                            let payload = json!({
                                "type": "client_tool_request",
                                "request_id": request_id,
                                "name": call.name,
                                "arguments": call.input,
                            });
                            let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(180),
                                orx,
                            ).await {
                                Ok(Ok(r)) => r,
                                _ => {
                                    state_clone.client_tool_tx.lock().unwrap()
                                        .remove(&request_id);
                                    json!({"error": "client tool timed out"}).to_string()
                                }
                            }
                        } else {
                            tracing::info!("[chat] dispatching MCP tool: {}", call.name);
                            dispatch_mcp_tool_with_async_chain(
                                &mcp_servers,
                                &call.name,
                                &call.input,
                            )
                            .await
                        };
                        progress_task.abort();

                        let result = if call.name == "vanga_search" {
                            if let Ok(arr) = serde_json::from_str::<Vec<Value>>(&result) {
                                if arr.is_empty() {
                                    "vanga_search returned 0 results. Do NOT cite any case law from training data. Tell the user honestly that you couldn't find matching cases.".to_string()
                                } else {
                                    build_vanga_system_prompt(&arr)
                                }
                            } else {
                                result
                            }
                        } else {
                            result
                        };

                        // Emit doc completion SSE events so the frontend
                        // fills in the document card with real data.
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&result) {
                            if call.name == "generate_docx" && val.get("error").is_none() {
                                doc_already_generated = true;
                                let filename = val["filename"].as_str().unwrap_or("document.docx");
                                let doc_id = val["doc_id"].as_str().unwrap_or("");
                                last_doc_uuid = Some(doc_id.to_string());
                                let next_idx = doc_label_map.len();
                                doc_label_map.insert(format!("doc-{}", next_idx), doc_id.to_string());
                                let payload = serde_json::json!({
                                    "type": "doc_created",
                                    "filename": filename,
                                    "download_url": format!("/document/{doc_id}/docx"),
                                    "document_id": doc_id,
                                });
                                let _ = tx.send(Ok(axum::response::sse::Event::default()
                                    .data(payload.to_string()))).await;
                            } else if call.name == "generate_docx" && val.get("error").is_some() {
                                let err_msg = val["error"].as_str().unwrap_or("Unknown error");
                                let payload = serde_json::json!({
                                    "type": "content_delta",
                                    "text": format!("\n\n**Demo Warning (Tool Error):** Failed to create the Word document because: {}\n\n", err_msg)
                                });
                                let _ = tx.send(Ok(axum::response::sse::Event::default().data(payload.to_string()))).await;
                            } else if call.name == "edit_document" && val.get("error").is_none() {
                                let filename = val["filename"].as_str().unwrap_or("edited.docx");
                                let doc_id = val["doc_id"].as_str().unwrap_or("");
                                let real_doc_id = doc_label_map
                                    .get(doc_id)
                                    .map(|s| s.as_str())
                                    .unwrap_or(doc_id);
                                let payload = serde_json::json!({
                                    "type": "doc_edited",
                                    "filename": filename,
                                    "download_url": format!("/document/{}/docx", real_doc_id),
                                    "document_id": real_doc_id,
                                    "version_id": val.get("version_id"),
                                    "version_number": val.get("version_number"),
                                    "annotations": val.get("annotations").unwrap_or(&serde_json::json!([])),
                                });
                                let _ = tx.send(Ok(axum::response::sse::Event::default()
                                    .data(payload.to_string()))).await;
                            }
                        }

                        // For diagnostics: when a tool result is short
                        // it's almost always an error envelope or a
                        // pointer to async work. Log the body verbatim
                        // so we can tell at a glance whether the model
                        // is going to refuse vs proceed.
                        if result.len() <= 200 {
                            tracing::info!(
                                "[chat] tool {} result ({} chars): {}",
                                call.name,
                                result.len(),
                                result
                            );
                        } else {
                            tracing::info!(
                                "[chat] tool {} result: {} chars",
                                call.name,
                                result.len()
                            );
                        }
                        current_messages.push(Message::tool_result(&call.id, &call.name, &result));
                    }
                }
            }
        }

        let got_done = !errored;
        let got_error: Option<String> = if errored { Some("see backend log".into()) } else { None };
        tracing::info!(
            "[chat] stream finished: chars={}, done={}, error={:?}",
            full_response.len(),
            got_done,
            got_error
        );

        // ─── POST-RESPONSE CITATION VALIDATION ───────────────────────────
        // The system prompt requires every Indian-case citation to be
        // accompanied by a verbatim quote ("The Court held: '...'") drawn
        // from this turn's tool output. Prompt enforcement is unreliable
        // for famous cases — model often paraphrases from training data.
        // This validator scans the final response for Kanoon citations
        // missing the verbatim quote pattern and appends a user-visible
        // warning block. Doesn't strip the citation, doesn't re-prompt —
        // just surfaces the gap so the lawyer knows which citations to
        // verify manually before relying on.
        let missing_quote_titles = validate_kanoon_quotes(&full_response);
        if !missing_quote_titles.is_empty() {
            let warning = build_quote_warning(&missing_quote_titles);
            // Emit as a content_delta so the warning appears at the end
            // of the chat response, styled like model output.
            let payload = json!({
                "type": "content_delta",
                "text": warning.clone(),
            });
            let _ = tx.send(Ok(Event::default().data(payload.to_string()))).await;
            // Append to full_response so it's persisted to the DB and
            // visible on reload of the chat history.
            full_response.push_str(&warning);
            tracing::warn!(
                "[chat] {} citation(s) missing verbatim quote: {:?}",
                missing_quote_titles.len(),
                missing_quote_titles
            );
        }

        // We hold the assistant-message id outside the if-block so the
        // citations-resolution step below can update the same row with
        // the parsed annotations JSON. Without that link the chat
        // history loses citations on reload (`get_messages` returns
        // content but not annotations) and `[g1]`/`[p1]` pills render
        // as plain text on old turns.
        let asst_msg_id: Option<String> = if !full_response.is_empty() {
            let id = uuid::Uuid::new_v4().to_string();
            let _ = sqlx::query(
                "INSERT INTO messages (id, chat_id, role, content) VALUES (?, ?, 'assistant', ?)",
            )
            .bind(&id)
            .bind(&chat_id_clone)
            .bind(&full_response)
            .execute(&state_clone.db)
            .await;

            let _ = sqlx::query("UPDATE chats SET updated_at = datetime('now') WHERE id = ?")
                .bind(&chat_id_clone)
                .execute(&state_clone.db)
                .await;
            Some(id)
        } else {
            None
        };

        // Parse the trailing <CITATIONS>…</CITATIONS> JSON block the model
        // is instructed to emit (see MIKE_SYSTEM_PROMPT). Resolve each
        // citation's `doc_id` (a chat-local label like "doc-0") back to the
        // real document UUID + filename so the frontend viewer can fetch
        // and highlight it.
        let mut id_by_label: HashMap<String, String> = HashMap::new();
        for (label, uuid) in &doc_label_map {
            id_by_label.insert(label.clone(), uuid.clone());
        }
        // Also fetch filenames so the citation entry contains it.
        let mut name_by_id: HashMap<String, String> = HashMap::new();
        for uuid in id_by_label.values() {
            if let Ok(Some((fname,))) = sqlx::query_as::<_, (String,)>(
                "SELECT filename FROM documents WHERE id = ? AND user_id = ?",
            )
            .bind(uuid)
            .bind(&auth.user_id)
            .fetch_optional(&state_clone.db)
            .await
            {
                name_by_id.insert(uuid.clone(), fname);
            }
        }

        // Build a tag → KB-entry index so we can resolve [g1]/[p1] back
        // to the source path the user-side viewer needs.
        let mut kb_by_tag: HashMap<String, RetrievedKbEntry> = HashMap::new();
        for entry in &kb_chunks_for_citations {
            kb_by_tag.insert(entry.tag.clone(), entry.clone());
        }

        // Build a corpus-identifier → tag fallback index so the citation
        // resolver can recover when the model invents a doc_id from the
        // <USER LIBRARY> inventory (e.g. "eurlex_32016R0679" or just
        // "32016R0679") instead of using the [gN] tag from the
        // <KNOWLEDGE BASE> section as instructed. Without this fallback
        // those citations get tagged source="attached", point at no
        // real document, and render as a 404 in the viewer.
        //
        // We index the same chunk under several normalised keys so a
        // model emitting any of "eurlex_32016R0679", "EUR-Lex/32016R0679",
        // "32016R0679", or "eurlex:32016R0679" still resolves.
        let mut corpus_ref_to_tag: HashMap<String, String> = HashMap::new();
        if !kb_by_tag.is_empty() {
            let doc_ids: std::collections::HashSet<String> = kb_chunks_for_citations
                .iter()
                .map(|e| e.document_id.clone())
                .collect();
            if !doc_ids.is_empty() {
                let placeholders = std::iter::repeat("?")
                    .take(doc_ids.len())
                    .collect::<Vec<_>>()
                    .join(",");
                let q = format!(
                    "SELECT id, corpus_id, corpus_identifier FROM documents \
                     WHERE user_id = ? AND id IN ({}) \
                       AND corpus_id IS NOT NULL AND corpus_identifier IS NOT NULL",
                    placeholders
                );
                let mut query = sqlx::query_as::<_, (String, String, String)>(&q)
                    .bind(&auth.user_id);
                for did in &doc_ids {
                    query = query.bind(did);
                }
                if let Ok(rows) = query.fetch_all(&state_clone.db).await {
                    // Build a doc_id → tag lookup once, then map every
                    // alias of (corpus_id, corpus_identifier) to it.
                    let mut tag_by_doc: HashMap<String, String> = HashMap::new();
                    for entry in &kb_chunks_for_citations {
                        tag_by_doc
                            .entry(entry.document_id.clone())
                            .or_insert_with(|| entry.tag.clone());
                    }
                    for (doc_uuid, corpus_id, ident) in rows {
                        let Some(tag) = tag_by_doc.get(&doc_uuid) else { continue };
                        let ident_lower = ident.to_ascii_lowercase();
                        let corpus_lower = corpus_id.to_ascii_lowercase();
                        for key in [
                            ident.clone(),
                            ident_lower.clone(),
                            format!("{corpus_id}_{ident}"),
                            format!("{corpus_lower}_{ident_lower}"),
                            format!("{corpus_id}:{ident}"),
                            format!("{corpus_lower}:{ident_lower}"),
                            format!("{corpus_id}/{ident}"),
                            format!("{corpus_lower}/{ident_lower}"),
                        ] {
                            corpus_ref_to_tag
                                .entry(key)
                                .or_insert_with(|| tag.clone());
                        }
                    }
                    if !corpus_ref_to_tag.is_empty() {
                        tracing::info!(
                            "[chat] built corpus-ref → tag fallback with {} aliases",
                            corpus_ref_to_tag.len()
                        );
                    }
                }
            }
        }

        let citations_json = extract_citations_block(&full_response).or_else(|| {
            // Fallback: model wrote [gN]/[pN] inline but skipped the
            // <CITATIONS> JSON block. Synthesise from markers so the
            // pills still render.
            synthesise_kb_citations_from_markers(&full_response, &kb_by_tag)
        });
        let citations_array: Vec<Value> = match citations_json {
            Some(v) => v
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|c| {
                    let label = c.get("doc_id").and_then(|x| x.as_str()).unwrap_or("");
                    let mut obj = c.as_object().cloned().unwrap_or_default();
                    obj.insert("type".into(), Value::String("citation_data".to_string()));

                    // Three resolution paths:
                    //  - "doc-N"           → attached document, lookup in id_by_label
                    //  - "g1" / "p1" / ... → KB chunk, lookup in kb_by_tag
                    //  - corpus identifier → KB chunk, via corpus_ref_to_tag
                    // Plus normalisation passes for variations the model
                    // produces in practice: "[g1]" (with brackets),
                    // "G1" (uppercase), "1" (just the number), and even
                    // "doc-0" emitted as a generic placeholder when no
                    // attached docs exist. The last fallback is the
                    // most robust: quote-based content matching against
                    // the kb chunks we actually fed to the model.
                    let original_label = label.to_string();
                    let normalised = original_label
                        .trim()
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .to_ascii_lowercase();
                    let mut resolved_label = original_label.clone();
                    if !kb_by_tag.contains_key(&resolved_label)
                        && !id_by_label.contains_key(&resolved_label)
                    {
                        // Try the normalised form first.
                        if kb_by_tag.contains_key(&normalised) {
                            resolved_label = normalised.clone();
                        } else if id_by_label.contains_key(&normalised) {
                            resolved_label = normalised.clone();
                        } else if let Some(tag) = corpus_ref_to_tag
                            .get(&original_label)
                            .or_else(|| corpus_ref_to_tag.get(&normalised))
                        {
                            tracing::info!(
                                "[chat] citation doc_id {:?} not a known label/tag; \
                                 retro-resolving via corpus alias to KB tag {:?}",
                                original_label,
                                tag
                            );
                            resolved_label = tag.clone();
                        } else if normalised.chars().all(|c| c.is_ascii_digit())
                            && !normalised.is_empty()
                        {
                            // Bare number like "1": if there's exactly
                            // one [gN] in kb_by_tag, that's almost
                            // certainly what the model meant.
                            let g_keys: Vec<&String> = kb_by_tag
                                .keys()
                                .filter(|k| k.starts_with('g'))
                                .collect();
                            if g_keys.len() == 1 {
                                tracing::info!(
                                    "[chat] citation doc_id {:?} is bare number; \
                                     mapping to sole KB tag {:?}",
                                    original_label,
                                    g_keys[0]
                                );
                                resolved_label = g_keys[0].clone();
                            } else {
                                let candidate = format!("g{normalised}");
                                if kb_by_tag.contains_key(&candidate) {
                                    resolved_label = candidate;
                                }
                            }
                        }

                        // Quote-based content match: when the model
                        // copied a verbatim excerpt of a chunk into the
                        // citation quote, we can find the chunk it
                        // came from and use that tag. Cheaper than the
                        // single-doc fallback below, and more accurate
                        // when chunks span multiple corpus docs.
                        // Requires ≥25-char prefix so a short phrase
                        // doesn't accidentally match every chunk.
                        if resolved_label == original_label
                            && !kb_by_tag.contains_key(&resolved_label)
                            && !id_by_label.contains_key(&resolved_label)
                        {
                            if let Some(quote) = obj.get("quote").and_then(|v| v.as_str()) {
                                let needle = quote
                                    .split_whitespace()
                                    .collect::<Vec<_>>()
                                    .join(" ")
                                    .to_lowercase();
                                let needle_prefix: String =
                                    needle.chars().take(120).collect();
                                if needle_prefix.chars().count() >= 25 {
                                    let mut hit: Option<&str> = None;
                                    for (tag, kb) in &kb_by_tag {
                                        let hay = kb
                                            .text
                                            .split_whitespace()
                                            .collect::<Vec<_>>()
                                            .join(" ")
                                            .to_lowercase();
                                        if hay.contains(&needle_prefix) {
                                            hit = Some(tag.as_str());
                                            break;
                                        }
                                    }
                                    if let Some(tag) = hit {
                                        tracing::info!(
                                            "[chat] citation doc_id {:?} resolved by \
                                             quote-content match to KB tag {:?}",
                                            original_label,
                                            tag
                                        );
                                        resolved_label = tag.to_string();
                                    }
                                }
                            }
                        }

                        // Single-corpus-doc fallback: when every KB
                        // chunk we surfaced for this turn points at
                        // the same underlying corpus document, all
                        // citations almost certainly mean that one
                        // doc — even a paraphrased quote with a
                        // hallucinated page is "talking about GDPR".
                        // Map the unresolved label to any tag from
                        // that doc so the citation pill at least
                        // opens the right viewer. Not safe when KB
                        // chunks span multiple docs (we'd guess).
                        if resolved_label == original_label
                            && !kb_by_tag.contains_key(&resolved_label)
                            && !id_by_label.contains_key(&resolved_label)
                            && !kb_by_tag.is_empty()
                        {
                            let mut doc_ids: std::collections::HashSet<&str> =
                                std::collections::HashSet::new();
                            for kb in kb_by_tag.values() {
                                doc_ids.insert(kb.document_id.as_str());
                            }
                            if doc_ids.len() == 1 {
                                // Pick the lowest-numbered g-tag if any,
                                // otherwise the first tag we see.
                                let mut keys: Vec<&String> =
                                    kb_by_tag.keys().collect();
                                keys.sort();
                                let chosen = keys
                                    .iter()
                                    .find(|k| k.starts_with('g'))
                                    .copied()
                                    .or_else(|| keys.first().copied());
                                if let Some(tag) = chosen {
                                    tracing::info!(
                                        "[chat] citation doc_id {:?} unresolvable; \
                                         all KB chunks share one corpus doc — \
                                         routing to KB tag {:?} (page may be \
                                         hallucinated, viewer still opens correct file)",
                                        original_label,
                                        tag
                                    );
                                    resolved_label = tag.clone();
                                    // The model's page is likely
                                    // hallucinated when it invented
                                    // the doc_id — drop it so the
                                    // viewer falls back to opening
                                    // page 1 / using PDF.js text
                                    // search on the quote.
                                    obj.remove("page");
                                }
                            }
                        }

                        if resolved_label != original_label {
                            obj.insert(
                                "doc_id".into(),
                                Value::String(resolved_label.clone()),
                            );
                        }
                    }
                    let label = resolved_label.as_str();
                    if let Some(kb) = kb_by_tag.get(label) {
                        // Strip our scanner's `[Page N]` markers from
                        // the quote — the model often copies them
                        // verbatim from the chunk text we fed it, but
                        // they don't exist in the underlying PDF, so
                        // PDF.js text-search can't match.
                        if let Some(q) = obj.get("quote").and_then(|v| v.as_str()) {
                            let cleaned = strip_page_markers(q);
                            if cleaned != q {
                                obj.insert("quote".into(), Value::String(cleaned));
                            }
                        }
                        let source = if kb.scope_label == "case" { "case_doc" } else { "kb" };
                        obj.insert("source".into(), Value::String(source.to_string()));
                        obj.insert("scope".into(), Value::String(kb.scope_label.to_string()));
                        obj.insert("path".into(), Value::String(kb.source_path.clone()));
                        obj.insert("chunk_index".into(), Value::Number(kb.chunk_index.into()));
                        obj.insert(
                            "document_id".into(),
                            Value::String(kb.document_id.clone()),
                        );
                        if kb.scope_label == "case" {
                            if let Some(ref cid) = case_id_for_citations {
                                obj.insert("case_id".into(), Value::String(cid.clone()));
                            }
                        }
                        let basename = std::path::Path::new(&kb.source_path)
                            .file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_else(|| kb.source_path.clone());
                        obj.insert("filename".into(), Value::String(basename));
                        // Page assignment: prefer the page the model
                        // emitted in <CITATIONS> if present. The model
                        // can see the literal `[Page N]` markers we
                        // prepend to each PDF page in the chunk text,
                        // and is more accurate per-quote than the
                        // chunker's coarse "page where this chunk
                        // STARTS" assignment — that one is wrong
                        // whenever a chunk spans multiple pages OR
                        // when the model picks a quote from the
                        // chunk's leading overlap section (which
                        // came from the previous chunk and may
                        // belong to a different page than the chunk
                        // is tagged with).
                        // Only stamp `kb.page` as a fallback when the
                        // model didn't provide a usable page.
                        let model_page_ok = obj
                            .get("page")
                            .map(|v| v.is_i64() || v.is_string())
                            .unwrap_or(false);
                        if !model_page_ok {
                            if let Some(p) = kb.page {
                                obj.insert("page".into(), Value::Number(p.into()));
                            }
                        }
                    } else if label.starts_with('v')
                        && label[1..].parse::<usize>().is_ok()
                    {
                        obj.insert("source".into(), Value::String("vanga".to_string()));
                        let title = obj.get("quote")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !title.is_empty() {
                            obj.insert("filename".into(), Value::String(title));
                        }
                    } else {
                        obj.insert("source".into(), Value::String("attached".to_string()));
                        let uuid = id_by_label.get(label).cloned();
                        let filename = uuid
                            .as_ref()
                            .and_then(|u| name_by_id.get(u))
                            .cloned()
                            .unwrap_or_default();
                        if let Some(uuid) = uuid {
                            obj.insert("document_id".into(), Value::String(uuid));
                        }
                        if !filename.is_empty() {
                            obj.insert("filename".into(), Value::String(filename));
                        }
                    }
                    Value::Object(obj)
                })
                .collect(),
            None => Vec::new(),
        };
        tracing::info!("[chat] parsed {} citations from response", citations_array.len());

        // Persist the citation annotations on the assistant message so
        // GET /chat/:id/messages can hand them back when the user
        // reopens this chat from the sidebar.
        if let Some(id) = &asst_msg_id {
            let annotations_json = if citations_array.is_empty() {
                None
            } else {
                Some(Value::Array(citations_array.clone()).to_string())
            };
            match sqlx::query("UPDATE messages SET annotations = ? WHERE id = ?")
                .bind(&annotations_json)
                .bind(id)
                .execute(&state_clone.db)
                .await
            {
                Ok(r) => tracing::info!(
                    "[chat] annotations persisted on message id={} rows_affected={} payload_bytes={}",
                    id,
                    r.rows_affected(),
                    annotations_json.as_ref().map(|s| s.len()).unwrap_or(0),
                ),
                Err(e) => tracing::error!(
                    "[chat] FAILED to persist annotations on id={}: {e}",
                    id
                ),
            }
        }

        // Diagnostic: log the doc_id/source/page of each parsed citation
        // so we can tell whether the model emitted attached-style numeric
        // refs vs KB-style g1/p1 tags, and whether kb_by_tag matched.
        for (i, c) in citations_array.iter().enumerate() {
            tracing::info!(
                "[chat]   citation #{i}: doc_id={:?} source={:?} page={:?} ref={:?}",
                c.get("doc_id").and_then(|v| v.as_str()),
                c.get("source").and_then(|v| v.as_str()),
                c.get("page"),
                c.get("ref"),
            );
        }

        if let Some(intake) = parse_intake_block(&full_response) {
            let payload = json!({ "type": "clarification_request", "questions": intake });
            let _ = tx
                .send(Ok(Event::default().data(payload.to_string())))
                .await;
        }

        let done_payload = json!({ "type": "citations", "citations": citations_array });
        let _ = tx
            .send(Ok(Event::default().data(done_payload.to_string())))
            .await;
    });

    let sse_stream = ReceiverStream::new(rx);
    Sse::new(sse_stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

/// Parse an [INTAKE]...[/INTAKE] block from the model's response into a
/// structured JSON array of questions with chips. Returns None if no block found.
fn parse_intake_block(text: &str) -> Option<Value> {
    let start = text.find("[INTAKE]")?;
    let end = text.find("[/INTAKE]")?;
    if end <= start { return None; }
    let block = &text[start + 8..end];
    let mut questions = Vec::new();
    for line in block.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        // Expected format: question_N: { text: "...", chips: ["...", "..."] }
        if let Some(rest) = line.strip_prefix("question_").and_then(|r| r.split_once(':').map(|(_, v)| v)) {
            let rest = rest.trim();
            // Parse the text field
            let text_val = extract_quoted_after(rest, "text:");
            // Parse chips array
            let chips = extract_chips_array(rest);
            if let Some(t) = text_val {
                let mut q = serde_json::Map::new();
                q.insert("text".into(), Value::String(t));
                q.insert("chips".into(), Value::Array(
                    chips.into_iter().map(Value::String).collect(),
                ));
                questions.push(Value::Object(q));
            }
        }
    }
    if questions.is_empty() { None } else { Some(Value::Array(questions)) }
}

fn extract_quoted_after(s: &str, key: &str) -> Option<String> {
    let idx = s.find(key)?;
    let after = &s[idx + key.len()..];
    let q1 = after.find('"')?;
    let q2 = after[q1 + 1..].find('"')?;
    Some(after[q1 + 1..q1 + 1 + q2].to_string())
}

fn extract_chips_array(s: &str) -> Vec<String> {
    let Some(start) = s.find("chips:") else { return Vec::new(); };
    let rest = &s[start + 6..];
    let Some(bracket_start) = rest.find('[') else { return Vec::new(); };
    let Some(bracket_end) = rest.find(']') else { return Vec::new(); };
    let inner = &rest[bracket_start + 1..bracket_end];
    inner.split(',')
        .filter_map(|c| {
            let trimmed = c.trim().trim_matches('"').trim_matches('\'');
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GET /chat/:id
// ---------------------------------------------------------------------------
async fn get_chat(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    let row: Option<(String, String, Option<String>, Option<String>, String)> =
        sqlx::query_as(
            "SELECT id, user_id, project_id, title, updated_at \
             FROM chats WHERE id = ? AND user_id = ?",
        )
        .bind(&id)
        .bind(&auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let (chat_id, user_id, project_id, title, updated_at) =
        row.ok_or_else(|| err(StatusCode::NOT_FOUND, "Chat not found"))?;

    let msg_rows: Vec<(String, String, Option<String>, String, Option<String>)> =
        sqlx::query_as(
            "SELECT id, role, content, created_at, annotations \
             FROM messages WHERE chat_id = ? ORDER BY created_at ASC",
        )
        .bind(&chat_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let with_annot = msg_rows
        .iter()
        .filter(|(_, role, _, _, ann)| role == "assistant" && ann.is_some())
        .count();
    tracing::info!(
        "[chat] GET /chat/{}: {} messages total, {} assistant rows with persisted annotations",
        chat_id,
        msg_rows.len(),
        with_annot,
    );

    let messages: Vec<Value> = msg_rows
        .into_iter()
        .map(|(mid, role, content, created_at, annotations)| {
            let content_value = if role == "assistant" {
                json!([{ "type": "content", "text": content.unwrap_or_default() }])
            } else {
                json!(content.unwrap_or_default())
            };
            // Hydrate annotations the same way the live SSE event does,
            // so the chat-history loader path delivers identical shape.
            // Re-apply `strip_page_markers` to each KB quote: rows
            // persisted before that fix landed contain the literal
            // `[Page N]` markers that PDF.js can't match — sanitising
            // on read makes old chats render correctly without a
            // destructive migration.
            let annotations_value = annotations
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .map(sanitise_annotations_quotes)
                .unwrap_or_else(|| Value::Array(Vec::new()));
            json!({
                "id": mid,
                "role": role,
                "content": content_value,
                "created_at": created_at,
                "annotations": annotations_value,
            })
        })
        .collect();

    Ok(Json(json!({
        "chat": {
            "id": chat_id,
            "user_id": user_id,
            "project_id": project_id,
            "title": title,
            "updated_at": updated_at,
        },
        "messages": messages,
    })))
}

// ---------------------------------------------------------------------------
// PATCH /chat/:id  — update title
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct PatchChatBody {
    title: Option<String>,
}

async fn patch_chat(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
    Json(body): Json<PatchChatBody>,
) -> ApiResult {
    let result = sqlx::query(
        "UPDATE chats SET title = COALESCE(?, title), updated_at = datetime('now') \
         WHERE id = ? AND user_id = ?",
    )
    .bind(&body.title)
    .bind(&id)
    .bind(&auth.user_id)
    .execute(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Chat not found"));
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// DELETE /chat/:id
// ---------------------------------------------------------------------------
async fn delete_chat(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    // Snapshot the cache-keyed paths of every doc linked to this chat
    // BEFORE the FK cascade (migration 0013) wipes the rows. We need
    // both storage_path (binary) and extracted_text_path so the
    // ref-count check can free the right files.
    let docs_to_check: Vec<(String, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT id, storage_path, extracted_text_path, content_hash \
             FROM documents WHERE chat_id = ? AND user_id = ?",
        )
        .bind(&id)
        .bind(&auth.user_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

    let result = sqlx::query("DELETE FROM chats WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&auth.user_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    if result.rows_affected() == 0 {
        return Err(err(StatusCode::NOT_FOUND, "Chat not found"));
    }

    // FK cascade has already removed every documents row that pointed
    // at this chat. For each unique content_hash we just lost, check
    // whether any other documents row (any chat / any user) still
    // references the same bytes. If not, the binary + extracted-text
    // files are safe to delete from disk. Hashes shared with another
    // chat keep their files alive.
    if !docs_to_check.is_empty() {
        if let Ok(storage) = make_storage() {
            let mut seen_hashes: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (doc_id, sp, txt, hash) in &docs_to_check {
                let Some(hash) = hash.as_ref() else { continue };
                if !seen_hashes.insert(hash.clone()) {
                    continue;
                }
                let still_referenced: Option<(i64,)> = sqlx::query_as(
                    "SELECT 1 FROM documents WHERE content_hash = ? LIMIT 1",
                )
                .bind(hash)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None);
                if still_referenced.is_some() {
                    tracing::info!(
                        "[chat] keeping cache files for hash {} (still referenced by another doc)",
                        hash
                    );
                    continue;
                }
                if let Some(key) = sp.as_ref() {
                    if let Err(e) = storage.delete(key).await {
                        tracing::warn!(
                            "[chat] failed to delete cache binary {} (doc {}): {}",
                            key,
                            doc_id,
                            e
                        );
                    }
                }
                if let Some(key) = txt.as_ref() {
                    if let Err(e) = storage.delete(key).await {
                        tracing::warn!(
                            "[chat] failed to delete cache text {} (doc {}): {}",
                            key,
                            doc_id,
                            e
                        );
                    }
                }
            }
            tracing::info!(
                "[chat] delete chat={} swept {} doc row(s), {} unique hash(es)",
                id,
                docs_to_check.len(),
                seen_hashes.len()
            );
        }
    }

    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------------------
// GET /chat/:id/messages
// ---------------------------------------------------------------------------
async fn get_messages(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<String>,
) -> ApiResult {
    // Verify ownership
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM chats WHERE id = ? AND user_id = ?")
            .bind(&id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    exists.ok_or_else(|| err(StatusCode::NOT_FOUND, "Chat not found"))?;

    let rows: Vec<(String, String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT id, role, content, created_at, annotations FROM messages \
         WHERE chat_id = ? ORDER BY created_at ASC",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let with_annot = rows
        .iter()
        .filter(|(_, role, _, _, ann)| role == "assistant" && ann.is_some())
        .count();
    tracing::info!(
        "[chat] GET /chat/{}/messages: {} rows total, {} assistant rows with persisted annotations",
        id,
        rows.len(),
        with_annot,
    );

    let messages: Vec<Value> = rows
        .into_iter()
        .map(|(id, role, content, created_at, annotations)| {
            // Hydrate annotations from the stored JSON so the chat-history
            // path delivers the same shape as the live SSE event. Falls
            // back to an empty array when the column is NULL (older
            // assistant turns from before migration 0012).
            let annotations_value = annotations
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or_else(|| Value::Array(Vec::new()));
            json!({
                "id": id,
                "role": role,
                "content": content,
                "created_at": created_at,
                "annotations": annotations_value,
            })
        })
        .collect();

    Ok(Json(json!({ "messages": messages })))
}

// ---------------------------------------------------------------------------
// POST /chat/:id/message  — SSE streaming
// Body: { content, model?, system_prompt? }
// Response: text/event-stream with delta/done events
// ---------------------------------------------------------------------------
#[derive(Deserialize)]
struct PostMessageBody {
    content: String,
    model: Option<String>,
    system_prompt: Option<String>,
}

async fn post_message(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(chat_id): Path<String>,
    Json(body): Json<PostMessageBody>,
) -> Response {
    // Verify ownership
    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM chats WHERE id = ? AND user_id = ?")
            .bind(&chat_id)
            .bind(&auth.user_id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();

    if exists.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({"detail": "Chat not found"}))).into_response();
    }

    // Persist user message
    let user_msg_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = sqlx::query(
        "INSERT INTO messages (id, chat_id, role, content) VALUES (?, ?, 'user', ?)",
    )
    .bind(&user_msg_id)
    .bind(&chat_id)
    .bind(&body.content)
    .execute(&state.db)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail": e.to_string()})),
        )
            .into_response();
    }

    // Load conversation history (last 50 messages)
    let history: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT role, content FROM messages WHERE chat_id = ? ORDER BY created_at ASC LIMIT 50")
            .bind(&chat_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    let messages: Vec<Message> = history
        .into_iter()
        .filter_map(|(role, content)| {
            let r = match role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => return None,
            };
            Some(Message { role: r, content: content.unwrap_or_default(), images: vec![], tool_calls: vec![], tool_call_id: None, tool_name: None, reasoning_content: None })
        })
        .collect();

    // Resolve model from request or user settings
    let user_settings = fetch_llm_settings(&state.db, &auth.user_id)
        .await
        .ok();

    let raw_model = body.model.clone().unwrap_or_else(|| {
        user_settings
            .as_ref()
            .and_then(|s| s.main_model.clone())
            .unwrap_or_else(|| "local:qwen2.5:3b".to_string())
    });
    let model = raw_model.clone();

    // Build per-provider config from saved settings.
    let local_config = if model.starts_with("local:") || model.starts_with("openai:") {
        user_settings.as_ref().and_then(|s| {
            let (base, key, model_name) = if model.starts_with("openai:") {
                (
                    s.openai_api_key
                        .as_ref()
                        .map(|_| "https://api.openai.com/v1".to_string())
                        .unwrap_or_default(),
                    s.openai_api_key.clone(),
                    s.openai_model.clone().unwrap_or_else(|| {
                        llm::strip_model_prefix(&model).to_string()
                    }),
                )
            } else if s.active_provider.as_deref() == Some("deepseek") {
                (
                    "https://api.deepseek.com/v1".to_string(),
                    s.local_api_key.clone(),
                    s.local_model.clone().unwrap_or_else(|| {
                        llm::strip_model_prefix(&model).to_string()
                    }),
                )
            } else {
                (
                    s.local_base_url.clone().unwrap_or_default(),
                    s.local_api_key.clone(),
                    s.local_model.clone().unwrap_or_else(|| {
                        llm::strip_model_prefix(&model).to_string()
                    }),
                )
            };
            if base.is_empty() {
                None
            } else {
                Some(LocalConfig {
                    base_url: base,
                    api_key: key.filter(|s| !s.trim().is_empty()),
                    model: model_name,
                })
            }
        })
    } else {
        None
    };

    let system_prompt = body.system_prompt.unwrap_or_default();

    let params = StreamParams {
        model: model.clone(),
        system_prompt,
        system_volatile: String::new(),
        messages,
        tools: vec![],
        max_iterations: 1,
        enable_thinking: false,
        local_config,
        claude_api_key: user_settings.as_ref().and_then(|s| s.claude_api_key.clone()),
        gemini_api_key: user_settings.as_ref().and_then(|s| s.gemini_api_key.clone()),
        gemini_region: user_settings.as_ref().and_then(|s| s.gemini_region.clone()),
    };

    // SSE stream
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    let state_clone = state.clone();
    let chat_id_clone = chat_id.clone();

    tokio::spawn(async move {
        let mut full_response = String::new();

        match llm::stream_chat(params).await {
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default().event("error").data(e.to_string())))
                    .await;
            }
            Ok(mut stream) => {
                while let Some(event) = stream.next().await {
                    match event {
                        Ok(StreamEvent::ContentDelta(text)) => {
                            full_response.push_str(&text);
                            let data = serde_json::to_string(&json!({ "delta": text }))
                                .unwrap_or_default();
                            if tx.send(Ok(Event::default().event("delta").data(data))).await.is_err() {
                                break;
                            }
                        }
                        Ok(StreamEvent::ReasoningDelta(text)) => {
                            let data = serde_json::to_string(&json!({ "type": "reasoning_delta", "text": text }))
                                .unwrap_or_default();
                            if tx.send(Ok(Event::default().event("delta").data(data))).await.is_err() {
                                break;
                            }
                        }
                        Ok(StreamEvent::ReasoningEnd) => {
                            let data = serde_json::to_string(&json!({ "type": "reasoning_block_end" }))
                                .unwrap_or_default();
                            let _ = tx.send(Ok(Event::default().event("delta").data(data))).await;
                        }
                        Ok(StreamEvent::Done) | Err(_) => break,
                        _ => {}
                    }
                }

                // Persist assistant message
                let asst_msg_id = uuid::Uuid::new_v4().to_string();
                let _ = sqlx::query(
                    "INSERT INTO messages (id, chat_id, role, content) VALUES (?, ?, 'assistant', ?)",
                )
                .bind(&asst_msg_id)
                .bind(&chat_id_clone)
                .bind(&full_response)
                .execute(&state_clone.db)
                .await;

                // Update chat timestamp
                let _ = sqlx::query(
                    "UPDATE chats SET updated_at = datetime('now') WHERE id = ?",
                )
                .bind(&chat_id_clone)
                .execute(&state_clone.db)
                .await;

                let done_data = serde_json::to_string(&json!({ "message_id": asst_msg_id }))
                    .unwrap_or_default();
                let _ = tx.send(Ok(Event::default().event("done").data(done_data))).await;
            }
        }
    });

    let sse_stream = ReceiverStream::new(rx);
    Sse::new(sse_stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

// ---------------------------------------------------------------------------
// POST /chat/:id/generate-title — short title from first user message
// ---------------------------------------------------------------------------
async fn generate_title(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(chat_id): Path<String>,
) -> ApiResult {
    let owns: Option<(String,)> = sqlx::query_as("SELECT id FROM chats WHERE id = ? AND user_id = ?")
        .bind(&chat_id)
        .bind(&auth.user_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;
    if owns.is_none() {
        return Err(err(StatusCode::NOT_FOUND, "Chat not found"));
    }

    let first: Option<(String,)> = sqlx::query_as(
        "SELECT content FROM messages WHERE chat_id = ? AND role = 'user' \
         ORDER BY created_at ASC LIMIT 1",
    )
    .bind(&chat_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    let Some((first_msg,)) = first else {
        return Ok(Json(json!({ "title": null })));
    };

    let user_settings = fetch_llm_settings(&state.db, &auth.user_id).await.ok();
    // Pick a model from user settings — prefer the active provider, then any
    // configured one. Falling back to Gemini default fails when the user only
    // has a Local/OpenAI key set, so try to match what the chat is actually using.
    //
    // Crucially every candidate model must have its endpoint/key configured —
    // otherwise we'd happily pick `local:llama3.2:3b` only to 502 because the
    // user never wrote a localBaseUrl.
    let is_usable = |m: &str, s: &crate::routes::user::LlmSettings| -> bool {
        if let Some(rest) = m.strip_prefix("local:") {
            if rest.is_empty() {
                return false;
            }
            // DeepSeek models use the local: prefix but don't need
            // local_base_url — hardcoded to api.deepseek.com.
            if s.active_provider.as_deref() == Some("deepseek") {
                return s.local_api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false)
                    || std::env::var("DEEPSEEK_API_KEY").is_ok_and(|k| !k.is_empty());
            }
            return s.local_base_url
                .as_deref()
                .map(|x| !x.trim().is_empty())
                .unwrap_or(false);
        }
        if let Some(rest) = m.strip_prefix("openai:") {
            return !rest.is_empty()
                && s.openai_api_key
                    .as_deref()
                    .map(|x| !x.trim().is_empty())
                    .unwrap_or(false);
        }
        if m.starts_with("claude") {
            return s
                .claude_api_key
                .as_deref()
                .map(|x| !x.trim().is_empty())
                .unwrap_or(false);
        }
        if m.starts_with("gemini") {
            return s
                .gemini_api_key
                .as_deref()
                .map(|x| !x.trim().is_empty())
                .unwrap_or(false);
        }
        false
    };
    let title_model = user_settings
        .as_ref()
        .and_then(|s| s.title_model.clone().filter(|m| is_usable(m, s)))
        .or_else(|| {
            user_settings
                .as_ref()
                .and_then(|s| s.main_model.clone().filter(|m| is_usable(m, s)))
        })
        .or_else(|| {
            user_settings.as_ref().and_then(|s| match s.active_provider.as_deref() {
                // For local/openai also require the corresponding endpoint
                // / API key to be configured — otherwise we'd pick a model
                // that has no way to be reached and the title generation
                // would 502.
                Some("local") => match (&s.local_model, &s.local_base_url) {
                    (Some(m), Some(b)) if !b.trim().is_empty() => Some(format!("local:{m}")),
                    _ => None,
                },
                Some("deepseek") => s.local_model.as_ref().map(|m| format!("local:{m}")),
                Some("openai") => match (&s.openai_model, &s.openai_api_key) {
                    (Some(m), Some(k)) if !k.trim().is_empty() => Some(format!("openai:{m}")),
                    _ => None,
                },
                Some("claude") => s
                    .claude_api_key
                    .as_ref()
                    .filter(|k| !k.trim().is_empty())
                    .map(|_| "claude-sonnet-4-6".to_string()),
                Some("gemini") => s
                    .gemini_api_key
                    .as_ref()
                    .filter(|k| !k.trim().is_empty())
                    .map(|_| "gemini-3-flash-preview".to_string()),
                _ => None,
            })
        })
        .or_else(|| {
            // No active_provider — pick first configured.
            let s = user_settings.as_ref()?;
            if let Some(m) = &s.local_model {
                if s.local_base_url.is_some() {
                    return Some(format!("local:{m}"));
                }
            }
            if let Some(m) = &s.openai_model {
                if s.openai_api_key.is_some() {
                    return Some(format!("openai:{m}"));
                }
            }
            if s.claude_api_key.is_some() { return Some("claude-sonnet-4-6".to_string()); }
            if s.gemini_api_key.is_some() { return Some("gemini-3-flash-preview".to_string()); }
            None
        })
        .unwrap_or_else(|| "local:qwen2.5:3b".to_string());

    tracing::info!("[chat] generate_title using model={title_model}");

    let local_config = if title_model.starts_with("local:") || title_model.starts_with("openai:") {
        user_settings.as_ref().and_then(|s| {
            let (base, key, mname) = if title_model.starts_with("openai:") {
                (
                    s.openai_api_key.as_ref().map(|_| "https://api.openai.com/v1".to_string()).unwrap_or_default(),
                    s.openai_api_key.clone(),
                    s.openai_model.clone().unwrap_or_default(),
                )
            } else if s.active_provider.as_deref() == Some("deepseek") {
                (
                    "https://api.deepseek.com/v1".to_string(),
                    s.local_api_key.clone(),
                    s.local_model.clone().unwrap_or_default(),
                )
            } else {
                (
                    s.local_base_url.clone().unwrap_or_default(),
                    s.local_api_key.clone(),
                    s.local_model.clone().unwrap_or_default(),
                )
            };
            if base.is_empty() { None } else {
                Some(LocalConfig {
                    base_url: base,
                    api_key: key.filter(|s| !s.trim().is_empty()),
                    model: if mname.is_empty() { llm::strip_model_prefix(&title_model).to_string() } else { mname },
                })
            }
        })
    } else { None };

    let prompt = format!(
        "Generate a concise 3-5 word title (no quotes, no punctuation) for a chat that begins with this user message:\n\n{}",
        first_msg.chars().take(500).collect::<String>()
    );

    let params = StreamParams {
        model: title_model.clone(),
        system_prompt: String::new(),
        system_volatile: String::new(),
        messages: vec![Message::user(prompt)],
        tools: vec![],
        max_iterations: 1,
        enable_thinking: false,
        local_config,
        claude_api_key: user_settings.as_ref().and_then(|s| s.claude_api_key.clone()),
        gemini_api_key: user_settings.as_ref().and_then(|s| s.gemini_api_key.clone()),
        gemini_region: user_settings.as_ref().and_then(|s| s.gemini_region.clone()),
    };

    let title_text = match llm::provider_for_model(&title_model) {
        llm::Provider::Claude => llm::claude::complete(params).await,
        llm::Provider::OpenAI => llm::local::complete(params).await,
        llm::Provider::Gemini => llm::gemini::complete(params).await,
    }
    .map_err(|e| err(StatusCode::BAD_GATEWAY, &e.to_string()))?;

    let title: String = title_text
        .lines()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace())
        .chars()
        .take(80)
        .collect();

    sqlx::query("UPDATE chats SET title = ?, updated_at = datetime('now') WHERE id = ?")
        .bind(&title)
        .bind(&chat_id)
        .execute(&state.db)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()))?;

    Ok(Json(json!({ "title": title })))
}

/// Extract document text from JSON-like model output.
/// Some models (e.g. qwen2.5-uncensored) output JSON wrapping document
/// content instead of plain markdown. This extracts all quoted strings
/// that look like document paragraphs (>15 chars with spaces).
fn extract_text_from_model_json(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if !trimmed.starts_with('{') {
        return None;
    }

    let mut paragraphs = Vec::new();
    let mut in_quote = false;
    let mut current = String::new();
    let mut escaped = false;

    for c in trimmed.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        if c == '\\' && in_quote {
            escaped = true;
            continue;
        }
        if c == '"' {
            if in_quote {
                if current.len() > 15 && current.contains(' ') {
                    paragraphs.push(current.clone());
                }
                current.clear();
            }
            in_quote = !in_quote;
        } else if in_quote {
            current.push(c);
        }
    }

    if paragraphs.is_empty() {
        return None;
    }

    Some(paragraphs.join("\n\n"))
}

fn clean_draft_text(text: &str) -> String {
    // Handle JSON-wrapped content: model output document as JSON instead of markdown
    if let Some(extracted) = extract_text_from_model_json(text) {
        return extracted;
    }

    let lines: Vec<&str> = text.lines().collect();
    let keywords = [
        "affidavit", "deed", "notice", "agreement", "contract",
        "in the court", "petition", "memorandum", "know all men", "by this",
        "most respectfully", "whereas"
    ];

    for (idx, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        let trimmed = lower.trim();
        if trimmed.is_empty() {
            continue;
        }
        for kw in &keywords {
            if trimmed.starts_with(kw) || trimmed.contains(&format!(" {} ", kw)) {
                return lines[idx..].join("\n");
            }
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::{extract_citations_block, sanitise_annotations_quotes, strip_page_markers, clean_draft_text, extract_text_from_model_json};
    use serde_json::{json, Value};

    #[test]
    fn test_clean_draft_text_strips_preamble() {
        let input = "Sure, here's the affidavit:\n\nAFFIDAVIT\nI, Ravi Sharma...";
        let out = clean_draft_text(input);
        assert_eq!(out, "AFFIDAVIT\nI, Ravi Sharma...");
    }

    #[test]
    fn test_clean_draft_text_handles_json_wrapped_content() {
        let input = r#"{"affidavit":{"I","Rahul Verma, S/o Sh. Mohan Sharma, R/o H-56, Uttam Nagar, New Delhi","1. That the deponent is a citizen of India and is fully conversant with the facts hereof.","That,"},"AffidavitForCheating":true}"#;
        let out = clean_draft_text(input);
        assert!(!out.starts_with('{'), "should not start with JSON brace");
        assert!(out.contains("Rahul Verma"), "should contain extracted text");
        assert!(out.contains("deponent is a citizen"), "should contain paragraph text");
    }

    #[test]
    fn test_extract_text_from_model_json_skips_short_strings() {
        let input = r#"{"key":"val","body":"This is a long paragraph with many words in it."}"#;
        let out = extract_text_from_model_json(input).unwrap();
        assert_eq!(out, "This is a long paragraph with many words in it.");
    }

    #[test]
    fn test_extract_text_from_model_json_returns_none_for_non_json() {
        assert!(extract_text_from_model_json("AFFIDAVIT\nI, Ravi Sharma...").is_none());
    }

    #[test]
    fn sanitise_annotations_quotes_strips_each_entry() {
        let input = json!([
            { "doc_id": "g1", "quote": "[Page 1]\nFirst quote", "page": 1 },
            { "doc_id": "g2", "quote": "Plain quote", "page": 2 },
            { "doc_id": "g3", "quote": "[Page 3] Mid [Page 5] tail", "page": 3 },
        ]);
        let out = sanitise_annotations_quotes(input);
        let arr = out.as_array().expect("array");
        assert_eq!(arr[0]["quote"], "First quote");
        assert_eq!(arr[1]["quote"], "Plain quote");
        assert_eq!(arr[2]["quote"], "Mid tail");
    }

    #[test]
    fn sanitise_annotations_quotes_passes_non_array_through() {
        let v = json!({ "not": "array" });
        assert_eq!(sanitise_annotations_quotes(v.clone()), v);
    }

    #[test]
    fn sanitise_annotations_quotes_preserves_other_fields() {
        let input = json!([{
            "doc_id": "g1",
            "quote": "[Page 1]\ntext",
            "page": 1,
            "source": "kb",
            "scope": "global",
            "filename": "a.pdf",
        }]);
        let out = sanitise_annotations_quotes(input);
        let obj = out.as_array().unwrap()[0].as_object().unwrap();
        assert_eq!(obj["quote"], Value::String("text".to_string()));
        assert_eq!(obj["source"], "kb");
        assert_eq!(obj["scope"], "global");
        assert_eq!(obj["filename"], "a.pdf");
        assert_eq!(obj["page"], 1);
    }

    #[test]
    fn strip_page_markers_drops_leading_marker() {
        let q = "[Page 1]\nModello [2026] per la Valutazione…";
        assert_eq!(
            strip_page_markers(q),
            "Modello [2026] per la Valutazione…"
        );
    }

    #[test]
    fn strip_page_markers_drops_inline_marker() {
        let q = "qualcosa qui [Page 5] e qualcosa lì";
        assert_eq!(
            strip_page_markers(q),
            "qualcosa qui e qualcosa lì"
        );
    }

    #[test]
    fn strip_page_markers_handles_multi_digit() {
        let q = "[Page 123]\ntesto pagina centoventitré";
        assert_eq!(strip_page_markers(q), "testo pagina centoventitré");
    }

    #[test]
    fn strip_page_markers_preserves_other_brackets() {
        // Real document brackets like [2026] or [art. 5] must survive.
        let q = "Articolo [art. 5] del 2026 [2026]";
        assert_eq!(strip_page_markers(q), q);
    }

    #[test]
    fn strip_page_markers_preserves_non_marker_text() {
        let q = "Plain quote with no markers at all.";
        assert_eq!(strip_page_markers(q), q);
    }

    #[test]
    fn strip_page_markers_handles_multiple_markers() {
        let q = "[Page 1]\nfoo [Page 2]\nbar";
        assert_eq!(strip_page_markers(q), "foo bar");
    }

    #[test]
    fn extracts_plain_block() {
        let text = "Some answer.\n<CITATIONS>[{\"doc\":\"a\",\"page\":1}]</CITATIONS>";
        let v = extract_citations_block(text).unwrap();
        assert_eq!(v, json!([{"doc":"a","page":1}]));
    }

    #[test]
    fn extracts_block_with_code_fence() {
        let text = "Answer.\n<CITATIONS>\n```json\n[{\"x\":1}]\n```\n</CITATIONS>";
        let v = extract_citations_block(text).unwrap();
        assert_eq!(v, json!([{"x":1}]));
    }

    #[test]
    fn case_insensitive_tag() {
        let text = "<citations>[]</citations>";
        let v = extract_citations_block(text).unwrap();
        assert_eq!(v, json!([]));
    }

    #[test]
    fn returns_none_for_no_block() {
        assert!(extract_citations_block("plain text").is_none());
    }

    #[test]
    fn returns_none_for_unclosed_block() {
        assert!(extract_citations_block("<CITATIONS>[1,2,3]").is_none());
    }

    #[test]
    fn returns_none_for_invalid_json() {
        assert!(extract_citations_block("<CITATIONS>not json</CITATIONS>").is_none());
    }

    #[test]
    fn picks_last_block_when_multiple() {
        // rfind on "<citations>" → last opening tag wins.
        let text = "<CITATIONS>[1]</CITATIONS> ... <CITATIONS>[2]</CITATIONS>";
        let v = extract_citations_block(text).unwrap();
        assert_eq!(v, json!([2]));
    }
}
