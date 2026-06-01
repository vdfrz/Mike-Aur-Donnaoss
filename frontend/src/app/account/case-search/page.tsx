"use client";

import { useEffect, useState } from "react";
import { HardDrive, Trash2, Loader2, ExternalLink, FileText } from "lucide-react";
import PoweredByIKanoon from "@/app/components/shared/PoweredByIKanoon";

const API_BASE =
    process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

/** Open a URL in the OS browser (Tauri command), falling back to window.open. */
async function openExternalUrl(url: string) {
    try {
        const tauri = await import("@tauri-apps/api/core");
        await tauri.invoke("open_external_url", { url });
    } catch {
        window.open(url, "_blank", "noopener,noreferrer");
    }
}

function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(0)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function formatDate(raw: string): string {
    const d = new Date(raw.replace(" ", "T"));
    if (isNaN(d.getTime())) return raw;
    return d.toLocaleDateString(undefined, {
        day: "numeric",
        month: "short",
        year: "numeric",
    });
}

// Strip the ".txt" we append when caching, for a cleaner title.
function cleanTitle(filename: string): string {
    return filename.replace(/\.txt$/i, "");
}

const SIZE_OPTIONS = [
    { label: "Small (100 MB)", value: 100 * 1024 * 1024 },
    { label: "Medium (500 MB)", value: 500 * 1024 * 1024 },
    { label: "Large (2 GB)", value: 2 * 1024 * 1024 * 1024 },
    { label: "Don't save any", value: 0 },
];

const CACHE_LIMIT_KEY = "vanga_cache_limit";
const DEFAULT_LIMIT = 500 * 1024 * 1024;

interface CachedJudgment {
    id: string;
    filename: string;
    corpus_identifier: string | null;
    size_bytes: number;
    created_at: string;
    status: string;
    source_url: string | null;
}

