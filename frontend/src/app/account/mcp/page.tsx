"use client";

import { useEffect, useState } from "react";
import {
    Plus,
    Trash2,
    Server,
    Globe,
    Terminal,
    Eye,
    EyeOff,
    Check,
    AlertCircle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

type Transport = "http" | "sse" | "stdio";

interface McpServer {
    name: string;
    transport: Transport;
    url?: string;
    command?: string;
    args: string[];
    env: Record<string, string>;
    headers: Record<string, string>;
    api_key?: string;
    enabled: boolean;
}

function blankServer(): McpServer {
    return {
        name: "",
        transport: "http",
        url: "",
        command: "",
        args: [],
        env: {},
        headers: {},
        api_key: "",
        enabled: true,
    };
}

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

async function api<T>(path: string, init?: RequestInit): Promise<T> {
    const res = await fetch(`${API_BASE}${path}`, {
        ...init,
        headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${getToken()}`,
            ...(init?.headers ?? {}),
        },
    });
    const text = await res.text();
    const data = text ? JSON.parse(text) : {};
    if (!res.ok) throw new Error(data.detail || `HTTP ${res.status}`);
    return data as T;
}

export default function McpServersPage() {
    const [servers, setServers] = useState<McpServer[]>([]);
    const [loading, setLoading] = useState(true);
    const [editing, setEditing] = useState<McpServer | null>(null);
    const [originalName, setOriginalName] = useState<string | null>(null);
    const [savedName, setSavedName] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);

    async function reload() {
        setLoading(true);
        try {
            const data = await api<{ servers: McpServer[] }>("/user/mcp-servers");
            setServers(data.servers ?? []);
        } catch (e: any) {
            setError(e.message);
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        void reload();
    }, []);

    const handleAdd = () => {
        setEditing(blankServer());
        setOriginalName(null);
        setError(null);
    };

    const handleEdit = (s: McpServer) => {
        setEditing({ ...s, api_key: s.api_key ?? "", url: s.url ?? "", command: s.command ?? "" });
        setOriginalName(s.name);
        setError(null);
    };

    const handleSave = async () => {
        if (!editing) return;
        setError(null);
        try {
            const path = originalName
                ? `/user/mcp-servers/${encodeURIComponent(originalName)}`
                : "/user/mcp-servers";
            const method = originalName ? "PUT" : "POST";
            await api(path, {
                method,
                body: JSON.stringify({
                    name: editing.name.trim(),
                    transport: editing.transport,
                    url: editing.transport === "stdio" ? null : (editing.url || null),
                    command: editing.transport === "stdio" ? (editing.command || null) : null,
                    args: editing.args.filter((a) => a.length > 0),
                    env: editing.env,
                    headers: editing.headers,
                    api_key: editing.api_key || null,
                    enabled: editing.enabled,
                }),
            });
            setSavedName(editing.name);
            setTimeout(() => setSavedName(null), 1500);
            setEditing(null);
            setOriginalName(null);
            await reload();
        } catch (e: any) {
            setError(e.message);
        }
    };

    const handleDelete = async (name: string) => {
        if (!confirm(`Delete MCP server "${name}"?`)) return;
        try {
            await api(`/user/mcp-servers/${encodeURIComponent(name)}`, { method: "DELETE" });
            await reload();
        } catch (e: any) {
            setError(e.message);
        }
    };

    const handleToggle = async (s: McpServer) => {
        try {
            await api(`/user/mcp-servers/${encodeURIComponent(s.name)}`, {
                method: "PUT",
                body: JSON.stringify({ ...s, api_key: s.api_key || null, enabled: !s.enabled }),
            });
            await reload();
        } catch (e: any) {
            setError(e.message);
        }
    };

    if (editing) {
        return (
            <McpEditor
                value={editing}
                isNew={originalName === null}
                onChange={setEditing}
                onCancel={() => {
                    setEditing(null);
                    setOriginalName(null);
                    setError(null);
                }}
                onSave={handleSave}
                error={error}
            />
        );
    }

    return (
        <div className="space-y-6 max-w-3xl">
            <div className="flex items-center justify-between">
                <div>
                    <h2 className="text-2xl font-medium font-serif">MCP Servers</h2>
                    <p className="text-sm text-gray-500 mt-1">
                        Connect tools and data sources via the Model Context
                        Protocol. Schema follows Anthropic&apos;s standard
                        (<code className="text-xs bg-gray-100 px-1 py-0.5 rounded">claude_desktop_config.json</code>).
                    </p>
                </div>
                <Button
                    onClick={handleAdd}
                    className="bg-black hover:bg-gray-900 text-white"
                >
                    <Plus className="h-4 w-4 mr-1" /> Add server
                </Button>
            </div>

            {error && (
                <div className="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-3">
                    {error}
                </div>
            )}

            {savedName && (
                <div className="text-sm text-green-700 bg-green-50 border border-green-200 rounded p-3 flex items-center gap-2">
                    <Check className="h-4 w-4" />
                    Saved &ldquo;{savedName}&rdquo;.
                </div>
            )}

            {loading ? (
                <div className="text-sm text-gray-400">Loading…</div>
            ) : servers.length === 0 ? (
                <div className="text-sm text-gray-500 bg-gray-50 rounded-lg p-6 text-center">
                    No MCP servers configured. Click <strong>Add server</strong> to connect one.
                </div>
            ) : (
                <ul className="space-y-2">
                    {servers.map((s) => (
                        <li
                            key={s.name}
                            className={`border rounded-lg p-3 flex items-center gap-3 ${
                                s.enabled ? "border-gray-200" : "border-gray-200 bg-gray-50"
                            }`}
                        >
                            <div className="shrink-0">
                                {s.transport === "stdio" ? (
                                    <Terminal className="h-5 w-5 text-gray-500" />
                                ) : (
                                    <Globe className="h-5 w-5 text-gray-500" />
                                )}
                            </div>
                            <div className="flex-1 min-w-0">
                                <div className="flex items-center gap-2">
                                    <span className="font-medium">{s.name}</span>
                                    <span className="text-xs px-1.5 py-0.5 rounded bg-gray-100 text-gray-600 uppercase">
                                        {s.transport}
                                    </span>
                                    {!s.enabled && (
                                        <span className="text-xs px-1.5 py-0.5 rounded bg-amber-50 text-amber-700">
                                            disabled
                                        </span>
                                    )}
                                </div>
                                <div className="text-xs text-gray-500 truncate">
                                    {s.transport === "stdio"
                                        ? `${s.command} ${s.args.join(" ")}`
                                        : s.url}
                                </div>
                            </div>
                            <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => handleToggle(s)}
                                title={s.enabled ? "Disable" : "Enable"}
                            >
                                {s.enabled ? "Disable" : "Enable"}
                            </Button>
                            <Button variant="ghost" size="sm" onClick={() => handleEdit(s)}>
                                Edit
                            </Button>
                            <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => handleDelete(s.name)}
                                className="text-red-600 hover:text-red-700"
                                title="Delete"
                            >
                                <Trash2 className="h-4 w-4" />
                            </Button>
                        </li>
                    ))}
                </ul>
            )}

            <div className="text-xs text-gray-400 pt-4 border-t">
                <p className="font-medium text-gray-500 mb-1">Schema reference</p>
                <pre className="bg-gray-50 rounded p-2 overflow-x-auto">{`{
  "name": "filesystem",
  "transport": "stdio",        // or "http" | "sse"
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-filesystem", "/path"],
  "env": { "FOO": "bar" }
}
{
  "name": "remote-api",
  "transport": "http",
  "url": "https://example.com/mcp",
  "headers": { "X-Custom": "value" },
  "api_key": "sk-..."           // becomes Authorization: Bearer
}`}</pre>
            </div>
        </div>
    );
}

interface PromptArg {
    name: string;
    description?: string;
    required?: boolean;
}
interface McpPrompt {
    name: string;
    description?: string;
    arguments?: PromptArg[];
}
interface McpResource {
    uri: string;
    name?: string;
    description?: string;
    mimeType?: string;
}
interface ProbeResult {
    ok: boolean;
    transport_detected?: Transport;
    suggested_url?: string | null;
    server_info?: { name?: string; version?: string };
    instructions?: string | null;
    tools?: { name: string; description?: string }[];
    tool_count?: number;
    prompts?: McpPrompt[];
    prompt_count?: number;
    resources?: McpResource[];
    resource_count?: number;
    hint?: string;
}

interface EditorProps {
    value: McpServer;
    isNew: boolean;
    onChange: (s: McpServer) => void;
    onCancel: () => void;
    onSave: () => void;
    error: string | null;
}

function McpEditor({ value, isNew, onChange, onCancel, onSave, error }: EditorProps) {
    const [revealKey, setRevealKey] = useState(false);
    const isStdio = value.transport === "stdio";
    const [probing, setProbing] = useState(false);
    const [probeResult, setProbeResult] = useState<ProbeResult | null>(null);
    const [probeError, setProbeError] = useState<string | null>(null);

    const handleProbe = async () => {
        setProbing(true);
        setProbeResult(null);
        setProbeError(null);
        try {
            const res = await fetch(`${API_BASE}/user/mcp-servers/probe`, {
                method: "POST",
                headers: {
                    "Content-Type": "application/json",
                    Authorization: `Bearer ${getToken()}`,
                },
                body: JSON.stringify({
                    url: value.url ?? "",
                    api_key: value.api_key || null,
                    headers: value.headers,
                }),
            });
            const text = await res.text();
            const data: ProbeResult & { detail?: string } = text
                ? JSON.parse(text)
                : ({} as ProbeResult);
            if (!res.ok) {
                throw new Error(data.detail || `HTTP ${res.status}`);
            }
            setProbeResult(data);
            // Auto-apply detected transport AND auto-correct URL when the
            // backend discovered the JSON-RPC handler on a sub-path.
            const patch: Partial<McpServer> = {};
            if (data.transport_detected && data.transport_detected !== value.transport) {
                patch.transport = data.transport_detected;
            }
            if (data.suggested_url && data.suggested_url !== value.url) {
                patch.url = data.suggested_url;
            }
            if (Object.keys(patch).length > 0) {
                onChange({ ...value, ...patch });
            }
        } catch (e: any) {
            setProbeError(e.message);
        } finally {
            setProbing(false);
        }
    };

    const set = (patch: Partial<McpServer>) => onChange({ ...value, ...patch });

    return (
        <div className="space-y-6 max-w-2xl">
            <div className="flex items-center gap-2">
                <Server className="h-5 w-5 text-gray-500" />
                <h2 className="text-2xl font-medium font-serif">
                    {isNew ? "Add MCP Server" : `Edit "${value.name}"`}
                </h2>
            </div>

            {error && (
                <div className="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-3 flex items-center gap-2">
                    <AlertCircle className="h-4 w-4 shrink-0" />
                    {error}
                </div>
            )}

            <section>
                <label className="text-sm text-gray-600 block mb-1">Name</label>
                <Input
                    value={value.name}
                    onChange={(e) => set({ name: e.target.value })}
                    placeholder="e.g. filesystem, jira, opencaselaw"
                />
            </section>

            <section>
                <label className="text-sm text-gray-600 block mb-2">Transport</label>
                <div className="grid grid-cols-3 gap-2">
                    {(["http", "sse", "stdio"] as Transport[]).map((t) => (
                        <button
                            key={t}
                            onClick={() => set({ transport: t })}
                            className={`text-left px-3 py-2 rounded-lg border text-sm transition-colors ${
                                value.transport === t
                                    ? "border-black bg-black text-white"
                                    : "border-gray-200 hover:border-gray-400 text-gray-700"
                            }`}
                        >
                            <div className="font-medium uppercase text-xs">{t}</div>
                            <div className="text-xs opacity-70 mt-0.5">
                                {t === "http" && "Streamable HTTP"}
                                {t === "sse" && "Server-Sent Events"}
                                {t === "stdio" && "Local subprocess"}
                            </div>
                        </button>
                    ))}
                </div>
            </section>

            {isStdio ? (
                <>
                    <section>
                        <label className="text-sm text-gray-600 block mb-1">Command</label>
                        <Input
                            value={value.command ?? ""}
                            onChange={(e) => set({ command: e.target.value })}
                            placeholder="npx"
                        />
                    </section>
                    <section>
                        <div className="flex items-center justify-between mb-1">
                            <label className="text-sm text-gray-600">Args (one per line)</label>
                            <span className="text-xs text-gray-400">order matters</span>
                        </div>
                        <textarea
                            className="w-full border border-input rounded-md px-3 py-2 text-sm font-mono min-h-[100px]"
                            value={value.args.join("\n")}
                            onChange={(e) =>
                                set({
                                    args: e.target.value.split("\n").map((s) => s.trim()).filter((s) => s),
                                })
                            }
                            placeholder={"-y\n@modelcontextprotocol/server-filesystem\n/path/to/dir"}
                        />
                    </section>
                    <section>
                        <KvEditor
                            label="Environment variables"
                            value={value.env}
                            onChange={(env) => set({ env })}
                            keyPlaceholder="FOO"
                            valuePlaceholder="bar"
                            isSecret
                        />
                    </section>
                    <div className="text-xs text-amber-700 bg-amber-50 border border-amber-200 rounded p-3">
                        ⚠️ <strong>stdio</strong> support requires the backend to spawn a child process; it is configured here but the runtime may not yet launch it. Remote (<strong>http/sse</strong>) servers are fully wired today.
                    </div>
                </>
            ) : (
                <>
                    <section>
                        <label className="text-sm text-gray-600 block mb-1">URL</label>
                        <div className="flex gap-2">
                            <Input
                                value={value.url ?? ""}
                                onChange={(e) => set({ url: e.target.value })}
                                placeholder="https://example.com/mcp"
                                className="flex-1"
                            />
                            <Button
                                type="button"
                                variant="outline"
                                onClick={handleProbe}
                                disabled={probing || !(value.url ?? "").trim()}
                                title="Try MCP initialize + tools/list, auto-detect transport"
                            >
                                {probing ? "Testing…" : "Test & detect"}
                            </Button>
                        </div>
                        {probeError && (
                            <div className="text-sm text-red-700 bg-red-50 border border-red-200 rounded p-2 mt-2 flex items-start gap-2">
                                <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                                <div>
                                    <div>{probeError}</div>
                                    {(probeError.includes("401") ||
                                        probeError.includes("403") ||
                                        probeError.toLowerCase().includes("authentication")) && (
                                        <div className="text-xs mt-1 opacity-80">
                                            Add an API Key below or set custom headers, then retry.
                                        </div>
                                    )}
                                </div>
                            </div>
                        )}
                        {probeResult && (
                            <div className="text-sm bg-emerald-50 border border-emerald-200 rounded p-3 mt-2">
                                <div className="flex items-center gap-2 mb-1">
                                    <Check className="h-4 w-4 text-emerald-700" />
                                    <span className="font-medium text-emerald-800">
                                        {probeResult.ok
                                            ? `Connected — transport detected: ${probeResult.transport_detected?.toUpperCase()}`
                                            : "Reachable but needs configuration"}
                                    </span>
                                </div>
                                {probeResult.suggested_url && (
                                    <div className="text-xs text-emerald-800 mt-1 bg-emerald-100 rounded px-2 py-1">
                                        Auto-corrected URL to <code>{probeResult.suggested_url}</code> (the JSON-RPC handler was on a sub-path).
                                    </div>
                                )}
                                {probeResult.server_info?.name && (
                                    <div className="text-xs text-emerald-700">
                                        Server: <code>{probeResult.server_info.name}</code>
                                        {probeResult.server_info.version && (
                                            <> v{probeResult.server_info.version}</>
                                        )}
                                    </div>
                                )}
                                {probeResult.hint && (
                                    <div className="text-xs text-amber-700 mt-1">{probeResult.hint}</div>
                                )}
                                <div className="text-xs text-emerald-700 mt-1 flex flex-wrap gap-3">
                                    {typeof probeResult.tool_count === "number" && (
                                        <span>Tools: <strong>{probeResult.tool_count}</strong></span>
                                    )}
                                    {typeof probeResult.prompt_count === "number" && (
                                        <span>Prompts (skills): <strong>{probeResult.prompt_count}</strong></span>
                                    )}
                                    {typeof probeResult.resource_count === "number" && (
                                        <span>Resources: <strong>{probeResult.resource_count}</strong></span>
                                    )}
                                </div>

                                {probeResult.instructions && (
                                    <details className="mt-2" open>
                                        <summary className="text-xs text-emerald-800 cursor-pointer font-medium">
                                            Server instructions (Markdown)
                                        </summary>
                                        <pre className="text-xs mt-1 bg-white border border-emerald-200 rounded p-2 overflow-x-auto whitespace-pre-wrap text-emerald-900">
                                            {probeResult.instructions}
                                        </pre>
                                    </details>
                                )}

                                {probeResult.tools && probeResult.tools.length > 0 && (
                                    <details className="mt-2">
                                        <summary className="text-xs text-emerald-800 cursor-pointer">
                                            Tools ({probeResult.tools.length})
                                        </summary>
                                        <ul className="text-xs mt-1 ml-4 list-disc text-emerald-900 space-y-0.5">
                                            {probeResult.tools.slice(0, 30).map((t) => (
                                                <li key={t.name}>
                                                    <code>{t.name}</code>
                                                    {t.description && (
                                                        <span className="text-emerald-700/80"> — {t.description}</span>
                                                    )}
                                                </li>
                                            ))}
                                            {probeResult.tools.length > 30 && (
                                                <li className="opacity-60">
                                                    …and {probeResult.tools.length - 30} more
                                                </li>
                                            )}
                                        </ul>
                                    </details>
                                )}

                                {probeResult.prompts && probeResult.prompts.length > 0 && (
                                    <details className="mt-2">
                                        <summary className="text-xs text-emerald-800 cursor-pointer">
                                            Prompts / skills ({probeResult.prompts.length})
                                        </summary>
                                        <ul className="text-xs mt-1 ml-4 list-disc text-emerald-900 space-y-1">
                                            {probeResult.prompts.slice(0, 30).map((p) => (
                                                <li key={p.name}>
                                                    <code>{p.name}</code>
                                                    {p.description && (
                                                        <span className="text-emerald-700/80"> — {p.description}</span>
                                                    )}
                                                    {p.arguments && p.arguments.length > 0 && (
                                                        <ul className="ml-4 list-square text-emerald-700/80">
                                                            {p.arguments.map((a) => (
                                                                <li key={a.name}>
                                                                    <code>{a.name}</code>
                                                                    {a.required ? " *" : ""}
                                                                    {a.description ? ` — ${a.description}` : ""}
                                                                </li>
                                                            ))}
                                                        </ul>
                                                    )}
                                                </li>
                                            ))}
                                        </ul>
                                    </details>
                                )}

                                {probeResult.resources && probeResult.resources.length > 0 && (
                                    <details className="mt-2">
                                        <summary className="text-xs text-emerald-800 cursor-pointer">
                                            Resources ({probeResult.resources.length})
                                        </summary>
                                        <ul className="text-xs mt-1 ml-4 list-disc text-emerald-900 space-y-0.5">
                                            {probeResult.resources.slice(0, 30).map((r) => (
                                                <li key={r.uri}>
                                                    <code>{r.name || r.uri}</code>
                                                    {r.mimeType && (
                                                        <span className="text-emerald-700/80"> [{r.mimeType}]</span>
                                                    )}
                                                    {r.description && (
                                                        <span className="text-emerald-700/80"> — {r.description}</span>
                                                    )}
                                                </li>
                                            ))}
                                        </ul>
                                    </details>
                                )}
                            </div>
                        )}
                    </section>
                    <section>
                        <label className="text-sm text-gray-600 block mb-1">
                            API Key (optional — sent as <code>Authorization: Bearer &lt;key&gt;</code>)
                        </label>
                        <div className="relative">
                            <Input
                                type={revealKey ? "text" : "password"}
                                value={value.api_key ?? ""}
                                onChange={(e) => set({ api_key: e.target.value })}
                                placeholder="sk-..."
                                className="pr-10"
                            />
                            <button
                                type="button"
                                onClick={() => setRevealKey((r) => !r)}
                                className="absolute inset-y-0 right-2 flex items-center text-gray-400 hover:text-gray-600"
                                aria-label={revealKey ? "Hide" : "Show"}
                            >
                                {revealKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                            </button>
                        </div>
                    </section>
                    <section>
                        <KvEditor
                            label="Custom headers"
                            value={value.headers}
                            onChange={(headers) => set({ headers })}
                            keyPlaceholder="X-Custom-Header"
                            valuePlaceholder="value"
                        />
                    </section>
                </>
            )}

            <section className="flex items-center gap-2">
                <input
                    id="enabled"
                    type="checkbox"
                    checked={value.enabled}
                    onChange={(e) => set({ enabled: e.target.checked })}
                />
                <label htmlFor="enabled" className="text-sm text-gray-700">
                    Enabled (tools are surfaced to the assistant)
                </label>
            </section>

            <div className="flex gap-2 pt-2">
                <Button onClick={onSave} className="bg-black hover:bg-gray-900 text-white">
                    {isNew ? "Add server" : "Save changes"}
                </Button>
                <Button variant="outline" onClick={onCancel}>
                    Cancel
                </Button>
            </div>
        </div>
    );
}

interface KvProps {
    label: string;
    value: Record<string, string>;
    onChange: (v: Record<string, string>) => void;
    keyPlaceholder?: string;
    valuePlaceholder?: string;
    isSecret?: boolean;
}

function KvEditor({ label, value, onChange, keyPlaceholder, valuePlaceholder, isSecret }: KvProps) {
    const entries = Object.entries(value);

    const setKey = (oldKey: string, newKey: string) => {
        const next: Record<string, string> = {};
        for (const [k, v] of Object.entries(value)) {
            next[k === oldKey ? newKey : k] = v;
        }
        onChange(next);
    };
    const setVal = (k: string, v: string) => onChange({ ...value, [k]: v });
    const remove = (k: string) => {
        const next = { ...value };
        delete next[k];
        onChange(next);
    };
    const add = () => onChange({ ...value, "": "" });

    return (
        <div>
            <div className="flex items-center justify-between mb-2">
                <label className="text-sm text-gray-600">{label}</label>
                <Button variant="outline" size="sm" onClick={add}>
                    <Plus className="h-3.5 w-3.5 mr-1" /> Add
                </Button>
            </div>
            {entries.length === 0 && (
                <div className="text-xs text-gray-400">None</div>
            )}
            <div className="space-y-2">
                {entries.map(([k, v], idx) => (
                    <div key={idx} className="flex gap-2">
                        <Input
                            value={k}
                            onChange={(e) => setKey(k, e.target.value)}
                            placeholder={keyPlaceholder ?? "key"}
                            className="flex-1"
                        />
                        <Input
                            type={isSecret ? "password" : "text"}
                            value={v}
                            onChange={(e) => setVal(k, e.target.value)}
                            placeholder={valuePlaceholder ?? "value"}
                            className="flex-1"
                        />
                        <Button
                            variant="ghost"
                            size="sm"
                            onClick={() => remove(k)}
                            className="text-red-600 hover:text-red-700"
                        >
                            <Trash2 className="h-4 w-4" />
                        </Button>
                    </div>
                ))}
            </div>
        </div>
    );
}
