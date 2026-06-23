import { useEffect, useRef, useState } from "react";
import type { FeedItem, AnalysisPhase } from "./analysisConstants";
import {
    PHASE_REASSURANCE,
    GENERIC_REASSURANCE,
} from "./analysisConstants";

// During extraction, silence is expected — use a longer threshold.
// During analysis, agents emit thinking snippets so silence means something stalled.
const SILENCE_THRESHOLD_EXTRACT_MS = 45_000;
const SILENCE_THRESHOLD_DEFAULT_MS = 25_000;
const CHECK_INTERVAL_MS = 5_000;
const MAX_REASSURANCES = 20;

export function useReassuranceInjector(
    feedItems: FeedItem[],
    currentPhase: AnalysisPhase | null,
    analysisRunning: boolean,
): FeedItem[] {
    const [reassurances, setReassurances] = useState<FeedItem[]>([]);
    const lastHighValueRef = useRef(Date.now());
    const messageIndexRef = useRef(0);
    const countRef = useRef(0);
    const prevPhaseRef = useRef(currentPhase);

    // Reset on phase change
    useEffect(() => {
        if (prevPhaseRef.current !== currentPhase) {
            setReassurances([]);
            messageIndexRef.current = 0;
            countRef.current = 0;
            lastHighValueRef.current = Date.now();
            prevPhaseRef.current = currentPhase;
        }
    }, [currentPhase]);

    // Track high-value events — any real progress resets the silence timer
    useEffect(() => {
        const highValue = feedItems.filter(
            (f) => f.type === "finding" || f.type === "phase_transition" ||
                   f.type === "extraction" ||
                   (f.type === "activity" && f.text.startsWith("✓")),
        );
        if (highValue.length > 0) {
            lastHighValueRef.current = Date.now();
        }
    }, [feedItems]);

    // Inject reassurance on silence
    useEffect(() => {
        if (!analysisRunning) return;
        const interval = setInterval(() => {
            const threshold = currentPhase === "extract"
                ? SILENCE_THRESHOLD_EXTRACT_MS
                : SILENCE_THRESHOLD_DEFAULT_MS;
            const elapsed = Date.now() - lastHighValueRef.current;
            if (elapsed < threshold) return;
            if (countRef.current >= MAX_REASSURANCES) return;

            const phaseMessages = currentPhase
                ? PHASE_REASSURANCE[currentPhase] ?? []
                : [];
            const pool = [...phaseMessages, ...GENERIC_REASSURANCE];
            const idx = messageIndexRef.current % pool.length;
            const text = pool[idx];
            messageIndexRef.current++;
            countRef.current++;
            lastHighValueRef.current = Date.now();

            setReassurances((prev) => [
                ...prev,
                {
                    id: `reassurance-${Date.now()}`,
                    type: "reassurance",
                    timestamp: Date.now(),
                    text,
                },
            ]);
        }, CHECK_INTERVAL_MS);
        return () => clearInterval(interval);
    }, [analysisRunning, currentPhase]);

    // Cleanup on analysis end
    useEffect(() => {
        if (!analysisRunning) {
            setReassurances([]);
            countRef.current = 0;
            messageIndexRef.current = 0;
        }
    }, [analysisRunning]);

    // Merge feed items and reassurances by timestamp
    if (reassurances.length === 0) return feedItems;
    const merged = [...feedItems, ...reassurances].sort(
        (a, b) => a.timestamp - b.timestamp,
    );
    return merged;
}
