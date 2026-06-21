"use client";

import { Download, FileText, Loader2 } from "lucide-react";
import { useState } from "react";
import { MOTION_KEYFRAMES } from "./MotionTokens";

interface DocumentCardProps {
  filename: string;
  downloadUrl: string;
  versionNumber?: number | null;
  isLoading?: boolean;
  onDownload?: () => void;
  onOpen?: () => void;
  isReloading?: boolean;
}

/**
 * Premium document card for doc_created / doc_download events.
 * Animates in with pop effect; hover lifts with subtle shadow.
 * Combines Open (if onOpen provided) and Download actions.
 */
export function DocumentCard({
  filename,
  downloadUrl,
  versionNumber,
  isLoading = false,
  onDownload,
  onOpen,
  isReloading = false,
}: DocumentCardProps) {
  const [isHovering, setIsHovering] = useState(false);

  const extMatch = filename.match(/\.(\w+)$/);
  const ext = extMatch ? extMatch[1].toUpperCase() : "FILE";
  const basename = extMatch
    ? filename.slice(0, -extMatch[0].length)
    : filename;

  const hasVersion =
    typeof versionNumber === "number" &&
    Number.isFinite(versionNumber) &&
    versionNumber > 0;

  const spinning = isReloading || isLoading;

  return (
    <>
      <style>{MOTION_KEYFRAMES}</style>
      <div
        className={`flex items-stretch border border-gray-200 rounded-xl overflow-hidden bg-white transition-all ${
          isHovering && !spinning ? "shadow-md" : "shadow-sm"
        }`}
        onMouseEnter={() => !spinning && setIsHovering(true)}
        onMouseLeave={() => setIsHovering(false)}
        style={{
          animation: "popIn 180ms ease-out",
          transform: isHovering && !spinning ? "translateY(-2px)" : "none",
        }}
      >
        {/* Left: content */}
        <div className="flex items-center gap-3 px-4 py-3 flex-1 min-w-0">
          {/* File icon */}
          <div className="text-blue-400 shrink-0">
            <FileText size={20} />
          </div>

          {/* Text content */}
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 min-w-0">
              <p className="text-base font-serif text-gray-900 truncate">
                {basename}
              </p>
              {hasVersion && (
                <span className="shrink-0 inline-flex items-center rounded-md border border-gray-200 bg-white px-1.5 py-0.5 text-[10px] font-medium text-gray-500">
                  V{versionNumber}
                </span>
              )}
            </div>
            <p className="text-xs text-blue-500 font-medium mt-1">{ext}</p>
          </div>
        </div>

        {/* Right: action buttons */}
        <div className="flex items-stretch border-l border-gray-200 shrink-0">
          {/* Open button (if callback provided) */}
          {onOpen && (
            <button
              onClick={onOpen}
              disabled={spinning}
              className="px-4 py-3 text-gray-600 hover:text-gray-800 hover:bg-gray-50 transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
              title="Open document"
            >
              <FileText size={13} />
            </button>
          )}

          {/* Download button */}
          <button
            onClick={onDownload}
            disabled={spinning}
            className="px-4 py-3 text-gray-600 hover:text-gray-800 hover:bg-gray-50 transition-colors disabled:opacity-50 disabled:cursor-not-allowed flex items-center gap-2"
            title="Download document"
          >
            {spinning ? (
              <Loader2 size={13} className="animate-spin" />
            ) : (
              <Download size={13} />
            )}
          </button>
        </div>
      </div>
    </>
  );
}
