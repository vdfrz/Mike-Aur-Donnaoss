"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useTranslations } from "next-intl";
import {
    AlertTriangle,
    ArrowLeft,
    ChevronDown,
    Download,
    Eye,
    EyeOff,
    FileText,
    Info,
    Library,
    Loader2,
    Pencil,
    Plus,
    Search,
    Send,
    ShieldCheck,
    Trash2,
    Upload,
    X,
} from "lucide-react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import KanoonVerifyBadge, { extractKanoonTid } from "@/app/components/assistant/KanoonVerifyBadge";
import {
    getCase,
    updateCase,
    addCaseDocuments,
    removeCaseDocument,
    analyzeCaseStream,
    generateCaseOutput,
    streamCaseChat,
    getCaseChat,
    uploadStandaloneDocument,
    resolveCasePrecedents,
    type ResolvedPrecedent,
    type ResolvedPrecedentCase,
} from "@/app/lib/mikeApi";
import { useOfflineMode } from "@/app/hooks/useOfflineMode";
import type {
    AnalysisProgress,
    AssistantEvent,
    CaseDetail,
    CaseDocument,
    CaseFinding,
    CaseOutput,
    CaseParty,
} from "@/app/components/shared/types";
import { DocPanel } from "@/app/components/shared/DocPanel";
import { MikeIcon } from "@/components/chat/mike-icon";
import { getRandomSnippet } from "@/app/data/thinkingSnippets";
import { ToolbarTabs } from "@/app/components/shared/ToolbarTabs";
import { PreResponseWrapper } from "@/app/components/shared/PreResponseWrapper";
import { AssistantWorkflowModal } from "@/app/components/assistant/AssistantWorkflowModal";
import { ToolActivityStream } from "@/app/components/assistant/ToolActivityStream";
import { DocumentCard } from "@/app/components/assistant/DocumentCard";
import { EditCard } from "@/app/components/assistant/EditCard";
import { ProgressChecklist } from "./ProgressChecklist";
import { InsightFeed } from "./InsightFeed";
import { AnalysisStatsBar, HeartbeatBand } from "./AnalysisStatsBar";
import { OcrTimeoutWarning, StuckStateRescue } from "./StuckStateRescue";
import { useReassuranceInjector } from "./useReassuranceInjector";
import type { AnalysisPhase, ExtractionProgress, AnalysisEstimate, FeedItem } from "./analysisConstants";
import { AGENT_DISPLAY_NAMES } from "./analysisConstants";
import { useUserProfile } from "@/contexts/UserProfileContext";
import { RegistryTab } from "./RegistryTab";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const API_BASE =
    process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

function parseParties(json: string | null): CaseParty[] {
    if (!json) return [];
    try {
        return JSON.parse(json);
    } catch {
        return [];
    }
}

function parseFindingContent(json: string | Record<string, unknown>): Record<string, unknown> {
    if (typeof json === "object" && json !== null) return json as Record<string, unknown>;
    try {
        return JSON.parse(json as string);
    } catch {
        return {};
    }
}

/** Extract all {source_doc_id, exact_quote} refs from a nested JSON value. */
function collectCitations(value: unknown): { source_doc_id: string; exact_quote: string }[] {
    const out: { source_doc_id: string; exact_quote: string }[] = [];
    function walk(v: unknown) {
        if (Array.isArray(v)) { v.forEach(walk); return; }
        if (v && typeof v === "object") {
            const obj = v as Record<string, unknown>;
            if (typeof obj.source_doc_id === "string" && typeof obj.exact_quote === "string") {
                out.push({ source_doc_id: obj.source_doc_id, exact_quote: obj.exact_quote });
            }
            Object.values(obj).forEach(walk);
        }
    }
    walk(value);
    return out;
}

/** Render a structured finding as readable paragraphs instead of raw JSON. */
function renderFindingContent(content: Record<string, unknown>, agentName: string, blurNames: string[] = []): React.ReactNode {
    const b = (text: string) => blurNames.length > 0 ? blurPartyNames(text, blurNames) : text;
    // Case summary
    if (agentName === "case_summary") {
        const c = content as { parties?: { petitioner?: string; respondent?: string }; court?: string; case_no?: string; stage?: string; factual_background?: string; legal_issues?: string[]; procedural_history?: { date?: string; event?: string }[]; current_posture?: string };
        return (
            <div className="space-y-3 text-sm text-gray-900 leading-relaxed">
                {c.parties && <div><span className="text-xs font-medium text-gray-500 uppercase tracking-wide">Parties</span><p className="mt-1">{b(c.parties.petitioner ?? "")} v. {b(c.parties.respondent ?? "")}</p></div>}
                {c.court && <div><span className="text-xs font-medium text-gray-500 uppercase tracking-wide">Court</span><p className="mt-1">{b(c.court)}{c.case_no ? ` — ${c.case_no}` : ""}{c.stage ? ` (${c.stage})` : ""}</p></div>}
                {c.factual_background && <div><span className="text-xs font-medium text-gray-500 uppercase tracking-wide">Facts</span><p className="mt-1 font-serif">{b(stripGrounding(c.factual_background))}</p></div>}
                {c.legal_issues && c.legal_issues.length > 0 && <div><span className="text-xs font-medium text-gray-500 uppercase tracking-wide">Legal issues</span><ul className="mt-1 list-disc list-inside space-y-1">{c.legal_issues.map((iss, i) => <li key={i} className="font-serif">{b(stripGrounding(String(iss)))}</li>)}</ul></div>}
                {c.procedural_history && c.procedural_history.length > 0 && <div><span className="text-xs font-medium text-gray-500 uppercase tracking-wide">Timeline</span><div className="mt-1 space-y-1">{c.procedural_history.map((ph, i) => <p key={i} className="font-serif"><span className="font-medium">{ph.date}:</span> {b(stripGrounding(String(ph.event)))}</p>)}</div></div>}
                {c.current_posture && <div><span className="text-xs font-medium text-gray-500 uppercase tracking-wide">Current posture</span><p className="mt-1 font-serif">{b(stripGrounding(c.current_posture))}</p></div>}
            </div>
        );
    }

    // Strengths & Weaknesses
    if (agentName === "strengths_weaknesses") {
        const c = content as { strengths?: { point: string; supporting_text?: string }[]; weaknesses?: { point: string; why_weak?: string; vulnerable_to?: string }[] };
        return (
            <div className="space-y-4 text-sm text-gray-900">
                {c.strengths && c.strengths.length > 0 && <div><span className="text-xs font-medium text-green-700 uppercase tracking-wide">Strengths</span><div className="mt-2 space-y-2">{c.strengths.map((s, i) => <div key={i} className="rounded-md bg-green-50 border border-green-100 px-3 py-2"><p className="font-serif font-medium">{b(stripGrounding(s.point))}</p>{s.supporting_text && <p className="mt-1 text-xs text-green-800 italic">&ldquo;{b(s.supporting_text)}&rdquo;</p>}</div>)}</div></div>}
                {c.weaknesses && c.weaknesses.length > 0 && <div><span className="text-xs font-medium text-red-700 uppercase tracking-wide">Weaknesses</span><div className="mt-2 space-y-2">{c.weaknesses.map((w, i) => <div key={i} className="rounded-md bg-red-50 border border-red-100 px-3 py-2"><p className="font-serif font-medium">{b(stripGrounding(w.point))}</p>{w.why_weak && <p className="mt-1 text-xs text-red-800">{b(stripGrounding(w.why_weak))}</p>}{w.vulnerable_to && <p className="mt-1 text-xs text-red-700 italic">Vulnerable to: {b(stripGrounding(w.vulnerable_to))}</p>}</div>)}</div></div>}
            </div>
        );
    }

    // Evidence gaps
    if (agentName === "evidence_gap") {
        const c = content as { gaps?: { what_is_missing: string; why_it_matters?: string; how_to_obtain?: string }[]; contradictions?: { conflict_description: string }[] };
        return (
            <div className="space-y-4 text-sm text-gray-900">
                {c.gaps && c.gaps.length > 0 && <div><span className="text-xs font-medium text-amber-700 uppercase tracking-wide">Missing evidence</span><div className="mt-2 space-y-2">{c.gaps.map((g, i) => <div key={i} className="rounded-md bg-amber-50 border border-amber-100 px-3 py-2"><p className="font-serif font-medium">{b(stripGrounding(g.what_is_missing))}</p>{g.why_it_matters && <p className="mt-1 text-xs text-amber-800">{b(stripGrounding(g.why_it_matters))}</p>}{g.how_to_obtain && <p className="mt-1 text-xs text-amber-700">How to obtain: {b(stripGrounding(g.how_to_obtain))}</p>}</div>)}</div></div>}
                {c.contradictions && c.contradictions.length > 0 && <div><span className="text-xs font-medium text-red-700 uppercase tracking-wide">Contradictions</span><div className="mt-2 space-y-2">{c.contradictions.map((ct, i) => <div key={i} className="rounded-md bg-red-50 border border-red-100 px-3 py-2"><p className="font-serif">{b(stripGrounding(ct.conflict_description))}</p></div>)}</div></div>}
            </div>
        );
    }

    // Risks
    if (agentName === "risk_assessor") {
        const c = content as { risks?: { risk_type: string; description: string; mitigation?: string }[] };
        return (
            <div className="space-y-2 text-sm text-gray-900">
                {c.risks && c.risks.map((r, i) => (
                    <div key={i} className="rounded-md bg-orange-50 border border-orange-100 px-3 py-2">
                        <span className="inline-block rounded-full bg-orange-200 px-2 py-0.5 text-[10px] font-medium text-orange-800 mb-1">{r.risk_type}</span>
                        <p className="font-serif">{b(stripGrounding(r.description))}</p>
                        {r.mitigation && <p className="mt-1 text-xs text-orange-800">Mitigation: {b(stripGrounding(r.mitigation))}</p>}
                    </div>
                ))}
            </div>
        );
    }

    // Generic: render key-value pairs for other agents
    return renderGenericContent(content, blurNames);
}

/** Render arrays/objects generically when no specific renderer exists. */
function renderGenericContent(content: Record<string, unknown>, blurNames: string[] = []): React.ReactNode {
    const bx = (text: string) => blurNames.length > 0 ? blurPartyNames(text, blurNames) : text;
    const entries = Object.entries(content).filter(([k]) => k !== "raw_text" && k !== "error");
    if (entries.length === 0 && typeof content.raw_text === "string") {
        return <p className="text-sm font-serif text-gray-900 leading-relaxed whitespace-pre-wrap">{bx(String(content.raw_text))}</p>;
    }
    return (
        <div className="space-y-3 text-sm text-gray-900">
            {entries.map(([key, value]) => (
                <div key={key}>
                    <span className="text-xs font-medium text-gray-500 uppercase tracking-wide">{key.replace(/_/g, " ")}</span>
                    <div className="mt-1 font-serif">{renderValue(value, blurNames)}</div>
                </div>
            ))}
        </div>
    );
}

function renderValue(value: unknown, blurNames: string[] = []): React.ReactNode {
    const bx = (text: string) => blurNames.length > 0 ? blurPartyNames(text, blurNames) : text;
    if (typeof value === "string") return <p className="leading-relaxed">{bx(stripGrounding(value))}</p>;
    if (Array.isArray(value)) return (
        <ul className="list-disc list-inside space-y-1">
            {value.map((item, i) => (
                <li key={i}>{typeof item === "string" ? bx(stripGrounding(item)) : typeof item === "object" && item !== null ? renderListItem(item as Record<string, unknown>, blurNames) : String(item)}</li>
            ))}
        </ul>
    );
    if (value && typeof value === "object") return renderListItem(value as Record<string, unknown>, blurNames);
    return <p>{String(value)}</p>;
}

function renderListItem(item: Record<string, unknown>, blurNames: string[] = []): React.ReactNode {
    const bx = (text: string) => blurNames.length > 0 ? blurPartyNames(text, blurNames) : text;
    const textKey = ["point", "argument", "action", "description", "what_is_missing", "point_of_law", "name_or_role", "act"].find(k => typeof item[k] === "string");
    const text = textKey ? String(item[textKey]) : JSON.stringify(item);
    return <span>{bx(stripGrounding(text))}</span>;
}

/** Strip inline {source_doc_id, exact_quote} JSON objects from display text. */
function stripGrounding(text: string): string {
    // Remove JSON grounding objects that were embedded inline by agents
    return text.replace(/\{[^{}]*"source_doc_id"\s*:\s*"[^"]*"[^{}]*\}/g, "").replace(/\s{2,}/g, " ").trim();
}

function blurPartyNames(text: string, names: string[]): React.ReactNode {
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
                <span
                    key={i}
                    className="select-none"
                    style={{ filter: 'blur(6px)', WebkitFilter: 'blur(6px)' }}
                >
                    {part}
                </span>
            );
        }
        return part;
    });
}

type Tab = "overview" | "findings" | "outputs" | "chat" | "registry";

const AGENT_LABELS: Record<string, string> = {
    // Real orchestrator agent names
    case_summary: "case_summary",
    strengths_weaknesses: "strengths_weaknesses",
    evidence_gap: "evidence_gaps",
    opposition_predictor: "predicted_opposition",
    strategy_recommender: "strategy",
    precedent_finder: "required_precedents",
    risk_assessor: "risks",
    // Legacy labels (old stub names)
    fact_extractor: "fact_extractor",
    legal_issue_identifier: "legal_issue_identifier",
    precedent_analyzer: "precedent_analyzer",
    timeline_builder: "timeline_builder",
    argument_mapper: "argument_mapper",
    evidence_evaluator: "evidence_evaluator",
};

const DOC_TYPE_OPTIONS = [
    "affidavit",
    "fir",
    "order",
    "evidence",
    "opposing_filing",
    "other",
] as const;

// ---------------------------------------------------------------------------
// Case chat citation cleanup
// ---------------------------------------------------------------------------

interface CaseChatCitation {
    ref: number;
    doc_id: string;
    page?: number;
    quote: string;
}

/** A case-chat message. Mirrors the assistant's MikeMessage shape (content +
 *  streamed agentic `events`) so the bespoke case chat can show real thinking,
 *  tool activity and redline edits while keeping its OWN case-scoped history. */
type CaseChatMsg = {
    role: string;
    content: string;
    citations?: CaseChatCitation[];
    events?: AssistantEvent[];
    workflow?: { id: string; title: string; prompt_md?: string | null };
};

