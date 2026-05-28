"use client";

import { useEffect, useState } from "react";
import { Search, Loader2, ExternalLink, X, Info } from "lucide-react";
import type { VangaResult } from "@/lib/vanga-search";

const BANNER_DISMISSED_KEY = "vanga_banner_dismissed";

const COURT_OPTIONS = [
    { label: "All Courts", value: "" },
    { label: "Delhi HC", value: "7_26" },
    { label: "Bombay HC", value: "27_1" },
    { label: "Madras HC", value: "33_10" },
    { label: "Calcutta HC", value: "19_16" },
    { label: "Karnataka HC", value: "29_3" },
    { label: "Kerala HC", value: "32_4" },
    { label: "Allahabad HC", value: "9_13" },
    { label: "Gujarat HC", value: "24_17" },
    { label: "Punjab & Haryana HC", value: "3_22" },
    { label: "Rajasthan HC", value: "8_9" },
    { label: "Telangana HC", value: "36_29" },
    { label: "Andhra Pradesh HC", value: "28_2" },
    { label: "Patna HC", value: "10_8" },
    { label: "Jharkhand HC", value: "20_7" },
    { label: "Gauhati HC", value: "18_6" },
    { label: "Madhya Pradesh HC", value: "23_23" },
    { label: "Orissa HC", value: "21_11" },
    { label: "Chhattisgarh HC", value: "22_18" },
    { label: "Uttarakhand HC", value: "5_15" },
    { label: "Himachal Pradesh HC", value: "2_5" },
    { label: "J&K HC", value: "1_12" },
];

