"use client";

import { useEffect, useRef } from "react";
import type { AnalysisProgress } from "@/app/components/shared/types";
import type { FeedItem } from "./analysisConstants";
import {
    AGENT_COLORS,
    AGENT_INITIALS,
    AGENT_DISPLAY_NAMES,
} from "./analysisConstants";
import { getRandomSnippet } from "@/app/data/thinkingSnippets";

function AgentAvatar({ agentName, size = 24 }: { agentName: string; size?: number }) {
    const colors = AGENT_COLORS[agentName] ?? { bg: "#F3F4F6", text: "#374151", border: "#9CA3AF" };
    const initials = AGENT_INITIALS[agentName] ?? "??";
    return (
        <span
            className="inline-flex items-center justify-center rounded-full shrink-0 font-medium"
            style={{
                width: size,
                height: size,
                backgroundColor: colors.bg,
                color: colors.text,
                fontSize: size * 0.4,
                border: `1px solid ${colors.border}`,
            }}
        >
            {initials}
        </span>
    );
}

function blurText(text: string, names: string[]) {
    if (names.length === 0) return text;
    const sorted = [...names].sort((a, b) => b.length - a.length);
    const pattern = new RegExp(
        `(${sorted.map(n => n.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`,
        'gi'
    );
    const parts = text.split(pattern);
    return parts.map((part, i) => {
        if (sorted.some(n => n.toLowerCase() === part.toLowerCase())) {
            return (
                <span key={i} className="select-none" style={{ filter: 'blur(6px)', WebkitFilter: 'blur(6px)' }}>{part}</span>
            );
        }
        return part;
    });
}

function ActivityCard({ item, blurNames = [] }: { item: FeedItem; blurNames?: string[] }) {
    return (
        <div className="flex items-start gap-2 py-1.5 analysis-feed-card">
            {item.agentName && <AgentAvatar agentName={item.agentName} size={20} />}
            <p className="text-xs text-gray-500 font-serif leading-relaxed">
                {blurNames.length > 0 ? blurText(item.text, blurNames) : item.text}
                {item.durationMs != null && (
                    <span className="text-gray-400 ml-1">({(item.durationMs / 1000).toFixed(1)}s)</span>
                )}
            </p>
        </div>
    );
}

function FindingCard({ item, blurNames = [] }: { item: FeedItem; blurNames?: string[] }) {
    const colors = item.agentName
        ? AGENT_COLORS[item.agentName] ?? { bg: "#F3F4F6", text: "#374151", border: "#9CA3AF" }
        : { bg: "#F3F4F6", text: "#374151", border: "#9CA3AF" };

    const severityColors: Record<string, { bg: string; text: string }> = {
        strength: { bg: "#DCFCE7", text: "#166534" },
        weakness: { bg: "#FEE2E2", text: "#991B1B" },
        gap:      { bg: "#FEF3C7", text: "#92400E" },
        risk:     { bg: "#FFEDD5", text: "#9A3412" },
        neutral:  { bg: "#F3F4F6", text: "#374151" },
    };
    const sev = severityColors[item.severity ?? "neutral"] ?? severityColors.neutral;

    return (
        <div
            className="rounded-lg border bg-white p-3 analysis-feed-card"
            style={{ borderColor: colors.border + "40" }}
        >
            <div className="flex items-center gap-2 mb-2">
                {item.agentName && <AgentAvatar agentName={item.agentName} />}
                <span className="text-xs font-medium text-gray-700">
                    {item.agentName ? (AGENT_DISPLAY_NAMES[item.agentName] ?? item.agentName) : "Agent"}
                </span>
                {item.severity && (
                    <span
                        className="ml-auto rounded-full px-2 py-0.5 text-[10px] font-medium"
                        style={{ backgroundColor: sev.bg, color: sev.text }}
                    >
                        {item.severity}
                    </span>
                )}
            </div>
            <p className="text-sm font-serif text-gray-900 leading-relaxed">{blurNames.length > 0 ? blurText(item.text, blurNames) : item.text}</p>
            {item.quote && (
                <div className="mt-2 pl-3 border-l-2 border-gray-200">
                    <p className="text-xs text-gray-500 italic font-serif">&ldquo;{blurNames.length > 0 ? blurText(item.quote, blurNames) : item.quote}&rdquo;</p>
                </div>
            )}
        </div>
    );
}

