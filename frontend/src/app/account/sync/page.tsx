"use client";

import { useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import { Folder, Plus, RefreshCw, Trash2, ChevronDown, ChevronRight, AlertCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { listProjects } from "@/app/lib/mikeApi";
import type { MikeProject } from "@/app/components/shared/types";
import ReindexProgress from "@/app/components/shared/ReindexProgress";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

interface SyncFolder {
    id: string;
    path: string;
    label: string | null;
    recursive: boolean;
    enabled: boolean;
    last_scan_at: string | null;
    project_id: string | null;
}

interface ScanStatusOut {
    status: "idle" | "running" | "done" | "failed";
    total: number;
    processed: number;
    indexed: number;
    skipped: number;
    failed: number;
    current_file?: string | null;
    /**
     * Coarse pipeline stage on the file currently being processed, set
     * by the scanner before each phase: `starting`, `extracting`,
     * `embedding`. Surfaced as a sub-line under the file name so the
     * user can tell silence-during-extract from silence-during-embed
     * (the latter often means "downloading e5-base for the first time").
     */
    current_step?: string | null;
    last_error?: string | null;
}

/**
 * Snapshot returned by GET /sync/model-status. Drives the
 * "Scaricamento modello…" progress bar above the folder list while
 * the embedding service is fetching its e5-base weights for the
 * first time.
 */
type ModelStatusOut =
    | { state: "idle" | "loading" | "ready" | "unavailable" }
    | {
          state: "downloading";
          downloaded: number;
          total: number | null;
          file: string;
      }
    | { state: "failed"; error: string };

interface SyncedFile {
    path: string;
    status: "ready" | "skipped" | "failed";
    document_id: string;
    skip_reason: string | null;
    size_bytes: number;
    chunk_count: number;
    indexed_at: string;
    mtime: string;
}

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
    const res = await fetch(`${API_BASE}${path}`, {
        ...init,
        headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${getToken()}`,
            ...(init.headers ?? {}),
        },
    });
    if (!res.ok) {
        const text = await res.text().catch(() => "");
        throw new Error(`HTTP ${res.status}: ${text || res.statusText}`);
    }
    if (res.status === 204) return undefined as T;
    return (await res.json()) as T;
}

/**
 * Above-the-folders banner that surfaces what the embedding model is
 * doing. Hidden when the model is `idle` or `ready` (steady states);
 * renders a percentage progress bar during `downloading` / `loading`,
 * and a red error block on `failed`.
 *
 * Note this is the **same** model whether the user has 1 folder or
 * 50 — there's only one EmbeddingService process-wide. So we render it
 * at the top of the page rather than per-folder.
 */
function ModelStatusBar({
    status,
    t,
}: {
    status: ModelStatusOut;
    t: ReturnType<typeof useTranslations>;
}) {
    if (status.state === "idle" || status.state === "ready" || status.state === "unavailable") {
        return null;
    }
    if (status.state === "failed") {
        return (
            <div className="text-sm text-red-600 bg-red-50 border border-red-200 rounded-md px-3 py-2 flex items-start gap-2">
                <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                <span>{t("modelFailed", { error: status.error })}</span>
            </div>
        );
    }

    const isDownload = status.state === "downloading";
    const pct =
        isDownload && status.total
            ? Math.min(100, Math.round((status.downloaded / status.total) * 100))
            : null;
    const mb = (n: number) => (n / 1_000_000).toFixed(1);

    return (
        <div className="border border-amber-200 bg-amber-50/60 rounded-md px-4 py-3 space-y-2">
            <div className="flex items-center justify-between gap-3 text-sm">
                <span className="font-medium text-amber-900">
                    {isDownload
                        ? t("modelDownloading", { file: status.file })
                        : t("modelLoading")}
                </span>
                {isDownload && status.total && (
                    <span className="tabular-nums text-amber-800 text-xs">
                        {mb(status.downloaded)} / {mb(status.total)} MB · {pct}%
                    </span>
                )}
                {isDownload && !status.total && (
                    <span className="tabular-nums text-amber-800 text-xs">
                        {mb(status.downloaded)} MB
                    </span>
                )}
            </div>
            <div className="h-1.5 bg-amber-100 rounded overflow-hidden">
                <div
                    className={`h-full bg-amber-500 transition-all ${
                        // While we don't know the total size or we're in
                        // the indeterminate "loading" phase, animate a
                        // ribbon instead of a fill so the user sees motion.
                        pct === null ? "animate-pulse w-1/3" : ""
                    }`}
                    style={
                        pct !== null
                            ? { width: `${pct}%` }
                            : undefined
                    }
                />
            </div>
            <p className="text-[11px] text-amber-700/80">{t("modelHint")}</p>
        </div>
    );
}

export default function SyncPage() {
    const t = useTranslations("Sync");
    const tCommon = useTranslations("Common");
    const [folders, setFolders] = useState<SyncFolder[]>([]);
    const [projects, setProjects] = useState<MikeProject[]>([]);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);

    // Add-folder form
    const [path, setPath] = useState("");
    const [label, setLabel] = useState("");
    const [recursive, setRecursive] = useState(true);
    const [scope, setScope] = useState<"global" | string>("global");
    const [adding, setAdding] = useState(false);

    // Per-folder live status, keyed by folder id.
    const [statuses, setStatuses] = useState<Record<string, ScanStatusOut>>({});
    // Epoch-ms timestamp a scan was kicked off, keyed by folder id. Drives
    // the elapsed timer / ETA in <ReindexProgress>.
    const [scanStart, setScanStart] = useState<Record<string, number>>({});
    const [expanded, setExpanded] = useState<string | null>(null);
    const [files, setFiles] = useState<Record<string, SyncedFile[]>>({});

    // Embedding-model lifecycle, polled while a scan is running so the
    // user sees the one-shot ~280 MB download progress instead of a
    // silent freeze on the first PDF.
    const [modelStatus, setModelStatus] = useState<ModelStatusOut>({
        state: "idle",
    });

    // Initial load
    useEffect(() => {
        let cancelled = false;
        Promise.all([api<SyncFolder[]>("/sync/folders"), listProjects()])
            .then(([fs, ps]) => {
                if (cancelled) return;
                setFolders(fs);
                setProjects(ps);
            })
            .catch((e) => {
                if (!cancelled) setError(String(e));
            })
            .finally(() => {
                if (!cancelled) setLoading(false);
            });
        return () => {
            cancelled = true;
        };
    }, []);

    // Poll statuses for any folder whose scan is running. We refresh
    // every 1.5s while at least one scan is in flight; otherwise idle.
    useEffect(() => {
        const ids = Object.entries(statuses)
            .filter(([, s]) => s.status === "running")
            .map(([id]) => id);
        if (ids.length === 0) return;
        const handle = setInterval(async () => {
            const updates: Record<string, ScanStatusOut> = {};
            await Promise.all(
                ids.map(async (id) => {
                    try {
                        const next = await api<ScanStatusOut>(
                            `/sync/folders/${id}/status`,
                        );
                        updates[id] = next;
                        const prev = statuses[id];
                        // Only log when something changed — keeps the
                        // console readable while the poll keeps ticking.
                        if (
                            !prev ||
                            prev.processed !== next.processed ||
                            prev.indexed !== next.indexed ||
                            prev.skipped !== next.skipped ||
                            prev.failed !== next.failed ||
                            prev.status !== next.status ||
                            prev.current_file !== next.current_file ||
                            prev.current_step !== next.current_step
                        ) {
                            const fileBasename = next.current_file
                                ? next.current_file.split(/[\\/]/).pop()
                                : null;
                            console.log(
                                `[sync ${id.slice(0, 8)}] ${next.status} ` +
                                    `${next.processed}/${next.total} ` +
                                    `(indexed=${next.indexed} skipped=${next.skipped} failed=${next.failed})` +
                                    (next.current_step
                                        ? ` · ${next.current_step}`
                                        : "") +
                                    (fileBasename ? ` · ${fileBasename}` : ""),
                            );
                        }
                    } catch {
                        // ignore — keep previous status
                    }
                }),
            );
            setStatuses((prev) => ({ ...prev, ...updates }));
        }, 1500);
        return () => clearInterval(handle);
    }, [statuses]);

    // Poll the embedding-model lifecycle while a scan is running OR
    // while the model is mid-download/load. Faster cadence (700ms)
    // than the per-folder status because the download throughput
    // updates byte-by-byte and the user wants to see the bar move.
    useEffect(() => {
        const anyRunning = Object.values(statuses).some(
            (s) => s.status === "running",
        );
        const inProgress =
            modelStatus.state === "downloading" ||
            modelStatus.state === "loading";
        if (!anyRunning && !inProgress) return;
        const handle = setInterval(async () => {
            try {
                const next = await api<ModelStatusOut>("/sync/model-status");
                setModelStatus((prev) => {
                    // Only log on state transitions or every ~5%
                    // download progress to keep the console readable.
                    if (next.state !== prev.state) {
                        console.log(`[rag] model state: ${prev.state} → ${next.state}`);
                    } else if (
                        next.state === "downloading" &&
                        prev.state === "downloading" &&
                        next.total &&
                        prev.total
                    ) {
                        const pNext = Math.floor(
                            (next.downloaded * 20) / next.total,
                        );
                        const pPrev = Math.floor(
                            (prev.downloaded * 20) / prev.total,
                        );
                        if (pNext !== pPrev) {
                            console.log(
                                `[rag] download ${next.file}: ${(
                                    (next.downloaded * 100) /
                                    next.total
                                ).toFixed(0)}% (${(next.downloaded / 1e6).toFixed(1)}/${(next.total / 1e6).toFixed(1)} MB)`,
                            );
                        }
                    }
                    return next;
                });
            } catch {
                // ignore — keep prior state
            }
        }, 700);
        return () => clearInterval(handle);
    }, [statuses, modelStatus.state]);

    const refreshFolders = async () => {
        const fs = await api<SyncFolder[]>("/sync/folders");
        setFolders(fs);
    };

    const handleAdd = async () => {
        const p = path.trim();
        if (!p) return;
        setAdding(true);
        setError(null);
        try {
            await api<{ id: string }>("/sync/folders", {
                method: "POST",
                body: JSON.stringify({
                    path: p,
                    label: label.trim() || null,
                    recursive,
                    project_id: scope === "global" ? null : scope,
                }),
            });
            setPath("");
            setLabel("");
            setRecursive(true);
            setScope("global");
            await refreshFolders();
        } catch (e) {
            setError(String(e));
        } finally {
            setAdding(false);
        }
    };

    const handleScan = async (id: string) => {
        const folder = folders.find((f) => f.id === id);
        console.log(
            `[sync] starting scan id=${id.slice(0, 8)} path=${folder?.path ?? "?"} ` +
                `scope=${folder?.project_id ? "project:" + folder.project_id.slice(0, 8) : "global"}`,
        );
        try {
            await api(`/sync/folders/${id}/scan`, { method: "POST" });
            // Optimistic — show "running" immediately so the bar appears.
            setScanStart((m) => ({ ...m, [id]: Date.now() }));
            setStatuses((s) => ({
                ...s,
                [id]: {
                    status: "running",
                    total: 0,
                    processed: 0,
                    indexed: 0,
                    skipped: 0,
                    failed: 0,
                },
            }));
        } catch (e) {
            console.error(`[sync] scan id=${id.slice(0, 8)} failed:`, e);
            setError(String(e));
        }
    };

    // Re-index every configured folder at once. Skips folders already
    // running so a double-click doesn't restart in-flight scans.
    const handleScanAll = async () => {
        const targets = folders.filter(
            (f) => statuses[f.id]?.status !== "running",
        );
        for (const f of targets) {
            await handleScan(f.id);
        }
    };

    const handleRemove = async (id: string) => {
        if (!confirm(t("removeConfirm"))) return;
        try {
            await api(`/sync/folders/${id}`, { method: "DELETE" });
            setFolders((fs) => fs.filter((f) => f.id !== id));
            setStatuses((s) => {
                const c = { ...s };
                delete c[id];
                return c;
            });
        } catch (e) {
            setError(String(e));
        }
    };

    const toggleExpand = async (id: string) => {
        if (expanded === id) {
            setExpanded(null);
            return;
        }
        setExpanded(id);
        if (!files[id]) {
            try {
                const fs = await api<SyncedFile[]>(`/sync/folders/${id}/files`);
                setFiles((prev) => ({ ...prev, [id]: fs }));
            } catch (e) {
                setError(String(e));
            }
        }
    };

    const projectName = (pid: string | null) =>
        pid ? projects.find((p) => p.id === pid)?.name ?? pid : null;

    const anyRunning = Object.values(statuses).some(
        (s) => s.status === "running",
    );

    if (loading) {
        return <div className="text-sm text-gray-400">{tCommon("loading")}</div>;
    }

    return (
        <div className="space-y-6 max-w-4xl">
            <div className="flex items-start justify-between gap-4">
                <div>
                    <h2 className="text-2xl font-medium font-serif mb-2">{t("title")}</h2>
                    <p className="text-sm text-gray-500 leading-relaxed">{t("subtitle")}</p>
                </div>
                {folders.length > 0 && (
                    <button
                        type="button"
                        onClick={handleScanAll}
                        disabled={anyRunning}
                        className="shrink-0 inline-flex items-center gap-1.5 rounded-md border border-gray-200 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50 disabled:opacity-50 transition-colors"
                    >
                        <RefreshCw
                            className={`h-3.5 w-3.5 ${anyRunning ? "animate-spin" : ""}`}
                        />
                        {anyRunning ? t("reindexingAll") : t("reindexAll")}
                    </button>
                )}
            </div>

            {error && (
                <div className="text-sm text-red-600 bg-red-50 border border-red-200 rounded-md px-3 py-2 flex items-start gap-2">
                    <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                    <span>{error}</span>
                </div>
            )}

            <ModelStatusBar status={modelStatus} t={t} />


            {/* Add folder */}
            <section className="border border-gray-200 rounded-lg p-4 space-y-3">
                <h3 className="text-sm font-medium">{t("addFolder")}</h3>
                <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                    <div className="md:col-span-2">
                        <label className="text-xs text-gray-500 block mb-1">
                            {t("folderPath")}
                        </label>
                        <Input
                            value={path}
                            onChange={(e) => setPath(e.target.value)}
                            placeholder={t("folderPathPlaceholder")}
                        />
                    </div>
                    <div>
                        <label className="text-xs text-gray-500 block mb-1">
                            {t("label")}
                        </label>
                        <Input
                            value={label}
                            onChange={(e) => setLabel(e.target.value)}
                            placeholder={t("labelPlaceholder")}
                        />
                    </div>
                    <div>
                        <label className="text-xs text-gray-500 block mb-1">
                            Scope
                        </label>
                        <select
                            value={scope}
                            onChange={(e) => setScope(e.target.value)}
                            className="w-full rounded-md border border-gray-200 bg-white px-3 py-2 text-sm hover:border-gray-400 focus:outline-none transition-colors"
                        >
                            <option value="global">Globale</option>
                            {projects.map((p) => (
                                <option key={p.id} value={p.id}>
                                    {p.name}
                                </option>
                            ))}
                        </select>
                    </div>
                </div>
                <div className="flex items-center justify-between">
                    <label className="flex items-center gap-2 text-sm text-gray-600">
                        <input
                            type="checkbox"
                            checked={recursive}
                            onChange={(e) => setRecursive(e.target.checked)}
                        />
                        {t("recursive")}
                    </label>
                    <Button
                        onClick={handleAdd}
                        disabled={adding || !path.trim()}
                        className="bg-black text-white hover:bg-gray-900"
                    >
                        <Plus className="h-3.5 w-3.5 mr-1" />
                        {adding ? tCommon("creating") : t("addFolder")}
                    </Button>
                </div>
            </section>

            {/* Folders list */}
            {folders.length === 0 ? (
                <p className="text-sm text-gray-400">{t("noFolders")}</p>
            ) : (
                <div className="space-y-3">
                    {folders.map((f) => {
                        const status = statuses[f.id];
                        const isRunning = status?.status === "running";
                        const isExpanded = expanded === f.id;
                        const fileList = files[f.id] ?? [];
                        return (
                            <div
                                key={f.id}
                                className="border border-gray-200 rounded-lg p-4"
                            >
                                <div className="flex items-start justify-between gap-3">
                                    <div className="flex items-start gap-3 min-w-0 flex-1">
                                        <Folder className="h-4 w-4 mt-1 text-gray-400 shrink-0" />
                                        <div className="min-w-0 flex-1">
                                            <div className="text-sm font-medium text-gray-900 truncate">
                                                {f.label ?? f.path}
                                            </div>
                                            <div className="text-xs text-gray-500 truncate">
                                                {f.path}
                                            </div>
                                            <div className="text-xs text-gray-500 mt-1 flex items-center gap-3">
                                                <span className="inline-flex items-center gap-1">
                                                    <span className={`inline-block h-1.5 w-1.5 rounded-full ${f.project_id ? "bg-blue-500" : "bg-green-500"}`} />
                                                    {f.project_id
                                                        ? `Progetto: ${projectName(f.project_id)}`
                                                        : "Globale"}
                                                </span>
                                                <span>
                                                    {t("lastScan")}:{" "}
                                                    {f.last_scan_at
                                                        ? new Date(f.last_scan_at).toLocaleString()
                                                        : t("never")}
                                                </span>
                                            </div>
                                        </div>
                                    </div>
                                    <div className="flex items-center gap-1 shrink-0">
                                        <button
                                            type="button"
                                            onClick={() => handleScan(f.id)}
                                            disabled={isRunning}
                                            className="rounded-md px-2.5 py-1.5 text-xs text-gray-700 hover:bg-gray-100 disabled:opacity-40 transition-colors flex items-center gap-1"
                                        >
                                            <RefreshCw
                                                className={`h-3.5 w-3.5 ${isRunning ? "animate-spin" : ""}`}
                                            />
                                            {isRunning ? t("reindexing") : t("reindex")}
                                        </button>
                                        <button
                                            type="button"
                                            onClick={() => handleRemove(f.id)}
                                            className="rounded-md p-1.5 text-gray-400 hover:text-red-600 hover:bg-red-50 transition-colors"
                                            aria-label={t("remove")}
                                        >
                                            <Trash2 className="h-3.5 w-3.5" />
                                        </button>
                                    </div>
                                </div>

                                {status && status.status !== "idle" && (
                                    <ReindexProgress
                                        indexed={status.indexed}
                                        total={status.total}
                                        status={status.status}
                                        startedAt={scanStart[f.id] ?? null}
                                        currentFile={status.current_file}
                                        currentStep={status.current_step}
                                        skipped={status.skipped}
                                        errorText={status.last_error}
                                        labels={{
                                            embedded: t("embedded"),
                                            remaining: t("remaining"),
                                            unit: t("documentsUnit"),
                                            failedLabel: t("scanFailed"),
                                        }}
                                    />
                                )}

                                <button
                                    type="button"
                                    onClick={() => toggleExpand(f.id)}
                                    className="mt-3 text-xs text-gray-500 hover:text-gray-800 flex items-center gap-1"
                                >
                                    {isExpanded ? (
                                        <ChevronDown className="h-3 w-3" />
                                    ) : (
                                        <ChevronRight className="h-3 w-3" />
                                    )}
                                    {isExpanded ? t("hideFiles") : t("showFiles")}
                                </button>

                                {isExpanded && (
                                    <div className="mt-2 border-t border-gray-100 pt-2 max-h-72 overflow-y-auto">
                                        {fileList.length === 0 ? (
                                            <p className="text-xs text-gray-400 py-2">
                                                {tCommon("loading")}
                                            </p>
                                        ) : (
                                            <div className="space-y-1 text-xs">
                                                {fileList.map((file) => (
                                                    <div
                                                        key={file.path}
                                                        className="flex items-start justify-between gap-2 py-1"
                                                    >
                                                        <div className="min-w-0 flex-1">
                                                            <div className="text-gray-700 truncate">
                                                                {file.path}
                                                            </div>
                                                            {file.skip_reason && (
                                                                <div className="text-gray-400 text-[10px]">
                                                                    {t("skipReason")}: {file.skip_reason}
                                                                </div>
                                                            )}
                                                        </div>
                                                        <div className="shrink-0 text-right">
                                                            <span
                                                                className={`inline-block px-1.5 py-0.5 rounded text-[10px] ${
                                                                    file.status === "ready"
                                                                        ? "bg-green-50 text-green-700 border border-green-200"
                                                                        : file.status === "skipped"
                                                                          ? "bg-gray-100 text-gray-600"
                                                                          : "bg-red-50 text-red-700 border border-red-200"
                                                                }`}
                                                            >
                                                                {t(`fileStatus.${file.status as "ready" | "skipped" | "failed"}`)}
                                                            </span>
                                                            {file.status === "ready" && (
                                                                <div className="text-gray-400 text-[10px] mt-0.5">
                                                                    {file.chunk_count} {t("chunks")}
                                                                </div>
                                                            )}
                                                        </div>
                                                    </div>
                                                ))}
                                            </div>
                                        )}
                                    </div>
                                )}
                            </div>
                        );
                    })}
                </div>
            )}
        </div>
    );
}
