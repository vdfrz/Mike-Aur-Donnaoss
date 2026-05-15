/// MCP client — Streamable HTTP transport (spec 2024-11-05+) with SSE fallback.
/// Mirrors backend/src/lib/mcp.ts logic.
/// No official Rust SDK exists; we implement the wire protocol directly via reqwest.
use anyhow::{anyhow, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::llm::ToolSchema;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub url: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    #[default]
    Http,
    Sse,
}

pub fn parse_servers_config() -> Vec<McpServerConfig> {
    let raw = std::env::var("MCP_SERVERS").unwrap_or_default();
    if raw.trim().is_empty() {
        return vec![];
    }
    match serde_json::from_str::<Vec<McpServerConfig>>(&raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[mcp] Invalid MCP_SERVERS env var: {e}");
            vec![]
        }
    }
}

// ---------------------------------------------------------------------------
// Client registry (lazy connect, one client per server)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct McpEntry {
    tool_names: Vec<String>,
    connected: bool,
}

pub struct McpRegistry {
    configs: Vec<McpServerConfig>,
    entries: RwLock<Vec<McpEntry>>,
    client: reqwest::Client,
}

impl McpRegistry {
    pub fn new() -> Self {
        let configs = parse_servers_config();
        let entries = (0..configs.len()).map(|_| McpEntry::default()).collect();
        Self {
            configs,
            entries: RwLock::new(entries),
            client: reqwest::Client::new(),
        }
    }

    fn headers(&self, config: &McpServerConfig) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert("Content-Type", "application/json".parse().unwrap());
        if let Some(key) = &config.api_key {
            h.insert(
                "Authorization",
                format!("Bearer {key}").parse().unwrap(),
            );
        }
        h
    }

    async fn rpc(&self, config: &McpServerConfig, method: &str, params: Value) -> Result<Value> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let resp = self
            .client
            .post(&config.url)
            .headers(self.headers(config))
            .json(&body)
            .send()
            .await?;
        let val: Value = resp.json().await?;
        if let Some(err) = val.get("error") {
            return Err(anyhow!("MCP RPC error from {}: {err}", config.name));
        }
        Ok(val["result"].clone())
    }

    pub async fn get_tools(&self) -> Vec<ToolSchema> {
        let mut tools = Vec::new();
        let mut entries = self.entries.write().await;

        for (i, config) in self.configs.iter().enumerate() {
            // initialize connection + list tools lazily
            match self.rpc(config, "tools/list", json!({})).await {
                Err(e) => {
                    tracing::warn!("[mcp] Failed to list tools from {}: {e}", config.name);
                    entries[i].connected = false;
                }
                Ok(result) => {
                    let tool_list = result["tools"].as_array().cloned().unwrap_or_default();
                    let names: Vec<String> = tool_list
                        .iter()
                        .filter_map(|t| t["name"].as_str().map(str::to_string))
                        .collect();
                    entries[i].tool_names = names;
                    entries[i].connected = true;

                    for t in &tool_list {
                        let name = t["name"].as_str().unwrap_or("").to_string();
                        let description = t["description"].as_str().unwrap_or("").to_string();
                        let parameters = t["inputSchema"].clone();
                        tools.push(ToolSchema {
                            kind: "function".to_string(),
                            function: crate::llm::ToolFunction { name, description, parameters },
                        });
                    }
                }
            }
        }
        tools
    }

    pub fn is_mcp_tool(&self, name: &str) -> bool {
        // Sync check — entries must already be populated via get_tools()
        if let Ok(entries) = self.entries.try_read() {
            return entries.iter().any(|e| e.tool_names.iter().any(|n| n == name));
        }
        false
    }

    pub async fn call_tool(&self, name: &str, input: Value) -> String {
        for (i, config) in self.configs.iter().enumerate() {
            let has_tool = {
                let entries = self.entries.read().await;
                entries[i].tool_names.iter().any(|n| n == name)
            };
            if !has_tool { continue; }

            match self.rpc(config, "tools/call", json!({ "name": name, "arguments": input })).await {
                Err(e) => {
                    return json!({ "error": format!("MCP server {} error: {e}", config.name) })
                        .to_string();
                }
                Ok(result) => {
                    let content = &result["content"];
                    if let Some(arr) = content.as_array() {
                        return arr
                            .iter()
                            .map(|c| c["text"].as_str().unwrap_or("").to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                    }
                    return result.to_string();
                }
            }
        }
        json!({ "error": format!("No MCP server handles tool: {name}") }).to_string()
    }
}

pub type SharedMcpRegistry = Arc<McpRegistry>;

pub fn make_registry() -> SharedMcpRegistry {
    Arc::new(McpRegistry::new())
}
