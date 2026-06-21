"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslations } from "next-intl";
import ReactMarkdown from "react-markdown";
import remarkMath from "remark-math";
import remarkGfm from "remark-gfm";
import rehypeKatex from "rehype-katex";
import "katex/dist/katex.min.css";
import { Download, FileText, Loader2 } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { applyOptimisticResolution } from "../assistant/EditCard";
import { DocView } from "./DocView";
import { DocxView } from "./DocxView";
import {
    displayCitationQuote,
    expandCitationToEntries,
    formatCitationPage,
} from "./types";
import type {
    CitationQuote,
    MikeCitationAnnotation,
    MikeEditAnnotation,
} from "./types";

function isDocxFilename(name: string | null | undefined): boolean {
    if (!name || typeof name !== "string") return false;
    const ext = name.split(".").pop()?.toLowerCase();
    return ext === "docx" || ext === "doc";
}

/**
 * Discriminated-union describing what the panel is showing above the viewer.
 *   - "document":  no header card, no label — just the viewer.
 *   - "citation":  "Citation Quote" card with the quoted text and page ref.
 *   - "edit":      "Tracked Change" card with the diff + Accept/Reject.
 */
export type DocPanelMode =
    | { kind: "document" }
    | { kind: "citation"; citation: MikeCitationAnnotation }
    | {
          kind: "edit";
          edit: MikeEditAnnotation;
          /**
           * True while an accept/reject request for this exact edit is in
           * flight. Scoped per-edit (not per-document) so sibling edits on
           * the same doc stay clickable.
           */
          isEditReloading?: boolean;
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
      };

interface Props {
    documentId: string;
    filename: string;
    versionId: string | null;
    versionNumber: number | null;
    mode: DocPanelMode;
    /**
     * KB-indexed file path. When set, the underlying fetchers go through
     * `/sync/kb-doc?path=<kbPath>` instead of the upload-flow endpoints.
     * Used by RAG citation tabs — see `ChatView.openCitation`.
     * `documentId` is still required as a stable React key but is
     * effectively ignored by the network layer.
     */
    kbPath?: string | null;
    /** Spinner on the Download button while an accept/reject is in flight. */
    isReloading?: boolean;
    warning?: string | null;
    onWarningDismiss?: () => void;
    initialScrollTop?: number | null;
    onScrollChange?: (scrollTop: number) => void;
    /** Markdown source for draft documents; when set, renders a formatted markdown view. */
    editableText?: string | null;
    /** Called when the "Render Word" button is clicked to convert markdown to DOCX. */
    onRenderWord?: () => void;
}

/**
 * Unified side-panel body for the assistant. Renders a single document
 * with optionally a citation quote OR a tracked change highlighted above
 * the viewer. No selector UI — caller picks the one thing to show; if the
 * user wants a different citation/edit, the panel gets a new tab.
 */
