"use client";

import { useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { Check, Eye, EyeOff, Server, Cpu, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useUserProfile } from "@/contexts/UserProfileContext";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

// Per-provider settings shape used in the form.
//
// Important: API keys are NEVER kept in form state — see `secretInputs`
// below. The state only knows whether a saved key exists on the backend
// (`*Saved` flags) so the UI can render "•••• saved" without the key
// ever being visible to the page's React tree.
interface LLMSettings {
    openaiSaved: boolean;
    openaiModel: string;
    claudeSaved: boolean;
    claudeModel: string;
    geminiSaved: boolean;
    geminiModel: string;
    geminiRegion: string;
    localBaseUrl: string;
    localSaved: boolean;
    localModel: string;
    deepseekSaved: boolean;
    activeProvider: "openai" | "claude" | "gemini" | "local" | "deepseek";
}

const GEMINI_REGION_IDS = [
    "global",
    "europe-west1",
    "europe-west4",
    "europe-west8",
    "europe-southwest1",
    "us-central1",
    "us-east4",
    "asia-southeast1",
    "asia-northeast1",
] as const;

const DEFAULTS: LLMSettings = {
    openaiSaved: false,
    openaiModel: "gpt-4o",
    claudeSaved: false,
    claudeModel: "claude-opus-4-5",
    geminiSaved: false,
    geminiModel: "gemini-2.5-flash",
    geminiRegion: "global",
    localBaseUrl: "",
    localSaved: false,
    localModel: "",
    deepseekSaved: false,
    activeProvider: "local",
};

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

interface BackendLLMSettings {
    openai_api_key: string | null;
    openai_model: string | null;
    claude_api_key: string | null;
    main_model: string | null;
    title_model: string | null;
    tabular_model: string | null;
    gemini_api_key: string | null;
    gemini_region: string | null;
    gemini_model: string | null;
    local_base_url: string | null;
    local_api_key: string | null;
    local_model: string | null;
    active_provider: string | null;
}

function fromBackend(s: BackendLLMSettings): LLMSettings {
    const allowed: LLMSettings["activeProvider"][] = [
        "openai",
        "claude",
        "gemini",
        "local",
        "deepseek",
    ];
    const ap = (s.active_provider ?? "") as LLMSettings["activeProvider"];
    return {
        openaiSaved: !!s.openai_api_key,
        openaiModel: s.openai_model ?? DEFAULTS.openaiModel,
        claudeSaved: !!s.claude_api_key,
        claudeModel: s.main_model ?? DEFAULTS.claudeModel,
        geminiSaved: !!s.gemini_api_key,
        geminiModel: s.gemini_model ?? DEFAULTS.geminiModel,
        geminiRegion: s.gemini_region ?? DEFAULTS.geminiRegion,
        localBaseUrl: s.local_base_url ?? "",
        localSaved: !!s.local_api_key,
        localModel: s.local_model ?? "",
        deepseekSaved: !!s.local_api_key,
        activeProvider: allowed.includes(ap) ? ap : DEFAULTS.activeProvider,
    };
}

async function loadSettings(): Promise<LLMSettings> {
    if (typeof window === "undefined") return DEFAULTS;
    const token = getToken();
    if (!token) return DEFAULTS;
    try {
        const res = await fetch(`${API_BASE}/user/llm-settings`, {
            headers: { Authorization: `Bearer ${token}` },
        });
        if (!res.ok) return DEFAULTS;
        const json = (await res.json()) as BackendLLMSettings;
        return fromBackend(json);
    } catch {
        return DEFAULTS;
    }
}

// Send a partial update. Backend uses COALESCE — fields we don't
// include keep their existing value. We only include API keys when the
// user has actually re-typed them (see callers); everything else is
// always sent because those fields are visible and editable.
//
// Returns the parsed error body (or `null` on success) so the caller
// can surface it. Previously the save was fire-and-forget, which hid
// 401/500 responses behind a green checkmark.
async function saveSettings(body: Record<string, unknown>): Promise<string | null> {
    const token = getToken();
    if (!token) {
        if (typeof window !== "undefined") window.location.href = "/login";
        return "Not authenticated — redirecting to login…";
    }
    const res = await fetch(`${API_BASE}/user/llm-settings`, {
        method: "PUT",
        headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${token}`,
        },
        body: JSON.stringify(body),
    });
    // Stale token from a previous DB / process. Clear local auth state so
    // the next render boots us back to /login instead of looping on 401.
    if (res.status === 401 && typeof window !== "undefined") {
        localStorage.removeItem("mike_auth_token");
        localStorage.removeItem("mike_auth_user");
        window.location.href = "/login";
        return "Session expired — please sign in again.";
    }
    if (!res.ok) {
        const text = await res.text().catch(() => "");
        return `HTTP ${res.status}: ${text || res.statusText}`;
    }
    return null;
}

type Provider = "openai" | "claude" | "gemini" | "local" | "deepseek";

export default function ModelsAndApiKeysPage() {
    const [settings, setSettings] = useState<LLMSettings>(DEFAULTS);
    const [saved, setSaved] = useState(false);
    const [saveError, setSaveError] = useState<string | null>(null);
    const [loaded, setLoaded] = useState(false);
    const t = useTranslations("Models");
    const tCommon = useTranslations("Common");
    const tRegions = useTranslations("Models.geminiRegions");
    // Re-fetch the global LLM config after a successful save so the chat
    // ModelToggle (which reads from this context) picks up the newly
    // configured provider — otherwise the user has to reload to see
    // their just-added Gemini / Claude / OpenAI key reflected.
    const { reloadProfile } = useUserProfile();

    // Plaintext API keys live OUTSIDE React state — they're only set when
    // the user types into the field and read once at save time. The DOM
    // input is a normal controlled `<input>` whose internal value is kept
    // by the browser; we read it via ref. This way the React tree never
    // holds the secret, which simplifies redaction in dev tools and
    // prevents accidental leaks via e.g. memoized children.
    const openaiRef = useRef<HTMLInputElement>(null);
    const claudeRef = useRef<HTMLInputElement>(null);
    const geminiRef = useRef<HTMLInputElement>(null);
    const localRef = useRef<HTMLInputElement>(null);
    const deepseekRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        let cancelled = false;
        loadSettings().then((s) => {
            if (cancelled) return;
            setSettings(s);
            setLoaded(true);
        });
        return () => {
            cancelled = true;
        };
    }, []);

    const set = (patch: Partial<LLMSettings>) =>
        setSettings((prev) => ({ ...prev, ...patch }));

    const handleSave = async () => {
        // Build the patch body. API keys: only include when the user
        // typed something — otherwise omit so the backend keeps the
        // existing value (COALESCE semantics).
        const body: Record<string, unknown> = {
            // Models / region / base URL — always editable, always sent.
            openai_model: settings.openaiModel || null,
            main_model: settings.claudeModel || null,
            gemini_model: settings.geminiModel || null,
            gemini_region:
                settings.geminiRegion && settings.geminiRegion !== "global"
                    ? settings.geminiRegion
                    : null,
            local_base_url: settings.localBaseUrl || null,
            local_model: settings.localModel || null,
            active_provider: settings.activeProvider,
        };

        const openaiTyped = openaiRef.current?.value ?? "";
        if (openaiTyped) body.openai_api_key = openaiTyped;
        const claudeTyped = claudeRef.current?.value ?? "";
        if (claudeTyped) body.claude_api_key = claudeTyped;
        const geminiTyped = geminiRef.current?.value ?? "";
        if (geminiTyped) body.gemini_api_key = geminiTyped;
        const localTyped = localRef.current?.value ?? "";
        if (localTyped) body.local_api_key = localTyped;
        const deepseekTyped = deepseekRef.current?.value ?? "";
        if (deepseekTyped) body.local_api_key = deepseekTyped;

        const err = await saveSettings(body);
        if (err) {
            // Don't fake-show "saved" when the backend rejected the request.
            // We also DON'T mark the api-key fields as saved or wipe the
            // input, so the user can retry with the typed value still
            // available.
            setSaveError(err);
            setSaved(false);
            return;
        }

        // Success path: clear typed values from the DOM and update the
        // chip flags from the in-memory state.
        if (openaiRef.current) openaiRef.current.value = "";
        if (claudeRef.current) claudeRef.current.value = "";
        if (geminiRef.current) geminiRef.current.value = "";
        if (localRef.current) localRef.current.value = "";
        if (deepseekRef.current) deepseekRef.current.value = "";

        setSettings((prev) => ({
            ...prev,
            openaiSaved: prev.openaiSaved || !!openaiTyped,
            claudeSaved: prev.claudeSaved || !!claudeTyped,
            geminiSaved: prev.geminiSaved || !!geminiTyped,
            localSaved: prev.localSaved || !!localTyped,
            deepseekSaved: prev.deepseekSaved || !!deepseekTyped,
        }));

        // Refresh the shared LLM config so the chat picker, sidebar, and
        // anything else reading from UserProfileContext see the change
        // immediately.
        await reloadProfile();

        setSaveError(null);
        setSaved(true);
        setTimeout(() => setSaved(false), 2000);
    };

    // Explicitly clear a stored key. Sends an empty string so the
    // backend writes "" → effectively cleared (we read `!!value` to
    // decide "is a key set?").
    const clearKey = async (provider: Provider) => {
        const field =
            provider === "openai"
                ? "openai_api_key"
                : provider === "claude"
                    ? "claude_api_key"
                    : provider === "gemini"
                        ? "gemini_api_key"
                        : "local_api_key";
        await saveSettings({ [field]: "" });
        setSettings((prev) => ({
            ...prev,
            ...(provider === "openai" && { openaiSaved: false }),
            ...(provider === "claude" && { claudeSaved: false }),
            ...(provider === "gemini" && { geminiSaved: false }),
            ...(provider === "local" && { localSaved: false }),
            ...(provider === "deepseek" && { deepseekSaved: false }),
        }));
        await reloadProfile();
    };

    const PROVIDERS: { id: LLMSettings["activeProvider"]; label: string }[] = [
        { id: "openai", label: t("openai") },
        { id: "claude", label: t("anthropic") },
        { id: "gemini", label: t("gemini") },
        { id: "deepseek", label: t("deepSeek") },
        { id: "local", label: t("local") },
    ];

    if (!loaded) {
        return (
            <div className="text-sm text-gray-400">{tCommon("loading")}</div>
        );
    }

    return (
        <div className="space-y-8 max-w-xl">
            {/* Active provider */}
            <section>
                <h2 className="text-2xl font-medium font-serif mb-4">{t("activeProvider")}</h2>
                <div className="grid grid-cols-2 gap-2">
                    {PROVIDERS.map((p) => (
                        <button
                            key={p.id}
                            onClick={() => set({ activeProvider: p.id })}
                            className={`text-left px-4 py-3 rounded-lg border text-sm font-medium transition-colors ${settings.activeProvider === p.id
                                    ? "border-black bg-black text-white"
                                    : "border-gray-200 hover:border-gray-400 text-gray-700"
                                }`}
                        >
                            {p.label}
                        </button>
                    ))}
                </div>
            </section>

            {/* OpenAI */}
            <section>
                <div className="flex items-center gap-2 mb-3">
                    <Cpu className="h-4 w-4 text-gray-500" />
                    <h2 className="text-lg font-medium">{t("openai")}</h2>
                </div>
                <div className="space-y-3">
                    <SecretField
                        label={t("apiKey")}
                        placeholder={t("apiKeyPlaceholder")}
                        inputRef={openaiRef}
                        keySaved={settings.openaiSaved}
                        onClear={() => clearKey("openai")}
                    />
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("model")}</label>
                        <Input
                            value={settings.openaiModel}
                            onChange={(e) => set({ openaiModel: e.target.value })}
                            placeholder={t("modelPlaceholder")}
                        />
                    </div>
                </div>
            </section>

            {/* Claude */}
            <section>
                <div className="flex items-center gap-2 mb-3">
                    <Cpu className="h-4 w-4 text-gray-500" />
                    <h2 className="text-lg font-medium">{t("anthropic")}</h2>
                </div>
                <div className="space-y-3">
                    <SecretField
                        label={t("apiKey")}
                        placeholder="sk-ant-…"
                        inputRef={claudeRef}
                        keySaved={settings.claudeSaved}
                        onClear={() => clearKey("claude")}
                    />
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("model")}</label>
                        <Input
                            value={settings.claudeModel}
                            onChange={(e) => set({ claudeModel: e.target.value })}
                            placeholder={t("modelPlaceholder")}
                        />
                    </div>
                </div>
            </section>

            {/* Gemini */}
            <section>
                <div className="flex items-center gap-2 mb-3">
                    <Cpu className="h-4 w-4 text-gray-500" />
                    <h2 className="text-lg font-medium">{t("gemini")}</h2>
                </div>
                <div className="space-y-3">
                    <SecretField
                        label={t("apiKey")}
                        placeholder="AI…"
                        inputRef={geminiRef}
                        keySaved={settings.geminiSaved}
                        onClear={() => clearKey("gemini")}
                    />
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("model")}</label>
                        <Input
                            value={settings.geminiModel}
                            onChange={(e) => set({ geminiModel: e.target.value })}
                            placeholder={t("modelPlaceholder")}
                        />
                    </div>
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">
                            {t("geminiRegion")}
                        </label>
                        <select
                            value={settings.geminiRegion}
                            onChange={(e) => set({ geminiRegion: e.target.value })}
                            className="w-full rounded-md border border-gray-200 bg-white px-3 py-2 text-sm hover:border-gray-400 focus:outline-none transition-colors"
                        >
                            {GEMINI_REGION_IDS.map((r) => (
                                <option key={r} value={r}>
                                    {tRegions(r as never)}
                                </option>
                            ))}
                        </select>
                        <p className="mt-1 text-xs text-gray-400">
                            {t("geminiRegionHint")}
                        </p>
                    </div>
                </div>
            </section>

            {/* DeepSeek */}
            <section>
                <div className="flex items-center gap-2 mb-1">
                    <Server className="h-4 w-4 text-gray-500" />
                    <h2 className="text-lg font-medium">{t("deepSeek")}</h2>
                </div>
                <p className="text-xs text-gray-400 mb-3">
                    Uses the OpenAI-compatible API at api.deepseek.com. Your DeepSeek API key from the .env file is pre-configured as a fallback.
                </p>
                <div className="space-y-3">
                    <SecretField
                        label={t("apiKey")}
                        placeholder="sk-…"
                        inputRef={deepseekRef}
                        keySaved={settings.deepseekSaved}
                        onClear={() => clearKey("deepseek")}
                    />
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("model")}</label>
                        <Input
                            value={settings.localModel || "deepseek-chat"}
                            onChange={(e) => set({ localModel: e.target.value })}
                            placeholder="deepseek-chat"
                        />
                    </div>
                </div>
            </section>

            {/* Local / OpenAI-compatible */}
            <section>
                <div className="flex items-center gap-2 mb-1">
                    <Server className="h-4 w-4 text-gray-500" />
                    <h2 className="text-lg font-medium">{t("local")}</h2>
                </div>
                <div className="space-y-3">
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("baseUrl")}</label>
                        <Input
                            value={settings.localBaseUrl}
                            onChange={(e) => set({ localBaseUrl: e.target.value })}
                            placeholder={t("baseUrlPlaceholder")}
                        />
                    </div>
                    <SecretField
                        label={t("apiKey")}
                        placeholder="optional"
                        inputRef={localRef}
                        keySaved={settings.localSaved}
                        onClear={() => clearKey("local")}
                    />
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("model")}</label>
                        <Input
                            value={settings.localModel}
                            onChange={(e) => set({ localModel: e.target.value })}
                            placeholder={t("modelPlaceholder")}
                        />
                    </div>
                </div>
            </section>

            {/* Save */}
            <div className="space-y-2">
                <Button
                    onClick={handleSave}
                    className="bg-black hover:bg-gray-900 text-white min-w-[120px]"
                >
                    {saved ? <><Check className="h-4 w-4 mr-1" />{tCommon("save")}</> : tCommon("save")}
                </Button>
                {saveError && (
                    <p className="text-sm text-red-600 bg-red-50 px-3 py-2 rounded-md border border-red-200">
                        {saveError}
                    </p>
                )}
            </div>
        </div>
    );
}

interface SecretFieldProps {
    label: string;
    placeholder: string;
    inputRef: React.RefObject<HTMLInputElement | null>;
    keySaved: boolean;
    onClear: () => void;
}

// Uncontrolled input — the typed value lives in the DOM, NOT in React
// state. When `keySaved` is true a chip is shown above the input so the
// user knows a key is already stored on the backend without seeing it,
// and can leave the input empty to keep that key untouched.
function SecretField({ label, placeholder, inputRef, keySaved, onClear }: SecretFieldProps) {
    const [reveal, setReveal] = useState(false);
    const t = useTranslations("Models");
    const tCommon = useTranslations("Common");
    return (
        <div>
            <div className="flex items-center justify-between mb-1">
                <label className="text-sm text-gray-600">{label}</label>
                {keySaved && (
                    <div className="flex items-center gap-2 text-xs">
                        <span className="inline-flex items-center gap-1 rounded-full bg-green-50 text-green-700 px-2 py-0.5 border border-green-200">
                            <ShieldCheck className="h-3 w-3" />
                            {t("apiKeyStored")}
                        </span>
                        <button
                            type="button"
                            onClick={onClear}
                            className="text-gray-400 hover:text-red-600 transition-colors"
                        >
                            {tCommon("delete")}
                        </button>
                    </div>
                )}
            </div>
            <div className="relative">
                <input
                    ref={inputRef}
                    type={reveal ? "text" : "password"}
                    defaultValue=""
                    placeholder={keySaved ? t("apiKeyKeepHint") : placeholder}
                    className="flex h-10 w-full rounded-md border border-gray-200 bg-white px-3 py-2 text-sm placeholder:text-gray-400 focus-visible:outline-none focus-visible:border-gray-400 disabled:cursor-not-allowed disabled:opacity-50 pr-10"
                    autoComplete="off"
                    spellCheck={false}
                />
                <button
                    type="button"
                    onClick={() => setReveal((r) => !r)}
                    className="absolute inset-y-0 right-2 flex items-center text-gray-400 hover:text-gray-600"
                    aria-label={reveal ? "Hide" : "Show"}
                >
                    {reveal ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                </button>
            </div>
        </div>
    );
}
