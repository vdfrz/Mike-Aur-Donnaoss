"use client";

import { useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import { PlusIcon, Upload, FolderUp, LayoutGridIcon, Loader2Icon } from "lucide-react";
import {
    DropdownMenu,
    DropdownMenuContent,
    DropdownMenuItem,
    DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { uploadStandaloneDocument } from "@/app/lib/mikeApi";
import type { MikeDocument } from "../shared/types";
import { DocumentSupportNotice } from "../shared/DocumentSupportNotice";

interface Props {
    onSelectDoc: (doc: MikeDocument) => void;
    onBrowseAll: () => void;
    selectedDocIds?: string[];
    /**
     * When false, upload raw bytes without the cache/text-extraction pass
     * (no OCR) — used by the Court Bundle page, which only needs the bytes
     * and so uploads near-instantly. Defaults to true (chat needs the text).
     */
    cacheUploads?: boolean;
}

export function AddDocButton({
    onSelectDoc,
    onBrowseAll,
    selectedDocIds = [],
    cacheUploads = true,
}: Props) {
    const tA = useTranslations("Assistant");
    const [isOpen, setIsOpen] = useState(false);
    const [uploading, setUploading] = useState(false);
    const fileInputRef = useRef<HTMLInputElement>(null);
    const folderInputRef = useRef<HTMLInputElement>(null);

    // `webkitdirectory` isn't in React's input typings — set it on the DOM node
    // directly so a folder picker (with all nested files) is offered.
    useEffect(() => {
        if (folderInputRef.current) {
            folderInputRef.current.setAttribute("webkitdirectory", "");
            folderInputRef.current.setAttribute("directory", "");
        }
    }, []);

    const ACCEPTED_EXT = new Set([
        "pdf", "docx", "doc", "rtf", "xlsx", "xls", "xlsb", "ods",
        "csv", "txt", "md", "png", "jpg", "jpeg", "tif", "tiff",
    ]);

    const handleUpload = async (e: React.ChangeEvent<HTMLInputElement>) => {
        const inputEl = e.target;
        // Folder picks include junk (.DS_Store, thumbs.db, etc.) — keep only
        // supported document types.
        const files = Array.from(inputEl.files || []).filter((f) => {
            const ext = f.name.split(".").pop()?.toLowerCase() ?? "";
            return ACCEPTED_EXT.has(ext);
        });
        if (!files.length) {
            inputEl.value = "";
            return;
        }
        setUploading(true);
        try {
            // allSettled so one bad file doesn't drop the whole batch.
            const results = await Promise.allSettled(
                files.map((f) =>
                    uploadStandaloneDocument(f, { cache: cacheUploads }),
                ),
            );
            let failed = 0;
            results.forEach((r) => {
                if (r.status === "fulfilled") onSelectDoc(r.value);
                else {
                    failed++;
                    console.error("Upload failed:", r.reason);
                }
            });
            if (failed > 0) {
                console.warn(
                    `${failed} of ${files.length} file(s) failed to upload.`,
                );
            }
        } finally {
            setUploading(false);
            inputEl.value = "";
        }
    };

    return (
        <>
            <input
                ref={fileInputRef}
                type="file"
                accept=".pdf,.docx,.doc,.rtf,.xlsx,.xls,.xlsb,.ods,.csv,.txt,.md,.png,.jpg,.jpeg,.tif,.tiff"
                multiple
                className="hidden"
                onChange={handleUpload}
            />
            <input
                ref={folderInputRef}
                type="file"
                multiple
                className="hidden"
                onChange={handleUpload}
            />
            <DropdownMenu onOpenChange={setIsOpen}>
                <DropdownMenuTrigger asChild>
                    <button
                        className={`flex items-center gap-1 px-2 h-8 rounded-lg text-sm transition-colors cursor-pointer ${
                            selectedDocIds.length > 0
                                ? "text-black hover:bg-gray-100"
                                : "text-gray-400 hover:text-gray-700 hover:bg-gray-100"
                        } ${isOpen ? "bg-gray-100" : ""}`}
                        title={tA("addDocuments")}
                        aria-label={tA("addDocuments")}
                    >
                        {selectedDocIds.length > 0 ? (
                            <span className="font-medium tabular-nums">{selectedDocIds.length}</span>
                        ) : (
                            <PlusIcon
                                className={`h-4 w-4 shrink-0 transition-transform duration-300 ${isOpen ? "rotate-[135deg]" : ""}`}
                            />
                        )}
                        <span className="hidden sm:inline">
                            {selectedDocIds.length === 1
                                ? tA("documentSingular")
                                : tA("documentsPlural")}
                        </span>
                    </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent
                    className="w-72 z-50"
                    side="bottom"
                    align="start"
                >
                    <DropdownMenuItem
                        className="cursor-pointer"
                        disabled={uploading}
                        onSelect={(e) => {
                            e.preventDefault();
                            fileInputRef.current?.click();
                        }}
                    >
                        {uploading ? (
                            <Loader2Icon className="h-4 w-4 mr-2 animate-spin text-gray-400" />
                        ) : (
                            <Upload className="h-4 w-4 mr-2 text-gray-500" />
                        )}
                        <span className="text-sm">
                            {uploading ? tA("uploadingFiles") : tA("uploadFiles")}
                        </span>
                    </DropdownMenuItem>
                    <DropdownMenuItem
                        className="cursor-pointer"
                        disabled={uploading}
                        onSelect={(e) => {
                            e.preventDefault();
                            folderInputRef.current?.click();
                        }}
                    >
                        <FolderUp className="h-4 w-4 mr-2 text-gray-500" />
                        <span className="text-sm">{tA("uploadFolder")}</span>
                    </DropdownMenuItem>
                    <DropdownMenuItem
                        className="cursor-pointer"
                        onClick={onBrowseAll}
                    >
                        <LayoutGridIcon className="h-4 w-4 mr-2 text-gray-500" />
                        <span className="text-sm">{tA("browseAll")}</span>
                    </DropdownMenuItem>
                    <div className="px-2 pt-1 pb-2">
                        <DocumentSupportNotice compact />
                    </div>
                </DropdownMenuContent>
            </DropdownMenu>
        </>
    );
}