export function DocPanel({
    documentId,
    filename,
    versionId,
    versionNumber,
    mode,
    kbPath,
    isReloading = false,
    warning,
    onWarningDismiss,
    initialScrollTop,
    onScrollChange,
    editableText,
    onRenderWord,
}: Props) {
    // Pick the viewer from the filename only, not from mode. Switching
    // headers (citation ↔ edit ↔ document) for the same document must
    // not unmount and remount the body — otherwise the user sees a full
    // re-fetch every time they toggle. Tracked-change rendering still
    // only lives in DocxView, which is fine because edits are DOCX-only.
    const tDV = useTranslations("DocViewer");
    const useDocxView = isDocxFilename(filename);

    const quotes: CitationQuote[] | undefined = useMemo(() => {
        if (mode.kind !== "citation") return undefined;
        return expandCitationToEntries(mode.citation);
    }, [mode]);

    const highlightEdit = useMemo(() => {
        if (mode.kind !== "edit") return null;
        return {
            key: `${mode.edit.edit_id}`,
            inserted_text: mode.edit.inserted_text,
            deleted_text: mode.edit.deleted_text,
            ins_w_id: mode.edit.ins_w_id ?? null,
            del_w_id: mode.edit.del_w_id ?? null,
        };
    }, [mode]);

    return (
        <div className="flex h-full flex-col px-3 pb-3">
            {mode.kind === "citation" ? (
                <CitationHeader
                    citation={mode.citation}
                    documentId={documentId}
                    versionId={versionId}
                    filename={filename}
                    kbPath={kbPath ?? null}
                    isReloading={isReloading}
                />
            ) : mode.kind === "edit" ? (
                <TrackedChangeHeader
                    mode={mode}
                    documentId={documentId}
                    versionId={versionId}
                    filename={filename}
                    kbPath={kbPath}
                    isReloading={isReloading}
                />
            ) : (
                <div className="flex items-center justify-end gap-2 py-2">
                    <div className="mr-auto flex min-w-0 items-center gap-2">
                        <span className="truncate text-sm text-gray-700">
                            {filename}
                        </span>
                        {versionNumber && versionNumber > 0 && (
                            <span className="shrink-0 inline-flex items-center rounded-md border border-gray-200 bg-white px-1.5 py-0.5 text-[10px] font-medium text-gray-600">
                                V{versionNumber}
                            </span>
                        )}
                    </div>
                    {editableText && onRenderWord && (
                        <button
                            onClick={onRenderWord}
                            disabled={isReloading}
                            className="shrink-0 border border-border rounded-md px-3 py-2 text-sm font-medium text-foreground hover:bg-blue-50 hover:text-blue-700 disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex items-center gap-1.5"
                        >
                            {isReloading ? (
                                <>
                                    <Loader2 size={13} className="animate-spin" />
                                    {tDV("rendering")}
                                </>
                            ) : (
                                <>
                                    <FileText size={13} />
                                    {tDV("renderWord")}
                                </>
                            )}
                        </button>
                    )}
                    {!editableText && (
                        <>
                            <OpenInWordButton
                                documentId={documentId}
                                kbPath={kbPath}
                            />
                            <DownloadButton
                                documentId={documentId}
                                versionId={versionId}
                                filename={filename}
                                kbPath={kbPath ?? undefined}
                                isReloading={isReloading}
                            />
                        </>
                    )}
                </div>
            )}

            {editableText ? (
                warning ? (
                    <div className="flex h-full min-h-0 flex-col">
                        <div className="flex items-start justify-between gap-2 border-b border-red-200 bg-red-50 px-4 py-2 text-sm text-red-700">
                            <span className="min-w-0">{warning}</span>
                            {onWarningDismiss && (
                                <button
                                    type="button"
                                    onClick={onWarningDismiss}
                                    className="shrink-0 text-red-400 hover:text-red-600"
                                    aria-label="Dismiss"
                                >
                                    ✕
                                </button>
                            )}
                        </div>
                        <div className="min-h-0 flex-1 overflow-hidden">
                            <MarkdownDocumentView markdown={editableText} />
                        </div>
                    </div>
                ) : (
                    <MarkdownDocumentView markdown={editableText} />
                )
            ) : useDocxView ? (
                <DocxView
                    documentId={documentId}
                    versionId={versionId ?? undefined}
                    kbPath={kbPath ?? undefined}
                    quotes={quotes}
                    highlightEdit={highlightEdit}
                    warning={warning ?? null}
                    onWarningDismiss={onWarningDismiss}
                    initialScrollTop={initialScrollTop ?? null}
                    onScrollChange={onScrollChange}
                />
            ) : (
                <DocView
                    doc={{
                        document_id: documentId,
                        version_id: versionId,
                    }}
                    kbPath={kbPath ?? undefined}
                    quotes={quotes}
                />
            )}
        </div>
    );
}

// ---------------------------------------------------------------------------
// Markdown document viewer
// ---------------------------------------------------------------------------

