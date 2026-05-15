"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { useTranslations } from "next-intl";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SiteLogo } from "@/components/site-logo";
import { CheckCircle2 } from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

export default function SetupPage() {
    const router = useRouter();
    const { isAuthenticated, authLoading, setSession } = useAuth();
    const t = useTranslations("Signup");
    const [username, setUsername] = useState("");
    const [pin, setPin] = useState("");
    const [pinConfirm, setPinConfirm] = useState("");
    const [displayName, setDisplayName] = useState("");
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [success, setSuccess] = useState(false);

    useEffect(() => {
        if (!authLoading && isAuthenticated && !success) {
            router.replace("/assistant");
            return;
        }
        if (authLoading) return;
        // If profile already exists, go to login
        fetch(`${API_BASE}/auth/status`)
            .then((r) => r.json())
            .then((d) => {
                if (!d.setup_required) router.replace("/login");
            })
            .catch(() => {});
    }, [authLoading, isAuthenticated, router, success]);

    const handleSetup = async (e: React.FormEvent) => {
        e.preventDefault();
        setError(null);

        if (pin.length < 4) {
            setError(t("errorPinTooShort"));
            return;
        }
        if (pin !== pinConfirm) {
            setError(t("errorPinMismatch"));
            return;
        }

        setLoading(true);
        try {
            const res = await fetch(`${API_BASE}/auth/setup`, {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({
                    username: username.trim(),
                    pin,
                    display_name: displayName.trim() || undefined,
                }),
            });
            const data = await res.json();
            if (!res.ok) throw new Error(data.detail || t("errorGeneric"));
            setSession(data.token, data.user);
            setSuccess(true);
            setTimeout(() => router.push("/assistant"), 1500);
        } catch (err) {
            setError(err instanceof Error ? err.message : t("errorGeneric"));
        } finally {
            setLoading(false);
        }
    };

    if (success) {
        return (
            <div className="min-h-dvh bg-white flex items-start justify-center px-6 pt-32 md:pt-40 pb-10 relative">
                <div className="absolute top-4 md:top-8 left-1/2 -translate-x-1/2">
                    <SiteLogo size="md" className="md:text-4xl" asLink />
                </div>
                <div className="w-full max-w-md">
                    <div className="bg-white border border-gray-200 rounded-2xl p-10 text-center shadow-sm">
                        <div className="mx-auto w-12 h-12 bg-green-50 rounded-full flex items-center justify-center mb-6">
                            <CheckCircle2 className="h-6 w-6 text-green-600" />
                        </div>
                        <h2 className="text-2xl font-semibold text-gray-900 mb-3">
                            {t("heading")}
                        </h2>
                        <p className="text-gray-600">…</p>
                    </div>
                </div>
            </div>
        );
    }

    return (
        <div className="min-h-dvh bg-white flex items-start justify-center px-6 pt-32 md:pt-40 pb-10 relative">
            <div className="absolute top-4 md:top-8 left-1/2 -translate-x-1/2">
                <SiteLogo size="md" className="md:text-4xl" asLink />
            </div>
            <div className="w-full max-w-sm">
                <div className="bg-white border border-gray-200 rounded-2xl p-8">
                    <h2 className="text-2xl font-serif mb-1">{t("heading")}</h2>
                    <p className="text-sm text-gray-500 mb-6">
                        {t("subheading")}
                    </p>

                    <form onSubmit={handleSetup} className="space-y-4">
                        <div>
                            <label
                                htmlFor="username"
                                className="block text-sm font-medium text-gray-700 mb-2"
                            >
                                {t("usernameLabel")}
                            </label>
                            <Input
                                id="username"
                                type="text"
                                value={username}
                                onChange={(e) => setUsername(e.target.value)}
                                placeholder={t("usernamePlaceholder")}
                                required
                                autoFocus
                                className="w-full"
                            />
                        </div>

                        <div>
                            <label
                                htmlFor="displayName"
                                className="block text-sm font-medium text-gray-700 mb-2"
                            >
                                {t("displayNameLabel")}
                            </label>
                            <Input
                                id="displayName"
                                type="text"
                                value={displayName}
                                onChange={(e) => setDisplayName(e.target.value)}
                                placeholder={t("displayNamePlaceholder")}
                                className="w-full"
                            />
                        </div>

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
                            />
                        </div>

                        <div>
                            <label
                                htmlFor="pinConfirm"
                                className="block text-sm font-medium text-gray-700 mb-2"
                            >
                                {t("confirmPinLabel")}
                            </label>
                            <Input
                                id="pinConfirm"
                                type="password"
                                inputMode="numeric"
                                pattern="[0-9]*"
                                maxLength={8}
                                value={pinConfirm}
                                onChange={(e) =>
                                    setPinConfirm(
                                        e.target.value.replace(/\D/g, "")
                                    )
                                }
                                placeholder={t("confirmPinPlaceholder")}
                                required
                                className="w-full tracking-widest text-center text-lg"
                            />
                        </div>

                        {error && (
                            <div className="text-red-600 text-sm bg-red-50 p-3 rounded">
                                {error}
                            </div>
                        )}

                        <Button
                            type="submit"
                            disabled={loading || username.trim().length === 0}
                            className="w-full bg-black hover:bg-gray-900 text-white"
                        >
                            {loading ? t("creating") : t("createButton")}
                        </Button>
                    </form>
                </div>
            </div>
        </div>
    );
}
