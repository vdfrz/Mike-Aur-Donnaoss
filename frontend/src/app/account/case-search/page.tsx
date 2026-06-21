"use client";

import { useEffect, useRef, useState } from "react";
import { HardDrive, Trash2, Loader2, ExternalLink, FileText, RefreshCw, Check, Eye, EyeOff, ShieldCheck } from "lucide-react";
import { useTranslations } from "next-intl";
import PoweredByIKanoon from "@/app/components/shared/PoweredByIKanoon";
import ReindexProgress from "@/app/components/shared/ReindexProgress";
import { getIndianKanoonConfig, putIndianKanoonConfig, type IndianKanoonConfig } from "@/app/lib/mikeApi";

interface ReindexStatusOut {
    status: "idle" | "running" | "done" | "failed";
    total: number;
    processed: number;
    indexed: number;
    current_file?: string | null;
    current_step?: string | null;
    last_error?: string | null;
}

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
    const t = useTranslations("IndianKanoon");
    const tCommon = useTranslations("Common");

    const [limit, setLimit] = useState(DEFAULT_LIMIT);
    const [docs, setDocs] = useState<CachedJudgment[]>([]);
    const [loading, setLoading] = useState(true);
    const [clearing, setClearing] = useState(false);
    const [confirmClear, setConfirmClear] = useState(false);
    const [reindexing, setReindexing] = useState(false);
    const [reindexNote, setReindexNote] = useState<string | null>(null);
    const [reindexStatus, setReindexStatus] = useState<ReindexStatusOut | null>(null);
    const [reindexStart, setReindexStart] = useState<number | null>(null);
    const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

    // BYOK state
    const [ikConfig, setIkConfig] = useState<IndianKanoonConfig | null>(null);
    const [ikLoading, setIkLoading] = useState(true);
    const [ikSaving, setIkSaving] = useState(false);
    const [ikSaveError, setIkSaveError] = useState<string | null>(null);
    const [ikSaved, setIkSaved] = useState(false);
    const [ikReveal, setIkReveal] = useState(false);
    const ikKeyRef = useRef<HTMLInputElement>(null);

    // Stop polling on unmount.
    useEffect(() => {
        return () => {
            if (pollRef.current) clearInterval(pollRef.current);
        };
    }, []);

    const totalBytes = docs.reduce((sum, d) => sum + (d.size_bytes || 0), 0);
    const count = docs.length;

    async function loadIkConfig() {
        setIkLoading(true);
        try {
            const config = await getIndianKanoonConfig();
            setIkConfig(config);
        } catch (err) {
            console.error("Failed to load Indian Kanoon config:", err);
            setIkConfig({ enabled: false, has_key: false });
        } finally {
            setIkLoading(false);
        }
    }

    useEffect(() => {
        const raw = localStorage.getItem(CACHE_LIMIT_KEY);
        if (raw) {
            const n = parseInt(raw, 10);
            if (!isNaN(n)) setLimit(n);
        }
        refreshDocs();
        loadIkConfig();
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

    async function handleReindex() {
        setReindexing(true);
        setReindexNote(null);
        try {
            const r = await fetch(`${API_BASE}/indian-kanoon/reindex`, {
                method: "POST",
                headers: { Authorization: `Bearer ${getToken()}` },
            });
            if (!r.ok) {
                setReindexNote("Couldn't start the rebuild — please try again.");
                setReindexing(false);
                return;
            }
            const data = await r.json();
            setReindexStart(Date.now());
            setReindexStatus({
                status: "running",
                total: data.count ?? count,
                processed: 0,
                indexed: 0,
            });

            // Poll the backend snapshot until the rebuild finishes. The
            // <ReindexProgress> component renders the embedded count, a live
            // timer and an ETA from this.
            if (pollRef.current) clearInterval(pollRef.current);
            pollRef.current = setInterval(async () => {
                try {
                    const sr = await fetch(
                        `${API_BASE}/indian-kanoon/reindex-status`,
                        { headers: { Authorization: `Bearer ${getToken()}` } },
                    );
                    if (!sr.ok) return;
                    const s: ReindexStatusOut = await sr.json();
                    setReindexStatus(s);
                    if (s.status === "done" || s.status === "failed") {
                        if (pollRef.current) clearInterval(pollRef.current);
                        pollRef.current = null;
                        setReindexing(false);
                        refreshDocs();
                    }
                } catch {
                    // transient — keep the previous snapshot
                }
            }, 1000);
        } catch {
            setReindexNote("Couldn't start the rebuild — please try again.");
            setReindexing(false);
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

    async function handleIkSave() {
        if (!ikConfig) return;
        setIkSaving(true);
        setIkSaveError(null);
        try {
            const typed = ikKeyRef.current?.value ?? "";
            await putIndianKanoonConfig({
                enabled: ikConfig.enabled,
                ik_api_key: typed || undefined,
            });
            if (ikKeyRef.current) ikKeyRef.current.value = "";
            setIkConfig((prev) =>
                prev
                    ? {
                        ...prev,
                        has_key: prev.has_key || !!typed,
                    }
                    : prev,
            );
            setIkSaved(true);
            setTimeout(() => setIkSaved(false), 2000);
        } catch (err) {
            setIkSaveError((err as Error).message || t("byokSaveError"));
        } finally {
            setIkSaving(false);
        }
    }

    async function handleIkClear() {
        if (!ikConfig) return;
        setIkSaving(true);
        try {
            await putIndianKanoonConfig({
                enabled: ikConfig.enabled,
                ik_api_key: "",
            });
            setIkConfig((prev) =>
                prev ? { ...prev, has_key: false } : prev,
            );
        } catch (err) {
            setIkSaveError((err as Error).message || t("byokSaveError"));
        } finally {
            setIkSaving(false);
        }
    }

    return (
        <div className="space-y-8">
            {/* BYOK Configuration */}
            {ikLoading ? (
                <div className="text-sm text-gray-400">{tCommon("loading")}</div>
            ) : ikConfig ? (
                <section className="border border-gray-200 rounded-lg p-4 space-y-4">
                    <div>
                        <h3 className="text-sm font-medium text-gray-900">
                            {t("byokTitle")}
                        </h3>
                        <p className="text-xs text-gray-500 mt-1">
                            {t("byokSubtitle")}
                        </p>
                    </div>

                    <div className="space-y-3">
                        {/* Enable toggle */}
                        <label className="flex items-center gap-3 cursor-pointer">
                            <input
                                type="checkbox"
                                checked={ikConfig.enabled}
                                onChange={(e) =>
                                    setIkConfig((prev) =>
                                        prev
                                            ? { ...prev, enabled: e.target.checked }
                                            : prev,
                                    )
                                }
                                className="accent-gray-900 rounded"
                            />
                            <span className="text-sm text-gray-700">
                                {t("byokEnabledLabel")}
                            </span>
                        </label>
                        <p className="text-xs text-gray-500 ml-6">
                            {t("byokEnabledHint")}
                        </p>

                        {/* API key field */}
                        <div>
                            <div className="flex items-center justify-between mb-1">
                                <label className="text-sm text-gray-600">
                                    {t("byokApiKeyLabel")}
                                </label>
                                {ikConfig.has_key && (
                                    <div className="flex items-center gap-2 text-xs">
                                        <span className="inline-flex items-center gap-1 rounded-full bg-green-50 text-green-700 px-2 py-0.5 border border-green-200">
                                            <ShieldCheck className="h-3 w-3" />
                                            {t("byokApiKeyStored")}
                                        </span>
                                        <button
                                            type="button"
                                            onClick={handleIkClear}
                                            disabled={ikSaving}
                                            className="text-gray-400 hover:text-red-600 transition-colors disabled:opacity-50"
                                        >
                                            {tCommon("delete")}
                                        </button>
                                    </div>
                                )}
                            </div>
                            <div className="relative">
                                <input
                                    ref={ikKeyRef}
                                    type={ikReveal ? "text" : "password"}
                                    defaultValue=""
                                    placeholder={
                                        ikConfig.has_key
                                            ? t("byokApiKeyKeepHint")
                                            : t("byokApiKeyPlaceholder")
                                    }
                                    className="flex h-10 w-full rounded-md border border-gray-200 bg-white px-3 py-2 text-sm placeholder:text-gray-400 focus-visible:outline-none focus-visible:border-gray-400 disabled:cursor-not-allowed disabled:opacity-50 pr-10"
                                    autoComplete="off"
                                    spellCheck={false}
                                    disabled={ikSaving}
                                />
                                <button
                                    type="button"
                                    onClick={() => setIkReveal((r) => !r)}
                                    className="absolute inset-y-0 right-2 flex items-center text-gray-400 hover:text-gray-600 disabled:opacity-50"
                                    aria-label={ikReveal ? "Hide" : "Show"}
                                    disabled={ikSaving}
                                >
                                    {ikReveal ? (
                                        <EyeOff className="h-4 w-4" />
                                    ) : (
                                        <Eye className="h-4 w-4" />
                                    )}
                                </button>
                            </div>
                        </div>

                        {/* Save button */}
                        <button
                            onClick={handleIkSave}
                            disabled={ikSaving}
                            className="inline-flex items-center gap-1.5 rounded-md bg-black text-white px-4 py-2 text-sm font-medium hover:bg-gray-900 disabled:opacity-50 transition-colors"
                        >
                            {ikSaved ? (
                                <>
                                    <Check className="h-4 w-4" />
                                    {tCommon("save")}
                                </>
                            ) : ikSaving ? (
                                <>
                                    <Loader2 className="h-4 w-4 animate-spin" />
                                    {t("byokSaveButton")}
                                </>
                            ) : (
                                t("byokSaveButton")
                            )}
                        </button>

                        {ikSaveError && (
                            <p className="text-sm text-red-600 bg-red-50 px-3 py-2 rounded-md border border-red-200">
                                {ikSaveError}
                            </p>
                        )}
                    </div>
                </section>
            ) : null}

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
                    <div className="flex items-center gap-2">
                        <div className="relative group">
                            <button
                                onClick={handleReindex}
                                disabled={reindexing || count === 0}
                                className="inline-flex items-center gap-1.5 rounded-md border border-gray-200 px-3 py-1.5 text-sm text-gray-700 hover:bg-gray-50 disabled:opacity-50 transition-colors"
                            >
                                {reindexing ? (
                                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                                ) : (
                                    <RefreshCw className="h-3.5 w-3.5" />
                                )}
                                Rebuild index
                            </button>
                            {/* Plain-language hover explainer. */}
                            <div className="pointer-events-none absolute right-0 top-full mt-2 w-72 rounded-lg bg-gray-900 px-3 py-2.5 text-xs leading-relaxed text-white shadow-xl opacity-0 transition-opacity duration-150 group-hover:opacity-100 z-50">
                                <span className="font-semibold">Helps Mike find the exact part of your saved cases when you search.</span>
                                <br />
                                <br />
                                Press it once. It won&apos;t re-download anything, cost
                                credits, or delete your cases.
                            </div>
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
                </div>
                {reindexNote && (
                    <p className="text-xs text-gray-500">{reindexNote}</p>
                )}
                {reindexStatus && reindexStatus.status !== "idle" && (
                    <ReindexProgress
                        indexed={reindexStatus.indexed}
                        total={reindexStatus.total}
                        status={reindexStatus.status}
                        startedAt={reindexStart}
                        currentFile={reindexStatus.current_file}
                        currentStep={reindexStatus.current_step}
                        errorText={reindexStatus.last_error}
                        labels={{
                            unit: "judgments",
                            failedLabel: "Rebuild failed — please try again.",
                        }}
                    />
                )}
                {totalBytes > 0 && limit > 0 && reindexStatus?.status !== "running" && (
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
