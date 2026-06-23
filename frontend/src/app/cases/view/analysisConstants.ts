export const AGENT_COLORS: Record<string, { bg: string; text: string; border: string }> = {
    case_summary:         { bg: "#EEEDFE", text: "#3C3489", border: "#534AB7" },
    strengths_weaknesses: { bg: "#EAF3DE", text: "#27500A", border: "#3B6D11" },
    evidence_gap:         { bg: "#FAEEDA", text: "#633806", border: "#854F0B" },
    opposition_predictor: { bg: "#E6F1FB", text: "#0C447C", border: "#185FA5" },
    strategy_recommender: { bg: "#FBEAF0", text: "#72243E", border: "#993556" },
    precedent_finder:     { bg: "#E1F5EE", text: "#085041", border: "#0F6E56" },
    risk_assessor:        { bg: "#FAECE7", text: "#712B13", border: "#993C1D" },
};

export const AGENT_INITIALS: Record<string, string> = {
    case_summary: "CS",
    strengths_weaknesses: "SW",
    evidence_gap: "EG",
    opposition_predictor: "OP",
    strategy_recommender: "SR",
    precedent_finder: "PF",
    risk_assessor: "RA",
};

export const AGENT_DISPLAY_NAMES: Record<string, string> = {
    case_summary: "Case summary",
    strengths_weaknesses: "Strengths & weaknesses",
    evidence_gap: "Evidence gaps",
    opposition_predictor: "Opposition predictor",
    strategy_recommender: "Strategy recommender",
    precedent_finder: "Precedent finder",
    risk_assessor: "Risk assessor",
};

export type AnalysisPhase = "extract" | "summarize" | "analyze" | "validate" | "report";

export const PHASES: { id: AnalysisPhase; label: string }[] = [
    { id: "extract", label: "Extract text" },
    { id: "summarize", label: "Case summary" },
    { id: "analyze", label: "Deep analysis" },
    { id: "validate", label: "Cross-validate" },
    { id: "report", label: "Final report" },
];

export type FeedItemType =
    | "activity"
    | "finding"
    | "phase_transition"
    | "reassurance"
    | "extraction";

export interface FeedItem {
    id: string;
    type: FeedItemType;
    timestamp: number;
    agentName?: string;
    text: string;
    subtext?: string;
    findingData?: Record<string, unknown>;
    severity?: "strength" | "weakness" | "gap" | "risk" | "neutral";
    quote?: string;
    durationMs?: number;
}

export interface ExtractionProgress {
    filename: string;
    docIndex: number;
    totalDocs: number;
    done: boolean;
    pageCount?: number;
    neededOcr?: boolean;
}

export interface AnalysisEstimate {
    totalPages: number;
    estimatedSeconds: number;
    hasOcr: boolean;
}

export const PHASE_REASSURANCE: Record<string, string[]> = {
    extract: [
        "OCR on scanned documents takes a moment — Mike is reading carefully",
        "Parsing document structure and extracting text from every page",
        "Complex PDFs with images require extra processing time",
    ],
    summarize: [
        "Building a comprehensive understanding of the case facts",
        "Identifying all parties, timelines, and legal issues",
    ],
    analyze: [
        "Complex analysis requires deep thinking — this is expected",
        "Your case is safe. Mike is thorough, not stuck.",
        "Each agent examines the case from a different legal angle",
        "Checking every argument against the evidence on record",
    ],
    validate: [
        "Cross-referencing findings across all seven agents",
        "Ensuring consistency between different analysis perspectives",
    ],
    report: [
        "Compiling final findings into structured output",
        "Almost done — organizing results for review",
    ],
};

export const GENERIC_REASSURANCE = [
    "Adjournment? Mike just heard his favorite word.",
    "Your case is safe. Mike is thorough, not stuck.",
    "Good analysis takes time — like a well-argued submission.",
    "Mike never cuts corners on legal analysis.",
    "Still working — complex documents deserve careful reading.",
];
