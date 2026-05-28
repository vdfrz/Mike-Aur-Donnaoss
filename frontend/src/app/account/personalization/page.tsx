"use client";

import { useEffect, useState } from "react";
import { useTranslations } from "next-intl";
import {
    getPersonalization,
    putPersonalization,
    deletePersonalization,
} from "@/app/lib/mikeApi";

export default function PersonalizationPage() {
    const t = useTranslations("Personalization");
    const [text, setText] = useState("");
    const [updatedAt, setUpdatedAt] = useState<string | null>(null);
    const [saving, setSaving] = useState(false);
    const [status, setStatus] = useState<{ type: "ok" | "err"; msg: string } | null>(null);
    const [confirmDelete, setConfirmDelete] = useState(false);

    useEffect(() => {
        getPersonalization().then((p) => {
            setText(p.profile_text);
            setUpdatedAt(p.updated_at);
        });
    }, []);

    async function handleSave() {
        setSaving(true);
        setStatus(null);
        try {
            const res = await putPersonalization(text);
            setUpdatedAt(res.updated_at);
            setStatus({ type: "ok", msg: t("saved") });
        } catch (e: unknown) {
            setStatus({ type: "err", msg: e instanceof Error ? e.message : "Failed" });
        } finally {
            setSaving(false);
        }
    }

    async function handleDelete() {
        setSaving(true);
        setStatus(null);
        try {
            await deletePersonalization();
            setText("");
            setUpdatedAt(null);
            setConfirmDelete(false);
            setStatus({ type: "ok", msg: t("cleared") });
        } catch (e: unknown) {
            setStatus({ type: "err", msg: e instanceof Error ? e.message : "Failed" });
        } finally {
            setSaving(false);
        }
    }

    return (
        <div className="space-y-6">
            <div>
                <h2 className="text-2xl font-medium font-eb-garamond mb-1">
                    {t("heading")}
                </h2>
                <p className="text-sm text-gray-500 max-w-lg">
                    {t("description")}
                </p>
            </div>

            <div className="max-w-lg">
                <textarea
                    value={text}
                    onChange={(e) => setText(e.target.value)}
                    placeholder={t("placeholder")}
                    rows={10}
                    maxLength={4000}
                    className="w-full rounded-md border border-gray-300 px-3 py-2 text-sm focus:outline-none focus:ring-2 focus:ring-black focus:border-transparent resize-y min-h-[200px]"
                />
                <div className="flex items-center justify-between mt-1">
                    <span className="text-xs text-gray-400">
                        {text.length}/4000
                    </span>
                    {updatedAt && (
                        <span className="text-xs text-gray-400">
                            {t("lastUpdated")}: {new Date(updatedAt).toLocaleDateString()}
                        </span>
                    )}
                </div>
            </div>

            <div className="flex items-center gap-3">
                <button
                    onClick={handleSave}
                    disabled={saving}
                    className="px-4 py-2 bg-black hover:bg-gray-900 text-white text-sm font-medium rounded-md disabled:opacity-50"
                >
                    {saving ? t("saving") : t("save")}
                </button>

                {text && !confirmDelete && (
                    <button
                        onClick={() => setConfirmDelete(true)}
                        className="px-4 py-2 text-sm font-medium text-red-600 hover:text-red-700"
                    >
                        {t("reset")}
                    </button>
                )}

                {confirmDelete && (
                    <div className="flex items-center gap-2">
                        <span className="text-sm text-gray-500">{t("confirmReset")}</span>
                        <button
                            onClick={handleDelete}
                            disabled={saving}
                            className="px-3 py-1 text-sm font-medium text-red-600 border border-red-300 rounded-md hover:bg-red-50"
                        >
                            {t("yes")}
                        </button>
                        <button
                            onClick={() => setConfirmDelete(false)}
                            className="px-3 py-1 text-sm font-medium text-gray-600 border border-gray-300 rounded-md hover:bg-gray-50"
                        >
                            {t("no")}
                        </button>
                    </div>
                )}
            </div>

            {status && (
                <p className={`text-sm ${status.type === "ok" ? "text-green-600" : "text-red-600"}`}>
                    {status.msg}
                </p>
            )}
        </div>
    );
}
