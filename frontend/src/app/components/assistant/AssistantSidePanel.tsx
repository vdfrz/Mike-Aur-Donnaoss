"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { X, ExternalLink, Loader2 } from "lucide-react";
import { DocPanel, type DocPanelMode } from "../shared/DocPanel";
import type {
    MikeCitationAnnotation,
    MikeEditAnnotation,
} from "../shared/types";

// ---------------------------------------------------------------------------
// Tab data
// ---------------------------------------------------------------------------
//
// Each tab represents ONE of:
//   - a document view (no specific annotation),
//   - a single citation quote,
//   - a single tracked change.
// There is no selector UI inside the panel — the user picks what to view
// by clicking a different tab (or opening a new one from a citation pill,
// an EditCard's View button, or the download card).

type CommonTab = {
    id: string;
    documentId: string;
    filename: string;
    versionId: string | null;
    versionNumber: number | null;
    warning?: string | null;
    initialScrollTop?: number | null;
    /**
     * KB-indexed local file path. When set, the panel fetches bytes via
     * `/sync/kb-doc?path=<kbPath>` instead of the upload-flow endpoints.
     * Set by `ChatView.openCitation` when the citation has source `kb`
     * (auto-retrieval) or `tool` (search_kb fetch).
     */
    kbPath?: string | null;
};

export type DocumentTab = CommonTab & { kind: "document" };

export type CitationTab = CommonTab & {
    kind: "citation";
    citation: MikeCitationAnnotation;
};

export type EditTab = CommonTab & {
    kind: "edit";
    edit: MikeEditAnnotation;
};

export type IKTab = {
    id: string;
    documentId: string;
    filename: string;
    kind: "ik";
    url: string;
    query?: string;
    versionId: null;
    versionNumber: null;
    warning?: string | null;
    initialScrollTop?: number | null;
    kbPath?: string | null;
};

export type AssistantSidePanelTab = DocumentTab | CitationTab | EditTab | IKTab;

interface Props {
    tabs: AssistantSidePanelTab[];
    activeTabId: string | null;
    onActivateTab: (id: string) => void;
    onCloseTab: (id: string) => void;
    onCloseAll: () => void;
    /**
     * Parent-driven reloading flag per document. Download buttons in
     * DocPanel show a spinner iff this returns true for the tab's
     * documentId. Used to signal "accept/reject in flight".
     */
    isEditorReloading?: (documentId: string) => boolean;
    /**
     * True while an accept/reject for this exact edit is in flight.
     * Disables the panel's Accept/Reject buttons for only the edit
     * currently being resolved — sibling edits stay clickable.
     */
    isEditReloading?: (editId: string) => boolean;
    onEditResolveStart?: (args: {
        editId: string;
        documentId: string;
        verb: "accept" | "reject";
    }) => void;
    onEditResolved?: (args: {
        editId: string;
        documentId: string;
        status: "accepted" | "rejected";
        versionId: string | null;
        downloadUrl: string | null;
    }) => void;
    onEditError?: (args: {
        editId: string;
        documentId: string;
        versionId: string | null;
        message: string;
    }) => void;
    onWarningDismiss?: (tabId: string) => void;
    onScrollChange?: (tabId: string, scrollTop: number) => void;
}

const API_BASE =
    process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

const IK_STOP_WORDS = new Set([
    "the","a","an","in","of","for","on","to","and","or","by","with",
    "from","as","at","it","is","be","has","have","been","was","were",
    "are","does","do","did","can","will","shall","may","would","could",
    "should","that","this","these","those","not","but","if","so","then",
    "also","its","his","her","their","our","your","my","he","she","they",
    "we","you","i","me","him","us","them","who","which","what","where",
    "when","how","why","under","section","case","court","held","act",
]);

function extractKeywords(text: string): string[] {
    return text
        .toLowerCase()
        .split(/[^a-z0-9]+/)
        .filter((w) => w.length > 2 && !IK_STOP_WORDS.has(w));
}

