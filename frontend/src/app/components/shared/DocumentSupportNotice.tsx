"use client";

import { Info } from "lucide-react";
import { useTranslations } from "next-intl";

/**
 * Static informational box explaining what document formats are
 * indexed for RAG and which ones require a vision/OCR pathway.
 *
 * Drop this above any "Upload files" affordance so the user
 * understands ex-ante why their image or scanned PDF won't be
 * searchable. Pure markup — no behaviour, no callbacks.
 */
export function DocumentSupportNotice({ compact = false }: { compact?: boolean }) {
    const t = useTranslations("DocumentSupport");
    return (
        <div className="rounded-md border border-amber-200 bg-amber-50/60 px-3 py-2 text-xs text-amber-900">
            <div className="flex items-start gap-2">
                <Info className="h-3.5 w-3.5 shrink-0 mt-0.5 text-amber-600" />
                <div className="flex flex-col gap-1">
                    {!compact && (
                        <p className="font-medium">{t("noticeTitle")}</p>
                    )}
                    <p>{t("indexed")}</p>
                    <p>{t("notIndexedImages")}</p>
                    <p>{t("notIndexedScannedPdf")}</p>
                </div>
            </div>
        </div>
    );
}

/**
 * Predicate: does the filename's extension belong to a format that
 * lives outside the RAG pipeline (images today, with scanned-PDF
 * detection happening server-side after upload)?
 */
export function isImageExtension(filename: string): boolean {
    const ext = filename.split(".").pop()?.toLowerCase() ?? "";
    return ["png", "jpg", "jpeg", "tif", "tiff", "gif", "bmp", "webp"].includes(
        ext,
    );
}

/**
 * Small inline chip rendered next to a per-file row in upload lists.
 * Two variants:
 *   - kind="image"   → plain warning (no OCR consideration applies).
 *   - kind="scanned" → text differs depending on whether an OCR-capable
 *     MCP server is connected; the parent passes `ocrConnected` so the
 *     same chip can flip to a positive tone when it is.
 */
export function FileSupportBadge({
    kind,
    ocrConnected = false,
    short = false,
}: {
    kind: "image" | "scanned";
    ocrConnected?: boolean;
    short?: boolean;
}) {
    const t = useTranslations("DocumentSupport");
    const labelKey = (() => {
        if (kind === "image") {
            return short ? "imageBadgeShort" : "imageBadge";
        }
        if (ocrConnected) return "scannedBadgeOcr";
        return short ? "scannedBadgeShort" : "scannedBadge";
    })();
    const tone =
        kind === "scanned" && ocrConnected
            ? "border-emerald-200 bg-emerald-50 text-emerald-800"
            : "border-amber-200 bg-amber-50 text-amber-800";
    return (
        <span
            className={`inline-flex shrink-0 items-center rounded-full border px-2 py-0.5 text-[10px] font-medium ${tone}`}
        >
            {t(labelKey)}
        </span>
    );
}