function PhaseTransitionCard({ item }: { item: FeedItem }) {
    return (
        <div className="flex items-center gap-3 py-3 analysis-feed-card">
            <div className="flex-1 h-px bg-gray-200" />
            <p className="text-[11px] text-gray-400 italic font-serif whitespace-nowrap">{item.text}</p>
            <div className="flex-1 h-px bg-gray-200" />
        </div>
    );
}

function ReassuranceCard({ item }: { item: FeedItem }) {
    return (
        <div className="flex items-center gap-3 py-2 analysis-feed-card min-w-0">
            <div className="shrink-0 w-8 h-px bg-gray-100" />
            <p className="text-[11px] text-gray-400 italic font-serif text-center flex-1 min-w-0">{item.text}</p>
            <div className="shrink-0 w-8 h-px bg-gray-100" />
        </div>
    );
}

function ExtractionCard({ item, demoMode }: { item: FeedItem; demoMode?: boolean }) {
    return (
        <div className="flex items-center gap-2 py-1 analysis-feed-card min-w-0">
            <span className="inline-flex items-center justify-center w-5 h-5 rounded-full bg-gray-100 text-gray-500 text-[10px] shrink-0">📄</span>
            <p className={`text-xs text-gray-500 font-serif truncate${demoMode ? " select-none" : ""}`} style={demoMode ? { filter: "blur(5px)" } : undefined}>{item.text}</p>
        </div>
    );
}

function ThinkingBubble({ agentName, snippet }: { agentName: string; snippet: string }) {
    return (
        <div className="flex items-start gap-2 py-1.5 opacity-70 min-w-0">
            <AgentAvatar agentName={agentName} size={20} />
            <div className="flex items-center gap-1.5 min-w-0 flex-1">
                <span className="inline-flex items-baseline gap-0.5 shrink-0">
                    <span className="w-1 h-1 rounded-full bg-gray-400 animate-[bounce_1.4s_infinite_0s]" />
                    <span className="w-1 h-1 rounded-full bg-gray-400 animate-[bounce_1.4s_infinite_0.2s]" />
                    <span className="w-1 h-1 rounded-full bg-gray-400 animate-[bounce_1.4s_infinite_0.4s]" />
                </span>
                <span className="text-[11px] text-gray-400 font-serif italic truncate">
                    {snippet || getRandomSnippet()}
                </span>
            </div>
        </div>
    );
}

export function InsightFeed({
    items,
    activeAgents,
    demoMode = false,
    blurNames = [],
}: {
    items: FeedItem[];
    activeAgents: AnalysisProgress[];
    demoMode?: boolean;
    blurNames?: string[];
}) {
    const containerRef = useRef<HTMLDivElement>(null);
    const isAtBottomRef = useRef(true);

    useEffect(() => {
        const el = containerRef.current;
        if (!el) return;
        const onScroll = () => {
            isAtBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        };
        el.addEventListener("scroll", onScroll);
        return () => el.removeEventListener("scroll", onScroll);
    }, []);

    // Auto-scroll
    useEffect(() => {
        if (isAtBottomRef.current && containerRef.current) {
            containerRef.current.scrollTo({
                top: containerRef.current.scrollHeight,
                behavior: "smooth",
            });
        }
    }, [items.length, activeAgents.length]);

    const runningAgents = activeAgents.filter((a) => a.status === "running");

    return (
        <div ref={containerRef} className="flex-1 overflow-y-auto overflow-x-hidden px-4 py-4 space-y-1.5 min-w-0">
            {items.map((item) => {
                switch (item.type) {
                    case "activity":
                        return <ActivityCard key={item.id} item={item} blurNames={blurNames} />;
                    case "finding":
                        return <FindingCard key={item.id} item={item} blurNames={blurNames} />;
                    case "phase_transition":
                        return <PhaseTransitionCard key={item.id} item={item} />;
                    case "reassurance":
                        return <ReassuranceCard key={item.id} item={item} />;
                    case "extraction":
                        return <ExtractionCard key={item.id} item={item} demoMode={demoMode} />;
                    default:
                        return null;
                }
            })}

            {/* Thinking bubbles for running agents */}
            {runningAgents.map((agent) => (
                <ThinkingBubble
                    key={`thinking-${agent.agent_name}`}
                    agentName={agent.agent_name}
                    snippet={agent.thinking?.slice(-80) ?? ""}
                />
            ))}
        </div>
    );
}
