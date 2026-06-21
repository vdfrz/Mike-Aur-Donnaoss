"use client";

import { useUserProfile } from "@/contexts/UserProfileContext";
import { useSelectedModel } from "./useSelectedModel";
import {
    DEFAULT_MODEL_ID,
    buildAvailableModelsFromConfig,
} from "../components/assistant/ModelToggle";

const LAST_CLOUD_MODEL_KEY = "mike:lastCloudModel";
const PREFERRED_LOCAL_MODEL = "local:mike-legal";

/**
 * "Offline mode" = the selected chat model is a locally-hosted model (group
 * "Local"). We derive it from the already-persisted selected model rather than
 * holding a separate boolean, so the two can never drift out of sync.
 *
 * Note: DeepSeek models also use the `local:` id prefix but are a cloud service
 * (group "DeepSeek"), so we key off the model's group, not the id prefix.
 */
export function useOfflineMode() {
    const { profile } = useUserProfile();
    const [model, setModel] = useSelectedModel();

    const localModels = buildAvailableModelsFromConfig(profile?.llm).filter(
        (m) => m.group === "Local",
    );
    const localIds = localModels.map((m) => m.id);
    const isOffline = localIds.includes(model);
    // Offline is only possible once a local endpoint is configured (then the
    // picker lists Local models — see buildAvailableModelsFromConfig).
    const canGoOffline = localModels.length > 0;

    function goOffline() {
        if (localModels.length === 0) return;
        try {
            // Remember the cloud model so "Go Online" can restore it.
            if (!localIds.includes(model)) {
                window.localStorage.setItem(LAST_CLOUD_MODEL_KEY, model);
            }
        } catch {
            /* localStorage unavailable — fall back to DEFAULT on return */
        }
        const target = localIds.includes(PREFERRED_LOCAL_MODEL)
            ? PREFERRED_LOCAL_MODEL
            : localModels[0].id;
        setModel(target);
    }

    function goOnline() {
        let last: string | null = null;
        try {
            last = window.localStorage.getItem(LAST_CLOUD_MODEL_KEY);
        } catch {
            last = null;
        }
        // Restore the remembered cloud model, or fall back to the default.
        setModel(last && !last.startsWith("local:") ? last : DEFAULT_MODEL_ID);
    }

    return { isOffline, canGoOffline, goOffline, goOnline };
}
