"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { useLocale, useTranslations } from "next-intl";
import { Plus, Briefcase, FileText, Loader2 } from "lucide-react";
import { listCases, createCase } from "@/app/lib/mikeApi";
import type { MikeCase, CaseParty } from "@/app/components/shared/types";

function formatDate(iso: string, locale: string) {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return "—";
    return d.toLocaleDateString(locale, {
        day: "numeric",
        month: "short",
        year: "numeric",
    });
}

function parseParties(json: string | null): CaseParty[] {
    if (!json) return [];
    try {
        return JSON.parse(json);
    } catch {
        return [];
    }
}

function partiesSummary(parties: CaseParty[]): string {
    if (parties.length === 0) return "";
    const petitioners = parties.filter((p) => p.role === "petitioner" || p.role === "appellant");
    const respondents = parties.filter((p) => p.role === "respondent");
    if (petitioners.length > 0 && respondents.length > 0) {
        return `${petitioners.map((p) => p.name).join(", ")} v. ${respondents.map((p) => p.name).join(", ")}`;
    }
    return parties.map((p) => p.name).join(", ");
}

export default function CasesPage() {
    const [cases, setCases] = useState<MikeCase[]>([]);
    const [loading, setLoading] = useState(true);
    const [creating, setCreating] = useState(false);
    const router = useRouter();
    const t = useTranslations("Cases");
    const locale = useLocale();

    useEffect(() => {
        listCases()
            .then(setCases)
            .catch(() => setCases([]))
            .finally(() => setLoading(false));
    }, []);

    async function handleNewCase() {
        setCreating(true);
        try {
            const c = await createCase({ title: "Untitled Case" });
            router.push(`/cases/${c.id}`);
        } catch {
            setCreating(false);
        }
    }

    return (
        <div className="flex h-full flex-col overflow-hidden">
            {/* Header */}
            <div className="flex items-center justify-between border-b border-gray-200 px-6 py-4 shrink-0">
                <h1 className="text-2xl font-serif font-semibold text-gray-900">
                    {t("title")}
                </h1>
                <button
                    onClick={handleNewCase}
                    disabled={creating}
                    className="inline-flex items-center gap-1.5 rounded-md bg-gray-900 px-3 py-2 text-sm font-medium text-white hover:bg-gray-800 disabled:opacity-50 transition-colors"
                >
                    {creating ? (
                        <Loader2 className="h-4 w-4 animate-spin" />
                    ) : (
                        <Plus className="h-4 w-4" />
                    )}
                    {t("newCase")}
                </button>
            </div>

            {/* Content */}
            <div className="flex-1 overflow-y-auto px-6 py-6">
                {loading ? (
                    <div className="flex items-center justify-center py-20">
                        <div className="h-6 w-6 animate-spin rounded-full border-2 border-gray-300 border-t-gray-700" />
                    </div>
                ) : cases.length === 0 ? (
                    <div className="flex flex-col items-center justify-center py-20 text-center">
                        <Briefcase className="h-12 w-12 text-gray-300 mb-4" />
                        <p className="text-sm font-medium text-gray-700 mb-1">
                            {t("noCase")}
                        </p>
                        <p className="text-xs text-gray-500 max-w-sm">
                            {t("noCaseHint")}
                        </p>
                    </div>
                ) : (
                    <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                        {cases.map((c) => {
                            const parties = parseParties(c.parties_json);
                            const summary = partiesSummary(parties);
                            return (
                                <button
                                    key={c.id}
                                    onClick={() => router.push(`/cases/${c.id}`)}
                                    className="flex flex-col rounded-lg border border-gray-200 bg-white p-4 text-left hover:border-gray-300 hover:shadow-sm transition-all"
                                >
                                    <div className="flex items-start justify-between gap-2 mb-2">
                                        <h3 className="text-sm font-medium text-gray-900 line-clamp-2">
                                            {c.title}
                                        </h3>
                                        <span
                                            className={`shrink-0 inline-flex items-center rounded-full px-2 py-0.5 text-[10px] font-medium ${
                                                c.status === "active"
                                                    ? "bg-green-50 text-green-700"
                                                    : "bg-gray-100 text-gray-600"
                                            }`}
                                        >
                                            {c.status === "active"
                                                ? t("active")
                                                : t("archived")}
                                        </span>
                                    </div>
                                    {c.court && (
                                        <p className="text-xs text-gray-500 mb-1">
                                            {c.court}
                                        </p>
                                    )}
                                    {summary && (
                                        <p className="text-xs text-gray-600 mb-2 line-clamp-1">
                                            {summary}
                                        </p>
                                    )}
                                    <div className="mt-auto flex items-center justify-between pt-3 border-t border-gray-100">
                                        <div className="flex items-center gap-1 text-xs text-gray-400">
                                            <FileText className="h-3 w-3" />
                                            <span>
                                                {c.document_count ?? 0}{" "}
                                                {t("documents").toLowerCase()}
                                            </span>
                                        </div>
                                        <span className="text-[11px] text-gray-400">
                                            {formatDate(c.updated_at, locale)}
                                        </span>
                                    </div>
                                </button>
                            );
                        })}
                    </div>
                )}
            </div>
        </div>
    );
}
