"use client";

import React, {
    createContext,
    useContext,
    useEffect,
    useState,
    ReactNode,
    useCallback,
} from "react";
import { useAuth } from "@/contexts/AuthContext";

const API_BASE =
    typeof process !== "undefined"
        ? (process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001")
        : "http://localhost:3001";

// LLM credentials and per-provider config, sourced exclusively from the
// backend (`/user/llm-settings`). Stored server-side in user_settings
// (data\storage\) — never mirrored into localStorage to avoid XSS exposure
// and to ensure the configuration follows the data folder, not the
// browser origin.
export interface LLMConfig {
    claudeApiKey: string | null;
    claudeModel: string | null;
    geminiApiKey: string | null;
    geminiModel: string | null;
    geminiRegion: string | null;
    openaiApiKey: string | null;
    openaiModel: string | null;
    localBaseUrl: string | null;
    localApiKey: string | null;
    localModel: string | null;
    activeProvider: "openai" | "claude" | "gemini" | "local" | "deepseek" | null;
    selectedModel: string | null;
}

interface UserProfile {
    displayName: string | null;
    organisation: string | null;
    messageCreditsUsed: number;
    creditsResetDate: string;
    creditsRemaining: number;
    tier: string;
    tabularModel: string;
    claudeApiKey: string | null;
    geminiApiKey: string | null;
    llm: LLMConfig;
}

interface UserProfileContextType {
    profile: UserProfile | null;
    loading: boolean;
    updateDisplayName: (name: string) => Promise<boolean>;
    updateOrganisation: (organisation: string) => Promise<boolean>;
    updateModelPreference: (
        field: "tabularModel",
        value: string,
    ) => Promise<boolean>;
    updateApiKey: (
        provider: "claude" | "gemini",
        value: string | null,
    ) => Promise<boolean>;
    /**
     * Persist the user's last-picked chat model on the backend
     * (`user_settings.title_model` is reused as the "selected" slot for
     * now to avoid a schema migration). Called by useSelectedModel when
     * the picker changes; updates the in-memory profile so dependents
     * re-render without a full reload.
     */
    setSelectedModel: (id: string) => Promise<void>;
    reloadProfile: () => Promise<void>;
    incrementMessageCredits: () => Promise<boolean>;
}

const UserProfileContext = createContext<UserProfileContextType | undefined>(
    undefined,
);

interface BackendLlmSettings {
    main_model?: string | null;
    title_model?: string | null;
    tabular_model?: string | null;
    claude_api_key?: string | null;
    gemini_api_key?: string | null;
    gemini_region?: string | null;
    gemini_model?: string | null;
    openai_api_key?: string | null;
    openai_model?: string | null;
    local_base_url?: string | null;
    local_api_key?: string | null;
    local_model?: string | null;
    active_provider?: string | null;
}

function defaultLlmConfig(): LLMConfig {
    return {
        claudeApiKey: null,
        claudeModel: null,
        geminiApiKey: null,
        geminiModel: null,
        geminiRegion: null,
        openaiApiKey: null,
        openaiModel: null,
        localBaseUrl: null,
        localApiKey: null,
        localModel: null,
        activeProvider: null,
        selectedModel: null,
    };
}

function llmFromBackend(s: BackendLlmSettings | null): LLMConfig {
    if (!s) return defaultLlmConfig();
    const allowed: LLMConfig["activeProvider"][] = [
        "openai",
        "claude",
        "gemini",
        "deepseek",
        "local",
    ];
    const ap = (s.active_provider ?? null) as LLMConfig["activeProvider"];
    return {
        claudeApiKey: s.claude_api_key ?? null,
        claudeModel: s.main_model ?? null,
        geminiApiKey: s.gemini_api_key ?? null,
        geminiModel: s.gemini_model ?? null,
        geminiRegion: s.gemini_region ?? null,
        openaiApiKey: s.openai_api_key ?? null,
        openaiModel: s.openai_model ?? null,
        localBaseUrl: s.local_base_url ?? null,
        localApiKey: s.local_api_key ?? null,
        localModel: s.local_model ?? null,
        activeProvider: ap && allowed.includes(ap) ? ap : null,
        selectedModel: s.title_model ?? null,
    };
}

export function UserProfileProvider({ children }: { children: ReactNode }) {
    const { user, isAuthenticated } = useAuth();
    const [profile, setProfile] = useState<UserProfile | null>(null);
    const [loading, setLoading] = useState(true);

    const defaultProfile = useCallback((): UserProfile => {
        const futureResetDate = new Date();
        futureResetDate.setDate(futureResetDate.getDate() + 30);
        return {
            displayName: null,
            organisation: null,
            messageCreditsUsed: 0,
            creditsResetDate: futureResetDate.toISOString(),
            creditsRemaining: 999999,
            tier: "Local",
            tabularModel: "local",
            claudeApiKey: null,
            geminiApiKey: null,
            llm: defaultLlmConfig(),
        };
    }, []);

    const loadProfile = useCallback(async (_userId: string) => {
        try {
            const token = typeof window !== "undefined"
                ? localStorage.getItem("mike_auth_token")
                : null;
            const headers: Record<string, string> = token
                ? { Authorization: `Bearer ${token}` }
                : {};
            const [statusRes, llmRes] = await Promise.all([
                fetch(`${API_BASE}/auth/status`, { headers }),
                fetch(`${API_BASE}/user/llm-settings`, { headers }),
            ]);
            if (!statusRes.ok) throw new Error("status error");
            const status = await statusRes.json();
            const p = defaultProfile();
            if (status.user?.display_name) p.displayName = status.user.display_name;
            // Backend is the only source for LLM credentials/settings — no
            // localStorage fallback to avoid leaking API keys to scripts in
            // the page and to keep the data portable across machines.
            const llmJson = llmRes.ok
                ? ((await llmRes.json()) as BackendLlmSettings)
                : null;
            p.llm = llmFromBackend(llmJson);
            p.claudeApiKey = p.llm.claudeApiKey;
            p.geminiApiKey = p.llm.geminiApiKey;
            setProfile(p);
        } catch {
            setProfile(defaultProfile());
        } finally {
            setLoading(false);
        }
    }, [defaultProfile]);

    useEffect(() => {
        if (isAuthenticated && user) {
            setLoading(true);
            loadProfile(user.id);
        } else {
            setProfile(null);
            setLoading(false);
        }
    }, [isAuthenticated, user, loadProfile]);

    const updateDisplayName = useCallback(
        async (displayName: string): Promise<boolean> => {
            const trimmed = displayName.trim();
            // Optimistic UI update so the field reflects the new value
            // immediately; the PUT below persists it to user_profiles
            // (data\storage\) so it survives a MikeRust restart.
            setProfile((prev) => (prev ? { ...prev, displayName: trimmed } : null));
            if (typeof window === "undefined") return true;
            const token = localStorage.getItem("mike_auth_token");
            if (!token) return false;
            try {
                const res = await fetch(`${API_BASE}/user/profile`, {
                    method: "PUT",
                    headers: {
                        "Content-Type": "application/json",
                        Authorization: `Bearer ${token}`,
                    },
                    body: JSON.stringify({ display_name: trimmed || null }),
                });
                return res.ok;
            } catch {
                return false;
            }
        },
        [],
    );

    const updateOrganisation = useCallback(
        async (organisation: string): Promise<boolean> => {
            setProfile((prev) => (prev ? { ...prev, organisation } : null));
            return true;
        },
        [],
    );

    const updateModelPreference = useCallback(
        async (field: "tabularModel", value: string): Promise<boolean> => {
            setProfile((prev) => (prev ? { ...prev, [field]: value } : null));
            return true;
        },
        [],
    );

    const updateApiKey = useCallback(
        async (provider: "claude" | "gemini", value: string | null): Promise<boolean> => {
            const stateField = provider === "claude" ? "claudeApiKey" : "geminiApiKey";
            const normalized = value?.trim() ? value.trim() : null;
            setProfile((prev) => (prev ? { ...prev, [stateField]: normalized } : null));
            return true;
        },
        [],
    );

    const setSelectedModel = useCallback(async (id: string): Promise<void> => {
        setProfile((prev) =>
            prev ? { ...prev, llm: { ...prev.llm, selectedModel: id } } : null,
        );
        if (typeof window === "undefined") return;
        const token = localStorage.getItem("mike_auth_token");
        if (!token) return;
        // Best-effort PATCH so the picker remembers the choice across
        // sessions even when the user moves to a new machine. We use the
        // existing /user/llm-settings PUT and only send the changed slot.
        try {
            const current = await fetch(`${API_BASE}/user/llm-settings`, {
                headers: { Authorization: `Bearer ${token}` },
            });
            const json = current.ok
                ? ((await current.json()) as BackendLlmSettings)
                : {};
            await fetch(`${API_BASE}/user/llm-settings`, {
                method: "PUT",
                headers: {
                    "Content-Type": "application/json",
                    Authorization: `Bearer ${token}`,
                },
                body: JSON.stringify({ ...json, title_model: id }),
            });
        } catch {
            // Backend offline; the in-memory state still drives the UI.
        }
    }, []);

    const reloadProfile = useCallback(async () => {
        if (user) await loadProfile(user.id);
    }, [user, loadProfile]);

    const incrementMessageCredits = useCallback(async (): Promise<boolean> => {
        setProfile((prev) =>
            prev ? { ...prev, messageCreditsUsed: prev.messageCreditsUsed + 1 } : null,
        );
        return true;
    }, []);

    return (
        <UserProfileContext.Provider
            value={{
                profile,
                loading,
                updateDisplayName,
                updateOrganisation,
                updateModelPreference,
                updateApiKey,
                setSelectedModel,
                reloadProfile,
                incrementMessageCredits,
            }}
        >
            {children}
        </UserProfileContext.Provider>
    );
}

export function useUserProfile() {
    const context = useContext(UserProfileContext);
    if (context === undefined) {
        throw new Error(
            "useUserProfile must be used within a UserProfileProvider",
        );
    }
    return context;
}
