"use client";

import { useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { Trash2, Plus } from "lucide-react";
import {
    listStatuteActs,
    ingestStatute,
    deleteStatuteAct,
    type StatuteAct,
} from "@/app/lib/mikeApi";
import ReindexProgress from "@/app/components/shared/ReindexProgress";

interface IngestState {
    total: number;
    indexed: number;
    startedAt: number;
    status: "running" | "done" | "failed";
    stage: string;
    host: string;
    errorText?: string;
    truncated?: boolean;
}

export default function StatutesPage() {
    const t = useTranslations("Statutes");
    const tCommon = useTranslations("Common");

    const [acts, setActs] = useState<StatuteAct[]>([]);
    const [loading, setLoading] = useState(true);
    const [showAdd, setShowAdd] = useState(false);
    const [url, setUrl] = useState("");
    const [ingest, setIngest] = useState<IngestState | null>(null);
    const inputRef = useRef<HTMLInputElement>(null);

    async function loadActs() {
        try {
            const result = await listStatuteActs();
            setActs(result);
        } catch (err) {
            console.error("Failed to load statutes:", err);
        } finally {
            setLoading(false);
        }
    }

    useEffect(() => {
        loadActs();
    }, []);

    function stageLabel(stage: string): string {
        if (stage === "fetching") return t("stageFetching");
        if (stage === "reading") return t("stageReading");
        return t("stageParsing");
    }

    async function handleIngest() {
        const target = url.trim();
        if (!target || ingest?.status === "running") return;

        setIngest({
            total: 0,
            indexed: 0,
            startedAt: Date.now(),
            status: "running",
            stage: "fetching",
            host: "",
        });

        try {
            const response = await ingestStatute(target);
            if (!response.ok || !response.body) {
                throw new Error(await response.text());
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
                    if (dataStr === "[DONE]") continue;

                    let event: {
                        type: string;
                        stage?: string;
                        host?: string;
                        total?: number;
                        indexed?: number;
                        sections?: number;
                        message?: string;
                        truncated?: boolean;
                    };
                    try {
                        event = JSON.parse(dataStr);
                    } catch {
                        console.error("Failed to parse SSE event:", trimmed);
                        continue;
                    }

                    setIngest((prev) => {
                        if (!prev) return prev;
                        switch (event.type) {
                            case "stage":
                                return {
                                    ...prev,
                                    stage: event.stage ?? prev.stage,
                                    host: event.host || prev.host,
                                };
                            case "parsed":
                                return { ...prev, total: event.total ?? prev.total };
                            case "progress":
                                return {
                                    ...prev,
                                    indexed: event.indexed ?? prev.indexed,
                                    total: event.total ?? prev.total,
                                };
                            case "done":
                                return {
                                    ...prev,
                                    status: "done",
                                    indexed: event.sections ?? prev.indexed,
                                    total: event.sections ?? prev.total,
                                    truncated: event.truncated,
                                };
                            case "error":
                                return {
                                    ...prev,
                                    status: "failed",
                                    errorText: event.message || t("genericError"),
                                };
                            default:
                                return prev;
                        }
                    });
                }
            }

            await loadActs();
            // Collapse the add box only on a clean run; leave it open on
            // failure so the user can fix the link and retry.
            setIngest((prev) => {
                if (prev?.status === "done") {
                    setShowAdd(false);
                    setUrl("");
                }
                return prev;
            });
        } catch (err) {
            setIngest((prev) =>
                prev
                    ? { ...prev, status: "failed", errorText: (err as Error).message }
                    : prev,
            );
        }
    }

    async function handleDelete(shortName: string) {
        if (!window.confirm(t("deleteConfirm"))) return;
        try {
            await deleteStatuteAct(shortName);
            await loadActs();
        } catch (err) {
            console.error("Failed to delete statute:", err);
        }
    }

    return (
        <div style={{ fontFamily: "var(--font-sans)", color: "var(--color-foreground)" }}>
            {/* Header */}
            <section style={{ marginBottom: 32 }}>
                <div
                    style={{
                        display: "flex",
                        alignItems: "flex-start",
                        justifyContent: "space-between",
                        gap: 16,
                    }}
                >
                    <div>
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
                                margin: 0,
                                maxWidth: 560,
                            }}
                        >
                            {t("description")}
                        </p>
                    </div>

                    <button
                        onClick={() => {
                            setShowAdd((v) => !v);
                            setTimeout(() => inputRef.current?.focus(), 0);
                        }}
                        style={{
                            display: "inline-flex",
                            alignItems: "center",
                            gap: 6,
                            flexShrink: 0,
                            padding: "8px 14px",
                            fontSize: 14,
                            fontWeight: 500,
                            borderRadius: 8,
                            border: "1px solid var(--color-border)",
                            background: "var(--card)",
                            color: "var(--color-foreground)",
                            cursor: "pointer",
                        }}
                    >
                        <Plus size={16} />
                        {t("addButton")}
                    </button>
                </div>
            </section>

            {/* Add dialog */}
            {showAdd && (
                <section
                    style={{
                        border: "1px solid var(--color-border)",
                        borderRadius: 12,
                        padding: 24,
                        marginBottom: 32,
                        background: "var(--card)",
                    }}
                >
                    <h2
                        style={{
                            fontFamily: "var(--font-serif)",
                            fontSize: 18,
                            fontWeight: 500,
                            margin: "0 0 6px",
                        }}
                    >
                        {t("dialogTitle")}
                    </h2>
                    <p
                        style={{
                            color: "var(--color-muted-foreground)",
                            fontSize: 14,
                            lineHeight: 1.6,
                            margin: "0 0 16px",
                            maxWidth: 560,
                        }}
                    >
                        {t("dialogHelp")}
                    </p>

                    <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
                        <input
                            ref={inputRef}
                            type="url"
                            value={url}
                            onChange={(e) => setUrl(e.target.value)}
                            onKeyDown={(e) => {
                                if (e.key === "Enter") handleIngest();
                                if (e.key === "Escape") setShowAdd(false);
                            }}
                            placeholder={t("urlPlaceholder")}
                            disabled={ingest?.status === "running"}
                            style={{
                                flex: 1,
                                minWidth: 240,
                                padding: "10px 12px",
                                fontSize: 14,
                                borderRadius: 8,
                                border: "1px solid var(--color-border)",
                                background: "var(--background)",
                                color: "var(--color-foreground)",
                            }}
                        />
                        <button
                            onClick={handleIngest}
                            disabled={!url.trim() || ingest?.status === "running"}
                            style={{
                                padding: "10px 16px",
                                fontSize: 14,
                                fontWeight: 500,
                                borderRadius: 8,
                                border: "none",
                                background: "#2563eb",
                                color: "#fff",
                                cursor:
                                    !url.trim() || ingest?.status === "running"
                                        ? "default"
                                        : "pointer",
                                opacity: !url.trim() || ingest?.status === "running" ? 0.5 : 1,
                            }}
                        >
                            {t("indexButton")}
                        </button>
                        <button
                            onClick={() => setShowAdd(false)}
                            disabled={ingest?.status === "running"}
                            style={{
                                padding: "10px 16px",
                                fontSize: 14,
                                fontWeight: 500,
                                borderRadius: 8,
                                border: "1px solid var(--color-border)",
                                background: "transparent",
                                color: "var(--color-foreground)",
                                cursor: "pointer",
                            }}
                        >
                            {t("cancel")}
                        </button>
                    </div>

                    {ingest && (
                        <>
                            <ReindexProgress
                                indexed={ingest.indexed}
                                total={ingest.total}
                                status={ingest.status}
                                startedAt={ingest.startedAt}
                                currentStep={stageLabel(ingest.stage)}
                                currentFile={ingest.host || null}
                                errorText={ingest.errorText}
                                labels={{
                                    embedded: "indexed",
                                    unit: "sections",
                                    doneVerb: "Indexed",
                                    failedLabel: "Indexing failed",
                                }}
                            />
                            {ingest.status === "done" && ingest.truncated && (
                                <p
                                    style={{
                                        color: "var(--color-muted-foreground)",
                                        fontSize: 12,
                                        margin: "8px 0 0",
                                    }}
                                >
                                    {t("truncatedNote")}
                                </p>
                            )}
                        </>
                    )}
                </section>
            )}

            {/* Acts list */}
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
            ) : acts.length === 0 ? (
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
                    <table style={{ width: "100%", borderCollapse: "collapse", fontSize: 14 }}>
                        <thead>
                            <tr
                                style={{
                                    background: "var(--card)",
                                    borderBottom: "1px solid var(--color-border)",
                                }}
                            >
                                {[t("act"), t("year"), t("category"), t("sections"), ""].map(
                                    (h, i) => (
                                        <th
                                            key={i}
                                            style={{
                                                textAlign: i === 4 ? "center" : "left",
                                                padding: "12px 16px",
                                                fontWeight: 600,
                                                fontSize: 13,
                                                color: "var(--color-muted-foreground)",
                                                textTransform: "uppercase",
                                                letterSpacing: 0.5,
                                            }}
                                        >
                                            {h}
                                        </th>
                                    ),
                                )}
                            </tr>
                        </thead>
                        <tbody>
                            {acts.map((act) => (
                                <tr
                                    key={act.id}
                                    style={{ borderBottom: "1px solid var(--color-border)" }}
                                >
                                    <td style={{ padding: "12px 16px", maxWidth: 360 }}>
                                        <div style={{ fontWeight: 500 }}>{act.short_name}</div>
                                        <div
                                            style={{
                                                fontSize: 12,
                                                color: "var(--color-muted-foreground)",
                                                wordBreak: "break-word",
                                            }}
                                        >
                                            {act.full_title}
                                        </div>
                                    </td>
                                    <td style={{ padding: "12px 16px" }}>{act.year ?? "—"}</td>
                                    <td style={{ padding: "12px 16px" }}>
                                        {act.category ? (
                                            <span
                                                style={{
                                                    fontSize: 12,
                                                    background: "rgba(0,0,0,0.06)",
                                                    padding: "4px 8px",
                                                    borderRadius: 4,
                                                    whiteSpace: "nowrap",
                                                }}
                                            >
                                                {act.category}
                                            </span>
                                        ) : (
                                            "—"
                                        )}
                                    </td>
                                    <td style={{ padding: "12px 16px" }}>
                                        {act.section_count} {t("sections").toLowerCase()}
                                    </td>
                                    <td style={{ padding: "12px 16px", textAlign: "center" }}>
                                        <button
                                            onClick={() => handleDelete(act.short_name)}
                                            title={tCommon("delete")}
                                            style={{
                                                background: "none",
                                                border: "none",
                                                cursor: "pointer",
                                                color: "var(--color-muted-foreground)",
                                                padding: 4,
                                                display: "inline-flex",
                                                alignItems: "center",
                                            }}
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
