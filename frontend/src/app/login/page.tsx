"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useRouter } from "next/navigation";
import { useTranslations } from "next-intl";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SiteLogo } from "@/components/site-logo";
import { useAuth } from "@/contexts/AuthContext";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

export default function LoginPage() {
    const router = useRouter();
    const { isAuthenticated, authLoading, setSession } = useAuth();
    const t = useTranslations("Login");
    const tCommon = useTranslations("Common");
    const [pin, setPin] = useState("");
    const [loading, setLoading] = useState(false);
    const [bioLoading, setBioLoading] = useState(false);
    // true = enrolled + hardware available → use Windows Hello flow
    const [bioEnabled, setBioEnabled] = useState(false);
    // "bio" = waiting for biometric, "pin" = showing PIN fallback
    const [mode, setMode] = useState<"bio" | "pin">("pin");
    const [error, setError] = useState<string | null>(null);
    const [checking, setChecking] = useState(true);
    const bioTriggeredRef = useRef(false);

    const handleBiometric = useCallback(async () => {
        if (bioLoading) return;
        setBioLoading(true);
        setError(null);
        try {
            const res = await fetch(`${API_BASE}/auth/unlock-biometric`, {
                method: "POST",
            });
            const data = await res.json();
            if (!res.ok) throw new Error(data.detail || t("errorBiometric"));
            setSession(data.token, data.user);
            router.push("/assistant");
        } catch {
            // User cancelled or biometric failed → fall back to PIN
            setError(null);
            setMode("pin");
        } finally {
            setBioLoading(false);
        }
    }, [bioLoading, router, setSession, t]);

    useEffect(() => {
        if (!authLoading && isAuthenticated) {
            router.replace("/assistant");
            return;
        }
        if (authLoading) return;

        // Check first run
        fetch(`${API_BASE}/auth/status`)
            .then((r) => r.json())
            .then((data) => {
                if (data.setup_required) {
                    router.replace("/signup");
                } else {
                    setChecking(false);
                }
            })
            .catch(() => setChecking(false));

        // Check Windows Hello state
        fetch(`${API_BASE}/auth/biometric-available`)
            .then((r) => r.json())
            .then((d) => {
                const enabled = d.available && d.enabled;
                setBioEnabled(enabled);
                if (enabled) setMode("bio");
            })
            .catch(() => {});
    }, [authLoading, isAuthenticated, router]);

    // Auto-trigger biometric once when mode switches to "bio"
    useEffect(() => {
        if (mode === "bio" && bioEnabled && !checking && !bioTriggeredRef.current) {
            bioTriggeredRef.current = true;
            handleBiometric();
        }
    }, [mode, bioEnabled, checking, handleBiometric]);

    const handlePin = async (e: React.FormEvent) => {
        e.preventDefault();
        setLoading(true);
        setError(null);
        try {
            const res = await fetch(`${API_BASE}/auth/unlock`, {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({ pin }),
            });
            const data = await res.json();
            if (!res.ok) throw new Error(data.detail || t("errorInvalidPin"));
            setSession(data.token, data.user);
            router.push("/assistant");
        } catch (err) {
            setError(err instanceof Error ? err.message : t("errorGeneric"));
        } finally {
            setLoading(false);
        }
    };

    if (checking || authLoading) {
        return (
            <div className="min-h-dvh flex items-center justify-center">
                <div className="text-gray-400 text-sm">{tCommon("loading")}</div>
            </div>
        );
    }

    // Biometric pending screen
    if (mode === "bio") {
        return (
            <div className="min-h-dvh bg-white flex items-start justify-center px-6 pt-32 md:pt-40 pb-10 relative">
                <div className="absolute top-4 md:top-8 left-1/2 -translate-x-1/2">
                    <SiteLogo size="md" className="md:text-4xl" asLink />
                </div>
                <div className="w-full max-w-sm">
                    <div className="bg-white border border-gray-200 rounded-2xl p-8 text-center space-y-6">
                        <h2 className="text-2xl font-serif">{t("heading")}</h2>
                        <div className="flex flex-col items-center gap-3">
                            <div className="w-16 h-16 rounded-full bg-gray-100 flex items-center justify-center">
                                {bioLoading ? (
                                    <svg className="animate-spin w-8 h-8 text-gray-400" viewBox="0 0 24 24" fill="none">
                                        <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
                                        <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8z" />
                                    </svg>
                                ) : (
                                    <svg className="w-8 h-8 text-gray-500" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
                                        <path strokeLinecap="round" strokeLinejoin="round" d="M7.864 4.243A7.5 7.5 0 0119.5 10.5c0 2.92-.556 5.709-1.568 8.268M5.742 6.364A7.465 7.465 0 004.5 10.5a7.464 7.464 0 01-1.15 3.993m1.989 3.559A11.209 11.209 0 008.25 10.5a3.75 3.75 0 117.5 0c0 .527-.021 1.049-.064 1.565M12 10.5a14.94 14.94 0 01-3.6 9.75m6.633-4.596a18.666 18.666 0 01-2.485 5.33" />
                                    </svg>
                                )}
                            </div>
                            <p className="text-sm text-gray-500">
                                {bioLoading ? t("loggingIn") : t("useBiometric")}
                            </p>
                        </div>
                        <Button
                            variant="ghost"
                            className="text-sm text-gray-500 w-full"
                            onClick={() => { bioTriggeredRef.current = false; setMode("pin"); }}
                        >
                            {t("biometricFallback")}
                        </Button>
                    </div>
                </div>
            </div>
        );
    }

    // PIN fallback screen
    return (
        <div className="min-h-dvh bg-white flex items-start justify-center px-6 pt-32 md:pt-40 pb-10 relative">
            <div className="absolute top-4 md:top-8 left-1/2 -translate-x-1/2">
                <SiteLogo size="md" className="md:text-4xl" asLink />
            </div>
            <div className="w-full max-w-sm">
                <div className="bg-white border border-gray-200 rounded-2xl p-8">
                    <h2 className="text-2xl font-serif mb-2">{t("heading")}</h2>
                    <p className="text-sm text-gray-500 mb-6">{t("subheading")}</p>

                    <form onSubmit={handlePin} className="space-y-4">
                        <div>
                            <label
                                htmlFor="pin"
                                className="block text-sm font-medium text-gray-700 mb-2"
                            >
                                {t("pinLabel")}
                            </label>
                            <Input
                                id="pin"
                                type="password"
                                inputMode="numeric"
                                pattern="[0-9]*"
                                maxLength={8}
                                value={pin}
                                onChange={(e) =>
                                    setPin(e.target.value.replace(/\D/g, ""))
                                }
                                placeholder={t("pinPlaceholder")}
                                required
                                className="w-full tracking-widest text-center text-lg"
                                autoFocus
                            />
                        </div>

                        {error && (
                            <div className="text-red-600 text-sm bg-red-50 p-3 rounded">
                                {error}
                            </div>
                        )}

                        <Button
                            type="submit"
                            disabled={loading || pin.length < 4}
                            className="w-full bg-black hover:bg-gray-900 text-white"
                        >
                            {loading ? t("loggingIn") : t("loginButton")}
                        </Button>
                    </form>

                    {bioEnabled && (
                        <Button
                            variant="ghost"
                            className="w-full mt-3 text-sm text-gray-500"
                            onClick={() => { bioTriggeredRef.current = false; setMode("bio"); handleBiometric(); }}
                            disabled={bioLoading}
                        >
                            {bioLoading ? t("loggingIn") : t("useBiometric")}
                        </Button>
                    )}
                </div>
            </div>
        </div>
    );
}
