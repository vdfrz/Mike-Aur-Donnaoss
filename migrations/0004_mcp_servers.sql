-- Per-user MCP server configurations.
-- Schema mirrors Anthropic's `claude_desktop_config.json` shape:
--   stdio (local) servers → command + args (JSON) + env (JSON)
--   remote (HTTP/SSE) servers → url + headers (JSON) + transport
-- The two modes are mutually exclusive: a row has either `command` set or `url`.

CREATE TABLE IF NOT EXISTS mcp_servers (
    user_id      TEXT NOT NULL REFERENCES user_profiles(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,                -- unique per user
    transport    TEXT NOT NULL DEFAULT 'http', -- 'http' | 'sse' | 'stdio'
    url          TEXT,                         -- for http/sse
    command      TEXT,                         -- for stdio
    args_json    TEXT NOT NULL DEFAULT '[]',   -- JSON array for stdio args
    env_json     TEXT NOT NULL DEFAULT '{}',   -- JSON object for stdio env
    headers_json TEXT NOT NULL DEFAULT '{}',   -- JSON object for http/sse headers
    api_key      TEXT,                         -- shortcut: becomes "Authorization: Bearer <key>"
    enabled      INTEGER NOT NULL DEFAULT 1,   -- 0=disabled, 1=enabled
    created_at   TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (user_id, name)
);