export default function CaseSearchPage() {
    const [query, setQuery] = useState("");
    const [courtCode, setCourtCode] = useState("");
    const [yearStart, setYearStart] = useState("2015");
    const [yearEnd, setYearEnd] = useState(String(new Date().getFullYear()));
    const [results, setResults] = useState<VangaResult[]>([]);
    const [loading, setLoading] = useState(false);
    const [loadingStatus, setLoadingStatus] = useState("");
    const [error, setError] = useState<string | null>(null);
    const [searched, setSearched] = useState(false);
    const [saveForLater, setSaveForLater] = useState(true);
    const [bannerDismissed, setBannerDismissed] = useState(true);

    useEffect(() => {
        setBannerDismissed(localStorage.getItem(BANNER_DISMISSED_KEY) === "1");
    }, []);

    function dismissBanner() {
        localStorage.setItem(BANNER_DISMISSED_KEY, "1");
        setBannerDismissed(true);
    }

    async function handleSearch(e: React.FormEvent) {
        e.preventDefault();
        if (!query.trim()) return;

        setLoading(true);
        setError(null);
        setSearched(true);
        setLoadingStatus("Searching judgments…");

        try {
            const { searchWithFullText } = await import("@/lib/vanga-search");
            const r = await searchWithFullText(
                {
                    query: query.trim(),
                    court_code: courtCode || undefined,
                    year_start: parseInt(yearStart) || undefined,
                    year_end: parseInt(yearEnd) || undefined,
                },
                saveForLater,
                (p) => {
                    if (p.phase === "searching") setLoadingStatus("Searching judgments…");
                    else if (p.phase === "loading") setLoadingStatus(`Loading ${p.loaded} of ${p.total} judgments…`);
                    else setLoadingStatus("Done");
                },
            );
            setResults(r);
        } catch (err) {
            setError(String(err));
            setResults([]);
        } finally {
            setLoading(false);
        }
    }

    return (
        <div className="flex h-full flex-col overflow-hidden">
            <div className="border-b border-gray-200 px-6 py-4 shrink-0">
                <h1 className="text-2xl font-serif font-semibold text-gray-900">
                    Case Search
                </h1>
                <p className="text-sm text-gray-500 mt-1">
                    Search Indian High Court judgments from the Vanga open dataset
                </p>
            </div>

            <div className="flex-1 overflow-y-auto px-6 py-6">
                {!bannerDismissed && (
                    <div className="mb-4 flex items-start gap-3 rounded-lg border border-blue-200 bg-blue-50 px-4 py-3 text-sm text-blue-800">
                        <Info className="h-4 w-4 shrink-0 mt-0.5 text-blue-500" />
                        <p className="flex-1">
                            Mike saves judgments you&apos;ve viewed for quick access later. You can change how many to keep, or clear them anytime, in Settings.
                        </p>
                        <button
                            onClick={dismissBanner}
                            className="shrink-0 text-blue-400 hover:text-blue-600 transition-colors"
                        >
                            <X className="h-4 w-4" />
                        </button>
                    </div>
                )}

                <form onSubmit={handleSearch} className="mb-6">
                    <div className="flex gap-3 items-end flex-wrap">
                        <div className="flex-1 min-w-[200px]">
                            <label className="block text-xs font-medium text-gray-600 mb-1">
                                Keywords
                            </label>
                            <div className="relative">
                                <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-gray-400" />
                                <input
                                    type="text"
                                    value={query}
                                    onChange={(e) => setQuery(e.target.value)}
                                    placeholder="e.g. Section 138 NI Act security cheque"
                                    className="w-full pl-9 pr-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-1 focus:ring-gray-400 focus:border-gray-400"
                                />
                            </div>
                        </div>
                        <div className="w-48">
                            <label className="block text-xs font-medium text-gray-600 mb-1">
                                Court
                            </label>
                            <select
                                value={courtCode}
                                onChange={(e) => setCourtCode(e.target.value)}
                                className="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-1 focus:ring-gray-400"
                            >
                                {COURT_OPTIONS.map((c) => (
                                    <option key={c.value} value={c.value}>
                                        {c.label}
                                    </option>
                                ))}
                            </select>
                        </div>
                        <div className="w-24">
                            <label className="block text-xs font-medium text-gray-600 mb-1">
                                From
                            </label>
                            <input
                                type="number"
                                value={yearStart}
                                onChange={(e) => setYearStart(e.target.value)}
                                min="1950"
                                max="2025"
                                className="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-1 focus:ring-gray-400"
                            />
                        </div>
                        <div className="w-24">
                            <label className="block text-xs font-medium text-gray-600 mb-1">
                                To
                            </label>
                            <input
                                type="number"
                                value={yearEnd}
                                onChange={(e) => setYearEnd(e.target.value)}
                                min="1950"
                                max="2025"
                                className="w-full px-3 py-2 text-sm border border-gray-300 rounded-md focus:outline-none focus:ring-1 focus:ring-gray-400"
                            />
                        </div>
                        <button
                            type="submit"
                            disabled={loading || !query.trim()}
                            className="inline-flex items-center gap-1.5 rounded-md bg-gray-900 px-4 py-2 text-sm font-medium text-white hover:bg-gray-800 disabled:opacity-50 transition-colors"
                        >
                            {loading ? (
                                <Loader2 className="h-4 w-4 animate-spin" />
                            ) : (
                                <Search className="h-4 w-4" />
                            )}
                            Search
                        </button>
                    </div>
                    <label
                        className="mt-2 inline-flex items-center gap-2 cursor-pointer"
                        title="If you don't want this search remembered on your computer, turn this off."
                    >
                        <input
                            type="checkbox"
                            checked={saveForLater}
                            onChange={(e) => setSaveForLater(e.target.checked)}
                            className="accent-gray-900"
                        />
                        <span className="text-xs text-gray-500">Save for later</span>
                    </label>
                </form>

                {loading && (
                    <div className="flex items-center justify-center py-20">
                        <div className="flex flex-col items-center gap-3">
                            <Loader2 className="h-6 w-6 animate-spin text-gray-400" />
                            <p className="text-sm text-gray-500">
                                {loadingStatus || "Initializing search engine…"}
                            </p>
                        </div>
                    </div>
                )}

                {error && (
                    <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
                        {error}
                    </div>
                )}

                {!loading && searched && results.length === 0 && !error && (
                    <div className="flex flex-col items-center justify-center py-20 text-center">
                        <Search className="h-12 w-12 text-gray-300 mb-4" />
                        <p className="text-sm font-medium text-gray-700 mb-1">
                            No judgments found
                        </p>
                        <p className="text-xs text-gray-500 max-w-sm">
                            Try broadening your search terms or adjusting the year range.
                        </p>
                    </div>
                )}

                {!loading && results.length > 0 && (
                    <div className="space-y-3">
                        <p className="text-xs text-gray-500 mb-3">
                            {results.length} result{results.length !== 1 ? "s" : ""}
                        </p>
                        {results.map((r, i) => (
                            <div
                                key={`${r.case_id}-${i}`}
                                className="rounded-lg border border-gray-200 bg-white p-4 hover:border-gray-300 hover:shadow-sm transition-all"
                            >
                                <div className="flex items-start justify-between gap-3">
                                    <div className="flex-1 min-w-0">
                                        <h3 className="text-sm font-medium text-gray-900 line-clamp-2">
                                            {r.title}
                                        </h3>
                                        <div className="flex items-center gap-2 mt-1 flex-wrap">
                                            <span className="text-xs text-gray-500">
                                                {r.court_name}
                                            </span>
                                            {r.decision_date && (
                                                <>
                                                    <span className="text-xs text-gray-300">|</span>
                                                    <span className="text-xs text-gray-500">
                                                        {r.decision_date}
                                                    </span>
                                                </>
                                            )}
                                            {r.judge && (
                                                <>
                                                    <span className="text-xs text-gray-300">|</span>
                                                    <span className="text-xs text-gray-500">
                                                        {r.judge}
                                                    </span>
                                                </>
                                            )}
                                        </div>
                                        {r.snippet && (
                                            <p className="text-xs text-gray-600 mt-2 line-clamp-3">
                                                {r.snippet}
                                            </p>
                                        )}
                                    </div>
                                    <a
                                        href={r.pdf_url}
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        className="shrink-0 inline-flex items-center gap-1 rounded-md border border-gray-200 px-2.5 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-50 transition-colors"
                                    >
                                        <ExternalLink className="h-3 w-3" />
                                        PDF
                                    </a>
                                </div>
                            </div>
                        ))}
                    </div>
                )}
            </div>
        </div>
    );
}
