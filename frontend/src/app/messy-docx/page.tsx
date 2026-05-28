"use client";

import { useCallback, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import {
    Upload,
    FileText,
    FileEdit,
    Download,
    Loader2,
    AlertCircle,
    CheckCircle2,
    ShieldCheck,
    X,
    Wand2,
} from "lucide-react";
import { Button } from "@/components/ui/button";

const API_BASE =
    process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

async function downloadDoc(docId: string, filename: string) {
    const res = await fetch(`${API_BASE}/document/${docId}/display`, {
        headers: { Authorization: `Bearer ${getToken()}` },
    });
    if (!res.ok) throw new Error(`Download failed: ${res.status}`);
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    a.click();
    URL.revokeObjectURL(url);
}

export default function MessyDocxPage() {
    const t = useTranslations("MessyDoc");

    const [file, setFile] = useState<File | null>(null);
    const [instructions, setInstructions] = useState("");
    const [isDragging, setIsDragging] = useState(false);
    const [cleaning, setCleaning] = useState(false);
    const [redactPii, setRedactPii] = useState(false);
    const [result, setResult] = useState<{
        doc_id: string;
        filename: string;
        size_bytes: number;
    } | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [downloading, setDownloading] = useState(false);

    const fileInputRef = useRef<HTMLInputElement>(null);

    const acceptedExtensions = [".docx", ".pdf", ".txt", ".md"];

    const handleFile = useCallback((f: File) => {
        const ext = "." + f.name.split(".").pop()?.toLowerCase();
        if (!acceptedExtensions.includes(ext)) {
            setError("Unsupported file type. Please upload a .docx, .pdf, .txt or .md file.");
            return;
        }
        setFile(f);
        setResult(null);
        setError(null);
    }, []);

    const onDrop = useCallback(
        (e: React.DragEvent) => {
            e.preventDefault();
            setIsDragging(false);
            const f = e.dataTransfer.files[0];
            if (f) handleFile(f);
        },
        [handleFile],
    );

    const onInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
        const f = e.target.files?.[0];
        if (f) handleFile(f);
    };

    const clean = async () => {
        if (!file) { setError(t("errorNoFile")); return; }
        if (!instructions.trim()) { setError(t("errorNoInstructions")); return; }

        setError(null);
        setResult(null);
        setCleaning(true);

        try {
            const formData = new FormData();
            formData.append("file", file);
            formData.append("instructions", instructions.trim());
            if (redactPii) formData.append("redact_pii", "true");

            const res = await fetch(`${API_BASE}/messy-doc/clean`, {
                method: "POST",
                headers: { Authorization: `Bearer ${getToken()}` },
                body: formData,
            });

            if (!res.ok) {
                const text = await res.text().catch(() => "");
                let detail = text;
                try { detail = JSON.parse(text).detail ?? text; } catch {}
                throw new Error(detail || `HTTP ${res.status}`);
            }

            const data = await res.json();
            setResult(data);
        } catch (e) {
            setError(String(e));
        } finally {
            setCleaning(false);
        }
    };

    const handleDownload = async () => {
        if (!result) return;
        setDownloading(true);
        try {
            await downloadDoc(result.doc_id, result.filename);
        } catch (e) {
            setError(String(e));
        } finally {
            setDownloading(false);
        }
    };

    const ext = file?.name.split(".").pop()?.toLowerCase() ?? "";
    const isPdf = ext === "pdf";

    return (
        <div className="space-y-6 max-w-3xl">
            {/* Header */}
            <div>
                <div className="flex items-center gap-2 mb-2">
                    <FileEdit className="h-5 w-5 text-blue-600" />
                    <h1 className="text-2xl font-medium font-serif">{t("title")}</h1>
                </div>
                <p className="text-sm text-gray-500 leading-relaxed">{t("subtitle")}</p>
            </div>

            {/* Drop zone */}
            <div
                onDrop={onDrop}
                onDragOver={(e) => { e.preventDefault(); setIsDragging(true); }}
                onDragLeave={() => setIsDragging(false)}
                onClick={() => fileInputRef.current?.click()}
                className={`relative border-2 border-dashed rounded-xl p-10 text-center cursor-pointer transition-all duration-200 ${
                    isDragging
                        ? "border-blue-400 bg-blue-50 scale-[1.01]"
                        : file
                        ? "border-green-400 bg-green-50"
                        : "border-gray-200 hover:border-gray-300 hover:bg-gray-50"
                }`}
            >
                <input
                    ref={fileInputRef}
                    type="file"
                    accept=".docx,.pdf,.txt,.md"
                    onChange={onInputChange}
                    className="hidden"
                />

                {file ? (
                    <div className="flex flex-col items-center gap-2">
                        {isPdf ? (
                            <div className="h-12 w-12 rounded-xl bg-red-100 flex items-center justify-center">
                                <FileText className="h-6 w-6 text-red-600" />
                            </div>
                        ) : (
                            <div className="h-12 w-12 rounded-xl bg-blue-100 flex items-center justify-center">
                                <FileText className="h-6 w-6 text-blue-600" />
                            </div>
                        )}
                        <div className="text-sm font-semibold text-gray-700">{file.name}</div>
                        <div className="text-xs text-gray-400">
                            {(file.size / 1024).toFixed(0)} KB · Click to change file
                        </div>
                        <button
                            type="button"
                            onClick={(e) => { e.stopPropagation(); setFile(null); setResult(null); }}
                            className="absolute top-3 right-3 p-1 rounded-full text-gray-400 hover:text-gray-600 hover:bg-gray-100"
                        >
                            <X className="h-4 w-4" />
                        </button>
                    </div>
                ) : (
                    <div className="flex flex-col items-center gap-3">
                        <div className="h-14 w-14 rounded-2xl bg-gray-100 flex items-center justify-center">
                            <Upload className="h-7 w-7 text-gray-400" />
                        </div>
                        <div>
                            <p className="text-sm font-medium text-gray-700">{t("dropLabel")}</p>
                            <p className="text-xs text-gray-400 mt-0.5">{t("dropHint")}</p>
                        </div>
                    </div>
                )}
            </div>

            {/* Instructions */}
            <div className="space-y-2">
                <label className="text-sm font-medium text-gray-700 block">
                    {t("instructionsLabel")}
                </label>
                <textarea
                    value={instructions}
                    onChange={(e) => setInstructions(e.target.value)}
                    placeholder={t("instructionsPlaceholder")}
                    rows={4}
                    className="w-full rounded-xl border border-gray-200 px-4 py-3 text-sm text-gray-800 placeholder:text-gray-400 focus:outline-none focus:ring-2 focus:ring-blue-400 focus:border-transparent resize-none leading-relaxed transition-all"
                />
            </div>

            {/* Redact PII toggle */}
            <div className="flex items-center gap-3 group relative">
                <button
                    type="button"
                    role="switch"
                    aria-checked={redactPii}
                    onClick={() => setRedactPii(!redactPii)}
                    className={`relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ${
                        redactPii ? "bg-blue-600" : "bg-gray-200"
                    }`}
                >
                    <span
                        className={`pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ${
                            redactPii ? "translate-x-4" : "translate-x-0"
                        }`}
                    />
                </button>
                <span className="text-sm text-gray-600 flex items-center gap-1.5">
                    <ShieldCheck className="h-4 w-4" />
                    {t("redactPii")}
                </span>
                <div className="absolute bottom-full left-0 mb-2 px-3 py-2 bg-gray-900 text-white text-xs rounded-lg opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none whitespace-nowrap z-50">
                    {t("redactPiiTooltip")}
                </div>
            </div>

            {/* Error */}
            {error && (
                <div className="text-sm text-red-600 bg-red-50 border border-red-200 rounded-lg px-4 py-3 flex items-start gap-2">
                    <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                    <span>{error}</span>
                </div>
            )}

            {/* Submit */}
            <Button
                onClick={clean}
                disabled={cleaning || !file || !instructions.trim()}
                className="w-full h-11 bg-gradient-to-b from-gray-800 to-black text-white rounded-xl hover:from-gray-700 hover:to-gray-900 disabled:opacity-40 transition-all flex items-center gap-2 justify-center text-sm font-medium shadow-sm"
            >
                {cleaning ? (
                    <>
                        <Loader2 className="h-4 w-4 animate-spin" />
                        {t("cleaning")}
                    </>
                ) : (
                    <>
                        <Wand2 className="h-4 w-4" />
                        {t("cleanButton")}
                    </>
                )}
            </Button>

            {/* Result */}
            {result && (
                <div className="border border-green-200 bg-green-50 rounded-xl p-5 space-y-3">
                    <div className="flex items-center gap-2">
                        <CheckCircle2 className="h-5 w-5 text-green-600" />
                        <span className="text-sm font-semibold text-green-800">
                            {t("successMessage")}
                        </span>
                    </div>
                    <div className="text-xs text-green-700">
                        {result.filename} · {(result.size_bytes / 1024).toFixed(0)} KB
                    </div>
                    <Button
                        onClick={handleDownload}
                        disabled={downloading}
                        className="flex items-center gap-2 bg-green-700 hover:bg-green-800 text-white rounded-lg px-4 py-2 text-sm font-medium transition-colors shadow-sm"
                    >
                        {downloading ? (
                            <Loader2 className="h-4 w-4 animate-spin" />
                        ) : (
                            <Download className="h-4 w-4" />
                        )}
                        {t("downloadButton")}
                    </Button>
                </div>
            )}
        </div>
    );
}
