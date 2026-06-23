"use client";

import { useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { Trash2, Upload, FolderOpen } from "lucide-react";
import {
    listCorpusFiles,
    uploadCorpusFiles,
    processCorpusFiles,
    updateCorpusFile,
    deleteCorpusFile,
    deleteCorpusBatch,
    getCorpusLimits,
    type CorpusFile,
    type CorpusLimits,
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

interface BatchState {
    batchId: string;
    batchLabel: string;
    total: number;
    startedAt: number;
}

interface PreflightFile {
    file: File;
    supported: boolean;
    reason?: string;
}

interface SkippedItem {
    filename: string;
    reason: string;
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
    const folderInputRef = useRef<HTMLInputElement>(null);
    const [limits, setLimits] = useState<CorpusLimits | null>(null);
    const [preflightFiles, setPreflightFiles] = useState<PreflightFile[]>([]);
    const [preflightOpen, setPreflightOpen] = useState(false);
    const [uploadingBatch, setUploadingBatch] = useState(false);
    const [skippedItems, setSkippedItems] = useState<SkippedItem[]>([]);
    const [activeBatchId, setActiveBatchId] = useState<string | null>(null);
    const pollIntervalRef = useRef<NodeJS.Timeout | null>(null);
    const pollRunIdRef = useRef(0);

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

    async function fetchLimits() {
        if (limits) return limits;
        try {
            const l = await getCorpusLimits();
            setLimits(l);
            return l;
        } catch (err) {
            console.error("Failed to load corpus limits:", err);
            return null;
        }
    }

    async function collectEntries(
        items: DataTransferItemList,
    ): Promise<File[]> {
        const files: File[] = [];

        async function walkEntries(
            entries: FileSystemEntry[],
        ): Promise<void> {
            for (const entry of entries) {
                if (entry.isFile) {
                    const fe = entry as FileSystemFileEntry;
                    const file = await new Promise<File>((resolve) => {
                        fe.file((f) => resolve(f));
                    });
                    files.push(file);
                } else if (entry.isDirectory) {
                    const de = entry as FileSystemDirectoryEntry;
                    const reader = de.createReader();
                    // readEntries yields at most 100 entries per call, so a
                    // directory with more children must be drained in a loop
                    // until it returns empty. Reading once would silently drop
                    // every file past the first 100.
                    let chunk: FileSystemEntry[];
                    do {
                        chunk = await new Promise<FileSystemEntry[]>(
                            (resolve) => {
                                reader.readEntries(resolve, () => resolve([]));
                            },
                        );
                        await walkEntries(chunk);
                    } while (chunk.length > 0);
                }
            }
        }

        const entries: FileSystemEntry[] = [];
        for (let i = 0; i < items.length; i++) {
            const item = items[i];
            const entry = item.webkitGetAsEntry();
            if (entry) entries.push(entry);
        }
        await walkEntries(entries);
        return files;
    }

    function computePreflightFiles(
        files: File[],
        limits: CorpusLimits,
    ): PreflightFile[] {
        return files.map((file) => {
            const ext = file.name.split(".").pop()?.toLowerCase() || "";
            if (!limits.supported_exts.includes(ext)) {
                return {
                    file,
                    supported: false,
                    reason: `unsupported type (.${ext})`,
                };
            }
            if (file.size > limits.max_file_bytes) {
                const sizeMb = (file.size / (1024 * 1024)).toFixed(1);
                const limitMb = (limits.max_file_bytes / (1024 * 1024)).toFixed(
                    1,
                );
                return {
                    file,
                    supported: false,
                    reason: `file is ${sizeMb} MB, limit is ${limitMb} MB`,
                };
            }
            return { file, supported: true };
        });
    }

    function getFolderName(files: File[]): string {
        if (files.length === 0) return "Folder upload";
        const first = files[0];
        const path = (first as any).webkitRelativePath || "";
        if (path) {
            const parts = path.split("/");
            return parts[0] || "Folder upload";
        }
        return "Folder upload";
    }

    useEffect(() => {
        loadFiles();
        fetchLimits();
    }, []);

    useEffect(() => {
        const stored = localStorage.getItem("firmFolderBatch");
        if (stored) {
            try {
                const batch: BatchState = JSON.parse(stored);
                setActiveBatchId(batch.batchId);
                resumePolling(batch);
            } catch {
                localStorage.removeItem("firmFolderBatch");
            }
        }
    }, []);

    function resumePolling(batch: BatchState) {
        if (pollIntervalRef.current) clearInterval(pollIntervalRef.current);

        const myRun = ++pollRunIdRef.current;
        let inFlight = false;

        const poll = async () => {
            if (inFlight) return;
            inFlight = true;

            try {
                const result = await listCorpusFiles(batch.batchId);
                if (pollRunIdRef.current !== myRun) return;

                const batchFiles = result.files.filter(
                    (f) => f.batch_id === batch.batchId,
                );

                const indexed = batchFiles.filter(
                    (f) => f.status === "ready",
                ).length;
                const skipped = batchFiles.filter(
                    (f) =>
                        f.status === "unsupported" ||
                        f.status === "failed",
                ).length;
                const allTerminal = batchFiles.length > 0 && batchFiles.every((f) =>
                    ["ready", "failed", "unsupported"].includes(f.status),
                );

                if (pollRunIdRef.current !== myRun) return;

                setIngest({
                    total: batch.total,
                    indexed,
                    startedAt: batch.startedAt,
                    status: allTerminal ? "done" : "running",
                });

                const newSkipped: SkippedItem[] = [];
                for (const f of batchFiles) {
                    if (f.status === "unsupported") {
                        newSkipped.push({
                            filename: f.filename,
                            reason: f.error || "Unsupported format",
                        });
                    } else if (f.status === "failed") {
                        newSkipped.push({
                            filename: f.filename,
                            reason: f.error || "Processing failed",
                        });
                    }
                }
                setSkippedItems(newSkipped);

                if (allTerminal) {
                    if (pollRunIdRef.current !== myRun) return;
                    if (pollIntervalRef.current)
                        clearInterval(pollIntervalRef.current);
                    localStorage.removeItem("firmFolderBatch");
                    setActiveBatchId(null);
                    await new Promise((resolve) => setTimeout(resolve, 500));
                    await loadFiles();
                }
            } catch (err) {
                console.error("Polling failed:", err);
            } finally {
                inFlight = false;
            }
        };

        poll();
        pollIntervalRef.current = setInterval(poll, 1500);
    }

    useEffect(() => {
        return () => {
            if (pollIntervalRef.current) clearInterval(pollIntervalRef.current);
        };
    }, []);

    async function handleFileSelect(
        event: React.ChangeEvent<HTMLInputElement>,
    ) {
        const selectedFiles = Array.from(event.target.files ?? []);
        if (!selectedFiles.length) return;

        const l = await fetchLimits();
        if (!l) return;

        const preflight = computePreflightFiles(selectedFiles, l);
        setPreflightFiles(preflight);
        setPreflightOpen(true);
    }

    async function handleFolderSelect(
        event: React.ChangeEvent<HTMLInputElement>,
    ) {
        const selectedFiles = Array.from(event.target.files ?? []);
        if (!selectedFiles.length) return;

        const l = await fetchLimits();
        if (!l) return;

        const preflight = computePreflightFiles(selectedFiles, l);
        setPreflightFiles(preflight);
        setPreflightOpen(true);
    }

    function handleDragOver(event: React.DragEvent<HTMLDivElement>) {
        event.preventDefault();
    }

    async function handleDrop(event: React.DragEvent<HTMLDivElement>) {
        event.preventDefault();

        const l = await fetchLimits();
        if (!l) return;

        const items = event.dataTransfer.items;
        const droppedFiles = await collectEntries(items);

        if (droppedFiles.length === 0) return;

        const preflight = computePreflightFiles(droppedFiles, l);
        setPreflightFiles(preflight);
        setPreflightOpen(true);
    }

    async function handleConfirmPreflight() {
        if (preflightFiles.length === 0 || !limits) return;

        const supported = preflightFiles.filter((pf) => pf.supported);
        if (supported.length === 0) {
            setPreflightOpen(false);
            return;
        }

        const filesToUpload = supported
            .slice(0, limits.max_docs)
            .map((pf) => pf.file);

        setPreflightOpen(false);
        setUploadingBatch(true);

        const preflightSkipped = preflightFiles
            .filter((pf) => !pf.supported)
            .map((pf) => ({
                filename: pf.file.name,
                reason: pf.reason || "Unknown",
            }));
        setSkippedItems(preflightSkipped);

        const batchId = crypto.randomUUID();
        const batchLabel = getFolderName(filesToUpload);

        try {
            const allAccepted: string[] = [];
            const allSkipped: SkippedItem[] = [...preflightSkipped];

            for (
                let i = 0;
                i < filesToUpload.length;
                i += 10
            ) {
                const chunk = filesToUpload.slice(i, i + 10);
                try {
                    const result = await uploadCorpusFiles(
                        chunk,
                        isTemplate,
                        batchId,
                        batchLabel,
                    );

                    allAccepted.push(...result.accepted);

                    for (const skip of result.skipped) {
                        if (
                            !allSkipped.some(
                                (s) => s.filename === skip.filename,
                            )
                        ) {
                            allSkipped.push({
                                filename: skip.filename,
                                reason: skip.reason,
                            });
                        }
                    }

                    if (result.duplicates.length > 0) {
                        console.warn("Duplicates skipped:", result.duplicates);
                    }
                } catch (chunkErr) {
                    console.error("Chunk upload failed:", chunkErr);
                    for (const file of chunk) {
                        if (
                            !allSkipped.some(
                                (s) => s.filename === file.name,
                            )
                        ) {
                            allSkipped.push({
                                filename: file.name,
                                reason: `upload failed: ${chunkErr instanceof Error ? chunkErr.message : "unknown error"}`,
                            });
                        }
                    }
                }
            }

            setSkippedItems(allSkipped);

            if (allAccepted.length > 0) {
                const startedAt = Date.now();
                setIngest({
                    total: allAccepted.length,
                    indexed: 0,
                    startedAt,
                    status: "running",
                });
                setActiveBatchId(batchId);
                localStorage.setItem(
                    "firmFolderBatch",
                    JSON.stringify({
                        batchId,
                        batchLabel,
                        total: allAccepted.length,
                        startedAt,
                    } as BatchState),
                );

                await processCorpusFiles(allAccepted);
                resumePolling({
                    batchId,
                    batchLabel,
                    total: allAccepted.length,
                    startedAt,
                });
            } else {
                setIngest({
                    total: 0,
                    indexed: 0,
                    startedAt: Date.now(),
                    status: "failed",
                    errorText: "No files were accepted for upload",
                });
            }
        } catch (err) {
            console.error("Upload failed:", err);
            setIngest((prev) =>
                prev
                    ? {
                          ...prev,
                          status: "failed",
                          errorText: "Upload failed",
                      }
                    : prev,
            );
            localStorage.removeItem("firmFolderBatch");
            setActiveBatchId(null);
        } finally {
            setUploadingBatch(false);
            if (fileInputRef.current) fileInputRef.current.value = "";
            if (folderInputRef.current) folderInputRef.current.value = "";
            setIsTemplate(false);
        }
    }

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
                                        ? {
                                              ...prev,
                                              indexed: Math.min(
                                                  prev.total,
                                                  prev.indexed + 1,
                                              ),
                                          }
                                        : prev,
                                );
                            }
                        } catch {
                            console.error(
                                "Failed to parse SSE event:",
                                trimmed,
                            );
                        }
                    }
                }

                setIngest((prev) =>
                    prev ? { ...prev, status: "done" } : prev,
                );
                await new Promise((resolve) => setTimeout(resolve, 500));
                await loadFiles();
                setProcessingMap({});
            }
        } catch (err) {
            console.error("Upload failed:", err);
            setIngest((prev) =>
                prev
                    ? {
                          ...prev,
                          status: "failed",
                          errorText: "Ingestion failed",
                      }
                    : prev,
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

    async function handleDeleteBatch(batchId: string) {
        try {
            await deleteCorpusBatch(batchId);
            await loadFiles();
        } catch (err) {
            console.error("Failed to delete batch:", err);
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
                onDragOver={handleDragOver}
                onDrop={handleDrop}
            >
                <div
                    style={{
                        display: "flex",
                        gap: 24,
                        justifyContent: "center",
                    }}
                >
                    <div
                        role="button"
                        tabIndex={0}
                        onClick={() => fileInputRef.current?.click()}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                fileInputRef.current?.click();
                            }
                        }}
                        style={{
                            cursor: "pointer",
                            display: "flex",
                            flexDirection: "column",
                            alignItems: "center",
                            gap: 12,
                            flex: 1,
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
                                Choose files
                            </p>
                        </div>
                    </div>

                    <div
                        style={{
                            borderLeft: "1px solid var(--color-border)",
                        }}
                    />

                    <div
                        role="button"
                        tabIndex={0}
                        onClick={() => folderInputRef.current?.click()}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                folderInputRef.current?.click();
                            }
                        }}
                        style={{
                            cursor: "pointer",
                            display: "flex",
                            flexDirection: "column",
                            alignItems: "center",
                            gap: 12,
                            flex: 1,
                        }}
                    >
                        <FolderOpen
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
                                Choose folder
                            </p>
                            <p
                                style={{
                                    fontSize: 13,
                                    color: "var(--color-muted-foreground)",
                                    margin: 0,
                                }}
                            >
                                Or drag and drop
                            </p>
                        </div>
                    </div>
                </div>

                <input
                    ref={fileInputRef}
                    type="file"
                    multiple
                    accept=".pdf,.docx,.odt,.rtf,.txt,.md,.csv,.xlsx,.xls,.ods,.png,.jpg,.jpeg,.tif,.tiff,.webp,.bmp"
                    onChange={handleFileSelect}
                    disabled={
                        uploading ||
                        uploadingBatch ||
                        ingest?.status === "running"
                    }
                    style={{ display: "none" }}
                />

                <input
                    ref={folderInputRef}
                    type="file"
                    multiple
                    {...({ webkitdirectory: "", directory: "" } as any)}
                    onChange={handleFolderSelect}
                    disabled={
                        uploading ||
                        uploadingBatch ||
                        ingest?.status === "running"
                    }
                    style={{ display: "none" }}
                />

                <div
                    style={{
                        marginTop: 16,
                        paddingTop: 16,
                        borderTop: "1px solid var(--color-border)",
                    }}
                >
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
                            onChange={(e) =>
                                setIsTemplate(e.target.checked)
                            }
                            disabled={
                                uploading ||
                                uploadingBatch ||
                                ingest?.status === "running"
                            }
                            style={{ cursor: "pointer" }}
                        />
                        <span>Mark as template</span>
                    </label>
                </div>
            </section>

            {/* Preflight Modal */}
            {preflightOpen && (
                <div
                    style={{
                        marginBottom: 32,
                        border: "1px solid var(--color-border)",
                        borderRadius: 12,
                        padding: 20,
                        background: "var(--card)",
                    }}
                >
                    <h3
                        style={{
                            fontSize: 16,
                            fontWeight: 600,
                            margin: "0 0 12px",
                            color: "var(--color-foreground)",
                        }}
                    >
                        Review before upload
                    </h3>

                    <div
                        style={{
                            fontSize: 14,
                            color: "var(--color-muted-foreground)",
                            marginBottom: 16,
                        }}
                    >
                        {(() => {
                            const supported = preflightFiles.filter(
                                (pf) => pf.supported,
                            ).length;
                            const skipped = preflightFiles.length - supported;
                            const msg =
                                supported === preflightFiles.length
                                    ? `${supported} file${supported !== 1 ? "s" : ""} ready to upload`
                                    : `${supported} file${supported !== 1 ? "s" : ""} supported, ${skipped} will be skipped`;
                            return msg;
                        })()}
                    </div>

                    {preflightFiles.some((pf) => !pf.supported) && (
                        <div
                            style={{
                                marginBottom: 16,
                                padding: 12,
                                background: "rgba(251, 146, 60, 0.08)",
                                borderRadius: 8,
                                borderLeft: "3px solid rgb(251, 146, 60)",
                            }}
                        >
                            <p
                                style={{
                                    fontSize: 13,
                                    fontWeight: 500,
                                    margin: "0 0 8px",
                                    color: "rgb(124, 45, 18)",
                                }}
                            >
                                Skipped files
                            </p>
                            <div
                                style={{
                                    fontSize: 12,
                                    color: "rgb(124, 45, 18)",
                                    maxHeight: 120,
                                    overflowY: "auto",
                                }}
                            >
                                {preflightFiles
                                    .filter((pf) => !pf.supported)
                                    .map((pf, i) => (
                                        <div key={i}>
                                            {pf.file.name} - {pf.reason}
                                        </div>
                                    ))}
                            </div>
                        </div>
                    )}

                    {(() => {
                        const supported = preflightFiles.filter(
                            (pf) => pf.supported,
                        ).length;
                        if (limits && supported > limits.max_docs) {
                            return (
                                <div
                                    style={{
                                        marginBottom: 16,
                                        padding: 12,
                                        background: "rgba(239, 68, 68, 0.08)",
                                        borderRadius: 8,
                                        borderLeft: "3px solid rgb(239, 68, 68)",
                                    }}
                                >
                                    <p
                                        style={{
                                            fontSize: 13,
                                            fontWeight: 500,
                                            margin: "0 0 8px",
                                            color: "rgb(127, 29, 29)",
                                        }}
                                    >
                                        Exceeds limit
                                    </p>
                                    <p
                                        style={{
                                            fontSize: 12,
                                            color: "rgb(127, 29, 29)",
                                            margin: 0,
                                        }}
                                    >
                                        You have {supported} files but the limit is{" "}
                                        {limits.max_docs}. Only the first{" "}
                                        {limits.max_docs} will be added.
                                    </p>
                                </div>
                            );
                        }
                        return null;
                    })()}

                    <div
                        style={{
                            display: "flex",
                            gap: 12,
                            justifyContent: "flex-end",
                        }}
                    >
                        <button
                            onClick={() => {
                                setPreflightOpen(false);
                                setPreflightFiles([]);
                            }}
                            style={{
                                padding: "8px 16px",
                                borderRadius: 6,
                                border: "1px solid var(--color-border)",
                                background: "transparent",
                                cursor: "pointer",
                                fontSize: 14,
                                fontWeight: 500,
                                color: "var(--color-foreground)",
                            }}
                        >
                            Cancel
                        </button>
                        <button
                            onClick={handleConfirmPreflight}
                            disabled={
                                preflightFiles.every((pf) => !pf.supported)
                            }
                            style={{
                                padding: "8px 16px",
                                borderRadius: 6,
                                border: "none",
                                background:
                                    preflightFiles.every((pf) => !pf.supported)
                                        ? "var(--color-muted-foreground)"
                                        : "var(--color-foreground)",
                                color: "white",
                                cursor: preflightFiles.every(
                                    (pf) => !pf.supported,
                                )
                                    ? "not-allowed"
                                    : "pointer",
                                fontSize: 14,
                                fontWeight: 500,
                            }}
                        >
                            Upload{" "}
                            {
                                preflightFiles.filter((pf) => pf.supported)
                                    .length
                            }{" "}
                            file{preflightFiles.filter((pf) => pf.supported).length !== 1 ? "s" : ""}
                        </button>
                    </div>
                </div>
            )}

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
                            unit: "files",
                            doneVerb: "Ingested",
                            failedLabel: "Ingestion failed",
                        }}
                    />
                </div>
            )}

            {/* Skipped Items Report */}
            {skippedItems.length > 0 && (
                <div
                    style={{
                        marginBottom: 32,
                        padding: 16,
                        background: "rgba(251, 146, 60, 0.08)",
                        borderRadius: 12,
                        borderLeft: "3px solid rgb(251, 146, 60)",
                    }}
                >
                    <h3
                        style={{
                            fontSize: 14,
                            fontWeight: 600,
                            margin: "0 0 12px",
                            color: "rgb(124, 45, 18)",
                        }}
                    >
                        {skippedItems.length} file{skippedItems.length !== 1 ? "s" : ""} could not be ingested
                    </h3>
                    <div
                        style={{
                            fontSize: 12,
                            color: "rgb(124, 45, 18)",
                            maxHeight: 200,
                            overflowY: "auto",
                        }}
                    >
                        {skippedItems.map((item, i) => (
                            <div
                                key={item.filename}
                                style={{
                                    marginBottom: 4,
                                    paddingBottom: 4,
                                    borderBottom:
                                        i < skippedItems.length - 1
                                            ? "1px solid rgba(124, 45, 18, 0.1)"
                                            : "none",
                                }}
                            >
                                <strong>{item.filename}</strong>
                                {" - "}
                                {item.reason}
                            </div>
                        ))}
                    </div>
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
                <div>
                    {(() => {
                        const grouped: Record<string, CorpusFile[]> = {};
                        for (const file of files) {
                            const key = file.batch_id || "__individual__";
                            if (!grouped[key]) grouped[key] = [];
                            grouped[key].push(file);
                        }

                        const batches = Object.entries(grouped).sort(
                            ([keyA], [keyB]) => {
                                if (keyA === "__individual__") return 1;
                                if (keyB === "__individual__") return -1;
                                return 0;
                            },
                        );

                        return batches.map(([batchKey, batchFiles]) => (
                            <div
                                key={batchKey}
                                style={{
                                    border: "1px solid var(--color-border)",
                                    borderRadius: 12,
                                    marginBottom: 20,
                                    overflow: "hidden",
                                }}
                            >
                                {batchKey !== "__individual__" && (
                                    <div
                                        style={{
                                            background: "var(--card)",
                                            borderBottom:
                                                "1px solid var(--color-border)",
                                            padding: "12px 16px",
                                            display: "flex",
                                            justifyContent: "space-between",
                                            alignItems: "center",
                                        }}
                                    >
                                        <div>
                                            <p
                                                style={{
                                                    fontSize: 14,
                                                    fontWeight: 600,
                                                    margin: "0 0 4px",
                                                    color: "var(--color-foreground)",
                                                }}
                                            >
                                                {
                                                    batchFiles[0]
                                                        ?.batch_label
                                                }
                                            </p>
                                            <p
                                                style={{
                                                    fontSize: 12,
                                                    color: "var(--color-muted-foreground)",
                                                    margin: 0,
                                                }}
                                            >
                                                {batchFiles.length} file
                                                {batchFiles.length !== 1
                                                    ? "s"
                                                    : ""}
                                            </p>
                                        </div>
                                        <button
                                            onClick={() =>
                                                handleDeleteBatch(batchKey)
                                            }
                                            disabled={
                                                batchKey === activeBatchId &&
                                                (ingest?.status === "running" ||
                                                    uploadingBatch)
                                            }
                                            title={
                                                batchKey === activeBatchId &&
                                                (ingest?.status === "running" ||
                                                    uploadingBatch)
                                                    ? "Cannot remove batch while uploading or indexing"
                                                    : ""
                                            }
                                            style={{
                                                padding: "6px 12px",
                                                fontSize: 12,
                                                border: "1px solid var(--color-border)",
                                                borderRadius: 6,
                                                background: "transparent",
                                                cursor:
                                                    batchKey === activeBatchId &&
                                                    (ingest?.status ===
                                                        "running" ||
                                                        uploadingBatch)
                                                        ? "not-allowed"
                                                        : "pointer",
                                                color: "var(--color-muted-foreground)",
                                                opacity:
                                                    batchKey === activeBatchId &&
                                                    (ingest?.status ===
                                                        "running" ||
                                                        uploadingBatch)
                                                        ? 0.5
                                                        : 1,
                                            }}
                                        >
                                            Remove batch
                                        </button>
                                    </div>
                                )}

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
                                                borderBottom:
                                                    "1px solid var(--color-border)",
                                            }}
                                        >
                                            <th
                                                style={{
                                                    textAlign: "left",
                                                    padding: "12px 16px",
                                                    fontWeight: 600,
                                                    fontSize: 13,
                                                    color: "var(--color-muted-foreground)",
                                                    textTransform:
                                                        "uppercase",
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
                                                    textTransform:
                                                        "uppercase",
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
                                                    textTransform:
                                                        "uppercase",
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
                                                    textTransform:
                                                        "uppercase",
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
                                                    textTransform:
                                                        "uppercase",
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
                                                    textTransform:
                                                        "uppercase",
                                                    letterSpacing: 0.5,
                                                }}
                                            >
                                                {tCommon("actions")}
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {batchFiles.map((file) => (
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
                                                        wordBreak:
                                                            "break-word",
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
                                                            flexWrap:
                                                                "wrap",
                                                        }}
                                                    >
                                                        {file.doc_type && (
                                                            <span
                                                                style={{
                                                                    fontSize: 12,
                                                                    background:
                                                                        "rgba(0,0,0,0.06)",
                                                                    padding:
                                                                        "4px 8px",
                                                                    borderRadius: 4,
                                                                    whiteSpace:
                                                                        "nowrap",
                                                                }}
                                                            >
                                                                {
                                                                    file.doc_type
                                                                }
                                                            </span>
                                                        )}
                                                        {file.case_type && (
                                                            <span
                                                                style={{
                                                                    fontSize: 12,
                                                                    background:
                                                                        "rgba(0,0,0,0.06)",
                                                                    padding:
                                                                        "4px 8px",
                                                                    borderRadius: 4,
                                                                    whiteSpace:
                                                                        "nowrap",
                                                                }}
                                                            >
                                                                {
                                                                    file.case_type
                                                                }
                                                            </span>
                                                        )}
                                                        {file.court && (
                                                            <span
                                                                style={{
                                                                    fontSize: 12,
                                                                    background:
                                                                        "rgba(0,0,0,0.06)",
                                                                    padding:
                                                                        "4px 8px",
                                                                    borderRadius: 4,
                                                                    whiteSpace:
                                                                        "nowrap",
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
                                                    <div
                                                        style={{
                                                            display: "flex",
                                                            flexDirection:
                                                                "column",
                                                            gap: 4,
                                                        }}
                                                    >
                                                        <span
                                                            style={{
                                                                fontSize: 12,
                                                                padding:
                                                                    "4px 8px",
                                                                borderRadius: 4,
                                                                fontWeight: 500,
                                                                backgroundColor:
                                                                    getStatusBadgeColor(
                                                                        file,
                                                                    ),
                                                                display:
                                                                    "inline-block",
                                                                width:
                                                                    "fit-content",
                                                            }}
                                                        >
                                                            {getStatusText(
                                                                file.status,
                                                            )}
                                                        </span>
                                                        {file.error && (
                                                            <span
                                                                style={{
                                                                    fontSize: 11,
                                                                    color: "rgb(220, 38, 38)",
                                                                }}
                                                            >
                                                                {file.error}
                                                            </span>
                                                        )}
                                                    </div>
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
                                                        checked={
                                                            file.is_template
                                                        }
                                                        onChange={() =>
                                                            handleToggleTemplate(
                                                                file.id,
                                                                file.is_template,
                                                            )
                                                        }
                                                        aria-label={`Mark ${file.filename} as template`}
                                                        style={{
                                                            cursor: "pointer",
                                                        }}
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
                                                            handleDelete(
                                                                file.id,
                                                            )
                                                        }
                                                        style={{
                                                            background:
                                                                "none",
                                                            border: "none",
                                                            cursor: "pointer",
                                                            color: "var(--color-muted-foreground)",
                                                            padding: 4,
                                                            display:
                                                                "inline-flex",
                                                            alignItems:
                                                                "center",
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
                        ));
                    })()}
                </div>
            )}
        </div>
    );
}
