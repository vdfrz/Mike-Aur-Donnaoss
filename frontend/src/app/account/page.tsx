"use client";

import { useState, useEffect } from "react";
import { useRouter } from "next/navigation";
import { useTranslations } from "next-intl";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { LogOut, Check, Fingerprint, KeyRound, ShieldCheck } from "lucide-react";
import { useAuth } from "@/contexts/AuthContext";
import { useUserProfile } from "@/contexts/UserProfileContext";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

export default function AccountPage() {
    const router = useRouter();
    const { user, signOut } = useAuth();
    const { profile, updateDisplayName } = useUserProfile();
    const t = useTranslations("Account");
    const tCommon = useTranslations("Common");

    // Profile
    const [displayName, setDisplayName] = useState("");
    const [isSavingName, setIsSavingName] = useState(false);
    const [nameSaved, setNameSaved] = useState(false);

    // PIN change
    const [currentPin, setCurrentPin] = useState("");
    const [newPin, setNewPin] = useState("");
    const [confirmPin, setConfirmPin] = useState("");
    const [pinLoading, setPinLoading] = useState(false);
    const [pinMsg, setPinMsg] = useState<{ ok: boolean; text: string } | null>(null);

    // Biometric
    const [bioAvailable, setBioAvailable] = useState(false);
    const [bioEnabled, setBioEnabled] = useState(false);
    const [bioLoading, setBioLoading] = useState(false);
    const [bioMsg, setBioMsg] = useState<{ ok: boolean; text: string } | null>(null);

    useEffect(() => {
        if (profile?.displayName) setDisplayName(profile.displayName);
    }, [profile]);

    useEffect(() => {
        fetch(`${API_BASE}/auth/biometric-available`)
            .then((r) => r.json())
            .then((d) => {
                setBioAvailable(d.available ?? false);
                setBioEnabled(d.enabled ?? false);
            })
            .catch(() => { });
    }, []);

    const handleSaveName = async () => {
        setIsSavingName(true);
        await updateDisplayName(displayName.trim());
        setIsSavingName(false);
        setNameSaved(true);
        setTimeout(() => setNameSaved(false), 2000);
    };

    const handleChangePin = async (e: React.FormEvent) => {
        e.preventDefault();
        setPinMsg(null);
        if (newPin.length < 4) {
            setPinMsg({ ok: false, text: t("errorPinTooShort") });
            return;
        }
        if (newPin !== confirmPin) {
            setPinMsg({ ok: false, text: t("errorPinMismatch") });
            return;
        }
        setPinLoading(true);
        try {
            const res = await fetch(`${API_BASE}/auth/change-pin`, {
                method: "POST",
                headers: {
                    "Content-Type": "application/json",
                    Authorization: `Bearer ${getToken()}`,
                },
                body: JSON.stringify({ current_pin: currentPin, new_pin: newPin }),
            });
            const text = await res.text();
            const data = text ? JSON.parse(text) : {};
            if (!res.ok) throw new Error(data.detail || `Error ${res.status}`);
            setPinMsg({ ok: true, text: t("savedPin") });
            setCurrentPin(""); setNewPin(""); setConfirmPin("");
        } catch (err) {
            setPinMsg({ ok: false, text: err instanceof Error ? err.message : t("errorWrongPin") });
        } finally {
            setPinLoading(false);
        }
    };

    const handleToggleBiometric = async () => {
        setBioLoading(true);
        setBioMsg(null);
        const endpoint = bioEnabled
            ? `${API_BASE}/auth/biometric-disable`
            : `${API_BASE}/auth/biometric-enable`;
        try {
            const res = await fetch(endpoint, {
                method: "POST",
                headers: { Authorization: `Bearer ${getToken()}` },
            });
            const text = await res.text();
            const data = text ? JSON.parse(text) : {};
            if (!res.ok) throw new Error(data.detail || `Error ${res.status}`);
            setBioEnabled(!bioEnabled);
            setBioMsg({ ok: true, text: bioEnabled ? t("biometricDisabled") : t("biometricEnabled") });
        } catch (err) {
            setBioMsg({ ok: false, text: err instanceof Error ? err.message : tCommon("error") });
        } finally {
            setBioLoading(false);
        }
    };

    const handleLogout = async () => {
        const token = getToken();
        if (token) {
            await fetch(`${API_BASE}/auth/logout`, {
                method: "POST",
                headers: { Authorization: `Bearer ${token}` },
            }).catch(() => { });
        }
        await signOut();
        router.push("/login");
    };

    if (!user) return null;

    const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");
    const bioLabel = isMac ? t("touchId") : t("windowsHello");

    return (
        <div className="space-y-8">
            {/* Profile */}
            <section>
                <h2 className="text-2xl font-medium font-serif mb-4">{t("profile")}</h2>
                <div className="space-y-4 max-w-md">
                    <div>
                        <label className="text-sm text-gray-600 block mb-2">{t("username")}</label>
                        <p className="text-base font-medium">{user.username ?? user.email}</p>
                    </div>
                    <div>
                        <label className="text-sm text-gray-600 block mb-2">{t("displayName")}</label>
                        <div className="flex gap-2">
                            <Input
                                type="text"
                                value={displayName}
                                onChange={(e) => setDisplayName(e.target.value)}
                                placeholder={t("displayNamePlaceholder")}
                                className="flex-1"
                            />
                            <Button
                                onClick={handleSaveName}
                                disabled={isSavingName || !displayName.trim() || nameSaved}
                                className="min-w-[80px] bg-black hover:bg-gray-900 text-white"
                            >
                                {isSavingName ? tCommon("saving") : nameSaved ? <><Check className="h-4 w-3 mr-1" />{tCommon("save")}</> : tCommon("save")}
                            </Button>
                        </div>
                    </div>
                </div>
            </section>

            {/* Change PIN */}
            <section>
                <div className="flex items-center gap-2 mb-4">
                    <KeyRound className="h-5 w-5 text-gray-500" />
                    <h2 className="text-2xl font-medium font-serif">{t("changePin")}</h2>
                </div>
                <form onSubmit={handleChangePin} className="space-y-3 max-w-sm">
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("currentPin")}</label>
                        <Input
                            type="password"
                            inputMode="numeric"
                            maxLength={8}
                            value={currentPin}
                            onChange={(e) => setCurrentPin(e.target.value.replace(/\D/g, ""))}
                            placeholder={t("currentPin")}
                            className="tracking-widest text-center"
                            required
                        />
                    </div>
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("newPin")}</label>
                        <Input
                            type="password"
                            inputMode="numeric"
                            maxLength={8}
                            value={newPin}
                            onChange={(e) => setNewPin(e.target.value.replace(/\D/g, ""))}
                            placeholder={t("newPin")}
                            className="tracking-widest text-center"
                            required
                        />
                    </div>
                    <div>
                        <label className="text-sm text-gray-600 block mb-1">{t("confirmNewPin")}</label>
                        <Input
                            type="password"
                            inputMode="numeric"
                            maxLength={8}
                            value={confirmPin}
                            onChange={(e) => setConfirmPin(e.target.value.replace(/\D/g, ""))}
                            placeholder={t("confirmNewPin")}
                            className="tracking-widest text-center"
                            required
                        />
                    </div>
                    {pinMsg && (
                        <p className={`text-sm ${pinMsg.ok ? "text-green-600" : "text-red-600"}`}>
                            {pinMsg.text}
                        </p>
                    )}
                    <Button
                        type="submit"
                        disabled={pinLoading}
                        className="bg-black hover:bg-gray-900 text-white"
                    >
                        {pinLoading ? tCommon("saving") : tCommon("save")}
                    </Button>
                </form>
            </section>

            {/* Biometric */}
            {bioAvailable && (
                <section>
                    <div className="flex items-center gap-2 mb-4">
                        <Fingerprint className="h-5 w-5 text-gray-500" />
                        <h2 className="text-2xl font-medium font-serif">{bioLabel}</h2>
                    </div>
                    <div className="flex items-center gap-4">
                        <div className={`flex items-center gap-2 text-sm px-3 py-1.5 rounded-full border ${bioEnabled ? "border-green-200 bg-green-50 text-green-700" : "border-gray-200 text-gray-500"}`}>
                            <ShieldCheck className="h-4 w-4" />
                            {bioEnabled ? t("biometricEnabled") : t("biometricDisabled")}
                        </div>
                        <Button
                            variant="outline"
                            onClick={handleToggleBiometric}
                            disabled={bioLoading}
                        >
                            {bioLoading ? "…" : bioEnabled ? t("disableBiometric") : t("enableBiometric")}
                        </Button>
                    </div>
                    {bioMsg && (
                        <p className={`text-sm mt-2 ${bioMsg.ok ? "text-green-600" : "text-red-600"}`}>
                            {bioMsg.text}
                        </p>
                    )}
                </section>
            )}

            {/* Actions */}
            <section className="pt-2">
                <h2 className="text-2xl font-medium font-serif mb-4">{tCommon("actions")}</h2>
                <Button variant="outline" onClick={handleLogout} className="w-full sm:w-auto">
                    <LogOut className="h-4 w-4 mr-2" />
                    {t("lockSignOut")}
                </Button>
            </section>
        </div>
    );
}