export default function CaseSearchSettingsPage() {
    const [limit, setLimit] = useState(DEFAULT_LIMIT);
    const [docs, setDocs] = useState<CachedJudgment[]>([]);
    const [loading, setLoading] = useState(true);
    const [clearing, setClearing] = useState(false);
    const [confirmClear, setConfirmClear] = useState(false);

    const totalBytes = docs.reduce((sum, d) => sum + (d.size_bytes || 0), 0);
    const count = docs.length;

    useEffect(() => {
        const raw = localStorage.getItem(CACHE_LIMIT_KEY);
        if (raw) {
            const n = parseInt(raw, 10);
            if (!isNaN(n)) setLimit(n);
        }
        refreshDocs();
    }, []);

    async function refreshDocs() {
        setLoading(true);
        try {
            const r = await fetch(`${API_BASE}/indian-kanoon/documents`, {
                headers: { Authorization: `Bearer ${getToken()}` },
            });
            if (r.ok) {
                const data = await r.json();
                setDocs(data.documents ?? []);
            }
        } catch {
            // backend unreachable — leave list empty
        } finally {
            setLoading(false);
        }
    }

    function handleLimitChange(value: number) {
        setLimit(value);
        localStorage.setItem(CACHE_LIMIT_KEY, String(value));
    }

    async function deleteOne(id: string) {
        // Optimistic removal.
        setDocs((prev) => prev.filter((d) => d.id !== id));
        try {
            await fetch(`${API_BASE}/indian-kanoon/documents/${id}`, {
                method: "DELETE",
                headers: { Authorization: `Bearer ${getToken()}` },
            });
        } catch {
            refreshDocs(); // restore truth on failure
        }
    }

    async function handleClear() {
        if (!confirmClear) {
            setConfirmClear(true);
            return;
        }
        setClearing(true);
        try {
            await Promise.all(
                docs.map((d) =>
                    fetch(`${API_BASE}/indian-kanoon/documents/${d.id}`, {
                        method: "DELETE",
                        headers: { Authorization: `Bearer ${getToken()}` },
                    }),
                ),
            );
            setDocs([]);
        } catch {
            refreshDocs();
        } finally {
            setClearing(false);
            setConfirmClear(false);
        }
    }

    return (
        <div className="space-y-8">
            <div>
                <h2 className="text-lg font-medium flex items-center gap-2">
                    <HardDrive className="h-5 w-5 text-gray-500" />
                    Case Search
                </h2>
                <p className="text-sm text-gray-500 mt-1">
                    Mike saves judgments you&apos;ve viewed so opening them again is
                    instant and works offline.
                </p>
            </div>

            <div className="border border-gray-200 rounded-lg p-4 space-y-4">
                <h3 className="text-sm font-medium text-gray-900">
                    Keep recent judgments handy
                </h3>
                <div className="space-y-2">
                    {SIZE_OPTIONS.map((opt) => (
                        <label
                            key={opt.value}
                            className="flex items-center gap-3 cursor-pointer"
                        >
                            <input
                                type="radio"
                                name="cache-limit"
                                checked={limit === opt.value}
                                onChange={() => handleLimitChange(opt.value)}
                                className="accent-gray-900"
                            />
                            <span className="text-sm text-gray-700">{opt.label}</span>
                        </label>
                    ))}
                </div>
                <p className="text-xs text-gray-500">
                    Older judgments are automatically removed when space runs low.
                </p>
            </div>

            <div className="border border-gray-200 rounded-lg p-4 space-y-3">
                <div className="flex items-center justify-between">
                    <div>
                        <h3 className="text-sm font-medium text-gray-900">
                            Saved judgments
                        </h3>
                        <p className="text-xs text-gray-500 mt-0.5">
                            Currently using {formatBytes(totalBytes)} across {count}{" "}
                            judgment{count !== 1 ? "s" : ""}
                        </p>
                    </div>
                    <button
                        onClick={handleClear}
                        disabled={clearing || count === 0}
                        className="inline-flex items-center gap-1.5 rounded-md border border-gray-200 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50 disabled:opacity-50 transition-colors"
                    >
                        {clearing ? (
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : (
                            <Trash2 className="h-3.5 w-3.5" />
                        )}
                        {confirmClear ? "Are you sure?" : "Clear saved judgments"}
                    </button>
                </div>
                {totalBytes > 0 && limit > 0 && (
                    <div className="h-1.5 bg-gray-100 rounded overflow-hidden">
                        <div
                            className="h-full bg-gray-700 transition-all"
                            style={{
                                width: `${Math.min(100, (totalBytes / limit) * 100)}%`,
                            }}
                        />
                    </div>
                )}

                {/* The actual list of cached judgments. */}
                <div className="divide-y divide-gray-100 border-t border-gray-100 pt-1">
                    {loading ? (
                        <div className="flex items-center gap-2 py-6 text-sm text-gray-500">
                            <Loader2 className="h-4 w-4 animate-spin" /> Loading saved
                            judgments…
                        </div>
                    ) : count === 0 ? (
                        <p className="py-6 text-sm text-gray-500">
                            No saved judgments yet. They appear here automatically once
                            Mike pulls a case in the assistant.
                        </p>
                    ) : (
                        docs.map((d) => (
                            <div
                                key={d.id}
                                className="flex items-center justify-between gap-3 py-2.5"
                            >
                                <div className="flex items-start gap-2 min-w-0">
                                    <FileText className="h-4 w-4 text-gray-400 mt-0.5 shrink-0" />
                                    <div className="min-w-0">
                                        <p className="text-sm text-gray-800 truncate">
                                            {cleanTitle(d.filename)}
                                        </p>
                                        <p className="text-xs text-gray-400">
                                            {formatDate(d.created_at)} ·{" "}
                                            {formatBytes(d.size_bytes)}
                                            {d.status !== "ready"
                                                ? ` · ${d.status}`
                                                : ""}
                                        </p>
                                    </div>
                                </div>
                                <div className="flex items-center gap-1 shrink-0">
                                    {d.source_url && (
                                        <button
                                            onClick={() => openExternalUrl(d.source_url!)}
                                            title="Open on Indian Kanoon"
                                            className="p-1.5 rounded-md text-gray-500 hover:bg-gray-50 hover:text-blue-600"
                                        >
                                            <ExternalLink className="h-3.5 w-3.5" />
                                        </button>
                                    )}
                                    <button
                                        onClick={() => deleteOne(d.id)}
                                        title="Remove this saved judgment"
                                        className="p-1.5 rounded-md text-gray-500 hover:bg-gray-50 hover:text-red-600"
                                    >
                                        <Trash2 className="h-3.5 w-3.5" />
                                    </button>
                                </div>
                            </div>
                        ))
                    )}
                </div>

                {count > 0 && (
                    <div className="pt-2 flex justify-end">
                        <PoweredByIKanoon />
                    </div>
                )}
            </div>
        </div>
    );
}
