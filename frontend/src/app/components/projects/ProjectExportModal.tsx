"use client";

import { useState } from "react";
import { useTranslations } from "next-intl";
import { createPortal } from "react-dom";
import { Download, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

interface Props {
    open: boolean;
    onClose: () => void;
    projectId: string;
    projectName: string;
}

/**
 * Dialog that asks for the recipient's email + optional chat
 * inclusion, then POSTs to /project/{id}/export, receives the binary
 * .mikeprj, and triggers a browser download. The encrypted file is
 * pinned to the recipient's email — see mikeprj/crypto.rs for why
 * that's "weak but documented" sharing.
 */
export function ProjectExportModal({ open, onClose, projectId, projectName }: Props) {
    const t = useTranslations("ProjectExport");
    const tCommon = useTranslations("Common");
    const [email, setEmail] = useState("");
    const [includeChats, setIncludeChats] = useState(false);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    if (!open) return null;

    const handleExport = async () => {
        const target = email.trim();
        if (!target) return;
        setBusy(true);
        setError(null);
        try {
            const token =
                typeof window !== "undefined"
                    ? localStorage.getItem("mike_auth_token") ?? ""
                    : "";
            const res = await fetch(
                `${API_BASE}/project/${projectId}/export`,
                {
                    method: "POST",
                    headers: {
                        "Content-Type": "application/json",
                        Authorization: `Bearer ${token}`,
                    },
                    body: JSON.stringify({
                        recipient_email: target,
                        include_chats: includeChats,
                    }),
                },
            );
            if (!res.ok) {
                const txt = await res.text().catch(() => "");
                throw new Error(`HTTP ${res.status}: ${txt || res.statusText}`);
            }
            const blob = await res.blob();
            const url = URL.createObjectURL(blob);
            const a = document.createElement("a");
            a.href = url;
            // Filename comes from Content-Disposition; fallback to project name.
            const cd = res.headers.get("content-disposition") ?? "";
            const m = cd.match(/filename="?([^";]+)"?/i);
            a.download = m?.[1] ?? `${projectName || "project"}.mikeprj`;
            document.body.appendChild(a);
            a.click();
            a.remove();
            setTimeout(() => URL.revokeObjectURL(url), 1500);
            onClose();
        } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
        } finally {
            setBusy(false);
        }
    };

    return createPortal(
        <div
            className="fixed inset-0 z-[200] flex items-center justify-center bg-black/20 backdrop-blur-xs"
            onClick={busy ? undefined : onClose}
        >
            <div
                className="w-full max-w-md rounded-2xl bg-white shadow-2xl flex flex-col"
                onClick={(e) => e.stopPropagation()}
            >
                <div className="flex items-start justify-between gap-3 px-5 pt-5 pb-2">
                    <h2 className="text-base font-medium text-gray-900">
                        {t("title")}
                    </h2>
                    <button
                        onClick={busy ? undefined : onClose}
                        className="rounded-lg p-1.5 text-gray-400 hover:bg-gray-100 hover:text-gray-600"
                    >
                        <X className="h-4 w-4" />
                    </button>
                </div>
                <div className="px-5 pb-2 pt-1 space-y-3">
                    <p className="text-sm text-gray-500 leading-relaxed">
                        {t("subtitle")}
                    </p>
                    <div>
                        <label className="text-xs font-medium text-gray-700 block mb-1">
                            {t("recipientEmail")}
                        </label>
                        <Input
                            type="email"
                            value={email}
                            onChange={(e) => setEmail(e.target.value)}
                            placeholder={t("recipientEmailPlaceholder")}
                            autoFocus
                        />
                    </div>
                    <div>
                        <label className="flex items-start gap-2 text-sm text-gray-700">
                            <input
                                type="checkbox"
                                checked={includeChats}
                                onChange={(e) => setIncludeChats(e.target.checked)}
                                className="mt-0.5"
                            />
                            <span>
                                {t("includeChats")}
                                <span className="block text-xs text-gray-400">
                                    {t("includeChatsHint")}
                                </span>
                            </span>
                        </label>
                    </div>
                    {error && (
                        <p className="text-xs text-red-600 bg-red-50 border border-red-200 rounded-md px-3 py-2">
                            {error}
                        </p>
                    )}
                </div>
                <div className="flex justify-end gap-2 px-5 pb-5 pt-3">
                    <button
                        onClick={busy ? undefined : onClose}
                        className="rounded-lg px-4 py-1.5 text-sm font-medium text-gray-700 hover:bg-gray-100"
                    >
                        {tCommon("cancel")}
                    </button>
                    <Button
                        onClick={handleExport}
                        disabled={busy || !email.trim()}
                        className="bg-black text-white hover:bg-gray-900"
                    >
                        <Download className="h-3.5 w-3.5 mr-1" />
                        {busy ? t("exporting") : t("exportNow")}
                    </Button>
                </div>
            </div>
        </div>,
        document.body,
    );
}
