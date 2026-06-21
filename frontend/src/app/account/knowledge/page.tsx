"use client";

import { useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { Trash2, Upload } from "lucide-react";
import {
    listCorpusFiles,
    uploadCorpusFiles,
    processCorpusFiles,
    updateCorpusFile,
    deleteCorpusFile,
    type CorpusFile,
} from "@/app/lib/mikeApi";
import ReindexProgress from "@/app/components/shared/ReindexProgress";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

interface ProcessingEvent {
    type: "stage" | "done" | "error";
    file_id: string;
    stage?: string;
    chunk_count?: number;
    doc_type?: string;
    message?: string;
}

interface FileStatusMap {
    [fileId: string]: ProcessingEvent;
}

interface IngestProgress {
    total: number;
    indexed: number;
    startedAt: number;
    status: "running" | "done" | "failed";
    errorText?: string;
}

export default function KnowledgePage() {
    const t = useTranslations("Knowledge");
    const tCommon = useTranslations("Common");

    const [files, setFiles] = useState<CorpusFile[]>([]);
    const [loading, setLoading] = useState(true);
    const [uploading, setUploading] = useState(false);
    const [isTemplate, setIsTemplate] = useState(false);
    const [processingMap, setProcessingMap] = useState<FileStatusMap>({});
    const [ingest, setIngest] = useState<IngestProgress | null>(null);
    const fileInputRef = useRef<HTMLInputElement>(null);

    async function loadFiles() {
        try {
            const result = await listCorpusFiles();
            setFiles(result.files);
        } catch (err) {
            console.error("Failed to load corpus files:", err);
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        loadFiles();
    }, []);

    async function handleUpload(event: React.ChangeEvent<HTMLInputElement>) {
        const selectedFiles = Array.from(event.target.files ?? []);
        if (!selectedFiles.length) return;

        setUploading(true);
        try {
            const result = await uploadCorpusFiles(selectedFiles, isTemplate);

            if (result.duplicates.length > 0) {
                console.warn("Duplicates skipped:", result.duplicates);
            }

            if (result.accepted.length > 0) {
                setIngest({
                    total: result.accepted.length,
                    indexed: 0,
                    startedAt: Date.now(),
                    status: "running",
                });

                const response = await processCorpusFiles(result.accepted);

                if (!response.ok || !response.body) {
                    throw new Error("Failed to start processing");
                }

                const reader = response.body.getReader();
                const decoder = new TextDecoder();
                let buffer = "";

                while (true) {
                    const { done, value } = await reader.read();
                    if (done) break;

                    buffer += decoder.decode(value, { stream: true });
                    const lines = buffer.split("\n");
                    buffer = lines.pop() || "";

                    for (const line of lines) {
                        const trimmed = line.trim();
                        if (!trimmed || !trimmed.startsWith("data:")) continue;

                        const dataStr = trimmed.slice(5).trim();
                        if (dataStr === "[DONE]") {
                            continue;
                        }

                        try {
                            const event: ProcessingEvent = JSON.parse(dataStr);
                            setProcessingMap((prev) => ({
                                ...prev,
                                [event.file_id]: event,
                            }));
                            if (event.type === "done" && event.file_id) {
                                setIngest((prev) =>
                                    prev
                                        ? { ...prev, indexed: Math.min(prev.total, prev.indexed + 1) }
                                        : prev,
                                );
                            }
                        } catch {
                            console.error("Failed to parse SSE event:", trimmed);
                        }
                    }
                }

                setIngest((prev) => (prev ? { ...prev, status: "done" } : prev));
                await new Promise((resolve) => setTimeout(resolve, 500));
                await loadFiles();
                setProcessingMap({});
            }
        } catch (err) {
            console.error("Upload failed:", err);
            setIngest((prev) =>
                prev ? { ...prev, status: "failed", errorText: "Ingestion failed" } : prev,
            );
        } finally {
            setUploading(false);
            if (fileInputRef.current) fileInputRef.current.value = "";
            setIsTemplate(false);
        }
    }

    async function handleToggleTemplate(id: string, current: boolean) {
        try {
            await updateCorpusFile(id, { is_template: !current });
            await loadFiles();
        } catch (err) {
            console.error("Failed to update corpus file:", err);
        }
    }

    async function handleDelete(id: string) {
        try {
            await deleteCorpusFile(id);
            await loadFiles();
        } catch (err) {
            console.error("Failed to delete corpus file:", err);
        }
    }

    function getStatusColor(file: CorpusFile): string {
        if (file.status === "ready") return "bg-green-50 border-green-200";
        if (file.status === "failed" || file.status === "unsupported")
            return "bg-red-50 border-red-200";
        return "bg-amber-50 border-amber-200";
    }

    function getStatusBadgeColor(file: CorpusFile): string {
        if (file.status === "ready") return "bg-green-100 text-green-800";
        if (file.status === "failed" || file.status === "unsupported")
            return "bg-red-100 text-red-800";
        return "bg-amber-100 text-amber-800";
    }

    function getStatusText(status: string): string {
        const statusLabels: Record<string, string> = {
            pending: "Pending",
            extracting: "Extracting…",
            chunking: "Chunking…",
            tagging: "Tagging…",
            ready: "Ready",
            failed: "Failed",
            unsupported: "Unsupported",
        };
        return statusLabels[status] || status;
    }

    return (
        <div style={{ fontFamily: "var(--font-sans)", color: "var(--color-foreground)" }}>
            {/* Header */}
            <section style={{ marginBottom: 32 }}>
                <h1
                    style={{
                        fontFamily: "var(--font-serif)",
                        fontSize: 32,
                        fontWeight: 500,
                        margin: "0 0 8px",
                        letterSpacing: "-0.01em",
                    }}
                >
                    {t("heading")}
                </h1>
                <p
                    style={{
                        color: "var(--color-muted-foreground)",
                        fontSize: 15,
                        lineHeight: 1.6,
                        margin: "0 0 24px",
                        maxWidth: 560,
                    }}
                >
                    {t("description")}
                </p>
            </section>

            {/* Upload Zone */}
            <section
                style={{
                    border: "2px dashed var(--color-border)",
                    borderRadius: 12,
                    padding: 24,
                    marginBottom: 32,
                    background: "var(--card)",
                    textAlign: "center",
                }}
            >
                <div
                    onClick={() => fileInputRef.current?.click()}
                    style={{
                        cursor: "pointer",
                        display: "flex",
                        flexDirection: "column",
                        alignItems: "center",
                        gap: 12,
                    }}
                >
                    <Upload
                        size={32}
                        style={{ color: "var(--color-muted-foreground)" }}
                    />
                    <div>
                        <p
                            style={{
                                fontSize: 15,
                                fontWeight: 500,
                                margin: "0 0 4px",
                                color: "var(--color-foreground)",
                            }}
                        >
                            {t("uploadLabel")}
                        </p>
                        <p
                            style={{
                                fontSize: 13,
                                color: "var(--color-muted-foreground)",
                                margin: 0,
                            }}
                        >
                            PDF, DOCX, ODT, RTF, TXT
                        </p>
                    </div>
                </div>

                <input
                    ref={fileInputRef}
                    type="file"
                    multiple
                    accept=".pdf,.docx,.odt,.rtf,.txt"
                    onChange={handleUpload}
                    disabled={uploading || ingest?.status === "running"}
                    style={{ display: "none" }}
                />

                <div style={{ marginTop: 16, paddingTop: 16, borderTop: "1px solid var(--color-border)" }}>
                    <label
                        style={{
                            display: "flex",
                            alignItems: "center",
                            gap: 8,
                            fontSize: 14,
                            cursor: "pointer",
                        }}
                    >
                        <input
                            type="checkbox"
                            checked={isTemplate}
                            onChange={(e) => setIsTemplate(e.target.checked)}
                            disabled={uploading || ingest?.status === "running"}
                            style={{ cursor: "pointer" }}
                        />
                        <span>{t("templateCheckbox")}</span>
                    </label>
                </div>
            </section>

            {ingest && (
                <div style={{ marginBottom: 32 }}>
                    <ReindexProgress
                        indexed={ingest.indexed}
                        total={ingest.total}
                        status={ingest.status}
                        startedAt={ingest.startedAt}
                        errorText={ingest.errorText}
                        labels={{
                            embedded: "ingested",
                            unit: "drafts",
                            doneVerb: "Ingested",
                            failedLabel: "Ingestion failed",
                        }}
                    />
                </div>
            )}

            {/* Files List/Table */}
            {loading ? (
                <div
                    style={{
                        border: "1px solid var(--color-border)",
                        borderRadius: 12,
                        padding: 24,
                        textAlign: "center",
                        color: "var(--color-muted-foreground)",
                    }}
                >
                    {tCommon("loading")}
                </div>
            ) : files.length === 0 ? (
                <div
                    style={{
                        border: "1px solid var(--color-border)",
                        borderRadius: 12,
                        padding: 24,
                        textAlign: "center",
                        color: "var(--color-muted-foreground)",
                        fontSize: 14,
                    }}
                >
                    {t("emptyState")}
                </div>
            ) : (
                <div
                    style={{
                        border: "1px solid var(--color-border)",
                        borderRadius: 12,
                        overflow: "hidden",
                    }}
                >
                    <table
                        style={{
                            width: "100%",
                            borderCollapse: "collapse",
                            fontSize: 14,
                        }}
                    >
                        <thead>
                            <tr
                                style={{
                                    background: "var(--card)",
                                    borderBottom: "1px solid var(--color-border)",
                                }}
                            >
                                <th
                                    style={{
                                        textAlign: "left",
                                        padding: "12px 16px",
                                        fontWeight: 600,
                                        fontSize: 13,
                                        color: "var(--color-muted-foreground)",
                                        textTransform: "uppercase",
                                        letterSpacing: 0.5,
                                    }}
                                >
                                    {t("filename")}
                                </th>
                                <th
                                    style={{
                                        textAlign: "left",
                                        padding: "12px 16px",
                                        fontWeight: 600,
                                        fontSize: 13,
                                        color: "var(--color-muted-foreground)",
                                        textTransform: "uppercase",
                                        letterSpacing: 0.5,
                                    }}
                                >
                                    {t("type")}
                                </th>
                                <th
                                    style={{
                                        textAlign: "left",
                                        padding: "12px 16px",
                                        fontWeight: 600,
                                        fontSize: 13,
                                        color: "var(--color-muted-foreground)",
                                        textTransform: "uppercase",
                                        letterSpacing: 0.5,
                                    }}
                                >
                                    {t("status")}
                                </th>
                                <th
                                    style={{
                                        textAlign: "left",
                                        padding: "12px 16px",
                                        fontWeight: 600,
                                        fontSize: 13,
                                        color: "var(--color-muted-foreground)",
                                        textTransform: "uppercase",
                                        letterSpacing: 0.5,
                                    }}
                                >
                                    {t("chunks")}
                                </th>
                                <th
                                    style={{
                                        textAlign: "center",
                                        padding: "12px 16px",
                                        fontWeight: 600,
                                        fontSize: 13,
                                        color: "var(--color-muted-foreground)",
                                        textTransform: "uppercase",
                                        letterSpacing: 0.5,
                                    }}
                                >
                                    {t("template")}
                                </th>
                                <th
                                    style={{
                                        textAlign: "center",
                                        padding: "12px 16px",
                                        fontWeight: 600,
                                        fontSize: 13,
                                        color: "var(--color-muted-foreground)",
                                        textTransform: "uppercase",
                                        letterSpacing: 0.5,
                                    }}
                                >
                                    {tCommon("actions")}
                                </th>
                            </tr>
                        </thead>
                        <tbody>
                            {files.map((file) => (
                                <tr
                                    key={file.id}
                                    style={{
                                        borderBottom:
                                            "1px solid var(--color-border)",
                                    }}
                                >
                                    <td
                                        style={{
                                            padding: "12px 16px",
                                            wordBreak: "break-word",
                                            maxWidth: 200,
                                        }}
                                    >
                                        {file.filename}
                                    </td>
                                    <td
                                        style={{
                                            padding: "12px 16px",
                                        }}
                                    >
                                        <div
                                            style={{
                                                display: "flex",
                                                gap: 6,
                                                flexWrap: "wrap",
                                            }}
                                        >
                                            {file.doc_type && (
                                                <span
                                                    style={{
                                                        fontSize: 12,
                                                        background:
                                                            "rgba(0,0,0,0.06)",
                                                        padding: "4px 8px",
                                                        borderRadius: 4,
                                                        whiteSpace: "nowrap",
                                                    }}
                                                >
                                                    {file.doc_type}
                                                </span>
                                            )}
                                            {file.case_type && (
                                                <span
                                                    style={{
                                                        fontSize: 12,
                                                        background:
                                                            "rgba(0,0,0,0.06)",
                                                        padding: "4px 8px",
                                                        borderRadius: 4,
                                                        whiteSpace: "nowrap",
                                                    }}
                                                >
                                                    {file.case_type}
                                                </span>
                                            )}
                                            {file.court && (
                                                <span
                                                    style={{
                                                        fontSize: 12,
                                                        background:
                                                            "rgba(0,0,0,0.06)",
                                                        padding: "4px 8px",
                                                        borderRadius: 4,
                                                        whiteSpace: "nowrap",
                                                    }}
                                                >
                                                    {file.court}
                                                </span>
                                            )}
                                        </div>
                                    </td>
                                    <td
                                        style={{
                                            padding: "12px 16px",
                                        }}
                                    >
                                        <span
                                            style={{
                                                fontSize: 12,
                                                padding: "4px 8px",
                                                borderRadius: 4,
                                                fontWeight: 500,
                                                backgroundColor:
                                                    getStatusBadgeColor(file),
                                            }}
                                        >
                                            {getStatusText(file.status)}
                                        </span>
                                    </td>
                                    <td
                                        style={{
                                            padding: "12px 16px",
                                            fontSize: 13,
                                        }}
                                    >
                                        {file.chunk_count > 0
                                            ? `${file.chunk_count} chunks`
                                            : "—"}
                                    </td>
                                    <td
                                        style={{
                                            padding: "12px 16px",
                                            textAlign: "center",
                                        }}
                                    >
                                        <input
                                            type="checkbox"
                                            checked={file.is_template}
                                            onChange={() =>
                                                handleToggleTemplate(
                                                    file.id,
                                                    file.is_template
                                                )
                                            }
                                            style={{ cursor: "pointer" }}
                                        />
                                    </td>
                                    <td
                                        style={{
                                            padding: "12px 16px",
                                            textAlign: "center",
                                        }}
                                    >
                                        <button
                                            onClick={() =>
                                                handleDelete(file.id)
                                            }
                                            style={{
                                                background: "none",
                                                border: "none",
                                                cursor: "pointer",
                                                color: "var(--color-muted-foreground)",
                                                padding: 4,
                                                display: "inline-flex",
                                                alignItems: "center",
                                            }}
                                            title="Delete"
                                        >
                                            <Trash2 size={16} />
                                        </button>
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>
            )}
        </div>
    );
}