function highlightHtml(
    html: string,
    query: string,
): { html: string; snippet: string | null } {
    const parser = new DOMParser();
    const doc = parser.parseFromString(`<div>${html}</div>`, "text/html");

    // Only consider leaf-ish blocks so we don't match a wrapping <div>
    // that contains every paragraph.
    const candidates = Array.from(
        doc.querySelectorAll("p, li, blockquote, td"),
    ).filter((el) => {
        const text = (el.textContent ?? "").trim();
        return text.length > 60;
    });

    // Normalize for substring matching — collapse whitespace + curly
    // quotes so Mike's chat quote survives minor typographic differences
    // versus the Kanoon doc text.
    function normalize(s: string): string {
        return s
            .toLowerCase()
            .replace(/[“”]/g, '"')
            .replace(/[‘’]/g, "'")
            .replace(/\s+/g, " ")
            .trim();
    }
    const normQuery = normalize(query);
    let best: { el: Element; score: number; matches: number } | null = null;

    // Strategy 1 (preferred): exact-substring phrase match. Try
    // chunks from sliding offsets in the query so a preamble like
    // "The Court held:" doesn't prevent matching the actual quote.
    if (normQuery.length >= 30) {
        for (const el of candidates) {
            const text = normalize(el.textContent ?? "");
            let maxOverlap = 0;
            for (let start = 0; start <= normQuery.length - 30; start += 15) {
                const remaining = normQuery.length - start;
                for (let len = Math.min(remaining, 300); len >= 30; len -= 20) {
                    const chunk = normQuery.slice(start, start + len);
                    if (text.includes(chunk)) {
                        if (len > maxOverlap) maxOverlap = len;
                        break;
                    }
                }
                if (maxOverlap >= 60) break;
            }
            if (maxOverlap >= 30) {
                const score = maxOverlap / Math.sqrt(text.length);
                if (!best || score > best.score) {
                    best = { el, score, matches: maxOverlap };
                }
            }
        }
    }

    // Strategy 2 (fallback): keyword density — the previous behavior.
    // Only runs if substring matching found nothing.
    if (!best) {
        const keywords = extractKeywords(query);
        if (keywords.length === 0) return { html, snippet: null };
        for (const el of candidates) {
            const text = (el.textContent ?? "").toLowerCase();
            const matches = keywords.filter((kw) => text.includes(kw)).length;
            if (matches < 2) continue;
            const score = matches / Math.sqrt(text.length);
            if (!best || score > best.score) {
                best = { el, score, matches };
            }
        }
    }

    if (!best) return { html, snippet: null };

    (best.el as HTMLElement).style.backgroundColor = "#fef08a";
    (best.el as HTMLElement).style.borderLeft = "3px solid #eab308";
    (best.el as HTMLElement).style.paddingLeft = "12px";
    (best.el as HTMLElement).style.padding = "8px 12px";
    (best.el as HTMLElement).style.margin = "8px 0";
    (best.el as HTMLElement).setAttribute("data-ik-match", "true");

    const snippet = (best.el.textContent ?? "").trim().slice(0, 280);

    return { html: doc.body.innerHTML, snippet };
}

function IKDocViewer({ url, title, query }: { url: string; title: string; query?: string }) {
    const [html, setHtml] = useState<string | null>(null);
    const [snippet, setSnippet] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const contentRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        const match = url.match(/\/doc\/(\d+)/);
        if (!match) {
            setError("Invalid Indian Kanoon URL");
            setLoading(false);
            return;
        }
        const tid = match[1];
        let cancelled = false;
        setLoading(true);
        setError(null);
        setSnippet(null);

        (async () => {
            try {
                const token =
                    typeof window !== "undefined"
                        ? localStorage.getItem("mike_auth_token")
                        : null;
                const resp = await fetch(
                    `${API_BASE}/indian-kanoon/doc-html/${tid}`,
                    {
                        headers: token
                            ? { Authorization: `Bearer ${token}` }
                            : {},
                    },
                );
                if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
                const data = await resp.json();
                if (!cancelled) {
                    const rawHtml = data.html ?? "";
                    if (query) {
                        const result = highlightHtml(rawHtml, query);
                        setHtml(result.html);
                        setSnippet(result.snippet);
                    } else {
                        setHtml(rawHtml);
                    }
                }
            } catch (e) {
                if (!cancelled)
                    setError(
                        e instanceof Error ? e.message : "Failed to load case",
                    );
            } finally {
                if (!cancelled) setLoading(false);
            }
        })();

        return () => {
            cancelled = true;
        };
    }, [url, query]);

    useEffect(() => {
        if (!html || !contentRef.current) return;
        requestAnimationFrame(() => {
            const firstMatch = contentRef.current?.querySelector(
                "[data-ik-match='true']",
            );
            if (firstMatch) {
                firstMatch.scrollIntoView({ behavior: "smooth", block: "center" });
            }
        });
    }, [html]);

    if (loading) {
        return (
            <div className="flex-1 flex items-center justify-center">
                <Loader2 className="h-6 w-6 animate-spin text-gray-400" />
                <span className="ml-2 text-sm text-gray-500">Loading case...</span>
            </div>
        );
    }
    if (error) {
        return (
            <div className="flex-1 flex items-center justify-center p-4">
                <p className="text-sm text-red-500">{error}</p>
            </div>
        );
    }
    return (
        <div className="flex-1 flex flex-col min-h-0">
            {snippet && (
                <div className="border-b border-gray-200 bg-yellow-50 px-4 py-3">
                    <div className="text-xs font-semibold text-gray-600 mb-1 uppercase tracking-wide">
                        Citation
                    </div>
                    <p className="text-sm text-gray-800 italic leading-relaxed">
                        &ldquo;{snippet}
                        {snippet.length >= 280 ? "..." : ""}&rdquo;
                    </p>
                </div>
            )}
            <div
                ref={contentRef}
                className="flex-1 overflow-y-auto p-4 prose prose-sm max-w-none"
                dangerouslySetInnerHTML={{ __html: html ?? "" }}
            />
        </div>
    );
}

