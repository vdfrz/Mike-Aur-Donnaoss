export type ModelProvider = "claude" | "gemini" | "openai" | "local";

export interface ApiKeys {
    claudeApiKey: string | null;
    geminiApiKey: string | null;
}

export function getModelProvider(modelId: string): ModelProvider | null {
    if (!modelId) return null;
    if (modelId.startsWith("openai:")) return "openai";
    if (modelId.startsWith("local:")) return "local";
    if (modelId.startsWith("claude")) return "claude";
    if (modelId.startsWith("gemini")) return "gemini";
    return null;
}

export function isModelAvailable(modelId: string, apiKeys: ApiKeys): boolean {
    const provider = getModelProvider(modelId);
    if (!provider) return false;
    switch (provider) {
        case "claude":
            return !!apiKeys.claudeApiKey?.trim();
        case "gemini":
            return !!apiKeys.geminiApiKey?.trim();
        case "openai":
        case "local":
            // OpenAI / Local entries only appear in the picker when the user
            // has configured them, so treat them as available by construction.
            return true;
    }
}

export function isProviderAvailable(
    provider: ModelProvider,
    apiKeys: ApiKeys,
): boolean {
    switch (provider) {
        case "claude":
            return !!apiKeys.claudeApiKey?.trim();
        case "gemini":
            return !!apiKeys.geminiApiKey?.trim();
        case "openai":
        case "local":
            return true;
    }
}

export function providerLabel(provider: ModelProvider): string {
    switch (provider) {
        case "claude":
            return "Anthropic (Claude)";
        case "gemini":
            return "Google (Gemini)";
        case "openai":
            return "OpenAI";
        case "local":
            return "Local";
    }
}