/** Extract citations from <CITATIONS> block and return cleaned text + parsed citations */
function extractCaseChatCitations(text: string): { cleaned: string; citations: CaseChatCitation[] } {
    let citations: CaseChatCitation[] = [];
    const match = text.match(/<CITATIONS>([\s\S]*?)<\/CITATIONS>/i);
    if (match) {
        try {
            const parsed = JSON.parse(match[1].trim());
            if (Array.isArray(parsed)) citations = parsed;
        } catch { /* ignore parse errors */ }
    }
    const cleaned = text.replace(/<CITATIONS>[\s\S]*?<\/CITATIONS>\s*$/i, "").trimEnd();
    return { cleaned, citations };
}

// ---------------------------------------------------------------------------
// Case chat bubble with clickable citations
// ---------------------------------------------------------------------------

/** Replace [1], [2], [1, 2] markers with inline-code tokens BEFORE ReactMarkdown
 *  so they don't get parsed as markdown link references. */
function preprocessCaseCitations(text: string): string {
    return text.replace(/\[(\d+(?:,\s*\d+)*)\]/g, (full, refsStr: string) => {
        const tokens = refsStr.split(",").map(s => s.trim()).flatMap(tok => {
            // Skip 4+ digit numbers (years like [2024])
            if (tok.length >= 4) return [];
            return [`\`§CIT:${parseInt(tok, 10)}§\`​`];
        });
        return tokens.length > 0 ? tokens.join("") : full;
    });
}

// Baked-in "thinking" snippets that rotate under the typing dots so the
// wait never reads as a dead spinner. Chat = conversational; outputs =
// drafting-flavoured.
const CHAT_THINKING = [
    "Reading the case file",
    "Checking the documents",
    "Connecting the dots",
    "Drafting a reply",
];

/**
 * Three bouncing dots followed by a slowly-rotating status snippet.
 * Pure cosmetic — the snippets cycle on a timer, they don't track real
 * backend progress (the chat/generate calls are single round-trips).
 */
function ThinkingDots({ snippets, quirky }: { snippets?: string[]; quirky?: boolean }) {
    // quirky → draw from the big legal-flavoured snippet pool (getRandomSnippet),
    // otherwise rotate the fixed `snippets` array in order.
    const [text, setText] = useState(() => (quirky ? getRandomSnippet() : (snippets?.[0] ?? "")));
    useEffect(() => {
        const tick = quirky
            ? () => setText(getRandomSnippet())
            : () => setText((cur) => {
                  const list = snippets ?? [];
                  const next = (list.indexOf(cur) + 1) % list.length;
                  return list[next] ?? cur;
              });
        const h = setInterval(tick, quirky ? 2500 : 1500);
        return () => clearInterval(h);
    }, [quirky, snippets]);
    return (
        <span className="inline-flex items-center gap-2 text-gray-400">
            <span className="inline-flex items-center gap-1">
                <span className="mike-dot" style={{ animationDelay: "0ms" }} />
                <span className="mike-dot" style={{ animationDelay: "180ms" }} />
                <span className="mike-dot" style={{ animationDelay: "360ms" }} />
            </span>
            <span className="text-xs italic">{text}…</span>
        </span>
    );
}

// Resolve a (possibly relative) backend download URL to an absolute one.
const CASE_API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "";
function resolveDocUrl(u: string): string {
    if (!u) return u;
    return /^https?:\/\//i.test(u) ? u : `${CASE_API_BASE}${u}`;
}

/** Collapsible reasoning panel for the case chat — mirrors the assistant's
 *  thinking disclosure and ToolActivityStream header styling. Auto-expands
 *  while streaming, collapses once the turn ends. */
function CaseThinkingBlock({ text, isStreaming }: { text: string; isStreaming: boolean }) {
    // Default-open while streaming, auto-collapsed once the turn ends, but a
    // manual toggle (once set) wins. Derived — no setState-in-effect.
    const [userToggled, setUserToggled] = useState<boolean | null>(null);
    const open = userToggled ?? isStreaming;
    if (!text.trim()) return null;
    return (
        <div className="rounded-lg border border-gray-200 bg-white overflow-hidden">
            <button
                onClick={() => setUserToggled(!open)}
                className="w-full flex items-center justify-between px-3 py-2 font-serif text-xs text-gray-600 hover:bg-gray-50 transition-colors"
            >
                <span className="flex items-center gap-2 min-w-0">
                    <span className="w-1.5 h-1.5 rounded-full bg-blue-500 shrink-0" />
                    <span className="font-medium">{isStreaming ? "Thinking…" : "Reasoning"}</span>
                </span>
                <ChevronDown size={12} className={`shrink-0 ml-2 transition-transform duration-200 ${open ? "" : "-rotate-90"}`} />
            </button>
            {open && (
                <div className="px-3 pb-2.5 text-xs text-gray-500 italic whitespace-pre-wrap leading-relaxed max-h-48 overflow-y-auto">
                    {text}
                </div>
            )}
        </div>
    );
}

function CaseChatBubble({ msg, blurNames, isStreaming = false }: {
    msg: CaseChatMsg;
    blurNames: string[];
    isStreaming?: boolean;
}) {
    const [activeCitation, setActiveCitation] = useState<CaseChatCitation | null>(null);
    const citations = msg.citations || [];
    const events = msg.events ?? [];

    // Strip <CITATIONS> block and preprocess [N] markers before ReactMarkdown
    const rawContent = msg.content.replace(/<CITATIONS>[\s\S]*?<\/CITATIONS>\s*$/i, "").trimEnd();
    const displayContent = preprocessCaseCitations(rawContent);

    // Streamed agentic events — same path as the global assistant.
    const reasoningText = events
        .filter((e): e is Extract<AssistantEvent, { type: "reasoning" }> => e.type === "reasoning")
        .map((e) => e.text)
        .join("\n\n")
        .trim();
    const hasTools = events.some((e) => e.type === "tool_call_start");
    const docCreated = events.filter((e): e is Extract<AssistantEvent, { type: "doc_created" }> => e.type === "doc_created");
    const docEdited = events.filter((e): e is Extract<AssistantEvent, { type: "doc_edited" }> => e.type === "doc_edited");
    const hasAgentic = reasoningText.length > 0 || hasTools || docCreated.length > 0 || docEdited.length > 0;
    const showThinkingDots = !msg.content && !hasAgentic;

    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const mdComponents: Record<string, any> = {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        code: ({ children, ...props }: any) => {
            const text = String(children);
            const citMatch = text.match(/^§CIT:(\d+)§$/);
            if (citMatch) {
                const refNum = parseInt(citMatch[1], 10);
                const cit = citations.find(c => c.ref === refNum);
                return (
                    <button
                        onClick={() => cit && setActiveCitation(activeCitation?.ref === cit.ref ? null : cit)}
                        className="inline-flex items-center justify-center mx-0.5 w-4 h-4 rounded-full bg-emerald-100 text-emerald-700 text-[10px] font-bold hover:bg-emerald-200 cursor-pointer align-super leading-none"
                        title={cit ? `${cit.doc_id} p.${cit.page || "?"}` : `Citation ${refNum}`}
                    >
                        {refNum}
                    </button>
                );
            }
            return <code {...props}>{children}</code>;
        },
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        a: ({ href, children, ...props }: any) => {
            const isIK = typeof href === "string" && href.includes("indiankanoon.org");
            const tid = isIK ? extractKanoonTid(href) : null;
            const link = (
                <a href={href} target="_blank" rel="noopener noreferrer" className="text-blue-600 underline hover:text-blue-700" {...props}>
                    {children}
                </a>
            );
            if (tid == null) return link;
            return (<>{link}<KanoonVerifyBadge tid={tid} title={String(children)} /></>);
        },
    };

    return (
        <div className="flex flex-col gap-2 max-w-[85%] min-w-0">
            {/* Real reasoning + tool activity, mirroring the assistant */}
            {reasoningText && <CaseThinkingBlock text={reasoningText} isStreaming={isStreaming} />}
            {hasTools && <ToolActivityStream events={events} isStreaming={isStreaming} />}

            {/* Assistant text */}
            {msg.content && (
                <div className="bg-gray-50 rounded-2xl rounded-bl-md px-4 py-3 text-gray-900 font-serif text-sm prose prose-sm max-w-none">
                    <ReactMarkdown remarkPlugins={[remarkGfm]} components={mdComponents}>
                        {displayContent}
                    </ReactMarkdown>
                    {activeCitation && (
                        <div className="mt-3 p-3 rounded-lg border border-emerald-200 bg-emerald-50 text-xs">
                            <div className="flex items-center justify-between mb-1">
                                <span className="font-semibold text-emerald-800">
                                    Source: {activeCitation.doc_id}{activeCitation.page ? `, p.${activeCitation.page}` : ""}
                                </span>
                                <button
                                    onClick={() => setActiveCitation(null)}
                                    className="text-emerald-600 hover:text-emerald-800 text-sm leading-none"
                                >
                                    ✕
                                </button>
                            </div>
                            <p className="text-gray-700 italic leading-relaxed">&ldquo;{activeCitation.quote}&rdquo;</p>
                        </div>
                    )}
                </div>
            )}

            {/* Redline + generated documents — same path as the assistant */}
            {(docCreated.length > 0 || docEdited.length > 0) && (
                <div className="space-y-2">
                    {docCreated.map((d, i) => (
                        <DocumentCard
                            key={`dc-${i}`}
                            filename={d.filename}
                            downloadUrl={resolveDocUrl(d.download_url)}
                            versionNumber={d.version_number ?? null}
                            isLoading={d.isStreaming}
                            onDownload={() => {
                                const u = resolveDocUrl(d.download_url);
                                if (u) window.open(u, "_blank");
                            }}
                        />
                    ))}
                    {docEdited.map((d, i) => (
                        <div key={`de-${i}`} className="space-y-2">
                            <DocumentCard
                                filename={d.filename}
                                downloadUrl={resolveDocUrl(d.download_url)}
                                versionNumber={d.version_number ?? null}
                                isLoading={d.isStreaming}
                                onDownload={() => {
                                    const u = resolveDocUrl(d.download_url);
                                    if (u) window.open(u, "_blank");
                                }}
                            />
                            {d.error ? (
                                <p className="text-xs text-red-600">{d.error}</p>
                            ) : d.annotations.length > 0 ? (
                                <div className="space-y-2">
                                    {d.annotations.map((ann) => (
                                        <EditCard key={ann.edit_id} annotation={ann} />
                                    ))}
                                </div>
                            ) : null}
                        </div>
                    ))}
                </div>
            )}

            {/* Initial connecting indicator */}
            {showThinkingDots && (
                <div className="w-fit bg-gray-50 rounded-2xl rounded-bl-md px-4 py-3">
                    <ThinkingDots snippets={CHAT_THINKING} />
                </div>
            )}
        </div>
    );
}

// ---------------------------------------------------------------------------
// Page
// ---------------------------------------------------------------------------

/** Resolve the model string to send for case chat based on user's LLM config.
 *  Mirrors the backend's resolve_analysis_model logic so the frontend can
 *  pass the right model without needing a backend recompile. */
function resolveChatModel(llm: { activeProvider?: string | null; claudeModel?: string | null; claudeApiKey?: string | null; geminiApiKey?: string | null; openaiModel?: string | null; openaiApiKey?: string | null; localModel?: string | null } | null): string | undefined {
    if (!llm) return undefined;
    if (llm.activeProvider === "claude" && llm.claudeApiKey) {
        return llm.claudeModel || "claude-sonnet-4-6";
    }
    if (llm.activeProvider === "gemini" && llm.geminiApiKey) {
        return "gemini-2.0-flash";
    }
    if (llm.activeProvider === "openai" && llm.openaiApiKey && llm.openaiModel) {
        return `openai:${llm.openaiModel}`;
    }
    return undefined; // let backend decide
}

