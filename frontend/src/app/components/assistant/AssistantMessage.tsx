"use client";

import { useId, useRef, useEffect, useState, useMemo } from "react";
import { useTranslations } from "next-intl";
import ReactMarkdown from "react-markdown";
import remarkMath from "remark-math";
import remarkGfm from "remark-gfm";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";
import { Copy, Check, ChevronDown, Download, Loader2, FileText } from "lucide-react";
import { MikeIcon } from "@/components/chat/mike-icon";
import { displayCitationQuote, formatCitationPage } from "../shared/types";
import type {
    AssistantEvent,
    MikeCitationAnnotation,
    MikeEditAnnotation,
} from "../shared/types";
import { EditCard, applyOptimisticResolution } from "./EditCard";
import { PreResponseWrapper } from "../shared/PreResponseWrapper";
import { THINKING_SNIPPETS, getRandomSnippet } from "../../data/thinkingSnippets";
import KanoonVerifyBadge, { extractKanoonTid } from "./KanoonVerifyBadge";
import PoweredByIKanoon from "../shared/PoweredByIKanoon";
import { ToolActivityStream } from "./ToolActivityStream";
import { DocumentCard } from "./DocumentCard";
import InlineClarification from "./InlineClarification";
import { MessageSkeleton } from "./MessageSkeleton";
import { MOTION_KEYFRAMES } from "./MotionTokens";

/**
 * Card rendered above the per-edit EditCards when a message produced
 * multiple tracked-change proposals. Lets the user resolve every pending
 * edit in one click by firing the per-edit accept/reject endpoint for each
 * pending annotation and forwarding each response to `onResolved` so the
 * parent can bump the viewer version, persist override URLs, etc.
 *
 * This intentionally doesn't apply the optimistic DOM mutation that
 * EditCard does — bulk operations touch many edits at once and the real
 * re-render from the latest version will reconcile within a second or so.
 */
function BulkEditActions({
    pending,
    filenameByDocId,
    onViewClick,
    onResolveStart,
    onResolved,
    onError,
}: {
    pending: {
        annotation: MikeEditAnnotation;
        filename: string;
    }[];
    filenameByDocId: Map<string, string>;
    onViewClick?: (ann: MikeEditAnnotation, filename: string) => void;
    onResolveStart?: (args: {
        editId: string;
        documentId: string;
        verb: "accept" | "reject";
    }) => void;
    onResolved?: (args: {
        editId: string;
        documentId: string;
        status: "accepted" | "rejected";
        versionId: string | null;
        downloadUrl: string | null;
    }) => void;
    onError?: (args: {
        editId: string;
        documentId: string;
        versionId: string | null;
        message: string;
    }) => void;
}) {
    const [busy, setBusy] = useState<"accept" | "reject" | null>(null);
    const [progress, setProgress] = useState<{
        done: number;
        total: number;
    } | null>(null);

    if (pending.length === 0) return null;

    const handleAll = async (verb: "accept" | "reject") => {
        if (busy) return;
        setBusy(verb);
        setProgress({ done: 0, total: pending.length });
        try {
                        const token = typeof window !== "undefined" ? localStorage.getItem("mike_auth_token") : null;
            const apiBase =
                process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

            // Sequential so the per-document version counter advances in a
            // predictable order and the viewer doesn't race between bumps.
            let done = 0;
            for (const { annotation } of pending) {
                onResolveStart?.({
                    editId: annotation.edit_id,
                    documentId: annotation.document_id,
                    verb,
                });
                // Optimistically mutate the DOM so the viewer reflects the
                // resolution immediately. Revert if the backend call fails.
                let revert: (() => void) | null = null;
                try {
                    revert = applyOptimisticResolution(annotation, verb);
                } catch (e) {
                    console.error(
                        "[BulkEditActions] optimistic update threw",
                        e,
                    );
                }
                try {
                    const resp = await fetch(
                        `${apiBase}/single-documents/${annotation.document_id}/edits/${annotation.edit_id}/${verb}`,
                        {
                            method: "POST",
                            headers: token
                                ? { Authorization: `Bearer ${token}` }
                                : undefined,
                        },
                    );
                    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
                    const data = (await resp.json()) as {
                        ok: boolean;
                        status?: "accepted" | "rejected";
                        version_id: string | null;
                        download_url: string | null;
                    };
                    const nextStatus =
                        data.status ??
                        (verb === "accept" ? "accepted" : "rejected");
                    onResolved?.({
                        editId: annotation.edit_id,
                        documentId: annotation.document_id,
                        status: nextStatus,
                        versionId: data.version_id,
                        downloadUrl: data.download_url,
                    });
                } catch (e) {
                    console.error("[BulkEditActions] resolve failed", e);
                    try {
                        revert?.();
                    } catch (revertErr) {
                        console.error(
                            "[BulkEditActions] revert threw",
                            revertErr,
                        );
                    }
                    onError?.({
                        editId: annotation.edit_id,
                        documentId: annotation.document_id,
                        versionId: annotation.version_id ?? null,
                        message:
                            verb === "accept"
                                ? "Couldn't save one or more accepts."
                                : "Couldn't save one or more rejects.",
                    });
                }
                done++;
                setProgress({ done, total: pending.length });
            }
        } finally {
            setBusy(null);
            setProgress(null);
        }
    };

    // Optional: show a tiny "View first" action so bulk doesn't lose the
    // in-viewer scroll-to behaviour entirely.
    const first = pending[0];

    return (
        <div className="flex items-center gap-2">
            <button
                onClick={() => handleAll("accept")}
                disabled={!!busy}
                className="px-2 py-1 text-xs rounded border border-gray-900 bg-gray-900 text-white hover:bg-gray-800 disabled:opacity-50 inline-flex items-center gap-1"
            >
                {busy === "accept" && (
                    <Loader2 className="h-3 w-3 animate-spin" />
                )}
                Accept all
            </button>
            <button
                onClick={() => handleAll("reject")}
                disabled={!!busy}
                className="px-2 py-1 text-xs rounded border border-gray-200 bg-white text-gray-700 hover:bg-gray-100 disabled:opacity-50 inline-flex items-center gap-1"
            >
                {busy === "reject" && (
                    <Loader2 className="h-3 w-3 animate-spin" />
                )}
                Reject all
            </button>
            {progress && (
                <span className="text-xs font-serif text-gray-500">
                    {progress.done}/{progress.total}
                </span>
            )}
            {onViewClick && first && (
                <button
                    onClick={() =>
                        onViewClick(first.annotation, first.filename)
                    }
                    disabled={!!busy}
                    className="ml-auto px-2 py-1 text-xs rounded border border-gray-200 bg-white text-gray-700 hover:bg-gray-100 disabled:opacity-50"
                >
                    View
                </button>
            )}
        </div>
    );
}

/**
 * Wraps the bulk accept/reject card and the per-edit EditCards in a single
 * minimisable container. The bulk actions and summary stay visible in the
 * header; the individual cards collapse via the chevron toggle.
 */
function EditCardsSection({
    pending,
    filenameByDocId,
    cards,
    resolvedCount,
    onViewClick,
    onResolveStart,
    onResolved,
    onError,
}: {
    pending: {
        annotation: MikeEditAnnotation;
        filename: string;
    }[];
    filenameByDocId: Map<string, string>;
    cards: React.ReactNode[];
    resolvedCount: number;
    onViewClick?: (ann: MikeEditAnnotation, filename: string) => void;
    onResolveStart?: (args: {
        editId: string;
        documentId: string;
        verb: "accept" | "reject";
    }) => void;
    onResolved?: (args: {
        editId: string;
        documentId: string;
        status: "accepted" | "rejected";
        versionId: string | null;
        downloadUrl: string | null;
    }) => void;
    onError?: (args: {
        editId: string;
        documentId: string;
        versionId: string | null;
        message: string;
    }) => void;
}) {
    const [isOpen, setIsOpen] = useState(true);
    const tA = useTranslations("Assistant");
    if (cards.length === 0) return null;

    const docCount = filenameByDocId.size;
    const summary =
        pending.length > 0
            ? docCount > 1
                ? `${pending.length} tracked changes across ${docCount} documents`
                : `${pending.length} tracked ${pending.length === 1 ? "change" : "changes"}`
            : docCount > 1
              ? `${resolvedCount} resolved tracked changes across ${docCount} documents`
              : `${resolvedCount} resolved tracked ${resolvedCount === 1 ? "change" : "changes"}`;

    return (
        <div className="border border-gray-200 rounded-lg bg-white overflow-hidden">
            {/* Row 1: summary + chevron */}
            <div className="flex items-center gap-2 px-3 pt-3">
                <p className="flex-1 min-w-0 text-sm font-serif text-gray-700 truncate">
                    {summary}
                </p>
                <button
                    onClick={() => setIsOpen((v) => !v)}
                    aria-label={isOpen ? tA("collapseEdits") : tA("expandEdits")}
                    className="shrink-0 rounded p-1 text-gray-500 hover:bg-gray-100 hover:text-gray-800 transition-colors"
                >
                    <ChevronDown
                        className={`h-4 w-4 transition-transform duration-200 ${isOpen ? "" : "-rotate-90"}`}
                    />
                </button>
            </div>
            {/* Row 2: bulk action buttons */}
            {pending.length > 0 && (
                <div className="px-3 pt-3">
                    <BulkEditActions
                        pending={pending}
                        filenameByDocId={filenameByDocId}
                        onViewClick={onViewClick}
                        onResolveStart={onResolveStart}
                        onResolved={onResolved}
                        onError={onError}
                    />
                </div>
            )}
            {/* Row 3: collapsible cards list */}
            {isOpen && (
                <div className="flex flex-col gap-2 px-3 pb-3 pt-3">
                    {cards}
                </div>
            )}
            {!isOpen && <div className="pb-3" />}
        </div>
    );
}

