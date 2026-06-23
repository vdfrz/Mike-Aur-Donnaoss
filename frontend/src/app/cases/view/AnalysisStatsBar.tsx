"use client";

import type { AnalysisProgress } from "@/app/components/shared/types";
import type { AnalysisPhase, AnalysisEstimate } from "./analysisConstants";
import { PHASES } from "./analysisConstants";
import { MikeIcon } from "@/components/chat/mike-icon";

function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatTime(seconds: number): string {
    const m = Math.floor(seconds / 60);
    const s = seconds % 60;
    return `${m}:${s.toString().padStart(2, "0")}`;
}

export function AnalysisStatsBar({
    totalPages,
    totalSizeBytes,
    elapsedSeconds,
    estimate,
    analysisRunning,
    completed,
    findingsCount,
    agentsDone,
}: {
    totalPages: number;
    totalSizeBytes: number;
    elapsedSeconds: number;
    estimate: AnalysisEstimate | null;
    analysisRunning: boolean;
    completed: boolean;
    findingsCount: number;
    agentsDone: number;
}) {
    if (!analysisRunning && !completed) return null;

    if (completed) {
        return (
            <div className="shrink-0 px-4 py-2 border-b border-gray-100 bg-gray-50">
                <p className="text-xs text-gray-500">
                    Completed in {formatTime(elapsedSeconds)} — {totalPages > 0 ? `${totalPages} pages, ` : ""}7 agents, {findingsCount} findings
                </p>
            </div>
        );
    }

    const remaining = estimate
        ? Math.max(0, estimate.estimatedSeconds - elapsedSeconds)
        : null;

    return (
        <div className="shrink-0 px-4 py-2 border-b border-gray-100">
            <div className="flex items-center gap-4 text-[11px] text-gray-500">
                {totalPages > 0 && (
                    <span>{totalPages} pages</span>
                )}
                {totalSizeBytes > 0 && (
                    <span>{formatBytes(totalSizeBytes)}</span>
                )}
                <span>{formatTime(elapsedSeconds)} elapsed</span>
                {remaining != null && (
                    <span>
                        {remaining > 0
                            ? `~${Math.ceil(remaining / 60)} min remaining`
                            : "Almost done…"}
                    </span>
                )}
            </div>
        </div>
    );
}

export function HeartbeatBand({
    currentPhase,
    findingsCount,
    agentProgress,
    onAbort,
}: {
    currentPhase: AnalysisPhase | null;
    findingsCount: number;
    agentProgress: AnalysisProgress[];
    onAbort: () => void;
}) {
    const phaseIndex = currentPhase
        ? PHASES.findIndex((p) => p.id === currentPhase)
        : -1;

    const doneCount = agentProgress.filter((a) => a.status === "done").length;

    return (
        <div className="shrink-0 flex items-center gap-3 px-4 py-1.5 border-b border-gray-100 text-[11px] text-gray-500">
            {/* Phase icons */}
            <div className="flex items-center gap-1.5">
                {PHASES.map((phase, idx) => (
                    <span key={phase.id} title={phase.label}>
                        {idx < phaseIndex ? (
                            <MikeIcon done size={12} />
                        ) : idx === phaseIndex ? (
                            <MikeIcon spin size={12} />
                        ) : (
                            <span className="inline-block w-2.5 h-2.5 rounded-full border border-gray-300" />
                        )}
                    </span>
                ))}
            </div>
            <span className="text-gray-300">|</span>
            <span>{findingsCount} insights</span>
            <span className="text-gray-300">|</span>
            <span>{doneCount} of 7 agents</span>
            <button
                onClick={onAbort}
                className="ml-auto text-gray-400 hover:text-red-500 transition-colors"
                title="Stop analysis"
            >
                ⏸
            </button>
        </div>
    );
}
