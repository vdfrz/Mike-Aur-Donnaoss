"use client";

import { useEffect, useState } from "react";
import { HardDrive, Trash2, Loader2 } from "lucide-react";

function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(0)} MB`;
    return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

const SIZE_OPTIONS = [
    { label: "Small (100 MB)", value: 100 * 1024 * 1024 },
    { label: "Medium (500 MB)", value: 500 * 1024 * 1024 },
    { label: "Large (2 GB)", value: 2 * 1024 * 1024 * 1024 },
    { label: "Don't save any", value: 0 },
];

const CACHE_LIMIT_KEY = "vanga_cache_limit";
const DEFAULT_LIMIT = 500 * 1024 * 1024;

export default function CaseSearchSettingsPage() {
    const [limit, setLimit] = useState(DEFAULT_LIMIT);
    const [totalBytes, setTotalBytes] = useState(0);
    const [count, setCount] = useState(0);
    const [clearing, setClearing] = useState(false);
    const [confirmClear, setConfirmClear] = useState(false);

    useEffect(() => {
        const raw = localStorage.getItem(CACHE_LIMIT_KEY);
        if (raw) {
            const n = parseInt(raw, 10);
            if (!isNaN(n)) setLimit(n);
        }
        refreshStats();
    }, []);

    async function refreshStats() {
        try {
            const { getCacheStats } = await import("@/lib/vanga-search");
            const stats = await getCacheStats();
            setTotalBytes(stats.totalBytes);
            setCount(stats.count);
        } catch {
            // IndexedDB unavailable
        }
    }

    function handleLimitChange(value: number) {
        setLimit(value);
        localStorage.setItem(CACHE_LIMIT_KEY, String(value));
        import("@/lib/vanga-search").then(({ setCacheLimit }) => setCacheLimit(value));
    }

    async function handleClear() {
        if (!confirmClear) {
            setConfirmClear(true);
            return;
        }
        setClearing(true);
        try {
            const { clearCache } = await import("@/lib/vanga-search");
            await clearCache();
            setTotalBytes(0);
            setCount(0);
        } catch {
            // ignore
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
                    Mike saves judgments you&apos;ve viewed so opening them again is faster.
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
                            Currently using {formatBytes(totalBytes)} across {count} judgment{count !== 1 ? "s" : ""}
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
                            style={{ width: `${Math.min(100, (totalBytes / limit) * 100)}%` }}
                        />
                    </div>
                )}
            </div>
        </div>
    );
}