function MarkdownDocumentView({ markdown }: { markdown: string }) {
    return (
        <div className="flex-1 overflow-auto bg-white">
            <div className="text-foreground mb-4 text-base prose prose-sm max-w-none font-serif px-4 py-6">
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
                        code: ({ node, children, ...props }) => (
                            <code
                                className="bg-gray-100 px-1.5 py-0.5 rounded text-sm font-serif"
                                {...props}
                            >
                                {children}
                            </code>
                        ),
                        blockquote: ({ node, ...props }) => (
                            <blockquote
                                className="border-l-4 border-gray-300 pl-4 italic my-4"
                                {...props}
                            />
                        ),
                        a: ({ node, href, children, ...props }) => (
                            <a
                                href={href}
                                className="text-blue-600 hover:text-blue-700 underline cursor-pointer"
                                {...props}
                            >
                                {children}
                            </a>
                        ),
                        hr: ({ node, ...props }) => (
                            <hr className="my-6 border-gray-200" {...props} />
                        ),
                    }}
                >
                    {markdown}
                </ReactMarkdown>
            </div>
        </div>
    );
}

// ---------------------------------------------------------------------------
// Header variants
// ---------------------------------------------------------------------------

function SectionLabel({ children }: { children: React.ReactNode }) {
    return <p className="text-xs font-medium text-gray-700">{children}</p>;
}

function CitationHeader({
    citation,
    documentId,
    versionId,
    filename,
    kbPath,
    isReloading,
}: {
    citation: MikeCitationAnnotation;
    documentId: string;
    versionId: string | null;
    filename: string;
    kbPath?: string | null;
    isReloading: boolean;
}) {
    const tDV = useTranslations("DocViewer");
    const displayQuote = displayCitationQuote(citation);
    const pagesLabel = formatCitationPage(citation);
    return (
        <div className="pt-2 pb-3">
            <div className="flex items-center gap-2 mb-2">
                <SectionLabel>{tDV("citation")}</SectionLabel>
                <div className="ml-auto flex items-center gap-2 shrink-0">
                    <OpenInWordButton
                        documentId={documentId}
                        kbPath={kbPath}
                    />
                    <DownloadButton
                        documentId={documentId}
                        versionId={versionId}
                        filename={filename}
                        kbPath={kbPath ?? undefined}
                        isReloading={isReloading}
                    />
                </div>
            </div>
            <div className="w-full rounded-md bg-gray-50 border border-gray-200 px-2 py-2">
                <p className="text-sm font-serif text-gray-600">
                    &ldquo;{displayQuote}&rdquo;
                    {pagesLabel && (
                        <span className="ml-1 text-gray-400">
                            ({pagesLabel})
                        </span>
                    )}
                </p>
            </div>
        </div>
    );
}

function TrackedChangeHeader({
    mode,
    documentId,
    versionId,
    filename,
    kbPath,
    isReloading,
}: {
    mode: Extract<DocPanelMode, { kind: "edit" }>;
    documentId: string;
    versionId: string | null;
    filename: string;
    kbPath?: string | null;
    isReloading: boolean;
}) {
    const tDV = useTranslations("DocViewer");
    const { edit, isEditReloading, onResolveStart, onResolved, onError } = mode;
    return (
        <div className="pt-2 pb-3">
            <div className="flex items-center gap-2 mb-2">
                <SectionLabel>{tDV("trackedChange")}</SectionLabel>
                <div className="ml-auto flex items-center gap-2 shrink-0">
                    <OpenInWordButton
                        documentId={documentId}
                        kbPath={kbPath ?? undefined}
                    />
                    <EditResolveButtons
                        edit={edit}
                        isReloading={isEditReloading}
                        onResolveStart={onResolveStart}
                        onResolved={onResolved}
                        onError={onError}
                    />
                    <DownloadButton
                        documentId={documentId}
                        versionId={versionId}
                        filename={filename}
                        isReloading={isReloading}
                    />
                </div>
            </div>
            {edit.reason && (
                <p className="mb-2 text-xs text-gray-500">{edit.reason}</p>
            )}
            <div className="w-full rounded-md bg-gray-50 border border-gray-200 px-2 py-2">
                <div className="text-sm leading-relaxed font-serif">
                    {edit.inserted_text && (
                        <span className="text-green-700">
                            {edit.inserted_text}
                        </span>
                    )}
                    {edit.deleted_text && (
                        <span className="text-red-600 line-through">
                            {edit.deleted_text}
                        </span>
                    )}
                </div>
            </div>
        </div>
    );
}

