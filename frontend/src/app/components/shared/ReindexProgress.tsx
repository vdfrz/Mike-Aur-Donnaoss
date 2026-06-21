"use client";

import { useEffect, useRef, useState } from "react";
import { Clock, CircleCheck, AlertCircle } from "lucide-react";

/**
 * Shared re-index progress display. Surfaces the embedded count as the
 * headline number, a live elapsed timer with an ETA, and a fill bar.
 * Used by the Sync (local folders) and Case Search (cached judgments)
 * settings pages, which both re-embed documents in the background.
 *
 * The timer is driven client-side from `startedAt` (epoch ms) so no
 * backend change is needed to show elapsed/ETA; it freezes on the last
 * value once `status` leaves "running".
 */
export interface ReindexProgressLabels {
    embedded: string; // "embedded"
    remaining: string; // "left"
    unit: string; // "documents"
    doneVerb: string; // "Indexed"
    failedLabel: string; // "Re-index failed"
}

const DEFAULT_LABELS: ReindexProgressLabels = {
    embedded: "embedded",
    remaining: "left",
    unit: "documents",
    doneVerb: "Indexed",
    failedLabel: "Re-index failed",
};

function fmtTime(totalSecs: number): string {
    const s = Math.max(0, Math.floor(totalSecs));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export default function ReindexProgress({
    indexed,
    total,
    status,
    startedAt,
    currentFile,
    currentStep,
    skipped = 0,
    errorText,
    labels: labelsProp,
}: {
    indexed: number;
    total: number;
    status: "running" | "done" | "failed";
    startedAt: number | null;
    currentFile?: string | null;
    currentStep?: string | null;
    skipped?: number;
    errorText?: string | null;
    labels?: Partial<ReindexProgressLabels>;
}) {
    const labels = { ...DEFAULT_LABELS, ...labelsProp };
    const [elapsed, setElapsed] = useState(0);
    const frozen = useRef(0);

    // Tick the elapsed clock only while running; freeze the last value
    // afterwards so the done/failed summary keeps the final time.
    useEffect(() => {
        if (status !== "running" || startedAt == null) {
            setElapsed(frozen.current);
            return;
        }
        const tick = () => {
            const e = (Date.now() - startedAt) / 1000;
            frozen.current = e;
            setElapsed(e);
        };
        tick();
        const h = setInterval(tick, 1000);
        return () => clearInterval(h);
    }, [status, startedAt]);

    const pct = total > 0 ? Math.min(100, Math.round((indexed / total) * 100)) : 0;
    const rate = elapsed > 0 ? indexed / elapsed : 0; // docs/sec
    const etaSecs = rate > 0 ? (total - indexed) / rate : null;
    const fileBase = currentFile ? currentFile.split(/[\\/]/).pop() : null;

    if (status === "done") {
        return (
            <div className="mt-3 flex items-center gap-2 text-sm text-green-700">
                <CircleCheck className="h-4 w-4 shrink-0" />
                <span>
                    {labels.doneVerb} {indexed} {labels.unit} in {fmtTime(elapsed)}
                    {skipped > 0 && <> · {skipped} skipped</>}
                </span>
            </div>
        );
    }

    if (status === "failed") {
        return (
            <div className="mt-3 flex items-start gap-2 text-sm text-red-600">
                <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                <span>{errorText || labels.failedLabel}</span>
            </div>
        );
    }

    return (
        <div className="mt-3">
            <div className="flex items-center justify-between gap-3 mb-2">
                <div className="flex items-baseline gap-1.5">
                    <span className="text-xl font-medium tabular-nums text-gray-900">
                        {indexed}
                    </span>
                    <span className="text-xs text-gray-500">
                        / {total} {labels.embedded}
                    </span>
                </div>
                <div className="flex items-center gap-3 text-xs text-gray-500 tabular-nums">
                    <span className="inline-flex items-center gap-1">
                        <Clock className="h-3.5 w-3.5" />
                        {fmtTime(elapsed)}
                    </span>
                    {etaSecs != null && etaSecs > 0 && (
                        <span className="text-gray-400">
                            ~{fmtTime(etaSecs)} {labels.remaining}
                        </span>
                    )}
                </div>
            </div>

            <div className="h-2 bg-gray-100 rounded-full overflow-hidden">
                <div
                    className="h-full bg-gray-900 rounded-full transition-all duration-300"
                    style={{ width: `${pct}%` }}
                />
            </div>

            <div className="flex items-center justify-between gap-3 mt-1.5">
                <div className="min-w-0 text-xs text-gray-400 truncate">
                    {fileBase && (
                        <>
                            {currentStep && (
                                <span className="inline-flex items-center rounded-full bg-gray-100 px-2 py-0.5 text-[10px] font-medium text-gray-600 mr-1.5">
                                    {currentStep}
                                </span>
                            )}
                            {fileBase}
                        </>
                    )}
                </div>
                <span className="text-xs text-gray-400 tabular-nums shrink-0">
                    {pct}%
                </span>
            </div>
        </div>
    );
}
