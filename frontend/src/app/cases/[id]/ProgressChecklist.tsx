"use client";

import { useState, useEffect } from "react";
import type { AnalysisProgress } from "@/app/components/shared/types";
import type {
    ExtractionProgress,
    AnalysisEstimate,
    AnalysisPhase,
} from "./analysisConstants";
import {
    AGENT_DISPLAY_NAMES,
} from "./analysisConstants";
import { MikeIcon } from "@/components/chat/mike-icon";
import { getRandomSnippet } from "@/app/data/thinkingSnippets";

// The 7 agents in the order they typically complete
const AGENT_ORDER = [
    "case_summary",
    "strengths_weaknesses",
    "evidence_gap",
    "opposition_predictor",
    "strategy_recommender",
    "precedent_finder",
    "risk_assessor",
] as const;

/** Per-agent cycling snippet row */
function AgentSnippetRow({ agentName, status, error }: {
    agentName: string;
    status: string;
    error?: string;
}) {
    const [snippet, setSnippet] = useState(() => getRandomSnippet());

    useEffect(() => {
        if (status !== "running") return;
        const interval = setInterval(() => {
            setSnippet(getRandomSnippet());
        }, 2500);
        return () => clearInterval(interval);
    }, [status]);

    return (
        <div className="flex items-start gap-2 py-1">
            <span className="shrink-0 mt-0.5">
                {status === "running" && <MikeIcon spin size={14} />}
                {status === "done" && <MikeIcon done size={14} />}
                {status === "error" && <MikeIcon error size={14} />}
                {status === "pending" && (
                    <span className="inline-flex items-center justify-center w-3.5 h-3.5 rounded-full border border-gray-300" />
                )}
            </span>
            <div className="min-w-0 flex-1">
                <span className={`text-xs leading-tight ${
                    status === "running" ? "font-medium text-gray-900" :
                    status === "done" ? "text-gray-500" :
                    status === "error" ? "text-red-600" : "text-gray-400"
                }`}>
                    {AGENT_DISPLAY_NAMES[agentName] ?? agentName}
                </span>
                {status === "running" && (
                    <p className="text-[11px] text-gray-400 font-serif italic mt-0.5 truncate">
                        {snippet}
                    </p>
                )}
                {status === "error" && error && (
                    <p className="text-[10px] text-red-500 mt-0.5 truncate">{error}</p>
                )}
            </div>
        </div>
    );
}

export function ProgressChecklist({
    currentPhase,
    extractions,
    agentProgress,
    estimate,
    elapsedSeconds,
    demoMode = false,
}: {
    currentPhase: AnalysisPhase | null;
    extractions: ExtractionProgress[];
    agentProgress: AnalysisProgress[];
    estimate: AnalysisEstimate | null;
    elapsedSeconds: number;
    demoMode?: boolean;
}) {
    const allExtracted = extractions.length > 0 && extractions.every((e) => e.done);
    const extracting = currentPhase === "extract";
    const doneCount = agentProgress.filter((a) => a.status === "done").length;
    const totalAgents = AGENT_ORDER.length;

    const remaining = estimate
        ? Math.max(0, estimate.estimatedSeconds - elapsedSeconds)
        : null;

    return (
        <div className="w-[220px] shrink-0 border-r border-gray-100 overflow-y-auto py-4 px-3">
            {/* Extraction phase */}
            <div className="mb-4">
                <div className="flex items-center gap-2 mb-1.5">
                    {allExtracted ? (
                        <MikeIcon done size={14} />
                    ) : extracting ? (
                        <MikeIcon spin size={14} />
                    ) : (
                        <span className="inline-flex items-center justify-center w-3.5 h-3.5 rounded-full border border-gray-300" />
                    )}
                    <span className={`text-xs font-medium ${
                        allExtracted ? "text-gray-500" : extracting ? "text-gray-900" : "text-gray-400"
                    }`}>
                        Extract text
                    </span>
                </div>
                {extractions.length > 0 && (
                    <div className="ml-6 space-y-0.5">
                        {extractions.map((ext) => (
                            <div key={ext.docIndex} className="flex items-center gap-1.5 text-[10px] text-gray-500 ml-1">
                                {ext.done ? (
                                    <span className="w-1.5 h-1.5 rounded-full bg-gray-300 shrink-0" />
                                ) : (
                                    <span className="w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0" />
                                )}
                                <span className={`truncate max-w-[150px]${demoMode ? " select-none" : ""}`} style={demoMode ? { filter: "blur(5px)" } : undefined}>{ext.filename}</span>
                                {ext.done && ext.pageCount != null && (
                                    <span className="text-gray-400 shrink-0">
                                        {ext.pageCount}p{ext.neededOcr ? " OCR" : ""}
                                    </span>
                                )}
                            </div>
                        ))}
                    </div>
                )}
            </div>

            {/* Divider */}
            <div className="h-px bg-gray-100 mb-3" />

            {/* Agent checklist — the 7 agents */}
            <div className="space-y-0.5">
                {AGENT_ORDER.map((agentName) => {
                    const ap = agentProgress.find((p) => p.agent_name === agentName);
                    const status = ap?.status ?? "pending";
                    return (
                        <AgentSnippetRow
                            key={agentName}
                            agentName={agentName}
                            status={status}
                            error={ap?.error}
                        />
                    );
                })}
            </div>

            {/* Footer */}
            <div className="mt-4 pt-3 border-t border-gray-100 space-y-1">
                <p className="text-[10px] text-gray-400">
                    {doneCount}/{totalAgents} agents complete
                </p>
                {remaining != null && remaining > 0 && (
                    <p className="text-[10px] text-gray-400">
                        ~{remaining > 60 ? `${Math.ceil(remaining / 60)} min` : `${remaining}s`} remaining
                    </p>
                )}
            </div>
        </div>
    );
}