const MIN_WIDTH = 300;
const MAX_WIDTH_OFFSET = 56; // sidebar width

export function AssistantSidePanel({
    tabs,
    activeTabId,
    onActivateTab,
    onCloseTab,
    onCloseAll,
    isEditorReloading,
    isEditReloading,
    onEditResolveStart,
    onEditResolved,
    onEditError,
    onWarningDismiss,
    onScrollChange,
}: Props) {
    const tA = useTranslations("Assistant");
    const panelRef = useRef<HTMLDivElement>(null);
    const [panelWidth, setPanelWidth] = useState(() =>
        typeof window !== "undefined"
            ? Math.round((window.innerWidth - MAX_WIDTH_OFFSET) / 2)
            : 600,
    );

    const dragStartX = useRef<number>(0);
    const dragStartWidth = useRef<number>(0);

    const onMouseDown = useCallback(
        (e: React.MouseEvent) => {
            e.preventDefault();
            dragStartX.current = e.clientX;
            dragStartWidth.current =
                panelRef.current?.offsetWidth ?? panelWidth;

            const onMouseMove = (ev: MouseEvent) => {
                const delta = dragStartX.current - ev.clientX;
                const maxWidth = window.innerWidth - MAX_WIDTH_OFFSET - 200;
                setPanelWidth(
                    Math.min(
                        maxWidth,
                        Math.max(MIN_WIDTH, dragStartWidth.current + delta),
                    ),
                );
            };
            const onMouseUp = () => {
                document.removeEventListener("mousemove", onMouseMove);
                document.removeEventListener("mouseup", onMouseUp);
                document.body.style.cursor = "";
                document.body.style.userSelect = "";
            };

            document.addEventListener("mousemove", onMouseMove);
            document.addEventListener("mouseup", onMouseUp);
            document.body.style.cursor = "col-resize";
            document.body.style.userSelect = "none";
        },
        [panelWidth],
    );

    const active = tabs.find((t) => t.id === activeTabId) ?? tabs[0] ?? null;
    if (!active) return null;

    return (
        <div
            ref={panelRef}
            className="flex h-full shrink-0 flex-col bg-white relative border-l border-gray-200 shadow-[-4px_0_12px_rgba(0,0,0,0.02)]"
            style={{ width: panelWidth }}
        >
            {/* Drag handle */}
            <div
                onMouseDown={onMouseDown}
                className="absolute left-0 top-0 h-full w-1 cursor-col-resize hover:bg-blue-400 transition-colors z-10"
                style={{ marginLeft: -2 }}
            />

            {/* Tab strip (Chrome-style) */}
            <div className="flex items-end gap-1 pr-2 pt-2 bg-gray-100">
                <div className="flex-1 flex items-end gap-1 overflow-x-auto pl-2 pr-2">
                    {tabs.map((tab) => {
                        const isActive = tab.id === active.id;
                        const showVersionBadge =
                            typeof tab.versionNumber === "number" &&
                            Number.isFinite(tab.versionNumber) &&
                            tab.versionNumber > 1;
                        return (
                            <div
                                key={tab.id}
                                onClick={() => onActivateTab(tab.id)}
                                className={`group relative flex items-center gap-1.5 pl-3 pr-1.5 h-8 min-w-0 max-w-[220px] rounded-t-lg cursor-pointer select-none transition-colors ${
                                    isActive
                                        ? "bg-white text-gray-800 before:content-[''] before:absolute before:bottom-0 before:-left-2 before:w-2 before:h-2 before:bg-[radial-gradient(circle_at_top_left,transparent_8px,white_9px)] after:content-[''] after:absolute after:bottom-0 after:-right-2 after:w-2 after:h-2 after:bg-[radial-gradient(circle_at_top_right,transparent_8px,white_9px)]"
                                        : "bg-gray-200/70 text-gray-600 hover:bg-gray-200"
                                }`}
                            >
                                <span
                                    className={`min-w-0 flex-1 truncate text-xs ${isActive ? "font-medium" : "font-normal"}`}
                                    title={tab.filename}
                                >
                                    {tab.filename}
                                </span>
                                {showVersionBadge && (
                                    <span
                                        className={`shrink-0 inline-flex items-center rounded border px-1 py-px text-[9px] font-medium ${
                                            isActive
                                                ? "border-gray-200 bg-white text-gray-600"
                                                : "border-gray-300 bg-white/70 text-gray-500"
                                        }`}
                                    >
                                        V{tab.versionNumber}
                                    </span>
                                )}
                                <button
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        onCloseTab(tab.id);
                                    }}
                                    className="shrink-0 rounded-full p-0.5 text-gray-400 hover:bg-gray-300 hover:text-gray-700"
                                >
                                    <X className="h-3 w-3" />
                                </button>
                            </div>
                        );
                    })}
                </div>
                <button
                    onClick={onCloseAll}
                    className="shrink-0 mb-1 ml-1 rounded-lg p-1.5 text-gray-400 hover:bg-gray-200 hover:text-gray-700"
                    title={tA("closePanel")}
                >
                    <X className="h-4 w-4" />
                </button>
            </div>

            {/* Tab bodies — all mounted, inactive ones hidden. Each tab
                preserves its state (scroll, docx-preview render, etc.)
                when inactive. */}
            <div className="flex-1 min-h-0 relative">
                {tabs.map((tab) => {
                    const isActive = tab.id === active.id;

                    if (tab.kind === "ik") {
                        return (
                            <div
                                key={tab.id}
                                className={`absolute inset-0 flex flex-col ${isActive ? "" : "invisible pointer-events-none"}`}
                                aria-hidden={!isActive}
                            >
                                {/* IK header bar */}
                                <div className="flex items-center justify-between px-3 py-2 border-b border-gray-200 bg-white">
                                    <span className="text-sm font-medium text-gray-700 truncate">
                                        Indian Kanoon
                                    </span>
                                    <button
                                        onClick={() => {
                                            import("@tauri-apps/api/core").then((tauri) => {
                                                tauri.invoke("open_external_url", { url: tab.url }).catch(() => {
                                                    window.open(tab.url, "_blank", "noopener,noreferrer");
                                                });
                                            }).catch(() => {
                                                window.open(tab.url, "_blank", "noopener,noreferrer");
                                            });
                                        }}
                                        className="flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs font-medium text-blue-600 hover:bg-blue-50 transition-colors"
                                    >
                                        <ExternalLink className="h-3.5 w-3.5" />
                                        Open on Indian Kanoon
                                    </button>
                                </div>
                                {/* IK document content */}
                                <IKDocViewer url={tab.url} title={tab.filename} query={tab.query} />
                            </div>
                        );
                    }

                    const mode: DocPanelMode =
                        tab.kind === "citation"
                            ? {
                                  kind: "citation",
                                  citation: tab.citation,
                              }
                            : tab.kind === "edit"
                              ? {
                                    kind: "edit",
                                    edit: tab.edit,
                                    isEditReloading:
                                        isEditReloading?.(tab.edit.edit_id) ??
                                        false,
                                    onResolveStart: onEditResolveStart,
                                    onResolved: onEditResolved,
                                    onError: onEditError,
                                }
                              : { kind: "document" };
                    return (
                        <div
                            key={tab.id}
                            className={`absolute inset-0 flex flex-col ${isActive ? "" : "invisible pointer-events-none"}`}
                            aria-hidden={!isActive}
                        >
                            <DocPanel
                                documentId={tab.documentId}
                                filename={tab.filename}
                                versionId={tab.versionId}
                                versionNumber={tab.versionNumber}
                                mode={mode}
                                kbPath={tab.kbPath ?? null}
                                isReloading={
                                    isEditorReloading?.(tab.documentId) ?? false
                                }
                                warning={tab.warning ?? null}
                                onWarningDismiss={() =>
                                    onWarningDismiss?.(tab.id)
                                }
                                initialScrollTop={tab.initialScrollTop ?? null}
                                onScrollChange={(scrollTop) =>
                                    onScrollChange?.(tab.id, scrollTop)
                                }
                            />
                        </div>
                    );
                })}
            </div>
        </div>
    );
}