export default function CaseWorkspacePage() {
    // Static export can only prerender a single static page, so the case id
    // travels in the query string (?id=<uuid>) instead of a dynamic path
    // segment. useSearchParams() requires the Suspense boundary in page.tsx.
    const caseId = useSearchParams().get("id") as string;
    const router = useRouter();
    const { profile } = useUserProfile();
    const tRaw = useTranslations("Cases");
    const t = ((key: string, ...args: unknown[]) => {
        try {
            // next-intl's t() throws on missing keys; swallow and use the key as a fallback
            // so a single missing string doesn't crash the whole page.
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            return (tRaw as any)(key, ...(args as any[]));
        } catch {
            return key;
        }
    }) as typeof tRaw;
    const tCommon = useTranslations("Common");
    // Case Prep needs a cloud model; it's unavailable while offline.
    const { isOffline } = useOfflineMode();

    const [detail, setDetail] = useState<CaseDetail | null>(null);
    const [loading, setLoading] = useState(true);
    const [activeTab, setActiveTab] = useState<Tab>("overview");

    // Inline editing
    const [editingTitle, setEditingTitle] = useState(false);
    const [titleDraft, setTitleDraft] = useState("");
    const [editingCourt, setEditingCourt] = useState(false);
    const [courtDraft, setCourtDraft] = useState("");

    // Analysis
    const [analysisRunning, setAnalysisRunning] = useState(false);
    const [agentProgress, setAgentProgress] = useState<AnalysisProgress[]>([]);
    const [analysisRedactPii, setAnalysisRedactPii] = useState(false);
    const [analysisError, setAnalysisError] = useState<string | null>(null);
    const [analysisStage, setAnalysisStage] = useState<string | null>(null);

    // Interactive analysis state
    const [extractions, setExtractions] = useState<ExtractionProgress[]>([]);
    // Filenames whose text extraction produced nothing (failed OCR / no text
    // layer). Surfaced as a prominent banner so a photo/scan the agents never
    // read is never silently skipped.
    const [noTextDocs, setNoTextDocs] = useState<string[]>([]);
    const [analysisEstimate, setAnalysisEstimate] = useState<AnalysisEstimate | null>(null);
    const [feedItems, setFeedItems] = useState<FeedItem[]>([]);
    const [currentPhase, setCurrentPhase] = useState<AnalysisPhase | null>(null);
    const [elapsedSeconds, setElapsedSeconds] = useState(0);
    const [analysisCompleted, setAnalysisCompleted] = useState(false);
    const [stuckDismissed, setStuckDismissed] = useState(false);
    const abortControllerRef = useRef<AbortController | null>(null);
    const analysisStartRef = useRef<number>(0);
    const agentStartTimesRef = useRef<Record<string, number>>({});

    // Outputs
    const [generatingOutput, setGeneratingOutput] = useState<string | null>(null);
    const [outputError, setOutputError] = useState<string | null>(null);

    // Doc panel
    const [openDocId, setOpenDocId] = useState<string | null>(null);
    const [openDocFilename, setOpenDocFilename] = useState<string>("");
    const [openDocOutput, setOpenDocOutput] = useState<CaseOutput | null>(null);

    // Doc attach modal
    const [showDocPicker, setShowDocPicker] = useState(false);

    // Direct upload
    const [uploading, setUploading] = useState(false);
    const [uploadProgress, setUploadProgress] = useState<string>("");
    const uploadInputRef = useRef<HTMLInputElement | null>(null);

    // Chat
    const [chatMessages, setChatMessages] = useState<CaseChatMsg[]>([]);
    // The backing chat row id. Threaded into every send so a reload resumes the
    // same conversation instead of spawning a fresh chat; null = start a new one
    // on the next send (also what "New conversation" resets it to).
    const [caseChatId, setCaseChatId] = useState<string | null>(null);
    const [chatInput, setChatInput] = useState("");
    const [chatLoading, setChatLoading] = useState(false);
    const [selectedWorkflow, setSelectedWorkflow] = useState<{ id: string; title: string; prompt_md?: string | null } | null>(null);
    const chatEndRef = useRef<HTMLDivElement>(null);

    // Collapsible findings
    const [expandedAgents, setExpandedAgents] = useState<Set<string>>(new Set());

    // Demo mode — blur party names
    const [demoMode, setDemoMode] = useState(false);

    // Document rename (client-side display names)
    const [docDisplayNames, setDocDisplayNames] = useState<Record<string, string>>({});
    const [editingDocId, setEditingDocId] = useState<string | null>(null);
    const [docNameDraft, setDocNameDraft] = useState("");

    const load = useCallback(async () => {
        try {
            const d = await getCase(caseId);
            setDetail(d);
        } catch {
            setDetail(null);
        } finally {
            setLoading(false);
        }
    }, [caseId]);

    // Missing id (e.g. /cases/view with no query) — bounce to the list.
    useEffect(() => {
        if (!caseId) router.replace("/cases");
    }, [caseId, router]);

    useEffect(() => {
        if (!caseId) return;
        load();
    }, [load, caseId]);

    // Restore the prior conversation on mount so a reload doesn't drop chat
    // history. Assistant turns are persisted with their raw <CITATIONS> block,
    // so we run them through the same extractor the live stream uses.
    useEffect(() => {
        let cancelled = false;
        (async () => {
            try {
                const { chat_id, messages } = await getCaseChat(caseId);
                if (cancelled || messages.length === 0) return;
                const hydrated: CaseChatMsg[] = messages.map((m) => {
                    if (m.role === "assistant") {
                        const { cleaned, citations } = extractCaseChatCitations(m.content);
                        return {
                            role: "assistant",
                            content: cleaned,
                            citations: citations.length > 0 ? citations : undefined,
                        };
                    }
                    return { role: m.role, content: m.content };
                });
                setChatMessages(hydrated);
                setCaseChatId(chat_id);
            } catch (e) {
                console.error("[case-chat] history restore failed", e);
            }
        })();
        return () => {
            cancelled = true;
        };
    }, [caseId]);

    // Scroll chat on new messages
    useEffect(() => {
        chatEndRef.current?.scrollIntoView({ behavior: "smooth" });
    }, [chatMessages]);

    // Elapsed timer during analysis
    useEffect(() => {
        if (!analysisRunning) return;
        analysisStartRef.current = Date.now();
        setElapsedSeconds(0);
        const interval = setInterval(() => {
            setElapsedSeconds(Math.floor((Date.now() - analysisStartRef.current) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [analysisRunning]);

    // Reassurance injector
    const enrichedFeedItems = useReassuranceInjector(feedItems, currentPhase, analysisRunning);

    const caseInfo = detail?.case_info;
    const documents = detail?.documents ?? [];
    const findings = detail?.findings ?? [];
    const outputs = detail?.outputs ?? [];
    const parties = useMemo(
        () => parseParties(caseInfo?.parties_json ?? null),
        [caseInfo?.parties_json],
    );

    // Document aggregate stats
    const totalPages = documents.reduce((sum, d) => sum + (d.page_count ?? 0), 0);
    const totalSizeBytes = documents.reduce((sum, d) => sum + (d.size_bytes ?? 0), 0);

    const summaryFinding = findings.find(
        (f) => f.agent_name === "CASE_SUMMARY_AGENT" || f.agent_name === "case_summary",
    );

    const blurNames = useMemo(() => {
        if (!demoMode) return [];
        const names: string[] = [];
        for (const p of parties) {
            if (p.name) names.push(p.name);
        }
        if (summaryFinding) {
            try {
                const content = typeof summaryFinding.content_json === "string"
                    ? JSON.parse(summaryFinding.content_json)
                    : summaryFinding.content_json;
                if (content.parties?.petitioner) names.push(content.parties.petitioner);
                if (content.parties?.respondent) names.push(content.parties.respondent);
                if (Array.isArray(content.parties?.other)) {
                    names.push(...content.parties.other);
                }
            } catch { /* ignore parse errors */ }
        }
        // Extract party names from document filenames as fallback
        // Indian legal docs often named "Petitioner V. Respondent.pdf"
        for (const doc of documents) {
            const basename = (doc.filename ?? "").replace(/\.[^.]+$/, "").trim();
            // Split on common "v." / "V." / "vs" / "Vs" / " v " patterns
            const vsMatch = basename.split(/\s+(?:v\.?|vs\.?|V\.?|Vs\.?)\s+/i);
            if (vsMatch.length >= 2) {
                for (const part of vsMatch) {
                    const cleaned = part.trim();
                    if (cleaned.length > 3) names.push(cleaned);
                }
            }
        }
        // Also extract the case title party names (title often = "X v. Y")
        if (caseInfo?.title) {
            const titleVs = caseInfo.title.split(/\s+(?:v\.?|vs\.?|V\.?|Vs\.?)\s+/i);
            if (titleVs.length >= 2) {
                for (const part of titleVs) {
                    const cleaned = part.trim();
                    if (cleaned.length > 3) names.push(cleaned);
                }
            }
        }
        return [...new Set(names)].filter(n => n.length > 3);
    }, [demoMode, parties, summaryFinding, documents, caseInfo?.title]);

    // Group findings by agent (excluding summary)
    const findingsByAgent = useMemo(() => {
        const map = new Map<string, CaseFinding[]>();
        for (const f of findings) {
            if (f.agent_name === "CASE_SUMMARY_AGENT" || f.agent_name === "case_summary") continue;
            const arr = map.get(f.agent_name) ?? [];
            arr.push(f);
            map.set(f.agent_name, arr);
        }
        return map;
    }, [findings]);

    // -----------------------------------------------------------------------
    // Inline edit handlers
    // -----------------------------------------------------------------------

    async function saveTitle() {
        setEditingTitle(false);
        const v = titleDraft.trim();
        if (!v || !caseInfo || v === caseInfo.title) return;
        setDetail((prev) =>
            prev
                ? {
                      ...prev,
                      case_info: { ...prev.case_info, title: v },
                  }
                : prev,
        );
        await updateCase(caseId, { title: v });
    }

    async function saveCourt() {
        setEditingCourt(false);
        const v = courtDraft.trim();
        if (!caseInfo) return;
        setDetail((prev) =>
            prev
                ? {
                      ...prev,
                      case_info: { ...prev.case_info, court: v || null },
                  }
                : prev,
        );
        await updateCase(caseId, { court: v || undefined });
    }

    // -----------------------------------------------------------------------
    // Document actions
    // -----------------------------------------------------------------------

    function getDocDisplayName(doc: CaseDocument): string {
        return docDisplayNames[doc.document_id] || doc.filename || doc.document_id.slice(0, 8);
    }

    function saveDocName(docId: string) {
        setEditingDocId(null);
        const v = docNameDraft.trim();
        if (!v) return;
        setDocDisplayNames((prev) => ({ ...prev, [docId]: v }));
    }

    async function handleRemoveDoc(documentId: string) {
        setDetail((prev) =>
            prev
                ? {
                      ...prev,
                      documents: prev.documents.filter(
                          (d) => d.document_id !== documentId,
                      ),
                  }
                : prev,
        );
        await removeCaseDocument(caseId, documentId);
    }

    async function handleDirectUpload(files: FileList | null) {
        if (!files || files.length === 0) return;
        setUploading(true);
        setUploadProgress(`0 / ${files.length}`);
        try {
            const newDocIds: { document_id: string; document_type: string }[] = [];
            for (let i = 0; i < files.length; i++) {
                const file = files[i];
                setUploadProgress(`${i + 1} / ${files.length}: ${file.name}`);
                try {
                    const doc = await uploadStandaloneDocument(file);
                    newDocIds.push({ document_id: doc.id, document_type: "other" });
                } catch (uploadErr) {
                    console.error(`[case-upload] upload failed for ${file.name}:`, uploadErr);
                }
            }
            if (newDocIds.length > 0) {
                await addCaseDocuments(caseId, newDocIds);
                if (typeof window !== "undefined") {
                    window.location.reload();
                }
            }
        } catch (e) {
            console.error("[case-upload] failed", e);
        } finally {
            setUploading(false);
            setUploadProgress("");
            if (uploadInputRef.current) uploadInputRef.current.value = "";
        }
    }

    // -----------------------------------------------------------------------
    // Analysis
    // -----------------------------------------------------------------------

    function handleAbortAnalysis() {
        abortControllerRef.current?.abort();
        setAnalysisRunning(false);
        setAnalysisStage(null);
    }

    async function handleRunAnalysis() {
        if (analysisRunning || isOffline) return;
        setAnalysisRunning(true);
        setAnalysisCompleted(false);
        setAgentProgress([]);
        setAnalysisError(null);
        setAnalysisStage(null);
        setExtractions([]);
        setNoTextDocs([]);
        setAnalysisEstimate(null);
        setFeedItems([]);
        setCurrentPhase(null);
        setStuckDismissed(false);
        agentStartTimesRef.current = {};
        setActiveTab("findings");

        const controller = new AbortController();
        abortControllerRef.current = controller;

        let streamErrored = false;
        let receivedDone = false;
        try {
            const resp = await analyzeCaseStream(caseId, analysisRedactPii);
            if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
            const reader = resp.body?.getReader();
            if (!reader) throw new Error("No body");
            const decoder = new TextDecoder();
            let buffer = "";
            while (true) {
                if (controller.signal.aborted) {
                    reader.cancel();
                    break;
                }
                const { done, value } = await reader.read();
                if (done) break;
                buffer += decoder.decode(value, { stream: true });
                const lines = buffer.split("\n");
                buffer = lines.pop() || "";
                for (const line of lines) {
                    const trimmed = line.trim();
                    if (!trimmed || !trimmed.startsWith("data:")) continue;
                    const dataStr = trimmed.slice(5).trim();
                    if (dataStr === "[DONE]") { receivedDone = true; continue; }
                    try {
                        const data = JSON.parse(dataStr);
                        const now = Date.now();

                        // --- Extraction events ---
                        if (data.type === "extracting_doc") {
                            setCurrentPhase("extract");
                            setExtractions((prev) => {
                                const next = prev.filter((e) => e.docIndex !== data.doc_index);
                                next.push({
                                    filename: data.filename,
                                    docIndex: data.doc_index,
                                    totalDocs: data.total_docs,
                                    done: false,
                                });
                                return next;
                            });
                            setFeedItems((prev) => [...prev, {
                                id: `extract-start-${data.doc_index}`,
                                type: "extraction",
                                timestamp: now,
                                text: `Extracting ${data.filename}…`,
                            }]);
                        }

                        if (data.type === "extracted_doc") {
                            setExtractions((prev) => {
                                const next = prev.filter((e) => e.docIndex !== data.doc_index);
                                next.push({
                                    filename: data.filename,
                                    docIndex: data.doc_index,
                                    totalDocs: data.total_docs,
                                    done: true,
                                    pageCount: data.page_count,
                                    neededOcr: data.needed_ocr,
                                });
                                return next;
                            });
                            // Flag a doc that extracted to nothing (failed OCR /
                            // no text layer). Guard on an explicit numeric 0 so an
                            // older backend that omits char_count never false-flags.
                            if (typeof data.char_count === "number" && data.char_count === 0) {
                                setNoTextDocs((prev) =>
                                    prev.includes(data.filename) ? prev : [...prev, data.filename],
                                );
                            }
                            setFeedItems((prev) => [...prev, {
                                id: `extract-done-${data.doc_index}`,
                                type: "extraction",
                                timestamp: now,
                                text: `✓ ${data.filename} — ${data.page_count} pages${data.needed_ocr ? " (OCR)" : ""}`,
                            }]);
                            // If all docs extracted, add phase transition
                            if (data.doc_index === data.total_docs - 1) {
                                setFeedItems((prev) => [...prev, {
                                    id: `phase-extract-done`,
                                    type: "phase_transition",
                                    timestamp: now + 1,
                                    text: "Text extraction complete — beginning analysis",
                                }]);
                            }
                        }

                        // --- Estimate ---
                        if (data.type === "estimate") {
                            setAnalysisEstimate({
                                totalPages: data.total_pages,
                                estimatedSeconds: data.estimated_seconds,
                                hasOcr: data.has_ocr,
                            });
                        }

                        // --- Agent status ---
                        if (data.type === "agent_status") {
                            if (data.status === "error" && data.error) {
                                setAnalysisError(data.error);
                                // An orchestrator-level error is catastrophic: the
                                // backend still emits a terminal [DONE] after it, so
                                // without this flag the finally would mark the run
                                // "completed" alongside the error (a green stats bar
                                // next to a red failure). Suppress completion. A
                                // single failed agent (other agents still ran) is not
                                // catastrophic and is left to complete partially.
                                if (data.agent_name === "orchestrator") {
                                    streamErrored = true;
                                }
                            }
                            // Update phase based on agent name
                            if (data.status === "running") {
                                agentStartTimesRef.current[data.agent_name] = now;
                                if (data.agent_name === "case_summary") {
                                    setCurrentPhase("summarize");
                                } else {
                                    setCurrentPhase("analyze");
                                }
                                setFeedItems((prev) => [...prev, {
                                    id: `agent-start-${data.agent_name}-${now}`,
                                    type: "activity",
                                    timestamp: now,
                                    agentName: data.agent_name,
                                    text: `${AGENT_DISPLAY_NAMES[data.agent_name] ?? data.agent_name} started…`,
                                }]);
                            }
                            if (data.status === "done") {
                                const startTime = agentStartTimesRef.current[data.agent_name];
                                const dur = startTime ? now - startTime : undefined;
                                setFeedItems((prev) => [...prev, {
                                    id: `agent-done-${data.agent_name}-${now}`,
                                    type: "activity",
                                    timestamp: now,
                                    agentName: data.agent_name,
                                    text: `✓ ${AGENT_DISPLAY_NAMES[data.agent_name] ?? data.agent_name} done`,
                                    durationMs: dur,
                                }]);
                            }

                            setAgentProgress((prev) => {
                                const existing = prev.find(
                                    (p) => p.agent_name === data.agent_name,
                                );
                                const next = prev.filter(
                                    (p) => p.agent_name !== data.agent_name,
                                );
                                next.push({
                                    agent_name: data.agent_name,
                                    status: data.status,
                                    error: data.error,
                                    thinking: existing?.thinking,
                                });
                                return next;
                            });
                        }

                        if (data.type === "stage" && data.message) {
                            setAnalysisStage(data.message);
                        }

                        if (data.type === "agent_thinking" && data.agent_name) {
                            setAgentProgress((prev) => {
                                const existing = prev.find(
                                    (p) => p.agent_name === data.agent_name,
                                );
                                const next = prev.filter(
                                    (p) => p.agent_name !== data.agent_name,
                                );
                                const prevThinking = existing?.thinking ?? "";
                                next.push({
                                    agent_name: data.agent_name,
                                    status: existing?.status ?? "running",
                                    error: existing?.error,
                                    thinking: (prevThinking + (data.snippet ?? "")).slice(-2000),
                                });
                                return next;
                            });
                        }

                        // --- Finding ---
                        if (data.type === "finding") {
                            setDetail((prev) => {
                                if (!prev) return prev;
                                return {
                                    ...prev,
                                    findings: [...prev.findings, data.finding],
                                };
                            });
                            setExpandedAgents((prev) => {
                                const next = new Set(prev);
                                next.add(data.finding.agent_name);
                                return next;
                            });
                            // Add to feed
                            const content = typeof data.finding.content_json === "string"
                                ? (() => { try { return JSON.parse(data.finding.content_json); } catch { return data.finding.content_json; } })()
                                : data.finding.content_json;
                            const agentName = data.finding.agent_name;
                            let severity: FeedItem["severity"] = "neutral";
                            let text = "";
                            let quote: string | undefined;

                            if (agentName === "strengths_weaknesses") {
                                const s = content?.strengths?.length ?? 0;
                                const w = content?.weaknesses?.length ?? 0;
                                text = `Found ${s} strengths, ${w} weaknesses`;
                                severity = w > s ? "weakness" : "strength";
                            } else if (agentName === "evidence_gap") {
                                const g = content?.gaps?.length ?? 0;
                                text = `Identified ${g} evidence gap${g !== 1 ? "s" : ""}`;
                                severity = "gap";
                            } else if (agentName === "risk_assessor") {
                                const r = content?.risks?.length ?? 0;
                                text = `Assessed ${r} risk${r !== 1 ? "s" : ""}`;
                                severity = "risk";
                            } else if (agentName === "case_summary") {
                                text = "Case summary complete";
                                severity = "neutral";
                            } else {
                                text = `${AGENT_DISPLAY_NAMES[agentName] ?? agentName} analysis complete`;
                            }

                            setFeedItems((prev) => [...prev, {
                                id: `finding-${data.finding.id}`,
                                type: "finding",
                                timestamp: now,
                                agentName,
                                text,
                                severity,
                                quote,
                                findingData: content,
                            }]);
                        }

                        // --- Compression ---
                        if (data.type === "compressing") {
                            setFeedItems((prev) => [...prev, {
                                id: `compress-${now}`,
                                type: "activity",
                                timestamp: now,
                                text: `Compressing context (${data.original_tokens} → ${data.target_tokens} tokens)…`,
                            }]);
                        }

                        // --- Done ---
                        if (data.type === "done") {
                            setCurrentPhase("report");
                            setFeedItems((prev) => [...prev, {
                                id: `phase-done`,
                                type: "phase_transition",
                                timestamp: now,
                                text: "Analysis complete",
                            }]);
                        }
                    } catch {
                        // skip bad lines
                    }
                }
            }
        } catch (e) {
            if (!controller.signal.aborted) {
                console.error("[case-analysis] stream error", e);
                streamErrored = true;
                setAnalysisError(
                    e instanceof Error
                        ? `Analysis interrupted before it finished: ${e.message}`
                        : "Analysis interrupted before it finished. The connection dropped and no completion was received.",
                );
            }
        } finally {
            setAnalysisRunning(false);
            setAnalysisStage(null);
            // Only mark the run complete when the stream actually finished, i.e.
            // the terminal [DONE] marker arrived. A dropped connection, a mid-run
            // error, or a stream that closed early WITHOUT [DONE] must surface as
            // an error, not a false "completed" with however many findings
            // happened to arrive before the connection died.
            if (!controller.signal.aborted) {
                if (receivedDone && !streamErrored) {
                    setAnalysisCompleted(true);
                } else if (!streamErrored) {
                    setAnalysisError(
                        "Analysis ended before it finished. The connection closed before a completion signal arrived; re-run to generate findings.",
                    );
                }
                // if streamErrored, analysisError was already set (in the catch,
                // or in the agent_status handler for an orchestrator error)
            }
            abortControllerRef.current = null;
            await load();
        }
    }

    // -----------------------------------------------------------------------
    // Output generation
    // -----------------------------------------------------------------------

    async function handleGenerateOutput(outputType: string, redactPii?: boolean) {
        setOutputError(null);
        setGeneratingOutput(outputType);
        try {
            const out = await generateCaseOutput(caseId, outputType, redactPii);
            setDetail((prev) =>
                prev
                    ? { ...prev, outputs: [...prev.outputs, out] }
                    : prev,
            );
            setActiveTab("outputs");
        } catch (e) {
            console.error("[case-output] failed", e);
            setOutputError(e instanceof Error ? e.message : "Failed to generate output");
        } finally {
            setGeneratingOutput(null);
        }
    }

    // -----------------------------------------------------------------------
    // Chat
    // -----------------------------------------------------------------------

    async function handleChatSend() {
        const text = chatInput.trim();
        if (!text || chatLoading) return;
        setChatInput("");
        const userMsg = { role: "user", content: text, ...(selectedWorkflow ? { workflow: selectedWorkflow } : {}) };
        // Append the user message AND an empty assistant placeholder in one
        // go. The empty bubble immediately renders the three-dot "thinking"
        // indicator, so the dots show the instant the question is asked —
        // not only once the server starts streaming a reply.
        setChatMessages((prev) => [
            ...prev,
            userMsg,
            { role: "assistant", content: "" },
        ]);
        setChatLoading(true);
        setSelectedWorkflow(null);

        const chatAbort = new AbortController();
        // Timeout after 30s if no response at all
        const timeout = setTimeout(() => chatAbort.abort(), 30000);

        try {
            const resp = await streamCaseChat({
                caseId,
                messages: [...chatMessages, userMsg],
                chat_id: caseChatId ?? undefined,
                model: resolveChatModel(profile?.llm ?? null),
                signal: chatAbort.signal,
            });
            clearTimeout(timeout);
            if (!resp.ok) {
                let detail = `HTTP ${resp.status}`;
                try { const j = await resp.json(); detail = j.detail || j.error || detail; } catch { /* ignore */ }
                throw new Error(detail);
            }
            const reader = resp.body?.getReader();
            if (!reader) throw new Error("No body");
            const decoder = new TextDecoder();
            let buffer = "";
            let assistantText = "";
            const events: AssistantEvent[] = [];
            // Placeholder assistant bubble already appended on send. The case
            // chat runs the SAME agentic chat root as the global assistant, so
            // it streams real reasoning, tool activity and redline edits — we
            // fold those into `events` (mirroring the assistant) and update the
            // last message in place, keeping this case's own history.
            const flush = () => {
                const txt = assistantText;
                const evs = events.length ? [...events] : undefined;
                setChatMessages((prev) => {
                    const next = [...prev];
                    next[next.length - 1] = {
                        role: "assistant",
                        content: txt,
                        events: evs,
                    };
                    return next;
                });
            };
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
                    try {
                        const data = JSON.parse(dataStr);
                        switch (data.type) {
                            case "chat_id":
                                // Emitted once when the backend creates a fresh
                                // chat row; capture it so the rest of this
                                // session (and the next reload) resume the same
                                // conversation instead of forking a new one.
                                if (typeof data.chatId === "string") setCaseChatId(data.chatId);
                                break;
                            case "content_delta":
                                assistantText += data.text ?? "";
                                break;
                            case "reasoning_delta": {
                                const last = events[events.length - 1];
                                if (last?.type === "reasoning" && last.isStreaming) {
                                    events[events.length - 1] = { type: "reasoning", text: last.text + (data.text ?? ""), isStreaming: true };
                                } else {
                                    events.push({ type: "reasoning", text: data.text ?? "", isStreaming: true });
                                }
                                break;
                            }
                            case "reasoning_block_end": {
                                const last = events[events.length - 1];
                                if (last?.type === "reasoning" && last.isStreaming) {
                                    events[events.length - 1] = { type: "reasoning", text: last.text };
                                }
                                break;
                            }
                            case "tool_call_start":
                                events.push({ type: "tool_call_start", name: data.name, isStreaming: true });
                                break;
                            case "tool_call_progress": {
                                for (let i = events.length - 1; i >= 0; i--) {
                                    const e = events[i];
                                    if (e.type === "tool_call_start" && e.isStreaming) {
                                        events[i] = { ...e, elapsedSecs: typeof data.elapsed_secs === "number" ? data.elapsed_secs : e.elapsedSecs };
                                        break;
                                    }
                                }
                                break;
                            }
                            case "doc_created_start":
                                events.push({ type: "doc_created", filename: data.filename, download_url: "", isStreaming: true });
                                break;
                            case "doc_created": {
                                const idx = events.findIndex((e) => e.type === "doc_created" && e.isStreaming && e.filename === data.filename);
                                const doc: AssistantEvent = {
                                    type: "doc_created",
                                    filename: data.filename,
                                    download_url: typeof data.download_url === "string" ? data.download_url : "",
                                    document_id: typeof data.document_id === "string" ? data.document_id : undefined,
                                    version_id: typeof data.version_id === "string" ? data.version_id : undefined,
                                    version_number: typeof data.version_number === "number" ? data.version_number : undefined,
                                    isStreaming: false,
                                };
                                if (idx >= 0) events[idx] = doc; else events.push(doc);
                                break;
                            }
                            case "doc_edited_start":
                                events.push({ type: "doc_edited", filename: data.filename, document_id: "", version_id: "", download_url: "", annotations: [], isStreaming: true });
                                break;
                            case "doc_edited": {
                                const idx = events.findIndex((e) => e.type === "doc_edited" && e.isStreaming && e.filename === data.filename);
                                const ed: AssistantEvent = {
                                    type: "doc_edited",
                                    filename: data.filename,
                                    document_id: typeof data.document_id === "string" ? data.document_id : "",
                                    version_id: typeof data.version_id === "string" ? data.version_id : "",
                                    version_number: typeof data.version_number === "number" ? data.version_number : null,
                                    download_url: typeof data.download_url === "string" ? data.download_url : "",
                                    annotations: Array.isArray(data.annotations) ? data.annotations : [],
                                    error: typeof data.error === "string" ? data.error : undefined,
                                    isStreaming: false,
                                };
                                if (idx >= 0) events[idx] = ed; else events.push(ed);
                                break;
                            }
                            default:
                                break;
                        }
                        flush();
                    } catch {
                        // skip malformed event
                    }
                }
            }
            // Stream done — strip the <CITATIONS> block out of the visible text,
            // attach parsed citations, and keep the streamed agentic events.
            const { cleaned, citations: parsedCitations } = extractCaseChatCitations(assistantText);
            const finalEvents = events.length ? [...events] : undefined;
            setChatMessages((prev) => {
                const next = [...prev];
                next[next.length - 1] = {
                    role: "assistant",
                    content: cleaned,
                    citations: parsedCitations.length > 0 ? parsedCitations : undefined,
                    events: finalEvents,
                };
                return next;
            });
        } catch (e) {
            console.error("[case-chat] error", e);
            const errMsg = e instanceof Error ? e.message : "Unknown error";
            // Replace the empty placeholder bubble (still showing the dots)
            // with the error, so it doesn't spin forever.
            setChatMessages((prev) => {
                const next = [...prev];
                const errText = `Something went wrong: ${errMsg}. Check that your LLM is configured in Settings.`;
                if (next.length > 0 && next[next.length - 1].role === "assistant" && next[next.length - 1].content === "") {
                    next[next.length - 1] = { role: "assistant", content: errText };
                    return next;
                }
                return [...next, { role: "assistant", content: errText }];
            });
        } finally {
            setChatLoading(false);
        }
    }

    // -----------------------------------------------------------------------
    // Render
    // -----------------------------------------------------------------------

    if (loading) {
        return (
            <div className="flex h-full items-center justify-center">
                <div className="h-6 w-6 animate-spin rounded-full border-2 border-gray-300 border-t-gray-700" />
            </div>
        );
    }

    if (!detail || !caseInfo) {
        return (
            <div className="flex h-full flex-col items-center justify-center text-center px-6">
                <p className="text-sm text-gray-600 mb-3">
                    Case not found.
                </p>
                <button
                    onClick={() => router.push("/cases")}
                    className="text-sm text-gray-700 hover:text-gray-900 underline"
                >
                    {tCommon("back")}
                </button>
            </div>
        );
    }

    const tabs: { id: Tab; label: string }[] = [
        { id: "overview", label: t("overview") },
        { id: "findings", label: t("findings") },
        { id: "outputs", label: t("outputs") },
        { id: "chat", label: t("chat") },
        { id: "registry", label: "Registry" },
    ];

    return (
        <div className="relative flex h-full overflow-hidden">
            {/* Advisory (Outputs tab): generation runs one at a time; rapid
                repeat clicks overlap requests and muddle the result. Anchored to
                the case workspace, not the viewport, so it tracks the nav sidebar
                when it collapses; the left offset only clears the 256px case
                sidebar. pointer-events-none so it never blocks the controls under
                it. */}
            {activeTab === "outputs" && (
                <div className="pointer-events-none absolute bottom-4 left-[272px] z-40 flex max-w-[248px] items-start gap-2.5 rounded-[10px] border border-border bg-card/95 px-3.5 py-2.5 shadow-sm backdrop-blur-sm">
                    <Info className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    <div className="min-w-0">
                        <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground">
                            One at a time
                        </p>
                        <p className="mt-0.5 text-[12px] leading-snug text-muted-foreground">
                            Let each output finish before starting the next. Pressing the
                            buttons in quick succession overlaps the requests and can muddle
                            the result.
                        </p>
                    </div>
                </div>
            )}

            {/* Left sidebar — case metadata & docs */}
            <div className="w-64 shrink-0 border-r border-gray-200 flex flex-col overflow-y-auto bg-white">
                {/* Back + Demo toggle */}
                <div className="flex items-center justify-between px-4 py-3 border-b border-gray-100">
                    <button
                        onClick={() => router.push("/cases")}
                        className="p-1 rounded hover:bg-gray-100 transition-colors text-gray-500 hover:text-gray-700"
                    >
                        <ArrowLeft className="h-3.5 w-3.5" />
                    </button>
                    <button
                        onClick={() => setDemoMode(!demoMode)}
                        className={`flex items-center gap-1.5 px-2.5 py-1 rounded-md text-xs transition-colors ${
                            demoMode
                                ? "bg-gray-900 text-white"
                                : "border border-gray-200 bg-white text-gray-500 hover:text-gray-700 hover:bg-gray-50"
                        }`}
                        title={demoMode ? "Demo mode: names blurred" : "Enable demo mode to blur party names"}
                    >
                        {demoMode ? <EyeOff className="h-3 w-3" /> : <Eye className="h-3 w-3" />}
                        {demoMode ? "Demo Mode" : "Demo"}
                    </button>
                </div>

                <div className="px-4 py-4 space-y-4">
                    {/* Title (inline editable) */}
                    <div>
                        {editingTitle ? (
                            <input
                                autoFocus
                                value={titleDraft}
                                onChange={(e) => setTitleDraft(e.target.value)}
                                onBlur={saveTitle}
                                onKeyDown={(e) => {
                                    if (e.key === "Enter") saveTitle();
                                    if (e.key === "Escape") setEditingTitle(false);
                                }}
                                className="w-full text-sm font-medium text-gray-900 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-ring"
                            />
                        ) : (
                            <button
                                onClick={() => {
                                    setTitleDraft(caseInfo.title);
                                    setEditingTitle(true);
                                }}
                                className="group flex items-center gap-1 text-left"
                            >
                                <h2 className="text-sm font-medium text-gray-900 line-clamp-3">
                                    {blurNames.length > 0 ? blurPartyNames(caseInfo.title, blurNames) : caseInfo.title}
                                </h2>
                                <Pencil className="h-3 w-3 text-gray-400 opacity-0 group-hover:opacity-100 transition-opacity shrink-0" />
                            </button>
                        )}
                    </div>

                    {/* Court */}
                    <div>
                        <p className="text-[11px] font-medium text-gray-500 mb-1">
                            {t("court")}
                        </p>
                        {editingCourt ? (
                            <input
                                autoFocus
                                value={courtDraft}
                                onChange={(e) => setCourtDraft(e.target.value)}
                                onBlur={saveCourt}
                                onKeyDown={(e) => {
                                    if (e.key === "Enter") saveCourt();
                                    if (e.key === "Escape") setEditingCourt(false);
                                }}
                                placeholder={t("courtPlaceholder")}
                                className="w-full text-xs text-gray-700 border border-gray-200 rounded px-2 py-1 focus:outline-none focus:ring-1 focus:ring-ring"
                            />
                        ) : (
                            <button
                                onClick={() => {
                                    setCourtDraft(caseInfo.court ?? "");
                                    setEditingCourt(true);
                                }}
                                className="group flex items-center gap-1"
                            >
                                <span className="text-xs text-gray-700">
                                    {caseInfo.court ? (blurNames.length > 0 ? blurPartyNames(caseInfo.court, blurNames) : caseInfo.court) : "—"}
                                </span>
                                <Pencil className="h-2.5 w-2.5 text-gray-400 opacity-0 group-hover:opacity-100 transition-opacity" />
                            </button>
                        )}
                    </div>

                    {/* Parties */}
                    <div>
                        <p className="text-[11px] font-medium text-gray-500 mb-1">
                            {t("parties")}
                        </p>
                        {parties.length === 0 ? (
                            <span className="text-xs text-gray-400">—</span>
                        ) : (
                            <ul className="space-y-0.5">
                                {parties.map((p, i) => (
                                    <li
                                        key={i}
                                        className="text-xs text-gray-700"
                                    >
                                        <span className="text-gray-500">
                                            {p.role}:
                                        </span>{" "}
                                        {blurNames.length > 0 ? blurPartyNames(p.name, blurNames) : p.name}
                                    </li>
                                ))}
                            </ul>
                        )}
                    </div>
                </div>

                {/* Documents section */}
                <div className="border-t border-gray-100 px-4 py-3 flex-1 min-h-0 flex flex-col">
                    <div className="flex items-center justify-between mb-2">
                        <p className="text-[11px] font-medium text-gray-500">
                            {t("documents")} ({documents.length})
                        </p>
                        <div className="flex items-center gap-2">
                            <button
                                onClick={() => uploadInputRef.current?.click()}
                                disabled={uploading}
                                className="text-xs text-gray-500 hover:text-gray-700 transition-colors disabled:opacity-50"
                                title="Upload files directly to this case (PDF, DOCX, images for OCR)"
                            >
                                {uploading ? (
                                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                                ) : (
                                    <Upload className="h-3.5 w-3.5" />
                                )}
                            </button>
                            <button
                                onClick={() => setShowDocPicker(true)}
                                className="p-1 border border-gray-200 bg-white text-gray-700 hover:bg-gray-100 rounded-md transition-colors"
                                title={t("addDocuments")}
                            >
                                <Plus className="h-3.5 w-3.5" />
                            </button>
                        </div>
                    </div>
                    <input
                        ref={uploadInputRef}
                        type="file"
                        multiple
                        accept=".pdf,.docx,.doc,.png,.jpg,.jpeg,.tiff,.tif"
                        className="hidden"
                        onChange={(e) => handleDirectUpload(e.target.files)}
                    />
                    {uploading && uploadProgress && (
                        <p className="text-[10px] text-gray-400 mb-1">
                            Uploading {uploadProgress}…
                        </p>
                    )}
                    {documents.length === 0 ? (
                        <p className="text-xs text-gray-400 py-2">
                            {t("noDocuments")}
                        </p>
                    ) : (
                        <div className="space-y-1 overflow-y-auto flex-1">
                            {documents.map((doc) => {
                                const displayName = getDocDisplayName(doc);
                                const ext = (doc.filename ?? "").split(".").pop()?.toUpperCase();
                                return (
                                <div
                                    key={doc.document_id}
                                    className="group flex items-center gap-2 rounded px-2 py-1.5 hover:bg-gray-50 transition-colors"
                                >
                                    {editingDocId === doc.document_id ? (
                                        <div className="flex-1 min-w-0 flex items-center gap-1.5">
                                            <FileText className="h-3.5 w-3.5 text-gray-400 shrink-0" />
                                            <input
                                                autoFocus
                                                value={docNameDraft}
                                                onChange={(e) => setDocNameDraft(e.target.value)}
                                                onBlur={() => saveDocName(doc.document_id)}
                                                onKeyDown={(e) => {
                                                    if (e.key === "Enter") saveDocName(doc.document_id);
                                                    if (e.key === "Escape") setEditingDocId(null);
                                                }}
                                                className="flex-1 min-w-0 text-xs text-gray-700 border border-gray-200 rounded px-1.5 py-0.5 focus:outline-none focus:ring-1 focus:ring-ring"
                                            />
                                        </div>
                                    ) : (
                                        <button
                                            onClick={() => {
                                                setOpenDocId(doc.document_id);
                                                setOpenDocFilename(displayName);
                                            }}
                                            onDoubleClick={(e) => {
                                                e.preventDefault();
                                                setDocNameDraft(displayName);
                                                setEditingDocId(doc.document_id);
                                            }}
                                            className="flex-1 min-w-0 flex flex-col gap-0.5 text-left"
                                            title="Double-click to rename"
                                        >
                                            <div className="flex items-center gap-1.5">
                                                <FileText className="h-3.5 w-3.5 text-gray-400 shrink-0" />
                                                <span className="text-xs text-gray-700 truncate">
                                                    {displayName}
                                                </span>
                                            </div>
                                            {(doc.page_count != null || doc.size_bytes != null) && (
                                                <span className="text-[10px] text-gray-400 ml-5">
                                                    {doc.page_count != null && `${doc.page_count}p`}
                                                    {doc.page_count != null && doc.size_bytes != null && " · "}
                                                    {doc.size_bytes != null && (doc.size_bytes < 1024 * 1024
                                                        ? `${(doc.size_bytes / 1024).toFixed(0)} KB`
                                                        : `${(doc.size_bytes / (1024 * 1024)).toFixed(1)} MB`)}
                                                </span>
                                            )}
                                        </button>
                                    )}
                                    {ext && (
                                        <span className="shrink-0 text-[10px] uppercase tracking-wide text-gray-400">{ext}</span>
                                    )}
                                    <button
                                        onClick={() =>
                                            handleRemoveDoc(doc.document_id)
                                        }
                                        className="shrink-0 opacity-0 group-hover:opacity-100 text-gray-400 hover:text-red-500 transition-all"
                                        title={t("removeDocument")}
                                    >
                                        <Trash2 className="h-3 w-3" />
                                    </button>
                                </div>
                                );
                            })}
                            {/* Aggregate stats card */}
                            {(totalPages > 0 || totalSizeBytes > 0) && (
                                <div className="mt-2 rounded-md bg-gray-50 px-2 py-1.5">
                                    <p className="text-[10px] text-gray-500">
                                        {documents.length} docs{totalPages > 0 && `, ${totalPages} pages`}
                                        {totalSizeBytes > 0 && ` · ${totalSizeBytes < 1024 * 1024
                                            ? `${(totalSizeBytes / 1024).toFixed(0)} KB`
                                            : `${(totalSizeBytes / (1024 * 1024)).toFixed(1)} MB`}`}
                                    </p>
                                </div>
                            )}
                            {/* OCR warning for large scanned PDFs */}
                            {documents.some((d) => d.file_type === "pdf" && (d.page_count ?? 0) > 50 && d.needs_ocr) && (
                                <div className="mt-1 rounded-md bg-amber-50 border border-amber-200 px-2 py-1.5">
                                    <p className="text-[10px] text-amber-700">
                                        ⚠ Large scanned PDF may need OCR (~2 min)
                                    </p>
                                </div>
                            )}
                        </div>
                    )}
                </div>

                {/* Run Analysis */}
                <div className="border-t border-gray-100 p-4 space-y-3">
                    <div className="flex items-center justify-between group relative">
                        <span className="text-xs font-medium text-gray-600 flex items-center gap-1">
                            <ShieldCheck className="h-3.5 w-3.5" />
                            {t("redactPii")}
                        </span>
                        <button
                            type="button"
                            role="switch"
                            aria-checked={analysisRedactPii}
                            onClick={() => setAnalysisRedactPii(!analysisRedactPii)}
                            className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ${
                                analysisRedactPii ? "bg-blue-600" : "bg-gray-200"
                            }`}
                        >
                            <span
                                className={`pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ${
                                    analysisRedactPii ? "translate-x-4" : "translate-x-0"
                                }`}
                            />
                        </button>
                        <div className="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 px-3 py-2 bg-gray-900 text-white text-xs rounded-lg opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none whitespace-nowrap z-50">
                            {t("redactPiiTooltip")}
                        </div>
                    </div>
                    <button
                        onClick={handleRunAnalysis}
                        disabled={analysisRunning || documents.length === 0 || isOffline}
                        title={isOffline ? tCommon("offlineBlocked") : undefined}
                        className="w-full inline-flex items-center justify-center gap-1.5 rounded-md bg-gray-900 px-3 py-2 text-xs font-medium text-white hover:bg-gray-800 disabled:opacity-50 transition-colors"
                    >
                        {analysisRunning ? (
                            <>
                                <MikeIcon spin size={14} />
                                {t("analysisRunning")}
                            </>
                        ) : (
                            t("runAnalysis")
                        )}
                    </button>
                </div>
            </div>

            {/* Center — tabs + content */}
            <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
                {/* Tab bar */}
                <div className="shrink-0">
                    <ToolbarTabs tabs={tabs} active={activeTab} onChange={setActiveTab} />
                </div>

                {/* Documents that produced no readable text (failed OCR / no text
                    layer). Surfaced prominently on every tab so the user knows the
                    agents never saw this content, rather than it being skipped
                    silently. Uses the design-system placeholder amber (#fdeede /
                    #9a4a00). */}
                {noTextDocs.length > 0 && (
                    <div className="shrink-0 mx-6 mt-4 rounded-[10px] border border-[#f0d8a8] bg-[#fdeede] px-4 py-3">
                        <div className="flex items-start gap-2.5">
                            <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-[#9a4a00]" />
                            <div className="min-w-0">
                                <p className="text-[13px] font-semibold text-[#9a4a00]">
                                    {noTextDocs.length} document{noTextDocs.length > 1 ? "s" : ""} produced no readable text
                                </p>
                                <p className="mt-0.5 text-[12px] leading-snug text-[#9a4a00]">
                                    The agents could not read {noTextDocs.length > 1 ? "these files" : "this file"} because no text was extracted. If {noTextDocs.length > 1 ? "they are" : "it is"} a photo or a scan, OCR may have failed. Re-upload a clearer copy or a searchable PDF, then run the analysis again.
                                </p>
                                <ul className="mt-1.5 flex flex-wrap gap-1.5">
                                    {noTextDocs.map((fn) => (
                                        <li
                                            key={fn}
                                            className="rounded-md border border-[#e6c896] bg-white/70 px-2 py-0.5 text-[11px] text-[#9a4a00]"
                                        >
                                            {fn}
                                        </li>
                                    ))}
                                </ul>
                            </div>
                        </div>
                    </div>
                )}

                {/* Tab content */}
                <div className="flex-1 overflow-y-auto animate-[fadeIn_150ms_ease]" key={activeTab}>
                    {activeTab === "overview" && (
                        <OverviewTab
                            summaryFinding={summaryFinding ?? null}
                            blurNames={blurNames}
                            t={t}
                        />
                    )}
                    {activeTab === "findings" && (
                        analysisRunning ? (
                            <div className="flex flex-col h-full overflow-hidden">
                                <AnalysisStatsBar
                                    totalPages={analysisEstimate?.totalPages ?? totalPages}
                                    totalSizeBytes={totalSizeBytes}
                                    elapsedSeconds={elapsedSeconds}
                                    estimate={analysisEstimate}
                                    analysisRunning={analysisRunning}
                                    completed={false}
                                    findingsCount={findings.length}
                                    agentsDone={agentProgress.filter((a) => a.status === "done").length}
                                />
                                <HeartbeatBand
                                    currentPhase={currentPhase}
                                    findingsCount={findings.length}
                                    agentProgress={agentProgress}
                                    onAbort={handleAbortAnalysis}
                                />
                                {/* Stuck state rescue */}
                                {analysisEstimate && elapsedSeconds > 150 && !stuckDismissed && (
                                    <StuckStateRescue
                                        elapsedSeconds={elapsedSeconds}
                                        totalPages={analysisEstimate.totalPages}
                                        onKeepWaiting={() => setStuckDismissed(true)}
                                        onAbort={handleAbortAnalysis}
                                    />
                                )}
                                {/* OCR timeout warning */}
                                {analysisEstimate && elapsedSeconds <= 150 && (
                                    <OcrTimeoutWarning
                                        elapsedSeconds={elapsedSeconds}
                                        estimatedSeconds={analysisEstimate.estimatedSeconds}
                                    />
                                )}
                                {/* Two-column layout */}
                                <div className="flex flex-1 min-h-0 overflow-hidden">
                                    <ProgressChecklist
                                        currentPhase={currentPhase}
                                        extractions={extractions}
                                        agentProgress={agentProgress}
                                        estimate={analysisEstimate}
                                        elapsedSeconds={elapsedSeconds}
                                        demoMode={demoMode}
                                    />
                                    <InsightFeed
                                        items={enrichedFeedItems}
                                        activeAgents={agentProgress}
                                        demoMode={demoMode}
                                        blurNames={blurNames}
                                    />
                                </div>
                            </div>
                        ) : (
                            <>
                                {analysisCompleted && (
                                    <AnalysisStatsBar
                                        totalPages={analysisEstimate?.totalPages ?? totalPages}
                                        totalSizeBytes={totalSizeBytes}
                                        elapsedSeconds={elapsedSeconds}
                                        estimate={analysisEstimate}
                                        analysisRunning={false}
                                        completed={true}
                                        findingsCount={findings.length}
                                        agentsDone={agentProgress.filter((a) => a.status === "done").length}
                                    />
                                )}
                                <FindingsTab
                                    caseId={caseId}
                                    findingsByAgent={findingsByAgent}
                                    agentProgress={agentProgress}
                                    analysisRunning={analysisRunning}
                                    analysisError={analysisError}
                                    analysisStage={analysisStage}
                                    expandedAgents={expandedAgents}
                                    setExpandedAgents={setExpandedAgents}
                                    blurNames={blurNames}
                                    t={t}
                                />
                            </>
                        )
                    )}
                    {activeTab === "outputs" && (
                        <OutputsTab
                            outputs={outputs}
                            generatingOutput={generatingOutput}
                            outputError={outputError}
                            onGenerate={(type, redactPii) => handleGenerateOutput(type, redactPii)}
                            onOpenDoc={(docId, filename, output) => {
                                setOpenDocId(docId);
                                setOpenDocFilename(filename);
                                setOpenDocOutput(output ?? null);
                            }}
                            blurNames={blurNames}
                            t={t}
                        />
                    )}
                    {activeTab === "chat" && (
                        <ChatTab
                            messages={chatMessages}
                            input={chatInput}
                            loading={chatLoading}
                            onInputChange={setChatInput}
                            onSend={handleChatSend}
                            chatEndRef={chatEndRef}
                            blurNames={blurNames}
                            t={t}
                            selectedWorkflow={selectedWorkflow}
                            onSelectWorkflow={(wf) => setSelectedWorkflow(wf)}
                            onClearWorkflow={() => setSelectedWorkflow(null)}
                            onNewConversation={() => {
                                setChatMessages([]);
                                setCaseChatId(null);
                            }}
                        />
                    )}
                    {activeTab === "registry" && (
                        <RegistryTab caseId={caseId} documents={documents} hasFindings={findings.length > 0} />
                    )}
                </div>
            </div>

            {/* Right — Doc panel */}
            {openDocId && (
                <div className="w-[400px] shrink-0 border-l border-gray-200 flex flex-col bg-white shadow-[-4px_0_12px_rgba(0,0,0,0.02)]">
                    <div className="flex items-center justify-between px-3 py-2 border-b border-gray-100">
                        <span className="text-xs text-gray-600 truncate">
                            {openDocFilename}
                        </span>
                        <button
                            onClick={() => {
                                setOpenDocId(null);
                                setOpenDocOutput(null);
                            }}
                            className="p-1 rounded hover:bg-gray-100 text-gray-400 hover:text-gray-700 transition-colors"
                        >
                            <X className="h-3.5 w-3.5" />
                        </button>
                    </div>
                    <div className="flex-1 overflow-hidden">
                        <DocPanel
                            documentId={openDocId}
                            filename={openDocFilename}
                            versionId={null}
                            versionNumber={null}
                            mode={{ kind: "document" }}
                        />
                    </div>
                </div>
            )}

            {/* Doc picker modal */}
            {showDocPicker && (
                <DocPickerModal
                    caseId={caseId}
                    existingDocIds={documents.map((d) => d.document_id)}
                    onClose={() => setShowDocPicker(false)}
                    onAttached={() => {
                        setShowDocPicker(false);
                        load();
                    }}
                    t={t}
                    tCommon={tCommon}
                />
            )}
        </div>
    );
}

// ---------------------------------------------------------------------------
// Overview tab
// ---------------------------------------------------------------------------

function OverviewTab({
    summaryFinding,
    blurNames,
    t,
}: {
    summaryFinding: CaseFinding | null;
    blurNames: string[];
    t: ReturnType<typeof useTranslations<"Cases">>;
}) {
    if (!summaryFinding) {
        return (
            <div className="flex flex-col items-center justify-center py-20 px-6 text-center">
                <p className="text-sm text-gray-500">{t("noSummary")}</p>
            </div>
        );
    }
    const content = parseFindingContent(summaryFinding.content_json);
    const citations = collectCitations(content);

    return (
        <div className="px-6 py-6 max-w-3xl">
            <div className="rounded-lg border border-gray-200 bg-white p-5">
                <h3 className="text-xs font-medium text-gray-500 mb-3">
                    {t("case_summary")}
                </h3>
                {renderFindingContent(content, "case_summary", blurNames)}
                {citations.length > 0 && (
                    <div className="mt-4 border-t border-gray-100 pt-3">
                        <p className="text-[10px] font-medium text-gray-400 uppercase tracking-wide mb-1">Sources ({citations.length})</p>
                        <div className="space-y-1">
                            {citations.map((c, i) => (
                                <div key={i} className="flex items-start gap-2 text-[11px]">
                                    <span className="shrink-0 inline-flex items-center rounded-full bg-gray-100 text-gray-900 hover:bg-gray-200 px-1.5 py-0.5 font-medium">{c.source_doc_id}</span>
                                    <span className="text-gray-500 italic leading-snug">&ldquo;{c.exact_quote}&rdquo;</span>
                                </div>
                            ))}
                        </div>
                    </div>
                )}
            </div>
        </div>
    );
}

// ---------------------------------------------------------------------------
// Findings tab
// ---------------------------------------------------------------------------

function ConfidenceMeter({ confidence }: { confidence: number }) {
    const conf = Math.max(0, Math.min(100, Math.round(confidence)));
    const bar = conf >= 70 ? "bg-green-500" : conf >= 40 ? "bg-amber-500" : "bg-red-500";
    const text = conf >= 70 ? "text-green-700" : conf >= 40 ? "text-amber-700" : "text-red-700";
    return (
        <div className="mt-1 flex items-center gap-2">
            <div className="h-1.5 w-24 overflow-hidden rounded-full bg-gray-200">
                <div className={`h-full rounded-full ${bar}`} style={{ width: `${conf}%` }} />
            </div>
            <span className={`text-[10px] font-medium ${text}`}>{conf}% confident</span>
            {conf < 40 && <span className="text-[10px] font-medium text-red-700">· low — review</span>}
        </div>
    );
}

function PrecedentCaseRow({ c }: { c: ResolvedPrecedentCase }) {
    return (
        <div className="text-[12px]">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                {c.kanoon_url ? (
                    <a
                        href={c.kanoon_url}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="font-medium text-blue-600 underline hover:text-blue-700"
                    >
                        {c.title ?? `Document ${c.tid}`}
                    </a>
                ) : (
                    <span className="font-medium text-gray-900">{c.title ?? `Document ${c.tid}`}</span>
                )}
                {c.court && <span className="text-[11px] text-gray-500">{c.court}</span>}
                <KanoonVerifyBadge tid={c.tid} title={c.title ?? ""} />
            </div>
            {typeof c.confidence === "number" && <ConfidenceMeter confidence={c.confidence} />}
            {c.reason && <p className="mt-1 text-[11px] leading-snug text-gray-500">{c.reason}</p>}
            {(c.relevant_paragraphs || c.snippet) && (
                <p className="mt-1 text-[11px] italic leading-snug text-gray-400">
                    &ldquo;{c.relevant_paragraphs || c.snippet}&rdquo;
                </p>
            )}
        </div>
    );
}

/** Auto-resolves precedent_finder suggestions into real Kanoon cases (with AI
 *  confidence) when the precedent finding is shown — no manual button. */
function PrecedentResolver({ caseId }: { caseId: string }) {
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState<string | null>(null);
    const [resolved, setResolved] = useState<ResolvedPrecedent[] | null>(null);

    useEffect(() => {
        let cancelled = false;
        (async () => {
            setLoading(true);
            setError(null);
            try {
                const data = await resolveCasePrecedents(caseId);
                if (!cancelled) setResolved(data);
            } catch (e) {
                if (!cancelled) setError(e instanceof Error ? e.message : "Failed to resolve precedents");
            } finally {
                if (!cancelled) setLoading(false);
            }
        })();
        return () => {
            cancelled = true;
        };
    }, [caseId]);

    return (
        <div className="mt-3 border-t border-gray-100 pt-3">
            <p className="mb-2 text-[10px] font-medium uppercase tracking-wide text-gray-400">
                Matched authorities (Indian Kanoon)
            </p>
            {loading && (
                <div className="flex items-center gap-2 text-xs text-gray-500">
                    <Loader2 className="h-3.5 w-3.5 animate-spin" /> Looking up & scoring precedents…
                </div>
            )}
            {error && <p className="text-xs text-red-600">{error}</p>}
            {!loading && !error && resolved && resolved.length === 0 && (
                <p className="text-xs text-gray-500">No precedents to resolve.</p>
            )}
            {!loading && resolved && resolved.length > 0 && (
                <div className="space-y-3">
                    {resolved.map((p, i) => (
                        <div key={i} className="rounded-md border border-gray-100 bg-gray-50 px-3 py-2">
                            {p.point_of_law && (
                                <p className="text-xs font-medium text-gray-800">{p.point_of_law}</p>
                            )}
                            {p.cases.length === 0 ? (
                                <p className="mt-1 text-[11px] text-gray-400">No matching case found.</p>
                            ) : (
                                <div className="mt-2 space-y-2.5">
                                    {p.cases.map((c, j) => (
                                        <PrecedentCaseRow key={j} c={c} />
                                    ))}
                                </div>
                            )}
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
}

function FindingsTab({
    findingsByAgent,
    agentProgress,
    analysisRunning,
    analysisError,
    analysisStage,
    expandedAgents,
    setExpandedAgents,
    blurNames,
    caseId,
    t,
}: {
    findingsByAgent: Map<string, CaseFinding[]>;
    agentProgress: AnalysisProgress[];
    caseId: string;
    analysisRunning: boolean;
    analysisError: string | null;
    analysisStage: string | null;
    expandedAgents: Set<string>;
    setExpandedAgents: React.Dispatch<React.SetStateAction<Set<string>>>;
    blurNames: string[];
    t: ReturnType<typeof useTranslations<"Cases">>;
}) {
    const toggleAgent = (name: string) => {
        setExpandedAgents((prev) => {
            const next = new Set(prev);
            if (next.has(name)) next.delete(name);
            else next.add(name);
            return next;
        });
    };

    // When viewing a finished analysis (e.g. after a reload) the agent cards
    // arrive collapsed, so results read as a wall of closed accordions. Expand
    // them once so findings are visible immediately; the user can still
    // collapse any card afterwards.
    const didAutoExpandRef = useRef(false);
    useEffect(() => {
        if (didAutoExpandRef.current || analysisRunning) return;
        if (findingsByAgent.size > 0) {
            didAutoExpandRef.current = true;
            setExpandedAgents((prev) => (prev.size === 0 ? new Set(findingsByAgent.keys()) : prev));
        }
    }, [analysisRunning, findingsByAgent, setExpandedAgents]);

    if (findingsByAgent.size === 0 && !analysisRunning && !analysisError) {
        return (
            <div className="flex flex-col items-center justify-center py-20 px-6 text-center">
                <Search className="h-8 w-8 text-gray-300 mb-3" />
                <p className="text-sm font-medium text-gray-700 mb-1">
                    {t("noFindings")}
                </p>
                <p className="text-xs text-gray-500 max-w-sm">
                    {t("noFindingsHint")}
                </p>
            </div>
        );
    }

    // Show agent progress during analysis
    const allAgents = Array.from(
        new Set([
            ...agentProgress.map((p) => p.agent_name),
            ...findingsByAgent.keys(),
        ]),
    );

    return (
        <div className="px-6 py-6 max-w-3xl space-y-3">
            {/* Stage banner (e.g. text extraction / OCR in progress) */}
            {analysisRunning && analysisStage && (
                <div className="rounded-lg border border-blue-200 bg-blue-50 p-3 mb-2 flex items-center gap-2">
                    <Loader2 className="h-3 w-3 animate-spin text-blue-500 shrink-0" />
                    <p className="text-xs text-blue-700">{analysisStage}</p>
                </div>
            )}
            {/* Progress indicators during analysis */}
            {agentProgress.length > 0 && (
                <PreResponseWrapper
                    stepCount={agentProgress.length}
                    shouldMinimize={!analysisRunning}
                    isStreaming={analysisRunning}
                >
                    {agentProgress.map((ap) => (
                        <AgentThinkingRow key={ap.agent_name} agent={ap} t={t} />
                    ))}
                </PreResponseWrapper>
            )}

            {/* Error banner */}
            {analysisError && (
                <div className="rounded-lg border border-red-200 bg-red-50 p-4 mb-4">
                    <p className="text-xs font-medium text-red-700 mb-1">Analysis failed</p>
                    <p className="text-xs text-red-600">{analysisError}</p>
                </div>
            )}

            {/* Finding cards */}
            {Array.from(findingsByAgent.entries()).map(
                ([agentName, agentFindings]) => {
                    const isExpanded = expandedAgents.has(agentName);
                    const label =
                        AGENT_LABELS[agentName] ?? agentName.toLowerCase();
                    return (
                        <div
                            key={agentName}
                            className="rounded-lg border border-gray-200 bg-white overflow-hidden"
                        >
                            <button
                                onClick={() => toggleAgent(agentName)}
                                className="w-full flex items-center gap-2 px-4 py-3 text-left hover:bg-gray-50 transition-colors"
                            >
                                <ChevronDown className={`h-3.5 w-3.5 text-gray-400 transition-transform duration-200 ${isExpanded ? "" : "-rotate-90"}`} />
                                <span className="text-sm font-medium text-gray-900">
                                    {t(label)}
                                </span>
                                <span className="ml-auto text-[10px] text-gray-400">
                                    {agentFindings.length}
                                </span>
                            </button>
                            {isExpanded && (
                                <div className="border-t border-gray-100 px-4 py-3 space-y-3 animate-[fadeIn_200ms_ease]">
                                    {agentFindings.map((f) => {
                                        const content = parseFindingContent(
                                            f.content_json,
                                        );
                                        const citations = collectCitations(content);
                                        const hasError = typeof content.error === "string";

                                        // Grounding validation stats from orchestrator
                                        const groundingStats = f.grounding_json
                                            ? (() => {
                                                  try {
                                                      const g = typeof f.grounding_json === "string"
                                                          ? JSON.parse(f.grounding_json)
                                                          : f.grounding_json;
                                                      return g as { total_references?: number; verified?: number; unverified?: { doc_id: string; quote: string; reason: string }[] };
                                                  } catch { return null; }
                                              })()
                                            : null;

                                        return (
                                            <div key={f.id}>
                                                {hasError ? (
                                                    <div className="rounded-md bg-red-50 border border-red-100 px-3 py-2">
                                                        <p className="text-sm text-red-800">Agent error: {String(content.error)}</p>
                                                    </div>
                                                ) : (
                                                    renderFindingContent(content, agentName, blurNames)
                                                )}

                                                {/* Resolve precedent suggestions into real Kanoon cases */}
                                                {agentName === "precedent_finder" && !hasError && (
                                                    <PrecedentResolver caseId={caseId} />
                                                )}

                                                {/* Inline citations from agent grounding */}
                                                {citations.length > 0 && (
                                                    <div className="mt-3 border-t border-gray-100 pt-2">
                                                        <p className="text-[10px] font-medium text-gray-400 uppercase tracking-wide mb-1">Sources ({citations.length})</p>
                                                        <div className="space-y-1">
                                                            {citations.map((c, i) => (
                                                                <div key={i} className="flex items-start gap-2 text-[11px]">
                                                                    <span className="shrink-0 inline-flex items-center rounded-full bg-gray-100 text-gray-900 hover:bg-gray-200 px-1.5 py-0.5 font-medium">{c.source_doc_id}</span>
                                                                    <span className="text-gray-500 italic leading-snug">&ldquo;{c.exact_quote}&rdquo;</span>
                                                                </div>
                                                            ))}
                                                        </div>
                                                    </div>
                                                )}

                                                {/* Grounding validation stats */}
                                                {groundingStats && groundingStats.total_references != null && (
                                                    <div className="mt-2 flex items-center gap-2">
                                                        <span className="text-[10px] text-gray-400">
                                                            {groundingStats.verified}/{groundingStats.total_references} quotes verified
                                                        </span>
                                                        {groundingStats.unverified && groundingStats.unverified.length > 0 && (
                                                            <span className="text-[10px] text-amber-600">
                                                                ({groundingStats.unverified.length} unverified)
                                                            </span>
                                                        )}
                                                    </div>
                                                )}
                                            </div>
                                        );
                                    })}
                                </div>
                            )}
                        </div>
                    );
                },
            )}
        </div>
    );
}

function AgentThinkingRow({
    agent,
    t,
}: {
    agent: AnalysisProgress;
    t: ReturnType<typeof useTranslations<"Cases">>;
}) {
    const [snippet, setSnippet] = useState(getRandomSnippet);

    useEffect(() => {
        if (agent.status !== "running") return;
        const interval = setInterval(() => {
            setSnippet(getRandomSnippet());
        }, 2500);
        return () => clearInterval(interval);
    }, [agent.status]);

    // Real reasoning streamed from the backend's agent_thinking SSE. Show
    // the live tail so the row reflects what the agent is actually thinking;
    // fall back to a rotating snippet only until the first token lands so the
    // row is never a dead spinner.
    const liveThinking = (agent.thinking ?? "").replace(/\s+/g, " ").trim();
    const runningLabel = liveThinking
        ? liveThinking.length > 200
            ? "…" + liveThinking.slice(-200)
            : liveThinking
        : snippet;

    return (
        <div className="flex items-start gap-2">
            <MikeIcon
                spin={agent.status === "running"}
                done={agent.status === "done"}
                error={agent.status === "error"}
                size={14}
            />
            <span
                title={agent.status === "running" && liveThinking ? liveThinking : undefined}
                className={`text-sm font-serif min-w-0 ${
                    agent.status === "running"
                        ? "text-gray-500 italic line-clamp-2"
                        : agent.status === "error"
                          ? "text-red-600 text-xs"
                          : "text-gray-700"
                }`}
            >
                {agent.status === "running"
                    ? runningLabel
                    : agent.status === "error"
                      ? agent.error || "Error"
                      : t(AGENT_LABELS[agent.agent_name] ?? agent.agent_name)}
            </span>
        </div>
    );
}

// ---------------------------------------------------------------------------
// Outputs tab
// ---------------------------------------------------------------------------

function OutputsTab({
    outputs,
    generatingOutput,
    outputError,
    onGenerate,
    onOpenDoc,
    blurNames,
    t,
}: {
    outputs: CaseOutput[];
    generatingOutput: string | null;
    outputError: string | null;
    onGenerate: (type: string, redactPii?: boolean) => void;
    onOpenDoc: (docId: string, filename: string, output?: CaseOutput) => void;
    blurNames: string[];
    t: ReturnType<typeof useTranslations<"Cases">>;
}) {
    const [redactPii, setRedactPii] = useState(false);

    const outputActions = [
        { type: "case_brief", label: t("generateBrief") },
        { type: "strategy_memo", label: t("generateMemo") },
        { type: "hearing_prep", label: t("generateHearing") },
        { type: "list_of_dates", label: t("generateListOfDates") },
        { type: "annexure_index", label: t("generateAnnexureIndex") },
    ];

    return (
        <div className="px-6 py-6 max-w-3xl">
            {/* Generate buttons + redact toggle */}
            <div className="flex flex-wrap items-center gap-2 mb-6">
                {outputActions.map((a) => (
                    <button
                        key={a.type}
                        onClick={() => onGenerate(a.type, redactPii)}
                        disabled={generatingOutput !== null}
                        className="inline-flex items-center gap-2 rounded-xl border border-gray-200 bg-white px-4 py-2.5 text-xs font-medium text-gray-700 hover:bg-gray-50 hover:border-gray-300 disabled:opacity-50 transition-all"
                    >
                        {generatingOutput === a.type ? (
                            <MikeIcon spin size={14} />
                        ) : (
                            <FileText className="h-3.5 w-3.5 text-gray-400" />
                        )}
                        {generatingOutput === a.type
                            ? t("generating")
                            : a.label}
                    </button>
                ))}

                {/* Redact PII toggle */}
                <div className="ml-auto flex items-center gap-2 group relative">
                    <button
                        type="button"
                        role="switch"
                        aria-checked={redactPii}
                        onClick={() => setRedactPii(!redactPii)}
                        className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ${
                            redactPii ? "bg-gray-900" : "bg-gray-200"
                        }`}
                    >
                        <span
                            className={`pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ${
                                redactPii ? "translate-x-4" : "translate-x-0"
                            }`}
                        />
                    </button>
                    <span className="text-xs font-medium text-gray-600 flex items-center gap-1">
                        <ShieldCheck className="h-3.5 w-3.5" />
                        {t("redactPii")}
                    </span>
                    <div className="absolute bottom-full left-1/2 -translate-x-1/2 mb-2 px-3 py-2 bg-gray-900 text-white text-xs rounded-lg opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none whitespace-nowrap z-50">
                        {t("redactPiiTooltip")}
                    </div>
                </div>
            </div>

            {outputError && (
                <div className="rounded-lg border border-red-200 bg-red-50 p-3 mb-6">
                    <p className="text-xs font-medium text-red-700 mb-0.5">Couldn&rsquo;t generate</p>
                    <p className="text-xs text-red-600">{outputError}</p>
                </div>
            )}

            {/* Inline generating card — three-dot indicator with rotating
                drafting snippets while an output is being produced. */}
            {generatingOutput && (
                <div className="mike-msg-in mb-6 flex items-center gap-3 rounded-xl border border-gray-100 bg-gray-50 px-4 py-3.5">
                    <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-gray-100 bg-white">
                        <FileText className="h-4 w-4 text-gray-400" />
                    </div>
                    <div className="min-w-0">
                        <div className="text-sm font-medium text-gray-800">
                            {outputActions.find((a) => a.type === generatingOutput)?.label}
                        </div>
                        <ThinkingDots quirky />
                    </div>
                </div>
            )}

            {outputs.length === 0 ? (
                <div className="flex flex-col items-center justify-center py-16 text-center">
                    <FileText className="h-8 w-8 text-gray-200 mb-3" />
                    <p className="text-sm font-medium text-gray-600 mb-1">
                        {t("noOutputs")}
                    </p>
                    <p className="text-xs text-gray-400 max-w-xs leading-relaxed">
                        {t("noOutputsHint")}
                    </p>
                </div>
            ) : (
                <div className="space-y-4">
                    {outputs.map((o) => (
                        <div
                            key={o.id}
                            className="rounded-xl border border-gray-100 bg-white p-5 shadow-sm"
                        >
                            <div className="flex items-center justify-between mb-3">
                                <span className="text-[11px] font-medium text-gray-400 uppercase tracking-wide">
                                    {o.output_type.replace(/_/g, " ")}
                                </span>
                                <div className="flex items-center gap-2">
                                    {o.docx_document_id && (
                                        <>
                                            <button
                                                onClick={() =>
                                                    onOpenDoc(
                                                        o.docx_document_id!,
                                                        `${o.output_type}.docx`,
                                                        o,
                                                    )
                                                }
                                                className="inline-flex items-center gap-1.5 rounded-lg border border-gray-200 px-2.5 py-1.5 text-xs font-medium text-gray-600 hover:bg-gray-50 hover:text-gray-800 transition-colors"
                                            >
                                                <FileText className="h-3.5 w-3.5" />
                                                View
                                            </button>
                                            <DownloadOutputButton
                                                documentId={
                                                    o.docx_document_id
                                                }
                                                filename={`${o.output_type}.docx`}
                                                t={t}
                                            />
                                        </>
                                    )}
                                </div>
                            </div>
                            <div className="text-sm font-serif text-gray-900 leading-relaxed prose prose-sm max-w-none line-clamp-8">
                                <ReactMarkdown remarkPlugins={[remarkGfm]}
                                    components={blurNames.length > 0 ? {
                                        p: ({ children }) => <p>{typeof children === 'string' ? blurPartyNames(children, blurNames) : children}</p>,
                                        li: ({ children }) => <li>{typeof children === 'string' ? blurPartyNames(children, blurNames) : children}</li>,
                                    } : undefined}
                                >
                                    {o.content_md}
                                </ReactMarkdown>
                            </div>
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
}

function DownloadOutputButton({
    documentId,
    filename,
    t,
}: {
    documentId: string;
    filename: string;
    t: ReturnType<typeof useTranslations<"Cases">>;
}) {
    const [busy, setBusy] = useState(false);

    async function handleDownload() {
        if (busy) return;
        setBusy(true);
        try {
            const token =
                typeof window !== "undefined"
                    ? localStorage.getItem("mike_auth_token")
                    : null;
            const resp = await fetch(
                `${API_BASE}/single-documents/${documentId}/docx`,
                {
                    headers: token
                        ? { Authorization: `Bearer ${token}` }
                        : {},
                },
            );
            if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
            const blob = await resp.blob();
            const url = URL.createObjectURL(blob);
            const a = document.createElement("a");
            a.href = url;
            a.download = filename;
            document.body.appendChild(a);
            a.click();
            a.remove();
            setTimeout(() => URL.revokeObjectURL(url), 1000);
        } finally {
            setBusy(false);
        }
    }

    return (
        <button
            onClick={handleDownload}
            disabled={busy}
            className="inline-flex items-center gap-1 rounded-lg border border-gray-200 px-2 py-1.5 text-xs font-medium text-gray-600 hover:bg-gray-100 hover:text-gray-800 disabled:opacity-50"
        >
            {busy ? (
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
                <Download className="h-3.5 w-3.5" />
            )}
            {t("downloadDocx")}
        </button>
    );
}

// ---------------------------------------------------------------------------
// Chat tab
// ---------------------------------------------------------------------------

function ChatTab({
    messages,
    input,
    loading,
    onInputChange,
    onSend,
    chatEndRef,
    blurNames,
    t,
    selectedWorkflow,
    onSelectWorkflow,
    onClearWorkflow,
    onNewConversation,
}: {
    messages: CaseChatMsg[];
    input: string;
    loading: boolean;
    onInputChange: (v: string) => void;
    onSend: () => void;
    chatEndRef: React.RefObject<HTMLDivElement | null>;
    blurNames: string[];
    t: ReturnType<typeof useTranslations<"Cases">>;
    selectedWorkflow: { id: string; title: string; prompt_md?: string | null } | null;
    onSelectWorkflow: (wf: { id: string; title: string; prompt_md?: string | null }) => void;
    onClearWorkflow: () => void;
    onNewConversation: () => void;
}) {
    const tA = useTranslations("Assistant");
    const [workflowModalOpen, setWorkflowModalOpen] = useState(false);
    const suggestions = [
        t("chatSuggest1"),
        t("chatSuggest2"),
        t("chatSuggest3"),
    ];

    return (
        <div className="flex flex-col h-full bg-gradient-to-b from-white to-gray-50/40">
            {/* Header — only once there's a conversation to clear. "New
                conversation" starts a fresh chat (clears the restored history
                and the backing chat id) without deleting the old one. */}
            {messages.length > 0 && (
                <div className="flex items-center justify-end border-b border-gray-100 bg-white/70 px-6 py-2 shrink-0">
                    <button
                        type="button"
                        onClick={onNewConversation}
                        className="inline-flex items-center gap-1.5 rounded-full border border-gray-200 bg-white px-3 py-1 text-xs text-gray-600 transition-colors hover:border-gray-300 hover:bg-gray-50"
                    >
                        <Plus className="h-3.5 w-3.5" />
                        New conversation
                    </button>
                </div>
            )}
            {/* Messages */}
            {messages.length === 0 ? (
                <div className="flex-1 flex flex-col items-center justify-center px-6 text-center">
                    <div className="flex h-14 w-14 items-center justify-center rounded-2xl border border-gray-100 bg-white shadow-sm">
                        <MikeIcon size={28} />
                    </div>
                    <p className="mt-4 text-base font-medium font-serif text-gray-800">
                        {t("chatEmptyTitle")}
                    </p>
                    <p className="mt-1 text-sm text-gray-400 max-w-sm leading-relaxed">
                        {t("chatEmptyHint")}
                    </p>
                    <div className="mt-6 flex flex-wrap justify-center gap-2 max-w-md">
                        {suggestions.map((s) => (
                            <button
                                key={s}
                                onClick={() => onInputChange(s)}
                                className="rounded-full border border-gray-200 bg-white px-3.5 py-1.5 text-xs text-gray-600 hover:border-gray-300 hover:bg-gray-50 transition-colors"
                            >
                                {s}
                            </button>
                        ))}
                    </div>
                </div>
            ) : (
                <div className="flex-1 overflow-y-auto px-6 py-6 space-y-5">
                    {messages.map((msg, i) => (
                        <div
                            key={i}
                            className={`mike-msg-in flex gap-3 ${msg.role === "user" ? "justify-end" : "justify-start"}`}
                        >
                            {msg.role === "assistant" && (
                                <div className="shrink-0 mt-0.5 flex h-7 w-7 items-center justify-center rounded-full border border-gray-100 bg-white">
                                    <MikeIcon size={16} />
                                </div>
                            )}
                            {msg.role === "user" ? (
                                <div className="max-w-[75%] bg-gray-900 text-white rounded-2xl rounded-br-md px-4 py-3 text-sm font-serif leading-relaxed shadow-sm">
                                    {msg.content}
                                </div>
                            ) : (
                                <CaseChatBubble msg={msg} blurNames={blurNames} isStreaming={loading && i === messages.length - 1} />
                            )}
                        </div>
                    ))}
                    <div ref={chatEndRef} />
                </div>
            )}

            {/* Input */}
            <div className="border-t border-gray-100 bg-white/80 backdrop-blur px-6 py-3 shrink-0">
                {/* Workflow chip */}
                {selectedWorkflow && (
                    <div className="flex flex-wrap gap-1.5 mb-2">
                        <div className="inline-flex items-center gap-1 pl-2.5 pr-1 py-0.5 rounded-full text-xs bg-blue-600 text-white border border-white/20 shadow backdrop-blur-sm">
                            <Library className="h-2.5 w-2.5 shrink-0" />
                            <span className="max-w-[140px] truncate">
                                {selectedWorkflow.title}
                            </span>
                            <button
                                type="button"
                                onClick={onClearWorkflow}
                                className="rounded-full p-0.5 ml-0.5 text-white/60 hover:text-white hover:bg-white/20 transition-colors"
                            >
                                <X className="h-2.5 w-2.5" />
                            </button>
                        </div>
                    </div>
                )}
                <div className="flex items-end gap-2 max-w-2xl mx-auto rounded-2xl border border-gray-200 bg-white px-2 py-1.5 focus-within:border-gray-300 focus-within:ring-1 focus-within:ring-gray-200 transition-colors">
                    <button
                        type="button"
                        onClick={() => setWorkflowModalOpen(true)}
                        aria-label={tA("openWorkflows")}
                        className={`flex items-center justify-center rounded-full w-8 h-8 transition-colors shrink-0 ${selectedWorkflow ? "bg-blue-50 text-blue-600" : "text-gray-400 hover:bg-gray-100 hover:text-gray-700"}`}
                    >
                        <Library className="h-4 w-4" />
                    </button>
                    <textarea
                        value={input}
                        onChange={(e) => onInputChange(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" && !e.shiftKey) {
                                e.preventDefault();
                                onSend();
                            }
                        }}
                        placeholder={t("chatPlaceholder")}
                        rows={1}
                        className="flex-1 resize-none bg-transparent px-2 py-1.5 text-sm font-serif focus:outline-none"
                    />
                    <button
                        onClick={onSend}
                        disabled={loading || !input.trim()}
                        className="inline-flex items-center justify-center rounded-full bg-gray-900 h-9 w-9 text-white hover:bg-gray-800 disabled:opacity-30 transition-colors"
                    >
                        {loading ? (
                            <Loader2 className="h-4 w-4 animate-spin" />
                        ) : (
                            <Send className="h-4 w-4" />
                        )}
                    </button>
                </div>
            </div>

            {/* Workflow modal */}
            <AssistantWorkflowModal
                open={workflowModalOpen}
                onClose={() => setWorkflowModalOpen(false)}
                onSelect={(wf) => {
                    onSelectWorkflow({ id: wf.id, title: wf.title, prompt_md: wf.prompt_md });
                    setWorkflowModalOpen(false);
                }}
            />
        </div>
    );
}

// ---------------------------------------------------------------------------
// Document picker modal
// ---------------------------------------------------------------------------

function DocPickerModal({
    caseId,
    existingDocIds,
    onClose,
    onAttached,
    t,
    tCommon,
}: {
    caseId: string;
    existingDocIds: string[];
    onClose: () => void;
    onAttached: () => void;
    t: ReturnType<typeof useTranslations<"Cases">>;
    tCommon: ReturnType<typeof useTranslations<"Common">>;
}) {
    const [docs, setDocs] = useState<
        { id: string; filename: string; file_type: string | null }[]
    >([]);
    const [loadingDocs, setLoadingDocs] = useState(true);
    const [selected, setSelected] = useState<
        Map<string, string>
    >(new Map());
    const [search, setSearch] = useState("");
    const [attaching, setAttaching] = useState(false);

    useEffect(() => {
        (async () => {
            try {
                const token =
                    typeof window !== "undefined"
                        ? localStorage.getItem("mike_auth_token")
                        : null;
                const apiBase =
                    process.env.NEXT_PUBLIC_API_BASE_URL ??
                    "http://localhost:3001";
                const resp = await fetch(`${apiBase}/single-documents`, {
                    headers: token
                        ? { Authorization: `Bearer ${token}` }
                        : {},
                });
                if (!resp.ok) throw new Error("Failed");
                const data = await resp.json();
                const list = Array.isArray(data)
                    ? data
                    : data.documents ?? [];
                setDocs(
                    list.map(
                        (d: {
                            id: string;
                            filename: string;
                            file_type?: string | null;
                        }) => ({
                            id: d.id,
                            filename: d.filename,
                            file_type: d.file_type ?? null,
                        }),
                    ),
                );
            } catch {
                setDocs([]);
            } finally {
                setLoadingDocs(false);
            }
        })();
    }, []);

    const q = search.toLowerCase();
    const available = docs.filter(
        (d) =>
            !existingDocIds.includes(d.id) &&
            (!q || d.filename.toLowerCase().includes(q)),
    );

    function toggleDoc(id: string) {
        setSelected((prev) => {
            const next = new Map(prev);
            if (next.has(id)) next.delete(id);
            else next.set(id, "other");
            return next;
        });
    }

    function setDocType(id: string, type: string) {
        setSelected((prev) => {
            const next = new Map(prev);
            next.set(id, type);
            return next;
        });
    }

    async function handleAttach() {
        if (selected.size === 0) return;
        setAttaching(true);
        try {
            const documents = Array.from(selected.entries()).map(
                ([document_id, document_type]) => ({
                    document_id,
                    document_type,
                }),
            );
            await addCaseDocuments(caseId, documents);
            onAttached();
        } catch (e) {
            console.error("[doc-picker] attach failed", e);
            setAttaching(false);
        }
    }

    return (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/30">
            <div className="bg-white rounded-lg shadow-xl w-full max-w-lg mx-4 flex flex-col max-h-[80vh]">
                {/* Header */}
                <div className="flex items-center justify-between px-5 py-4 border-b border-gray-200">
                    <h3 className="text-sm font-medium text-gray-900">
                        {t("selectDocuments")}
                    </h3>
                    <button
                        onClick={onClose}
                        className="p-1 rounded hover:bg-gray-100 text-gray-400 hover:text-gray-700 transition-colors"
                    >
                        <X className="h-4 w-4" />
                    </button>
                </div>

                {/* Search */}
                <div className="px-5 py-3 border-b border-gray-100">
                    <div className="relative">
                        <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-gray-400" />
                        <input
                            value={search}
                            onChange={(e) => setSearch(e.target.value)}
                            placeholder={t("searchDocuments")}
                            className="w-full pl-8 pr-3 py-1.5 text-sm border border-gray-200 rounded focus:outline-none focus:ring-1 focus:ring-ring"
                        />
                    </div>
                </div>

                {/* Document list */}
                <div className="flex-1 overflow-y-auto px-5 py-3">
                    {loadingDocs ? (
                        <div className="flex items-center justify-center py-8">
                            <Loader2 className="h-5 w-5 animate-spin text-gray-400" />
                        </div>
                    ) : available.length === 0 ? (
                        <p className="text-xs text-gray-500 py-4 text-center">
                            No documents available.
                        </p>
                    ) : (
                        <div className="space-y-1">
                            {available.map((doc) => {
                                const isSelected = selected.has(doc.id);
                                return (
                                    <div key={doc.id}>
                                        <button
                                            onClick={() => toggleDoc(doc.id)}
                                            className={`w-full flex items-center gap-2 rounded px-2 py-2 text-left transition-colors ${
                                                isSelected
                                                    ? "bg-gray-100"
                                                    : "hover:bg-gray-50"
                                            }`}
                                        >
                                            <div
                                                className={`h-4 w-4 rounded border shrink-0 flex items-center justify-center ${
                                                    isSelected
                                                        ? "bg-gray-900 border-gray-900"
                                                        : "border-gray-300"
                                                }`}
                                            >
                                                {isSelected && (
                                                    <svg
                                                        className="h-3 w-3 text-white"
                                                        viewBox="0 0 12 12"
                                                        fill="none"
                                                    >
                                                        <path
                                                            d="M2 6L5 9L10 3"
                                                            stroke="currentColor"
                                                            strokeWidth="2"
                                                            strokeLinecap="round"
                                                            strokeLinejoin="round"
                                                        />
                                                    </svg>
                                                )}
                                            </div>
                                            <FileText className="h-3.5 w-3.5 text-gray-400 shrink-0" />
                                            <span className="text-xs text-gray-700 truncate">
                                                {doc.filename}
                                            </span>
                                        </button>
                                        {isSelected && (
                                            <div className="pl-8 pb-1">
                                                <select
                                                    value={
                                                        selected.get(
                                                            doc.id,
                                                        ) ?? "other"
                                                    }
                                                    onChange={(e) =>
                                                        setDocType(
                                                            doc.id,
                                                            e.target.value,
                                                        )
                                                    }
                                                    className="text-[11px] border border-gray-200 rounded px-1.5 py-0.5 text-gray-600 focus:outline-none"
                                                >
                                                    {DOC_TYPE_OPTIONS.map(
                                                        (dt) => (
                                                            <option
                                                                key={dt}
                                                                value={dt}
                                                            >
                                                                {t(dt)}
                                                            </option>
                                                        ),
                                                    )}
                                                </select>
                                            </div>
                                        )}
                                    </div>
                                );
                            })}
                        </div>
                    )}
                </div>

                {/* Footer */}
                <div className="flex items-center justify-between px-5 py-3 border-t border-gray-200">
                    <span className="text-xs text-gray-500">
                        {selected.size} {t("selected")}
                    </span>
                    <div className="flex items-center gap-2">
                        <button
                            onClick={onClose}
                            className="px-3 py-1.5 text-xs font-medium text-gray-700 border border-gray-200 rounded hover:bg-gray-100 transition-colors"
                        >
                            {tCommon("cancel")}
                        </button>
                        <button
                            onClick={handleAttach}
                            disabled={selected.size === 0 || attaching}
                            className="inline-flex items-center gap-1 px-3 py-1.5 text-xs font-medium text-white bg-gray-900 rounded hover:bg-gray-800 disabled:opacity-50 transition-colors"
                        >
                            {attaching && (
                                <Loader2 className="h-3 w-3 animate-spin" />
                            )}
                            {t("attach")}
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
}
