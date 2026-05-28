"use client";

export function OcrTimeoutWarning({ elapsedSeconds, estimatedSeconds }: {
    elapsedSeconds: number;
    estimatedSeconds: number;
}) {
    const overBy = elapsedSeconds - estimatedSeconds;
    if (overBy < 30) return null;

    return (
        <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 mx-4 mb-2 analysis-feed-card">
            <p className="text-xs text-amber-700">
                ⏱ Taking longer than expected — large documents with OCR can take up to 3 minutes.
                Your analysis is still running.
            </p>
        </div>
    );
}

export function StuckStateRescue({
    elapsedSeconds,
    totalPages,
    onKeepWaiting,
    onAbort,
}: {
    elapsedSeconds: number;
    totalPages: number;
    onKeepWaiting: () => void;
    onAbort: () => void;
}) {
    return (
        <div
            className="rounded-lg border border-amber-200 bg-amber-50 p-4 mx-4 mb-2 analysis-feed-card"
            role="alert"
            aria-live="polite"
        >
            <p className="text-sm font-medium text-amber-900 mb-2">⏱ This is taking a while</p>
            <p className="text-xs text-amber-800 mb-3 leading-relaxed">
                Your documents{totalPages > 0 ? ` (${totalPages} pages)` : ""} include scanned PDFs
                that may require OCR processing. Your work is safe — agents sometimes take longer
                on complex documents.
            </p>
            <div className="flex items-center gap-2">
                <button
                    onClick={onKeepWaiting}
                    className="rounded-md border border-amber-300 bg-white px-3 py-1.5 text-xs font-medium text-amber-800 hover:bg-amber-50 transition-colors"
                >
                    Keep waiting
                </button>
                <button
                    onClick={onAbort}
                    className="rounded-md border border-amber-300 bg-white px-3 py-1.5 text-xs font-medium text-amber-800 hover:bg-amber-50 transition-colors"
                >
                    Stop and try again
                </button>
            </div>
        </div>
    );
}
