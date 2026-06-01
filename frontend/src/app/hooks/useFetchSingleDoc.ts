"use client";

import { useEffect, useRef, useState } from "react";

/**
 * /display returns either PDF bytes (when the active version has a PDF
 * rendition) or raw DOCX bytes otherwise. Reporting the type lets the
 * caller swap between DocView (PDF.js) and DocxView (docx-preview)
 * accordingly.
 */
export type DocResult =
    | { type: "pdf"; buffer: ArrayBuffer }
    | { type: "text"; text: string }
    | { type: "docx" }
    | null;

/**
 * Fetch a document's display-time bytes.
 *
 * Two source modes (mutually exclusive — pass exactly one):
 *  - `documentId` set → upload-flow document, hits `/display`. The
 *    backend returns PDF when a rendition exists (DocView consumes it),
 *    otherwise signals "docx" so the caller falls back to DocxView.
 *  - `kbPath` set → KB-indexed document, hits `/sync/kb-doc`. The
 *    backend serves the raw file with the actual content-type:
 *    application/pdf for .pdf, the DOCX MIME for .docx, etc. We
 *    branch on content-type the same way to feed DocView vs DocxView.
 */
export function useFetchSingleDoc(
    documentId: string | null | undefined,
    versionId?: string | null,
    kbPath?: string | null,
) {
    const [result, setResult] = useState<DocResult>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // Track the in-flight key + AbortController so re-runs of the
    // effect (StrictMode double-mount, parent re-render, etc.) cancel
    // the previous request rather than racing it. We DON'T early-return
    // on duplicate keys: if the component unmounts mid-fetch, `cancelled`
    // suppresses the setState but `loading` stays true; the next mount
    // must be allowed to re-run the fetch to drive `loading` back to
    // false. The earlier prevKeyRef-based dedupe broke exactly that
    // path and produced the infinite spinner on KB citations.
    const inflightAbortRef = useRef<AbortController | null>(null);

    const primaryKey = kbPath ?? documentId ?? null;

    useEffect(() => {
        if (!primaryKey) return;
        // Cancel any prior in-flight fetch from this hook instance.
        inflightAbortRef.current?.abort();
        const controller = new AbortController();
        inflightAbortRef.current = controller;

        setLoading(true);
        setError(null);
        setResult(null);

        let cancelled = false;

        (async () => {
            try {
                const token =
                    typeof window !== "undefined"
                        ? localStorage.getItem("mike_auth_token")
                        : null;
                if (cancelled) return;

                const apiBase =
                    process.env.NEXT_PUBLIC_API_BASE_URL ??
                    "http://localhost:3001";
                const url = kbPath
                    ? `${apiBase}/sync/kb-doc?path=${encodeURIComponent(kbPath)}`
                    : (() => {
                          const qs = versionId
                              ? `?version_id=${encodeURIComponent(versionId)}`
                              : "";
                          return `${apiBase}/single-documents/${documentId}/display${qs}`;
                      })();
                console.log(
                    `[fetch-doc] GET ${url} (kbPath=${kbPath ? "set" : "no"}, doc=${documentId ?? "?"})`,
                );
                const t0 = performance.now();
                const response = await fetch(url, {
                    headers: token
                        ? { Authorization: `Bearer ${token}` }
                        : {},
                    signal: controller.signal,
                });
                console.log(
                    `[fetch-doc] response status=${response.status} content-type=${response.headers.get("content-type")} ttfb=${Math.round(performance.now() - t0)}ms`,
                );
                if (!response.ok) throw new Error(`HTTP ${response.status}`);
                if (cancelled) return;

                const contentType =
                    response.headers.get("content-type") ?? "";
                if (contentType.includes("application/pdf")) {
                    const buffer = await response.arrayBuffer();
                    if (!cancelled) setResult({ type: "pdf", buffer });
                } else if (
                    contentType.includes("text/plain") ||
                    contentType.includes("text/html") ||
                    contentType.startsWith("text/")
                ) {
                    // Plain-text sources — e.g. cached Indian Kanoon judgments
                    // (.txt). Render as text, NOT through docx-preview (which
                    // throws "Can't find end of central directory" on non-zip
                    // bytes).
                    const text = await response.text();
                    if (!cancelled) setResult({ type: "text", text });
                } else {
                    // Drain the body so the connection is reusable, but the
                    // bytes are useless to the PDF viewer — the caller will
                    // fall back to DocxView, which fetches /docx (or
                    // /sync/kb-doc) itself.
                    await response.arrayBuffer().catch(() => {});
                    if (!cancelled) setResult({ type: "docx" });
                }
            } catch (e) {
                // AbortError is expected when the effect re-runs; silently
                // drop it. Real errors still bubble up to the UI.
                if ((e as { name?: string })?.name === "AbortError") {
                    return;
                }
                if (!cancelled) setError("Failed to load document.");
            } finally {
                if (!cancelled) setLoading(false);
            }
        })();

        return () => {
            cancelled = true;
            controller.abort();
        };
    }, [documentId, versionId, kbPath, primaryKey]);

    return { result, loading, error };
}
