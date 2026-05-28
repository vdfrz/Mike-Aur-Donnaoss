"use client";

import DashboardLayout from "../_dashboard-layout";
import { useEffect } from "react";
import { usePathname, useRouter } from "next/navigation";
import { useTranslations } from "next-intl";
import { Loader2 } from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";

interface TabDef {
    id: string;
    label: string;
    href: string;
}

interface TabGroup {
    /** Lowercase section header rendered above the group's tabs. */
    heading: string;
    tabs: TabDef[];
}

export default function AccountLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    const router = useRouter();
    const pathname = usePathname();
    const { isAuthenticated, authLoading } = useAuth();
    const tAccount = useTranslations("Account");
    const tCommon = useTranslations("Common");

    // Two semantic groups:
    //  1. Configurazione — what you set once and forget (account
    //     profile, LLM provider keys, MCP servers).
    //  2. Documenti & fonti — the live, ever-growing piece (local
    //     folder sync, authoritative legal corpora).
    // The split keeps the "set up the model" path far from the
    // "manage the daily knowledge base" path, which until now were
    // collapsed into one flat list.
    const groups: TabGroup[] = [
        {
            heading: tAccount("groupConfig"),
            tabs: [
                { id: "general", label: tAccount("generalLink"), href: "/account" },
                { id: "personalization", label: tAccount("personalizationLink"), href: "/account/personalization" },
                { id: "models", label: tAccount("modelsLink"), href: "/account/models" },
                { id: "mcp", label: tAccount("mcpLink"), href: "/account/mcp" },
            ],
        },
        {
            heading: tAccount("groupSources"),
            tabs: [
                { id: "local-docs", label: tAccount("localDocsLink"), href: "/account/sync" },
                { id: "eurlex", label: tAccount("eurlexLink"), href: "/account/eurlex" },
                { id: "indian-kanoon", label: tAccount("indianKanoonLink"), href: "/account/indian-kanoon" },
                { id: "case-search", label: tAccount("caseSearchLink"), href: "/account/case-search" },
            ],
        },
    ];

    useEffect(() => {
        if (!authLoading && !isAuthenticated) {
            router.push("/");
        }
    }, [isAuthenticated, authLoading, router]);

    if (authLoading) {
        return (
            <DashboardLayout>
                <div className="h-full flex items-center justify-center">
                    <Loader2 className="h-8 w-8 animate-spin text-blue-600" />
                </div>
            </DashboardLayout>
        );
    }

    if (!isAuthenticated) {
        return null;
    }

    return (
        <DashboardLayout>
            <div className="flex flex-col h-full md:overflow-y-auto px-6 py-6 md:py-10">
                <div className="max-w-5xl w-full mx-auto">
                    <h1 className="text-4xl font-medium mb-8 font-eb-garamond">
                        {tCommon("settings")}
                    </h1>

                    <div className="flex flex-col md:flex-row gap-6 md:gap-10">
                        <nav
                            aria-label={tCommon("settings")}
                            className="md:w-60 shrink-0 flex md:flex-col gap-6 md:gap-7 overflow-x-auto"
                        >
                            {groups.map((group, groupIdx) => (
                                <div
                                    key={group.heading}
                                    className="flex md:flex-col gap-1 min-w-0"
                                >
                                    <h2 className="hidden md:block text-[11px] font-semibold uppercase tracking-wider text-gray-400 px-3 mb-1 select-none">
                                        {group.heading}
                                    </h2>
                                    {/* Mobile: show a thin divider between groups. */}
                                    {groupIdx > 0 && (
                                        <div className="md:hidden self-center w-px h-6 bg-gray-200 mx-2" />
                                    )}
                                    {group.tabs.map((tab) => {
                                        const active = pathname === tab.href;
                                        return (
                                            <button
                                                key={tab.id}
                                                onClick={() => router.push(tab.href)}
                                                className={`text-left whitespace-nowrap px-3 py-2 rounded-md text-sm font-medium transition-colors ${
                                                    active
                                                        ? "bg-gray-100 text-gray-900"
                                                        : "text-gray-500 hover:text-gray-900 hover:bg-gray-50"
                                                }`}
                                            >
                                                {tab.label}
                                            </button>
                                        );
                                    })}
                                </div>
                            ))}
                        </nav>

                        <div className="flex-1 min-w-0">{children}</div>
                    </div>
                </div>
            </div>
        </DashboardLayout>
    );
}