// ---------------------------------------------------------------------------
// ResponseStatus
// ---------------------------------------------------------------------------

type StatusState = "active" | "error" | null;

function ResponseStatus({ status }: { status: StatusState }) {
    const [showDone, setShowDone] = useState(false);
    const [doneVisible, setDoneVisible] = useState(false);
    const wasActiveRef = useRef(false);

    const isActive = status === "active";
    const isError = status === "error";

    useEffect(() => {
        if (wasActiveRef.current && !isActive) {
            setShowDone(true);
            setDoneVisible(true);
            const t = setTimeout(() => setDoneVisible(false), 1500);
            return () => clearTimeout(t);
        } else if (!wasActiveRef.current && isActive) {
            setShowDone(false);
            setDoneVisible(false);
        }
        wasActiveRef.current = isActive;
    }, [isActive]);

    return (
        <div className="w-full h-9 flex items-center mb-2">
            <MikeIcon
                spin={isActive}
                done={showDone && doneVisible}
                error={isError}
                mike={!isError && !(showDone && doneVisible)}
                size={22}
            />
        </div>
    );
}

// ---------------------------------------------------------------------------
// Event block components
// ---------------------------------------------------------------------------

/** Picks one snippet and holds it stable for the lifecycle of this placeholder. */
function ThinkingPlaceholder({ showConnector }: { showConnector?: boolean }) {
    const snippet = useMemo(getRandomSnippet, []);
    return (
        <div className="flex items-center text-sm font-serif text-gray-500 relative">
            {showConnector && (
                <div className="absolute bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            <div className="w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
            <span className="ml-2">{snippet}</span>
        </div>
    );
}

/** Cycles through thinking snippets every 3s while a DOCX is being drafted. */
function DraftingPlaceholder({
    showConnector,
    elapsed,
}: {
    showConnector?: boolean;
    elapsed?: number;
}) {
    const [snippet, setSnippet] = useState(getRandomSnippet);

    useEffect(() => {
        const interval = setInterval(() => {
            setSnippet(getRandomSnippet());
        }, 3000);
        return () => clearInterval(interval);
    }, []);

    return (
        <div className="relative">
            {showConnector && (
                <div className="absolute bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            <div className="flex items-center text-sm font-serif text-gray-500">
                <div className="w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
                <span className="font-medium ml-2">Drafting your document</span>
                {typeof elapsed === "number" && elapsed > 0 && (
                    <span className="ml-2 text-xs text-gray-400 tabular-nums">
                        {elapsed}s
                    </span>
                )}
            </div>
            <div className="flex items-center text-sm font-serif text-gray-400 mt-1.5 ml-[14px]">
                <span className="italic">{snippet}</span>
            </div>
        </div>
    );
}

function ReasoningBlock({
    text,
    isStreaming,
    showConnector,
}: {
    text: string;
    isStreaming: boolean;
    showConnector?: boolean;
}) {
    const tA = useTranslations("Assistant");
    const [isOpen, setIsOpen] = useState(false);
    const [snippet, setSnippet] = useState(getRandomSnippet);

    useEffect(() => {
        if (!isStreaming) return;
        const interval = setInterval(() => {
            setSnippet(getRandomSnippet());
        }, 2000);
        return () => clearInterval(interval);
    }, [isStreaming]);

    const showContent = isOpen || isStreaming;

    return (
        <div className="relative">
            {showConnector && (
                <div className="absolute left-0 top-0 bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            <button
                onClick={() => !isStreaming && setIsOpen((v) => !v)}
                className="flex items-center text-sm font-serif text-gray-500 hover:text-gray-600 transition-colors"
            >
                {isStreaming ? (
                    <div className="w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
                ) : (
                    <div className="w-1.5 h-1.5 rounded-full bg-gray-300 shrink-0" />
                )}
                <span className="font-medium ml-2">
                    {isStreaming ? snippet : tA("thoughtProcess")}
                </span>
                {!isStreaming && (
                    <ChevronDown
                        size={10}
                        className={`ml-1 self-center transition-transform duration-200 ${isOpen ? "" : "-rotate-90"}`}
                    />
                )}
            </button>
            {showContent && (
                <div className="mt-2 ml-[14px] text-sm font-serif text-gray-400 prose prose-sm max-w-none [&>*]:text-gray-400 [&>*]:text-sm">
                    <ReactMarkdown
                        remarkPlugins={[remarkGfm]}
                        components={{
                            code: ({ node, ...props }) => (
                                <code
                                    className="font-serif text-gray-600"
                                    {...props}
                                />
                            ),
                        }}
                    >
                        {text}
                    </ReactMarkdown>
                </div>
            )}
        </div>
    );
}

function DocReadBlock({
    filename,
    onClick,
    showConnector,
    isStreaming,
}: {
    filename: string;
    onClick?: () => void;
    showConnector?: boolean;
    isStreaming?: boolean;
}) {
    const tA = useTranslations("Assistant");
    return (
        <div className="flex items-start text-sm font-serif text-gray-500 relative">
            {showConnector && (
                <div className="absolute bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            {isStreaming ? (
                <div className="mt-2 w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
            ) : (
                <div className="mt-2 w-1.5 h-1.5 rounded-full bg-green-400 shrink-0" />
            )}
            <div className="ml-2 min-w-0 flex-1 whitespace-normal break-words">
                <span className="font-medium">
                    {isStreaming ? tA("reading") : tA("read")}
                </span>{" "}
                {isStreaming ? (
                    <span>{filename}...</span>
                ) : onClick ? (
                    <button
                        onClick={onClick}
                        className="text-left hover:text-gray-700 transition-colors cursor-pointer"
                    >
                        {filename}
                    </button>
                ) : (
                    <span>{filename}</span>
                )}
            </div>
        </div>
    );
}

function DocFindBlock({
    filename,
    query,
    totalMatches,
    isStreaming,
    showConnector,
}: {
    filename: string;
    query: string;
    totalMatches: number;
    isStreaming?: boolean;
    showConnector?: boolean;
}) {
    const tA = useTranslations("Assistant");
    const label = isStreaming ? tA("finding") : tA("found");
    const matchSuffix = isStreaming
        ? ""
        : ` (${totalMatches} ${totalMatches === 1 ? "match" : "matches"})`;
    return (
        <div className="flex items-start text-sm font-serif text-gray-500 relative">
            {showConnector && (
                <div className="absolute bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            {isStreaming ? (
                <div className="mt-2 w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
            ) : (
                <div
                    className={`mt-2 w-1.5 h-1.5 rounded-full shrink-0 ${totalMatches > 0 ? "bg-green-400" : "bg-gray-300"}`}
                />
            )}
            <div className="ml-2 min-w-0 flex-1 whitespace-normal break-words">
                <span className="font-medium">{label}</span>{" "}
                <span>
                    &ldquo;{query}&rdquo;{matchSuffix}
                    <span className="ml-1 text-gray-400">in {filename}</span>
                    {isStreaming && "..."}
                </span>
            </div>
        </div>
    );
}

// Lawyerly, calm one-liners shown under the bundle timer so a long compile
// reads as work-in-progress, not a hang. No emoji, no exclamations (design rule).
const BUNDLE_THINKING = [
    "Cleaning up scanned pages so they merge without errors.",
    "Recomputing the index — page numbers shift as documents are added.",
    "Placing each annexure in the order you confirmed.",
    "Flattening malformed PDFs through the rendering engine.",
    "Stamping continuous page numbers across the bundle.",
];

function formatElapsed(seconds: number): string {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
}

/**
 * Live court-bundle compile card: a spinning Mike mark, the current stage, an
 * elapsed timer + soft remaining estimate, and a rotating "thinking" line —
 * font and format mirror the case-prep AnalysisStatsBar (text-[11px], gray-500,
 * tabular-nums, M:SS) inside the in-chat ToolActivityStream box style.
 */
function BundleProgressBlock({
    stage,
    stageCurrent,
    stageTotal,
    startedAt,
    showConnector,
}: {
    stage?: string;
    stageCurrent?: number;
    stageTotal?: number;
    startedAt?: number;
    showConnector?: boolean;
}) {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        const id = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(id);
    }, []);

    const elapsed = Math.max(0, Math.floor((now - (startedAt ?? now)) / 1000));
    const frac =
        stageCurrent && stageTotal ? ` — ${stageCurrent} of ${stageTotal}` : "";
    let remaining: number | null = null;
    if (stageCurrent && stageTotal && stageCurrent > 0 && elapsed > 1) {
        const per = elapsed / stageCurrent;
        remaining = Math.max(0, Math.round(per * (stageTotal - stageCurrent)));
    }
    const thinking =
        BUNDLE_THINKING[Math.floor(elapsed / 6) % BUNDLE_THINKING.length];

    return (
        <div className="flex items-start relative">
            {showConnector && (
                <div className="absolute left-0 top-0 bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            <div className="ml-2 flex-1 min-w-0 rounded-lg border border-gray-200 bg-white overflow-hidden">
                <div className="flex items-center gap-2 px-3 py-2.5 font-serif text-sm text-gray-600">
                    <MikeIcon spin size={14} />
                    <span className="font-medium">Compiling court bundle…</span>
                </div>
                <div className="px-3 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-gray-500 tabular-nums">
                    <span className="text-gray-700">
                        {stage ?? "Preparing"}
                        {frac}
                    </span>
                    <span className="text-gray-300">|</span>
                    <span>{formatElapsed(elapsed)} elapsed</span>
                    {remaining != null && (
                        <>
                            <span className="text-gray-300">|</span>
                            <span>
                                {remaining > 0
                                    ? `~${remaining}s remaining`
                                    : "Almost done…"}
                            </span>
                        </>
                    )}
                </div>
                <div className="px-3 pb-2.5 pt-1 text-[11px] italic text-gray-400">
                    {thinking}
                </div>
            </div>
        </div>
    );
}

function DocCreatedBlock({
    filename,
    showConnector,
    isStreaming,
}: {
    filename: string;
    showConnector?: boolean;
    isStreaming?: boolean;
}) {
    const tA = useTranslations("Assistant");
    return (
        <div className="flex items-start text-sm font-serif text-gray-500 relative">
            {showConnector && (
                <div className="absolute left-0 top-0 bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            {isStreaming ? (
                <div className="mt-2 w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
            ) : (
                <div className="mt-2 w-1.5 h-1.5 rounded-full bg-green-400 shrink-0" />
            )}
            <div className="ml-2 min-w-0 flex-1 whitespace-normal break-words">
                <span className="font-medium">
                    {isStreaming ? tA("creatingDoc") : tA("createdDoc")}
                </span>{" "}
                <span>{isStreaming ? `${filename}...` : filename}</span>
            </div>
        </div>
    );
}

function DocReplicatedBlock({
    filename,
    count,
    showConnector,
    isStreaming,
    hasError,
}: {
    filename: string;
    /**
     * How many consecutive replicates of this same source got collapsed
     * into this block. ≥ 1; only rendered when > 1.
     */
    count: number;
    showConnector?: boolean;
    isStreaming?: boolean;
    hasError?: boolean;
}) {
    const tA = useTranslations("Assistant");
    const label = isStreaming ? tA("replicating") : tA("replicated");
    const suffix =
        !isStreaming && count > 1 ? ` ${count} times` : isStreaming ? "..." : "";
    return (
        <div className="flex items-start text-sm font-serif text-gray-500 relative">
            {showConnector && (
                <div className="absolute left-0 top-0 bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            {isStreaming ? (
                <div className="mt-2 w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
            ) : (
                <div
                    className={`mt-2 w-1.5 h-1.5 rounded-full shrink-0 ${hasError ? "bg-red-400" : "bg-green-400"}`}
                />
            )}
            <div className="ml-2 min-w-0 flex-1 whitespace-normal break-words">
                <span className="font-medium">{label}</span>{" "}
                <span>
                    {filename}
                    {suffix}
                </span>
            </div>
        </div>
    );
}

function DocDownloadBlock({
    filename,
    download_url,
    onOpen,
    isReloading = false,
    versionNumber,
    isRendered = true,
    onRenderWord,
}: {
    filename: string;
    download_url: string;
    onOpen?: () => void;
    isReloading?: boolean;
    versionNumber?: number | null;
    isRendered?: boolean;
    onRenderWord?: () => void;
}) {
    const tA = useTranslations("Assistant");
    const hasVersion =
        typeof versionNumber === "number" &&
        Number.isFinite(versionNumber) &&
        versionNumber > 0;
    const extMatch = filename.match(/\.(\w+)$/);
    const ext = extMatch ? extMatch[1].toUpperCase() : "FILE";
    const rawBasename = extMatch
        ? filename.slice(0, -extMatch[0].length)
        : filename;
    // Strip any legacy "[Edited V3]" suffix that may still be baked into
    // older saved download filenames — the version is surfaced as a
    // separate tag now.
    const basename = rawBasename.replace(/\s*\[Edited V\d+\]\s*$/, "").trim();
    // Only backend-relative URLs are accepted. The download fetch carries
    // the user's bearer token, so any absolute URL from tool output is
    // refused to keep the token from leaking off-origin.
    const API_BASE =
        process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";
    const isSafeHref = download_url.startsWith("/");
    const href = isSafeHref ? `${API_BASE}${download_url}` : null;
    const [busy, setBusy] = useState(false);

    const handleDownload = async (e?: {
        stopPropagation?: () => void;
        preventDefault?: () => void;
    }) => {
        e?.stopPropagation?.();
        e?.preventDefault?.();
        if (busy || isReloading || !href) return;
        setBusy(true);
        try {
                        const token = typeof window !== "undefined" ? localStorage.getItem("mike_auth_token") : null;
            const resp = await fetch(href, {
                headers: token ? { Authorization: `Bearer ${token}` } : {},
            });
            if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
            const blob = await resp.blob();
            const blobUrl = URL.createObjectURL(blob);
            const a = document.createElement("a");
            a.href = blobUrl;
            a.download = filename;
            document.body.appendChild(a);
            a.click();
            a.remove();
            setTimeout(() => URL.revokeObjectURL(blobUrl), 1000);
        } finally {
            setBusy(false);
        }
    };

    const spinning = busy || isReloading;

    const body = (
        <div className="flex items-center gap-3 px-4 py-3 min-w-0 flex-1">
            <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2 min-w-0">
                    <p className="text-base font-serif text-gray-900 text-wrap">
                        {basename}
                    </p>
                    {hasVersion && (
                        <span className="shrink-0 inline-flex items-center rounded-md border border-gray-200 bg-white px-1.5 py-0.5 text-[10px] font-medium text-gray-500">
                            V{versionNumber}
                        </span>
                    )}
                </div>
                <p className="text-xs text-blue-500 mt-0.5">{ext}</p>
            </div>
        </div>
    );

    const actionIcon = spinning ? (
        <div
            aria-disabled
            className="shrink-0 flex items-center border-l border-gray-200 px-6 bg-white text-gray-400 cursor-not-allowed"
        >
            <Loader2 size={13} className="animate-spin" />
        </div>
    ) : isRendered ? (
        <button
            type="button"
            onClick={handleDownload}
            className="shrink-0 flex items-center border-l border-gray-200 px-6 bg-white text-gray-400 hover:bg-gray-100 hover:text-gray-600 transition-colors cursor-pointer"
        >
            <Download size={13} />
        </button>
    ) : (
        <button
            type="button"
            onClick={onRenderWord}
            disabled={!onRenderWord}
            className="shrink-0 flex items-center border-l border-gray-200 px-4 bg-white text-gray-700 hover:bg-blue-50 hover:text-blue-700 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed font-medium text-xs"
        >
            <FileText size={13} className="mr-1.5" />
            {tA("render")}
        </button>
    );

    if (onOpen) {
        return (
            <div className="flex items-stretch border border-gray-200 rounded-lg overflow-hidden w-full font-sans bg-gray-50">
                <button
                    type="button"
                    onClick={onOpen}
                    className="flex items-stretch flex-1 min-w-0 text-left hover:bg-gray-100 transition-colors cursor-pointer"
                >
                    {body}
                </button>
                {actionIcon}
            </div>
        );
    }

    if (spinning || !isRendered) {
        return (
            <div className="flex items-stretch border border-gray-200 rounded-lg overflow-hidden w-full font-sans bg-gray-50">
                {body}
                {actionIcon}
            </div>
        );
    }

    return (
        <div className="flex items-stretch border border-gray-200 rounded-lg overflow-hidden w-full font-sans bg-gray-50">
            <button
                type="button"
                onClick={handleDownload}
                className="flex items-stretch flex-1 min-w-0 text-left hover:bg-gray-100 transition-colors cursor-pointer"
            >
                {body}
            </button>
            {actionIcon}
        </div>
    );
}

function WorkflowAppliedBlock({
    title,
    showConnector,
    onClick,
}: {
    title: string;
    showConnector?: boolean;
    onClick?: () => void;
}) {
    return (
        <div className="flex items-start text-sm font-serif text-gray-500 relative">
            {showConnector && (
                <div className="absolute bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            <div className="mt-2 w-1.5 h-1.5 rounded-full bg-green-400 shrink-0" />
            <div className="ml-2 min-w-0 flex-1 whitespace-normal break-words">
                <span className="font-medium">Applied Workflow</span>{" "}
                {onClick ? (
                    <button
                        onClick={onClick}
                        className="text-left hover:text-gray-700 transition-colors cursor-pointer"
                    >
                        {title}
                    </button>
                ) : (
                    <span>{title}</span>
                )}
            </div>
        </div>
    );
}

function DocEditedBlock({
    filename,
    showConnector,
    isStreaming,
    hasError,
}: {
    filename: string;
    showConnector?: boolean;
    isStreaming?: boolean;
    hasError?: boolean;
}) {
    const tA = useTranslations("Assistant");
    return (
        <div className="flex items-start text-sm font-serif text-gray-500 relative">
            {showConnector && (
                <div className="absolute left-0 top-0 bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
            )}
            {isStreaming ? (
                <div className="mt-2 w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
            ) : hasError ? (
                <div className="mt-2 w-1.5 h-1.5 rounded-full bg-red-500 shrink-0" />
            ) : (
                <div className="mt-2 w-1.5 h-1.5 rounded-full bg-green-400 shrink-0" />
            )}
            <div className="ml-2 min-w-0 flex-1 whitespace-normal break-words">
                <span className="font-medium">
                    {isStreaming
                        ? tA("editing")
                        : hasError
                          ? tA("editFailed")
                          : tA("edited")}
                </span>{" "}
                <span>{isStreaming ? `${filename}...` : filename}</span>
            </div>
        </div>
    );
}

// ---------------------------------------------------------------------------
// Citation preprocessing
// ---------------------------------------------------------------------------

function preprocessCitations(
    text: string,
    annotations: MikeCitationAnnotation[],
    citationsList: MikeCitationAnnotation[],
): string {
    // Strip the <CITATIONS>…</CITATIONS> JSON block (the model emits it for
    // the backend; users shouldn't see raw JSON in their chat). NOT anchored
    // to end-of-string: the backend may append a "missing verbatim quote"
    // warning AFTER the block, which previously left it visible. Global so any
    // stray block is removed wherever it lands.
    text = text.replace(/<CITATIONS>[\s\S]*?<\/CITATIONS>/gi, "").trimEnd();

    // Inline citation markers come in three flavours:
    //   * `[1]`, `[2]`     — attached-document citations (ref-by-number).
    //   * `[g1]`, `[g2]`   — KB chunks from the global pool, ref-by-doc_id.
    //   * `[p1]`, `[p2]`   — KB chunks from a project pool.
    // The regex captures one or more such tokens separated by commas:
    // `[1, 2]`, `[g1, g3]`, `[1, g2]` are all valid.
    return text.replace(/\[((?:g|p)?\d+(?:,\s*(?:g|p)?\d+)*)\]/gi, (full, refsStr) => {
        const tokens = (refsStr as string)
            .split(",")
            .map((s) => s.trim())
            .flatMap((tok) => {
                const isAlpha = /^[gp]\d+$/i.test(tok);
                // Heuristic: pure numeric tokens with 4+ digits are
                // almost always years embedded in the prose ("2026"),
                // not citation refs. Skip them silently — they'd never
                // resolve anyway, and the warn-log would drown the
                // console for every such occurrence on every render.
                if (!isAlpha && tok.length >= 4) return [];
                const ann = isAlpha
                    ? annotations.find(
                          (a) =>
                              a.doc_id === tok.toLowerCase() &&
                              (a.source === "kb" || a.source === "tool"),
                      )
                    : annotations.find((a) => a.ref === parseInt(tok, 10));
                if (!ann) {
                    // No-op when the streaming hasn't yet delivered the
                    // citations event (annotations === []). The render
                    // re-runs after each content_delta, so warning every
                    // time would spam thousands of lines. The
                    // `[chat] citations received` log in useAssistantChat
                    // remains the authoritative signal.
                    return [];
                }
                // De-duplicate: reuse an existing pill number for the same
                // source instead of pushing a second copy. Otherwise the same
                // case cited twice renders as pills "1" and "2" that open the
                // same tab, so clicking the second looks like it does nothing.
                let idx = citationsList.findIndex(
                    (c) =>
                        c === ann ||
                        (c.ref === ann.ref &&
                            c.doc_id === ann.doc_id &&
                            c.source === ann.source),
                );
                if (idx < 0) {
                    idx = citationsList.length;
                    citationsList.push(ann); // close inline-citation push
                }
                return [`\`§${idx}§\`\u200B`];
            });
        return tokens.length > 0 ? tokens.join("") : full;
    });
}

// ---------------------------------------------------------------------------
// Markdown renderer (shared config)
// ---------------------------------------------------------------------------

function MarkdownContent({
    text,
    citationsList,
    onCitationClick,
    onIKLinkClick,
    divRef,
}: {
    text: string;
    citationsList: MikeCitationAnnotation[];
    onCitationClick?: (c: MikeCitationAnnotation) => void;
    onIKLinkClick?: (url: string, title: string, context: string) => void;
    divRef?: React.RefObject<HTMLDivElement | null>;
}) {
    // IK API agreement requires the "Powered by IKanoon" badge alongside any
    // results sourced from Indian Kanoon. Render once per message that cites IK.
    const hasIKContent = text.includes("indiankanoon.org");
    return (
        <div
            ref={divRef}
            className="text-gray-900 mb-4 text-base prose prose-sm max-w-none font-serif"
        >
            <ReactMarkdown
                remarkPlugins={[
                    [remarkMath, { singleDollarTextMath: false }],
                    remarkGfm,
                ]}
                rehypePlugins={[rehypeKatex]}
                components={{
                    table: ({ node, ...props }) => (
                        <div className="overflow-x-auto my-4">
                            <table
                                className="min-w-full divide-y divide-gray-300 border border-gray-200 rounded-lg overflow-hidden"
                                {...props}
                            />
                        </div>
                    ),
                    thead: ({ node, ...props }) => (
                        <thead className="bg-gray-50" {...props} />
                    ),
                    tbody: ({ node, ...props }) => (
                        <tbody
                            className="divide-y divide-gray-200 bg-white"
                            {...props}
                        />
                    ),
                    tr: ({ node, ...props }) => <tr {...props} />,
                    th: ({ node, ...props }) => (
                        <th
                            className="px-3 py-3.5 text-left text-sm font-semibold text-gray-900"
                            {...props}
                        />
                    ),
                    td: ({ node, ...props }) => (
                        <td
                            className="whitespace-normal px-3 py-4 text-sm text-gray-900"
                            {...props}
                        />
                    ),
                    h1: ({ node, ...props }) => (
                        <h1
                            className="mt-6 mb-4 text-3xl font-serif font-semibold"
                            {...props}
                        />
                    ),
                    h2: ({ node, ...props }) => (
                        <h2
                            className="mt-5 mb-3 text-2xl font-serif font-semibold"
                            {...props}
                        />
                    ),
                    h3: ({ node, ...props }) => (
                        <h3
                            className="text-xl font-semibold mt-4 mb-2"
                            {...props}
                        />
                    ),
                    h4: ({ node, ...props }) => (
                        <h4
                            className="text-lg font-semibold mt-4 mb-2"
                            {...props}
                        />
                    ),
                    p: ({ node, ...props }) => {
                        const parent = (node as any)?.parent;
                        if (parent?.type === "listItem") {
                            return (
                                <p
                                    className="inline leading-7 m-0"
                                    {...props}
                                />
                            );
                        }
                        return <p className="mb-4 leading-7" {...props} />;
                    },
                    ul: ({ node, ...props }) => (
                        <ul
                            className="list-disc list-outside mb-4 pl-6"
                            {...props}
                        />
                    ),
                    ol: ({ node, ...props }) => (
                        <ol
                            className="list-decimal list-outside mb-4 pl-6"
                            {...props}
                        />
                    ),
                    li: ({ node, ...props }) => (
                        <li className="mb-2 leading-7" {...props} />
                    ),
                    strong: ({ node, ...props }) => (
                        <strong className="font-semibold" {...props} />
                    ),
                    em: ({ node, ...props }) => (
                        <em className="italic" {...props} />
                    ),
                    code: ({ node, children, ...props }) => {
                        const text = String(children);
                        const citMatch = text.match(/^§(\d+)§$/);
                        if (citMatch) {
                            const idx = parseInt(citMatch[1]);
                            const annotation = citationsList[idx];
                            if (annotation) {
                                // KB citations come from the RAG retrieval — the
                                // tooltip surfaces the source path and chunk
                                // index instead of a page number, and the pill
                                // gets a distinct styling so the user can tell
                                // at a glance "this is from my library, not
                                // from the attached doc".
                                const isKb = annotation.source === "kb" || annotation.source === "tool";
                                const isVanga = annotation.source === "vanga";
                                const tooltipText = isVanga
                                    ? `${annotation.filename}: "${displayCitationQuote(annotation)}"`
                                    : isKb
                                    ? `${annotation.scope === "project" ? "Progetto" : "Libreria"} · ${annotation.filename}` +
                                      (annotation.chunk_index !== undefined
                                          ? ` (chunk ${annotation.chunk_index})`
                                          : "") +
                                      `: "${displayCitationQuote(annotation)}"`
                                    : `${formatCitationPage(annotation)}: "${displayCitationQuote(annotation)}"`;
                                const pillClass = isVanga
                                    ? "bg-amber-50 text-amber-700 hover:bg-amber-100 border border-amber-200"
                                    : isKb
                                    ? annotation.scope === "project"
                                        ? "bg-blue-50 text-blue-700 hover:bg-blue-100 border border-blue-200"
                                        : "bg-emerald-50 text-emerald-700 hover:bg-emerald-100 border border-emerald-200"
                                    : "bg-gray-100 text-gray-900 hover:bg-gray-200";
                                return (
                                    <button
                                        onClick={() => {
                                            console.log(
                                                "[AssistantMessage] citation clicked",
                                                annotation,
                                            );
                                            onCitationClick?.(annotation);
                                        }}
                                        className={`mx-0.5 inline-flex items-center justify-center rounded-full w-4 h-4 text-[10px] font-medium transition-colors align-super ${pillClass}`}
                                        title={tooltipText}
                                    >
                                        {idx + 1}
                                    </button>
                                );
                            }
                        }
                        return (
                            <code
                                className="bg-gray-100 px-1.5 py-0.5 rounded text-sm font-serif"
                                {...props}
                            >
                                {children}
                            </code>
                        );
                    },
                    blockquote: ({ node, ...props }) => (
                        <blockquote
                            className="border-l-4 border-gray-300 pl-4 italic my-4"
                            {...props}
                        />
                    ),
                    a: ({ node, href, children, ...props }) => {
                        const isIKLink = href && href.includes("indiankanoon.org");
                        const kanoonTid = isIKLink ? extractKanoonTid(href ?? undefined) : null;
                        const linkText = typeof children === "string"
                            ? children
                            : Array.isArray(children)
                              ? children.map((c) => (typeof c === "string" ? c : "")).join("")
                              : "Indian Kanoon";
                        const handleClick = (e: React.MouseEvent) => {
                            e.preventDefault();
                            if (!href) return;
                            if (isIKLink && onIKLinkClick) {
                                const linkEl = e.currentTarget as HTMLElement;
                                const paraEl = linkEl.closest("p, li, div, blockquote");
                                const paragraphText = paraEl?.textContent ?? linkText;
                                // The system prompt makes Mike write a verbatim quote
                                // after every citation, BUT the model often renders the
                                // quote as a separate Markdown blockquote rather than
                                // inline. So look for the verbatim quote in this order:
                                //   1. A nearby <blockquote> following the citation paragraph
                                //   2. A "..." quoted passage inside the citation paragraph
                                //   3. Fallback: the whole paragraph
                                let highlightTarget: string | null = null;

                                // (1) Look at next 1-3 siblings of the citation paragraph
                                // for a blockquote with substantial text. This catches
                                // Mike's pattern: "[citation]. summary. The snippet reads:
                                // \n\n> verbatim quote here"
                                let sibling: Element | null = paraEl?.nextElementSibling ?? null;
                                for (let i = 0; i < 3 && sibling; i++) {
                                    if (sibling.tagName === "BLOCKQUOTE") {
                                        const t = (sibling.textContent ?? "").trim();
                                        if (t.length >= 40) {
                                            highlightTarget = t;
                                            break;
                                        }
                                    }
                                    sibling = sibling.nextElementSibling;
                                }

                                // (2) Look for an inline “...” quote in the citation
                                // paragraph AND nearby siblings — the model often
                                // places the verbatim quote in a following paragraph.
                                if (!highlightTarget) {
                                    const textsToCheck = [paragraphText];
                                    let sib2: Element | null = paraEl?.nextElementSibling ?? null;
                                    for (let j = 0; j < 3 && sib2; j++) {
                                        textsToCheck.push((sib2.textContent ?? "").trim());
                                        sib2 = sib2.nextElementSibling;
                                    }
                                    for (const txt of textsToCheck) {
                                        const quoteMatch =
                                            txt.match(/[""]([^””]{30,400})[””]/) ||
                                            txt.match(/[‘’]([^’’]{30,400})[’’]/);
                                        if (quoteMatch) {
                                            highlightTarget = quoteMatch[1];
                                            break;
                                        }
                                    }
                                }

                                // (3) Fallback to the whole paragraph.
                                if (!highlightTarget) highlightTarget = paragraphText;

                                onIKLinkClick(href, linkText, highlightTarget);
                                return;
                            }
                            const url = href;
                            import("@tauri-apps/api/core").then((tauri) => {
                                tauri.invoke("open_external_url", { url }).catch(() => {
                                    window.open(url, "_blank", "noopener,noreferrer");
                                });
                            }).catch(() => {
                                window.open(url, "_blank", "noopener,noreferrer");
                            });
                        };
                        return (
                            <>
                                <a
                                    href={href}
                                    className="text-blue-600 hover:text-blue-700 underline cursor-pointer"
                                    onClick={handleClick}
                                    {...props}
                                >
                                    {children}
                                </a>
                                {kanoonTid !== null && (
                                    <KanoonVerifyBadge tid={kanoonTid} title={linkText} />
                                )}
                            </>
                        );
                    },
                    hr: ({ node, ...props }) => (
                        <hr className="my-6 border-gray-200" {...props} />
                    ),
                }}
            >
                {text}
            </ReactMarkdown>
            {hasIKContent && (
                <div className="mt-3 not-prose">
                    <PoweredByIKanoon />
                </div>
            )}
        </div>
    );
}

// ---------------------------------------------------------------------------
// Structured clarification question renderer for ask_clarifying_questions tool
// ---------------------------------------------------------------------------

function StructuredClarificationRenderer({
    event,
}: {
    event: Extract<AssistantEvent, { type: "clarification" }>;
}) {
    const [selectedByQuestion, setSelectedByQuestion] = useState<
        Record<number, string[]>
    >({});
    const [submitted, setSubmitted] = useState(false);
    const [error, setError] = useState(false);

    const handleOptionClick = (
        qi: number,
        label: string,
        multiSelect?: boolean,
    ) => {
        setSelectedByQuestion((prev) => {
            const current = prev[qi] ?? [];
            if (multiSelect) {
                // Toggle: add if not present, remove if present
                const next = current.includes(label)
                    ? current.filter((x) => x !== label)
                    : [...current, label];
                return { ...prev, [qi]: next };
            } else {
                // Single select: replace with this option
                return { ...prev, [qi]: [label] };
            }
        });
    };

    // proceed=true is the "Draft now" escape (no answers); proceed=false submits selections.
    const submit = async (proceed: boolean) => {
        setError(false);
        const apiBase = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";
        const token =
            typeof window !== "undefined"
                ? localStorage.getItem("mike_auth_token")
                : null;

        const answers = proceed
            ? []
            : event.questions.map((q, qi) => ({
                  question: q.text,
                  selected: selectedByQuestion[qi] ?? [],
              }));

        try {
            await fetch(`${apiBase}/chat/client-tool-result`, {
                method: "POST",
                headers: {
                    "Content-Type": "application/json",
                    ...(token ? { Authorization: `Bearer ${token}` } : {}),
                },
                body: JSON.stringify({
                    request_id: event.request_id,
                    result: JSON.stringify({ answers, proceed }),
                }),
            });
            setSubmitted(true);
        } catch (err) {
            console.error("[clarification] submit error:", err);
            setError(true);
        }
    };

    const isDisabled = submitted;

    // Once answered, collapse the whole form into a compact confirmation so
    // it doesn't linger as a live, clickable form above the draft.
    if (submitted) {
        const anySelected = event.questions.some(
            (_, qi) => (selectedByQuestion[qi] ?? []).length > 0,
        );
        return (
            <div className="mt-2 p-3 bg-gray-50 rounded-lg border border-gray-200">
                <p className="text-xs font-semibold text-gray-600">✓ Answered</p>
                {anySelected ? (
                    event.questions.map((q, qi) => {
                        const sel = selectedByQuestion[qi] ?? [];
                        if (sel.length === 0) return null;
                        return (
                            <p key={qi} className="text-xs text-gray-500 mt-1">
                                {q.header ? `${q.header}: ` : ""}
                                {sel.join(", ")}
                            </p>
                        );
                    })
                ) : (
                    <p className="text-xs text-gray-500 mt-1">
                        Proceeding with placeholders for any unknown details.
                    </p>
                )}
            </div>
        );
    }

    return (
        <div className="flex flex-col gap-4 mt-2 p-4 bg-gray-50 rounded-lg border border-gray-200">
            {event.questions.map((q, qi) => (
                <div key={qi} className="flex flex-col gap-2">
                    {q.header && (
                        <p className="text-xs font-bold text-gray-600 uppercase tracking-wide">
                            {q.header}
                        </p>
                    )}
                    <p className="text-sm font-medium text-gray-800">{q.text}</p>
                    {q.options && q.options.length > 0 && (
                        <div className="flex flex-col gap-2">
                            {q.options.map((opt) => {
                                const isSelected = (
                                    selectedByQuestion[qi] ?? []
                                ).includes(opt.label);
                                const isMulti = !!q.multiSelect;
                                return (
                                    <div key={opt.label} className="flex flex-col">
                                        <button
                                            disabled={isDisabled}
                                            onClick={() =>
                                                handleOptionClick(
                                                    qi,
                                                    opt.label,
                                                    isMulti,
                                                )
                                            }
                                            className={`relative px-3 py-2 text-sm rounded-md border transition-colors text-left ${
                                                isSelected
                                                    ? "bg-blue-100 border-blue-300 text-blue-900"
                                                    : "bg-white border-gray-300 text-gray-700 hover:bg-gray-50"
                                            } ${isDisabled ? "opacity-50 cursor-not-allowed" : ""}`}
                                        >
                                            <span>{opt.label}</span>
                                            {isMulti && isSelected && (
                                                <span className="ml-1 text-xs">
                                                    ✓
                                                </span>
                                            )}
                                        </button>
                                        {opt.description && (
                                            <p className="text-xs text-gray-500 mt-1 ml-1">
                                                {opt.description}
                                            </p>
                                        )}
                                    </div>
                                );
                            })}
                        </div>
                    )}
                </div>
            ))}
            <div className="flex gap-2 mt-2">
                <button
                    disabled={isDisabled}
                    onClick={() => submit(false)}
                    className="px-4 py-2 text-sm font-medium text-white bg-blue-600 rounded-md hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                >
                    Submit
                </button>
                <button
                    disabled={isDisabled}
                    onClick={() => submit(true)}
                    className="px-4 py-2 text-sm font-medium text-gray-700 bg-white border border-gray-300 rounded-md hover:bg-gray-50 disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
                >
                    Draft now
                </button>
            </div>
            {error && (
                <p className="text-xs text-red-600 mt-1">
                    Couldn't submit — please try again.
                </p>
            )}
        </div>
    );
}

// ---------------------------------------------------------------------------
// Main component
// ---------------------------------------------------------------------------

interface Props {
    content: string;
    events?: AssistantEvent[];
    isStreaming?: boolean;
    isError?: boolean;
    /** Human-readable error text rendered alongside the red Mike icon. */
    errorMessage?: string;
    annotations?: MikeCitationAnnotation[];
    onCitationClick?: (citation: MikeCitationAnnotation) => void;
    minHeight?: string;
    onWorkflowClick?: (workflowId: string) => void;
    onEditViewClick?: (ann: MikeEditAnnotation, filename: string) => void;
    /**
     * Opens the editor panel for a document without auto-highlighting any
     * specific edit. Used by the download card click — opening a doc to
     * read/download shouldn't jump the viewer to the first edit.
     */
    onOpenDocument?: (args: {
        documentId: string;
        filename: string;
        versionId: string | null;
        versionNumber: number | null;
        editableText?: string | null;
    }) => void;
    /**
     * Fires immediately when the user clicks Accept / Reject (single card
     * or the bulk "Accept all" / "Reject all"), before the backend call.
     * Parents use this to flip download cards / editor viewers into a
     * "saving" state for the duration of the round-trip.
     */
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
    isDocReloading?: (documentId: string) => boolean;
    /**
     * True while an accept/reject request for this specific edit is in
     * flight. Used to disable just that edit's Accept/Reject controls
     * (sibling edits on the same doc stay clickable).
     */
    isEditReloading?: (editId: string) => boolean;
    /**
     * External override for individual edit statuses. When present, an
     * EditCard looks up its edit_id here and treats the mapped value
     * ("accepted" / "rejected") as authoritative — used so bulk-resolved
     * edits flip their per-card UI without per-card clicks.
     */
    resolvedEditStatuses?: Record<string, "accepted" | "rejected">;
    onIKLinkClick?: (url: string, title: string, context: string) => void;
    onRenderWord?: (documentId: string) => void;
    docRenderedOverrides?: Record<string, string>;
    onSendMessage?: (text: string) => void;
}

export function AssistantMessage({
    content: _content,
    events,
    isStreaming = false,
    isError = false,
    errorMessage,
    annotations = [],
    onCitationClick,
    minHeight = "0px",
    onWorkflowClick,
    onEditViewClick,
    onOpenDocument,
    onEditResolveStart,
    onEditResolved,
    onEditError,
    isDocReloading,
    isEditReloading,
    resolvedEditStatuses,
    onIKLinkClick,
    onRenderWord,
    docRenderedOverrides,
    onSendMessage,
}: Props) {
    const messageKey = useId();
    const tA = useTranslations("Assistant");
    const contentDivRef = useRef<HTMLDivElement | null>(null);
    const [isCopied, setIsCopied] = useState(false);
    // Per-document override of the download URL, set as Accept/Reject resolves
    // each tracked change and produces a new version.
    const [resolvedOverrides, setResolvedOverrides] = useState<
        Record<string, string>
    >({});

    const handleEditResolved = (args: {
        editId: string;
        documentId: string;
        status: "accepted" | "rejected";
        versionId: string | null;
        downloadUrl: string | null;
    }) => {
        console.log("[AssistantMessage] handleEditResolved", args);
        if (args.downloadUrl) {
            setResolvedOverrides((prev) => ({
                ...prev,
                [args.documentId]: args.downloadUrl as string,
            }));
        }
        onEditResolved?.(args);
    };

    const status: StatusState = isError
        ? "error"
        : isStreaming
          ? "active"
          : null;

    // Pre-process citations for all content events. Each [N] marker resolves
    // to exactly one annotation (models are instructed to use shared refs
    // only for cross-page continuations via the [[PAGE_BREAK]] sentinel).
    const citationsList: MikeCitationAnnotation[] = [];
    const processedTexts: string[] = [];
    // Strip [INTAKE]...[/INTAKE] blocks — they're rendered as structured
    // clarification chips below, not as raw text in the message body.
    const stripIntakeBlock = (text: string): string => {
        return text
            .replace(/\[INTAKE\][\s\S]*?\[\/INTAKE\]/g, "")
            .replace(/\n{3,}/g, "\n\n")
            .trim();
    };
    if (events) {
        for (const event of events) {
            processedTexts.push(
                event.type === "content"
                    ? preprocessCitations(
                          stripIntakeBlock(event.text),
                          annotations,
                          citationsList,
                      )
                    : "",
            );
        }
    }
    const handleCopy = async () => {
        try {
            let html = "";
            let plainText = "";
            if (contentDivRef.current) {
                const clone = contentDivRef.current.cloneNode(
                    true,
                ) as HTMLElement;
                html = clone.innerHTML;
                plainText = clone.textContent || "";
            }
            const item = new ClipboardItem({
                "text/html": new Blob([html], { type: "text/html" }),
                "text/plain": new Blob([plainText], { type: "text/plain" }),
            });
            await navigator.clipboard.write([item]);
            setIsCopied(true);
            setTimeout(() => setIsCopied(false), 2000);
        } catch {
            // ignore
        }
    };

    // When the message produced (or is producing) a document, hide raw
    // streamed text — the user only wants the DOCX download card.
    // Show thinking snippets while the draft is being generated.
    const hasDocCreated = events?.some((e) => e.type === "doc_created") ?? false;
    const hasDocxToolCall = events?.some(
        (e) => e.type === "tool_call_start" && e.name === "generate_docx",
    ) ?? false;
    // Hide content when a DOCX is being created OR has been created
    const suppressContent = hasDocCreated || hasDocxToolCall;

    const lastContentIdx = events
        ? events.reduce(
              (last, e, idx) => (e.type === "content" ? idx : last),
              -1,
          )
        : -1;

    // Walk events in chronological order and group consecutive non-content
    // events into their own PreResponseWrapper. Content events render
    // between wrappers, so reasoning/tool chatter that arrives after the
    // model has already streamed some prose gets its own wrapper.
    type EventGroup =
        | { kind: "pre"; events: AssistantEvent[]; indices: number[] }
        | {
              kind: "content";
              event: Extract<AssistantEvent, { type: "content" }>;
              index: number;
          };

    const groups: EventGroup[] = [];
    if (events) {
        let current: Extract<EventGroup, { kind: "pre" }> | null = null;
        events.forEach((e, i) => {
            // Skip content events entirely when we're generating a DOCX —
            // the raw markdown is garbled and the user only wants the file.
            if (e.type === "content" && suppressContent) {
                return;
            }
            // Tool-call steps are rendered by ToolActivityStream (above), not
            // in a PreResponseWrapper here — skip them to avoid double-render.
            if (e.type === "tool_call_start") {
                return;
            }
            if (e.type === "content") {
                if (current) {
                    groups.push(current);
                    current = null;
                }
                groups.push({ kind: "content", event: e, index: i });
            } else {
                if (!current)
                    current = { kind: "pre", events: [], indices: [] };
                current.events.push(e);
                current.indices.push(i);
            }
        });
        if (current) groups.push(current);
    }

    const hasContentAfter = (groupIdx: number): boolean => {
        for (let i = groupIdx + 1; i < groups.length; i++) {
            const g = groups[i];
            if (g.kind === "content" && g.event.text.length > 0) return true;
        }
        return false;
    };

    const renderEvent = (
        event: AssistantEvent,
        i: number,
        allEvents: AssistantEvent[],
        globalIdx: number,
    ) => {
        const nextEvent = allEvents[i + 1];
        const showConnector =
            nextEvent !== undefined && nextEvent.type !== "content";

        if (event.type === "content") {
            const isLastContent = globalIdx === lastContentIdx;
            const processed = processedTexts[globalIdx];
            return (
                <div key={globalIdx}>
                    <MarkdownContent
                        text={processed}
                        citationsList={citationsList}
                        onCitationClick={onCitationClick}
                        divRef={isLastContent ? contentDivRef : undefined}
                    />
                </div>
            );
        }
        if (event.type === "reasoning") {
            return (
                <ReasoningBlock
                    key={globalIdx}
                    text={event.text}
                    isStreaming={!!event.isStreaming}
                    showConnector={showConnector}
                />
            );
        }
        if (event.type === "tool_call_start") {
            const elapsed = event.elapsedSecs ?? 0;
            const showSlowHint = elapsed >= 10;
            // For generate_docx, show cycling thinking snippets
            // instead of the generic "Running generate_docx..."
            if (event.name === "generate_docx") {
                return (
                    <DraftingPlaceholder
                        key={globalIdx}
                        showConnector={showConnector}
                        elapsed={elapsed}
                    />
                );
            }
            const toolLabel = event.name ? `${event.name}` : "tool";
            const displayLabel = event.progressLabel || `${tA("running")} ${toolLabel}...`;
            return (
                <div key={globalIdx} className="relative">
                    <div className="flex items-center text-sm font-serif text-gray-500">
                        {showConnector && (
                            <div className="absolute bottom-0 w-[1px] bg-gray-300 top-[13px] left-[2.5px] h-[calc(100%+11px)]" />
                        )}
                        <div className="w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
                        {event.progressLabel ? (
                            <span className="ml-2">{displayLabel}</span>
                        ) : (
                            <>
                                <span className="font-medium ml-2">{tA("running")}</span>
                                <span className="ml-1">{toolLabel}...</span>
                            </>
                        )}
                        {elapsed > 0 && !event.progressLabel && (
                            <span className="ml-2 text-xs text-gray-400 tabular-nums">
                                {elapsed}s
                            </span>
                        )}
                    </div>
                    {showSlowHint && (
                        <div className="ml-4 mt-1 text-xs text-amber-700 italic leading-snug">
                            {tA("slowToolHint")}
                        </div>
                    )}
                </div>
            );
        }
        if (event.type === "thinking") {
            return <ThinkingPlaceholder key={globalIdx} showConnector={showConnector} />;
        }
        if (event.type === "doc_read") {
            const ann = annotations.find((a) => a.filename === event.filename);
            return (
                <DocReadBlock
                    key={globalIdx}
                    filename={event.filename}
                    isStreaming={event.isStreaming}
                    onClick={
                        !event.isStreaming && ann && onCitationClick
                            ? () => onCitationClick(ann)
                            : undefined
                    }
                    showConnector={showConnector}
                />
            );
        }
        if (event.type === "doc_find") {
            return (
                <DocFindBlock
                    key={globalIdx}
                    filename={event.filename}
                    query={event.query}
                    totalMatches={event.total_matches}
                    isStreaming={!!event.isStreaming}
                    showConnector={showConnector}
                />
            );
        }
        if (event.type === "doc_created") {
            // While a court bundle is compiling, show the live timer/stage card
            // instead of the one-line "creating…" row.
            const isBundleCompiling =
                event.isStreaming &&
                (event.stage !== undefined ||
                    event.filename.includes("Court_Bundle"));
            if (isBundleCompiling) {
                return (
                    <BundleProgressBlock
                        key={globalIdx}
                        stage={event.stage}
                        stageCurrent={event.stageCurrent}
                        stageTotal={event.stageTotal}
                        startedAt={event.startedAt}
                        showConnector={showConnector}
                    />
                );
            }
            return (
                <DocCreatedBlock
                    key={globalIdx}
                    filename={event.filename}
                    isStreaming={event.isStreaming}
                    showConnector={showConnector}
                />
            );
        }
        if (event.type === "doc_replicated") {
            // The backend now does N copies in one tool call and reports
            // count + copies on a single event, so no consecutive-event
            // aggregation needed.
            return (
                <DocReplicatedBlock
                    key={globalIdx}
                    filename={event.filename}
                    count={event.count}
                    isStreaming={!!event.isStreaming}
                    hasError={!!event.error}
                    showConnector={showConnector}
                />
            );
        }
        if (event.type === "doc_edited") {
            return (
                <DocEditedBlock
                    key={globalIdx}
                    filename={event.filename}
                    isStreaming={event.isStreaming}
                    hasError={!!event.error}
                    showConnector={showConnector}
                />
            );
        }
        if (event.type === "workflow_applied") {
            return (
                <WorkflowAppliedBlock
                    key={globalIdx}
                    title={event.title}
                    showConnector={showConnector}
                    onClick={
                        onWorkflowClick
                            ? () => onWorkflowClick(event.workflow_id)
                            : undefined
                    }
                />
            );
        }
        if (event.type === "doc_download") {
            const API_BASE =
                process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";
            const isSafeHref = event.download_url.startsWith("/");
            const href = isSafeHref ? `${API_BASE}${event.download_url}` : null;

            const handleDownload = async () => {
                if (!href) return;
                try {
                    const token =
                        typeof window !== "undefined"
                            ? localStorage.getItem("mike_auth_token")
                            : null;
                    const resp = await fetch(href, {
                        headers: token ? { Authorization: `Bearer ${token}` } : {},
                    });
                    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
                    const blob = await resp.blob();
                    const blobUrl = URL.createObjectURL(blob);
                    const a = document.createElement("a");
                    a.href = blobUrl;
                    a.download = event.filename;
                    document.body.appendChild(a);
                    a.click();
                    a.remove();
                    setTimeout(() => URL.revokeObjectURL(blobUrl), 1000);
                } catch (err) {
                    console.error("[AssistantMessage] download error:", err);
                }
            };

            return (
                <DocumentCard
                    key={globalIdx}
                    filename={event.filename}
                    downloadUrl={event.download_url}
                    onDownload={handleDownload}
                />
            );
        }
        return null;
    };

    return (
        <div style={{ minHeight }}>
            <style>{MOTION_KEYFRAMES}</style>
            <ResponseStatus status={status} />
            <div className="w-full font-inter relative mt-2">
                {/* Tool activity stream — single owner of tool-call steps;
                    persists after streaming as a collapsed summary */}
                {events?.some((e) => e.type === "tool_call_start") && (
                    <div className="mb-4">
                        <ToolActivityStream events={events} isStreaming={isStreaming} />
                    </div>
                )}
                {events && events.length > 0 ? (
                    <div className="flex flex-col gap-4">
                        {/* Cold-state placeholder only: show the skeleton while
                            waiting for the first content token, but suppress it
                            once any reasoning/tool group ("Working…") is already
                            on screen — otherwise it's a redundant grey bar above
                            the indicator that already owns the waiting state. */}
                        {isStreaming && lastContentIdx < 0 && groups.length === 0 && (
                            <MessageSkeleton />
                        )}
                        {groups.map((g, gIdx) => {
                            if (g.kind === "content") {
                                const isLastContent =
                                    g.index === lastContentIdx;
                                return (
                                    <div
                                        key={`c-${g.index}`}
                                        style={{
                                            animation: "slideInUp 180ms ease-out backwards",
                                        }}
                                    >
                                        <MarkdownContent
                                            text={processedTexts[g.index]}
                                            citationsList={citationsList}
                                            onCitationClick={onCitationClick}
                                            onIKLinkClick={onIKLinkClick}
                                            divRef={
                                                isLastContent
                                                    ? contentDivRef
                                                    : undefined
                                            }
                                        />
                                    </div>
                                );
                            }
                            const subsequentContent = hasContentAfter(gIdx);
                            const wrapperIsStreaming = g.events.some(
                                (event) =>
                                    "isStreaming" in event &&
                                    !!event.isStreaming,
                            );
                            return (
                                <PreResponseWrapper
                                    key={`p-${g.indices[0]}`}
                                    stepCount={g.events.length}
                                    shouldMinimize={subsequentContent}
                                    isStreaming={wrapperIsStreaming}
                                >
                                    {g.events.map((event, i) =>
                                        renderEvent(
                                            event,
                                            i,
                                            g.events,
                                            g.indices[i],
                                        ),
                                    )}
                                </PreResponseWrapper>
                            );
                        })}
                        {/* Clarification chips — rendered directly without
                            PreResponseWrapper so they're always visible.
                            NOTE: Structured clarifications (with request_id) are now
                            rendered as a floating modal in ChatView, so we only render
                            the legacy [INTAKE] path here. */}
                        {events.filter((e) => e.type === "clarification").map((e, i) => {
                            if (e.type !== "clarification") return null;
                            // Structured clarifications (with request_id) render inline
                            // in the thread as the Concept B card. Legacy [INTAKE]
                            // events fall through to the quick-reply chips below.
                            if (e.request_id) {
                                return (
                                    <InlineClarification
                                        key={`clarify-${i}`}
                                        event={e}
                                    />
                                );
                            }
                            // Legacy [INTAKE] path with chips
                            return (
                                <div key={`clarify-${i}`} className="flex flex-col gap-3 mt-2">
                                    {e.questions.map((q, qi) => (
                                        <div key={qi}>
                                            <p className="text-sm font-medium text-gray-700 mb-1.5">{q.text}</p>
                                            <div className="flex flex-wrap gap-1.5">
                                                {(q.chips ?? []).map((chip) => (
                                                    <button
                                                        key={chip}
                                                        className="px-3 py-1 text-sm rounded-full border border-gray-300 bg-white text-gray-700 hover:bg-gray-100 hover:border-gray-400 transition-colors"
                                                        onClick={() => onSendMessage?.(chip)}
                                                    >
                                                        {chip}
                                                    </button>
                                                ))}
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            );
                        })}
                        {/* Bulk accept/reject + per-edit cards — below the
                            response content, only after streaming stops,
                            rendered above the download card. */}
                        {!isStreaming &&
                            (() => {
                                const editedEvents = events.filter(
                                    (e) =>
                                        e.type === "doc_edited" &&
                                        !e.isStreaming,
                                ) as Extract<
                                    AssistantEvent,
                                    { type: "doc_edited" }
                                >[];
                                const pending: {
                                    annotation: MikeEditAnnotation;
                                    filename: string;
                                }[] = [];
                                const filenameByDocId = new Map<
                                    string,
                                    string
                                >();
                                // Effective status = external override if any, else the annotation's DB status.
                                const statusOf = (ann: MikeEditAnnotation) =>
                                    resolvedEditStatuses?.[ann.edit_id] ??
                                    ann.status;
                                for (const e of editedEvents) {
                                    filenameByDocId.set(
                                        e.document_id,
                                        e.filename,
                                    );
                                    for (const ann of e.annotations) {
                                        if (statusOf(ann) === "pending") {
                                            pending.push({
                                                annotation: ann,
                                                filename: e.filename,
                                            });
                                        }
                                    }
                                }
                                const cards = editedEvents.flatMap((e) =>
                                    e.annotations.map((ann) => (
                                        <EditCard
                                            key={`editcard-${ann.edit_id}`}
                                            annotation={ann}
                                            resolvedStatus={
                                                resolvedEditStatuses?.[
                                                    ann.edit_id
                                                ]
                                            }
                                            isReloading={
                                                isEditReloading?.(
                                                    ann.edit_id,
                                                ) ?? false
                                            }
                                            onViewClick={(a) =>
                                                onEditViewClick?.(a, e.filename)
                                            }
                                            onResolveStart={onEditResolveStart}
                                            onResolved={handleEditResolved}
                                            onError={onEditError}
                                        />
                                    )),
                                );
                                const resolvedCount = editedEvents.reduce(
                                    (acc, e) =>
                                        acc +
                                        e.annotations.filter(
                                            (a) => statusOf(a) !== "pending",
                                        ).length,
                                    0,
                                );
                                // If there's only one edit total, skip the
                                // minimisable wrapper / bulk-actions UI and
                                // render the bare EditCard — no value in
                                // bulk controls for a single item.
                                if (cards.length <= 1) {
                                    return cards;
                                }
                                return (
                                    <EditCardsSection
                                        pending={pending}
                                        filenameByDocId={filenameByDocId}
                                        cards={cards}
                                        resolvedCount={resolvedCount}
                                        onViewClick={onEditViewClick}
                                        onResolveStart={onEditResolveStart}
                                        onResolved={handleEditResolved}
                                        onError={onEditError}
                                    />
                                );
                            })()}
                    </div>
                ) : null}

                {isError && (
                    <div className="mt-2 flex items-start gap-2 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm font-serif text-red-700">
                        <span className="leading-snug">
                            {errorMessage ?? tA("genericError")}
                        </span>
                    </div>
                )}

                {/* Download card for each edited doc — only after streaming
                    stops, and deduped per document (keep the latest edit). */}
                {events &&
                    !isStreaming &&
                    (() => {
                        const edited = events.filter(
                            (
                                e,
                            ): e is Extract<
                                AssistantEvent,
                                { type: "doc_edited" }
                            > =>
                                e.type === "doc_edited" &&
                                !e.isStreaming &&
                                !!e.download_url,
                        );
                        const latestByDoc = new Map<
                            string,
                            (typeof edited)[number]
                        >();
                        for (const e of edited)
                            latestByDoc.set(e.document_id, e);
                        return Array.from(latestByDoc.values()).map((e) => (
                            <div
                                key={`edited-download-${e.document_id}`}
                                className="flex flex-col gap-2 mt-2 mb-3"
                            >
                                <DocDownloadBlock
                                    filename={e.filename}
                                    download_url={
                                        resolvedOverrides[e.document_id] ??
                                        e.download_url
                                    }
                                    versionNumber={e.version_number ?? null}
                                    onOpen={
                                        onOpenDocument
                                            ? () =>
                                                  onOpenDocument({
                                                      documentId: e.document_id,
                                                      filename: e.filename,
                                                      versionId:
                                                          e.version_id ?? null,
                                                      versionNumber:
                                                          e.version_number ??
                                                          null,
                                                  })
                                            : onEditViewClick &&
                                                e.annotations[0]
                                              ? () =>
                                                    onEditViewClick(
                                                        e.annotations[0],
                                                        e.filename,
                                                    )
                                              : undefined
                                    }
                                    isReloading={
                                        isDocReloading?.(e.document_id) ?? false
                                    }
                                />
                            </div>
                        ));
                    })()}

                {/* Doc cards for created docs — show both rendered (download_url)
                    and draft (body) markdown. */}
                {events &&
                    !isStreaming &&
                    events.some(
                        (e) => e.type === "doc_created" && (e.download_url || e.body),
                    ) && (
                        <div className="flex flex-col gap-2 mt-2 mb-3">
                            {(
                                events.filter(
                                    (e) =>
                                        e.type === "doc_created" &&
                                        (e.download_url || e.body),
                                ) as Extract<
                                    AssistantEvent,
                                    { type: "doc_created" }
                                >[]
                            ).map((e, i) => {
                                const documentId = e.document_id;
                                const versionId = e.version_id ?? null;
                                const versionNumber = e.version_number ?? null;
                                const canOpen =
                                    !!onOpenDocument && !!documentId;
                                const overrideUrl = documentId
                                    ? docRenderedOverrides?.[documentId]
                                    : undefined;
                                const effectiveUrl =
                                    overrideUrl ?? e.download_url;
                                const isRendered = !!effectiveUrl;
                                return (
                                    <DocDownloadBlock
                                        key={i}
                                        filename={e.filename}
                                        download_url={effectiveUrl}
                                        versionNumber={versionNumber}
                                        isRendered={isRendered}
                                        onOpen={
                                            canOpen
                                                ? () =>
                                                      onOpenDocument!({
                                                          documentId:
                                                              documentId!,
                                                          filename: e.filename,
                                                          versionId,
                                                          versionNumber,
                                                          editableText:
                                                              e.body ?? null,
                                                      })
                                                : undefined
                                        }
                                        onRenderWord={
                                            canOpen &&
                                            !isRendered &&
                                            documentId &&
                                            onRenderWord
                                                ? () => onRenderWord(documentId)
                                                : undefined
                                        }
                                    />
                                );
                            })}
                        </div>
                    )}

                {/* Copy button */}
                <div className="flex items-center gap-2 pt-2 pb-4 md:pb-8 font-sans justify-start">
                    {!isStreaming && (
                        <button
                            className="p-1.5 rounded text-gray-500 hover:text-gray-700 hover:bg-gray-100"
                            onClick={handleCopy}
                        >
                            {isCopied ? (
                                <Check className="h-3.5 w-3.5 text-green-600" />
                            ) : (
                                <Copy className="h-3.5 w-3.5" />
                            )}
                        </button>
                    )}
                </div>
            </div>
        </div>
    );
}