// ---------------------------------------------------------------------------
// Accept / Reject controls
// ---------------------------------------------------------------------------

function EditResolveButtons({
    edit,
    isReloading,
    onResolveStart,
    onResolved,
    onError,
}: {
    edit: MikeEditAnnotation;
    /**
     * True while an accept/reject for any edit on this document is in
     * flight (triggered from here, the inline EditCard, the bulk bar, or
     * elsewhere). Disables both buttons so the user can't double-submit
     * while a resolution is racing to change the status.
     */
    isReloading?: boolean;
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
    const [busy, setBusy] = useState(false);
    const [status, setStatus] = useState<"pending" | "accepted" | "rejected">(
        edit.status,
    );
    // Sync with the prop when this edit is resolved elsewhere (bulk
    // accept/reject, inline per-edit card, another open panel for the same
    // edit). Skips while our own request is in flight so we don't flicker.
    useEffect(() => {
        if (busy) return;
        setStatus(edit.status);
    }, [edit.status, edit.edit_id, busy]);
    const resolved = status !== "pending";

    const handle = useCallback(
        async (verb: "accept" | "reject") => {
            if (busy || resolved) return;
            setBusy(true);
            onResolveStart?.({
                editId: edit.edit_id,
                documentId: edit.document_id,
                verb,
            });
            // Optimistically mutate the DOM in the open viewer so the
            // change reflects immediately. Revert if the backend errors.
            let revert: (() => void) | null = null;
            try {
                revert = applyOptimisticResolution(edit, verb);
            } catch (e) {
                console.error(
                    "[DocPanel] optimistic update threw",
                    e,
                );
            }
            try {
                                const token = typeof window !== "undefined" ? localStorage.getItem("mike_auth_token") : null;
                const apiBase =
                    process.env.NEXT_PUBLIC_API_BASE_URL ??
                    "http://localhost:3001";
                const resp = await fetch(
                    `${apiBase}/single-documents/${edit.document_id}/edits/${edit.edit_id}/${verb}`,
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
                setStatus(nextStatus);
                onResolved?.({
                    editId: edit.edit_id,
                    documentId: edit.document_id,
                    status: nextStatus,
                    versionId: data.version_id,
                    downloadUrl: data.download_url,
                });
            } catch (e) {
                console.error("[DocPanel] resolve failed", e);
                try {
                    revert?.();
                } catch (revertErr) {
                    console.error(
                        "[DocPanel] revert threw",
                        revertErr,
                    );
                }
                onError?.({
                    editId: edit.edit_id,
                    documentId: edit.document_id,
                    versionId: edit.version_id ?? null,
                    message:
                        verb === "accept"
                            ? "Couldn't save accept — please retry."
                            : "Couldn't save reject — please retry.",
                });
            } finally {
                setBusy(false);
            }
        },
        [busy, resolved, edit, onResolveStart, onResolved, onError],
    );

    const inFlight = busy || !!isReloading;
    return (
        <div className="flex items-center gap-2">
            <button
                onClick={() => handle("accept")}
                disabled={inFlight || resolved}
                className="inline-flex items-center gap-1 rounded-lg border border-gray-900 bg-gray-900 px-2 py-1.5 text-xs font-medium text-white hover:bg-gray-800 disabled:opacity-50 disabled:cursor-not-allowed"
            >
                {status === "accepted" ? "Accepted" : "Accept"}
            </button>
            <button
                onClick={() => handle("reject")}
                disabled={inFlight || resolved}
                className="inline-flex items-center gap-1 rounded-lg border border-gray-200 bg-white px-2 py-1.5 text-xs font-medium text-gray-700 hover:bg-gray-100 disabled:opacity-50 disabled:cursor-not-allowed"
            >
                {status === "rejected" ? "Rejected" : "Reject"}
            </button>
        </div>
    );
}

// ---------------------------------------------------------------------------
// Open in Word button
// ---------------------------------------------------------------------------

function OpenInWordButton({
    documentId,
    kbPath,
}: {
    documentId: string;
    kbPath?: string | null;
}) {
    const [busy, setBusy] = useState(false);

    const handleClick = async () => {
        if (busy) return;
        setBusy(true);
        try {
            if (kbPath) {
                // RAG citation doc — file already on disk at kbPath.
                // Shell out via the Tauri IPC command.
                await invoke("open_file_in_word", { path: kbPath });
            } else {
                // Upload-flow doc — use the existing backend route.
                const token =
                    typeof window !== "undefined"
                        ? localStorage.getItem("mike_auth_token")
                        : null;
                const apiBase =
                    process.env.NEXT_PUBLIC_API_BASE_URL ??
                    "http://localhost:3001";
                const resp = await fetch(`${apiBase}/desktop/open-word`, {
                    method: "POST",
                    headers: {
                        "Content-Type": "application/json",
                        ...(token
                            ? { Authorization: `Bearer ${token}` }
                            : {}),
                    },
                    body: JSON.stringify({ document_id: documentId }),
                });
                if (!resp.ok) {
                    const err = await resp.json().catch(() => ({}));
                    throw new Error(
                        err.detail ?? `HTTP ${resp.status}`,
                    );
                }
            }
        } catch (e) {
            console.error("[DocPanel] open in word failed", e);
        } finally {
            setBusy(false);
        }
    };

    return (
        <button
            onClick={handleClick}
            disabled={busy}
            className="inline-flex items-center gap-1 rounded-lg border border-gray-200 px-2 py-1.5 text-xs font-medium text-gray-600 hover:bg-gray-100 hover:text-gray-800 disabled:opacity-50 disabled:cursor-not-allowed"
        >
            {busy ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
                <FileText className="h-3.5 w-3.5" />
            )}
            Open in Word
        </button>
    );
}

// ---------------------------------------------------------------------------
// Download button
// ---------------------------------------------------------------------------

function DownloadButton({
    documentId,
    versionId,
    filename,
    kbPath,
    isReloading,
}: {
    documentId: string;
    versionId: string | null;
    filename: string;
    /** When set, download via /sync/kb-doc instead of the upload flow. */
    kbPath?: string | null;
    isReloading?: boolean;
}) {
    const [busy, setBusy] = useState(false);

    const handleClick = async () => {
        if (busy || isReloading) return;
        setBusy(true);
        try {
                        const token = typeof window !== "undefined" ? localStorage.getItem("mike_auth_token") : null;
            const apiBase =
                process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";
            const url = kbPath
                ? `${apiBase}/sync/kb-doc?path=${encodeURIComponent(kbPath)}`
                : (() => {
                      const qs = versionId
                          ? `?version_id=${encodeURIComponent(versionId)}`
                          : "";
                      return `${apiBase}/single-documents/${documentId}/docx${qs}`;
                  })();
            const resp = await fetch(url, {
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
    return (
        <button
            onClick={handleClick}
            disabled={spinning}
            className="inline-flex items-center gap-1 rounded-lg border border-gray-200 px-2 py-1.5 text-xs font-medium text-gray-600 hover:bg-gray-100 hover:text-gray-800 disabled:opacity-50 disabled:cursor-not-allowed"
        >
            {spinning ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
                <Download className="h-3.5 w-3.5" />
            )}
            Download
        </button>
    );
}
