"use client";

import { useCallback } from "react";
import { useUserProfile, type LLMConfig } from "@/contexts/UserProfileContext";
import {
    DEFAULT_MODEL_ID,
    buildAvailableModelsFromConfig,
} from "../components/assistant/ModelToggle";

// Selected model is persisted in user_settings on the backend (via
// UserProfileContext.setSelectedModel) — not in localStorage — so the
// preference travels with the data folder. Fallback when nothing is
// stored: pick the first available model, or DEFAULT_MODEL_ID as last resort.
function pickInitial(llm: LLMConfig | undefined): string {
    if (!llm) return DEFAULT_MODEL_ID;
    const available = buildAvailableModelsFromConfig(llm);
    const allowed = new Set(available.map((m) => m.id));
    // 1. Previously selected model (still available)
    if (llm.selectedModel && allowed.has(llm.selectedModel)) {
        return llm.selectedModel;
    }
    // 2. DEFAULT_MODEL_ID if available
    if (allowed.has(DEFAULT_MODEL_ID)) return DEFAULT_MODEL_ID;
    // 3. First available model
    if (available.length > 0) return available[0].id;
    // 4. Last resort
    return llm.selectedModel ?? DEFAULT_MODEL_ID;
}

export function useSelectedModel(): [string, (id: string) => void] {
    const { profile, setSelectedModel } = useUserProfile();
    const model = pickInitial(profile?.llm);

    const onChange = useCallback(
        (id: string) => {
            const next = id && id.trim() ? id : DEFAULT_MODEL_ID;
            void setSelectedModel(next);
        },
        [setSelectedModel],
    );

    return [model, onChange];
}
