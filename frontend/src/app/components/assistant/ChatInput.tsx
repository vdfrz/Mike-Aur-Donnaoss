"use client";

import {
    useState,
    useCallback,
    useMemo,
    useRef,
    forwardRef,
    useImperativeHandle,
} from "react";
import {
    ArrowRight,
    Check,
    File,
    FileText,
    FolderOpen,
    Library,
    Square,
    X,
} from "lucide-react";
import { useTranslations } from "next-intl";
import { AddDocButton } from "./AddDocButton";
import { AddDocumentsModal } from "../shared/AddDocumentsModal";
import { AssistantWorkflowModal } from "./AssistantWorkflowModal";
import { ApiKeyMissingModal } from "../shared/ApiKeyMissingModal";
import { ModelToggle } from "./ModelToggle";
import { useSelectedModel } from "@/app/hooks/useSelectedModel";
import { useUserProfile } from "@/contexts/UserProfileContext";
import {
    getModelProvider,
    isModelAvailable,
    type ModelProvider,
} from "@/app/lib/modelAvailability";
import type { MikeDocument, MikeMessage } from "../shared/types";

export interface ChatInputHandle {
    addDoc: (doc: MikeDocument) => void;
}

interface Props {
    onSubmit: (message: MikeMessage) => void;
    onCancel: () => void;
    isLoading: boolean;
    hideAddDocButton?: boolean;
    hideWorkflowButton?: boolean;
    onProjectsClick?: () => void;
    projectName?: string;
    projectCmNumber?: string | null;
}

export const ChatInput = forwardRef<ChatInputHandle, Props>(function ChatInput(
    {
        onSubmit,
        onCancel,
        isLoading,
        hideAddDocButton,
        hideWorkflowButton,
        onProjectsClick,
        projectName,
        projectCmNumber,
    }: Props,
    ref,
) {
    const t = useTranslations("Assistant");
    const [value, setValue] = useState("");
    const [attachedDocs, setAttachedDocs] = useState<MikeDocument[]>([]);
    const [selectedWorkflow, setSelectedWorkflow] = useState<{
        id: string;
        title: string;
    } | null>(null);
    const [model, setModel] = useSelectedModel();
    const { profile } = useUserProfile();
    // Memoize so child effects depending on `apiKeys` don't re-run every render.
    const apiKeys = useMemo(
        () => ({
            claudeApiKey: profile?.claudeApiKey ?? null,
            geminiApiKey: profile?.geminiApiKey ?? null,
        }),
        [profile?.claudeApiKey, profile?.geminiApiKey],
    );
    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const [docSelectorOpen, setDocSelectorOpen] = useState(false);
    const [workflowModalOpen, setWorkflowModalOpen] = useState(false);
    const [apiKeyModalProvider, setApiKeyModalProvider] =
        useState<ModelProvider | null>(null);

    useImperativeHandle(ref, () => ({
        addDoc: (doc: MikeDocument) => {
            setAttachedDocs((prev) => {
                if (prev.some((d) => d.id === doc.id)) return prev;
                return [...prev, doc];
            });
        },
    }));

    const handleAddDocFromProject = useCallback((doc: MikeDocument) => {
        setAttachedDocs((prev) => {
            if (prev.some((d) => d.id === doc.id)) return prev;
            return [...prev, doc];
        });
    }, []);

    const handleAddDocsFromSelector = useCallback(
        (selectedDocs: MikeDocument[]) => {
            setAttachedDocs((prev) => {
                const existing = new Set(prev.map((d) => d.id));
                return [
                    ...prev,
                    ...selectedDocs.filter((d) => !existing.has(d.id)),
                ];
            });
        },
        [],
    );

    const handleChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
        setValue(e.target.value);
        const el = e.target;
        el.style.height = "auto";
        el.style.height = `${el.scrollHeight}px`;
    };

    const handleSubmit = () => {
        const query = value.trim();
        if (!query || isLoading) return;
        if (!isModelAvailable(model, apiKeys)) {
            setApiKeyModalProvider(getModelProvider(model));
            return;
        }
        setValue("");
        if (textareaRef.current) {
            textareaRef.current.style.height = "auto";
        }

        const files = attachedDocs.map((d) => ({
            filename: d.filename,
            document_id: d.id,
        }));
        setAttachedDocs([]);
        const wf = selectedWorkflow;
        setSelectedWorkflow(null);

        onSubmit?.({
            role: "user",
            content: query,
            files: files.length > 0 ? files : undefined,
            workflow: wf ?? undefined,
            model,
        });
    };

    const handleActionClick = () => {
        if (isLoading) {
            onCancel();
        } else {
            handleSubmit();
        }
    };

    const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
        if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            handleSubmit();
        }
    };

    return (
        <>
            <div className="w-full flex justify-center">
                <div className="w-full border border-gray-300 dark:border-white/10 rounded-[28px] bg-white shadow-sm transition-all focus-within:shadow-md focus-within:border-gray-400">
                    {/* Attached chips */}
                    {(selectedWorkflow || attachedDocs.length > 0) && (
                        <div className="flex flex-wrap gap-1.5 px-4 pt-3 pb-1">
                            {selectedWorkflow && (
                                <div className="inline-flex items-center gap-1 pl-2.5 pr-1 py-0.5 rounded-full text-xs bg-blue-600 text-white border border-white/20 shadow backdrop-blur-sm">
                                    <Library className="h-2.5 w-2.5 shrink-0" />
                                    <span className="max-w-[140px] truncate">
                                        {selectedWorkflow.title}
                                    </span>
                                    <button
                                        type="button"
                                        onClick={() =>
                                            setSelectedWorkflow(null)
                                        }
                                        className="rounded-full p-0.5 ml-0.5 text-white/60 hover:text-white hover:bg-white/20 transition-colors"
                                    >
                                        <X className="h-2.5 w-2.5" />
                                    </button>
                                </div>
                            )}
                            {attachedDocs.map((doc) => {
                                const ft = doc.file_type?.toLowerCase();
                                const isPdf = ft === "pdf";
                                return (
                                    <div
                                        key={doc.id}
                                        className="inline-flex items-center gap-1 pl-2 pr-1 py-0.5 rounded-full text-xs text-white shadow border border-white/20 bg-black backdrop-blur-sm"
                                    >
                                        {isPdf ? (
                                            <FileText className="h-2.5 w-2.5 shrink-0 text-red-400" />
                                        ) : (
                                            <File className="h-2.5 w-2.5 shrink-0 text-blue-400" />
                                        )}
                                        <span className="max-w-[140px] truncate">
                                            {doc.filename}
                                        </span>
                                        <button
                                            type="button"
                                            onClick={() =>
                                                setAttachedDocs((prev) =>
                                                    prev.filter(
                                                        (d) => d.id !== doc.id,
                                                    ),
                                                )
                                            }
                                            className="rounded-full p-0.5 ml-0.5 text-white/60 hover:text-white hover:bg-white/20 transition-colors"
                                        >
                                            <X className="h-2.5 w-2.5" />
                                        </button>
                                    </div>
                                );
                            })}
                        </div>
                    )}

                    {/* Single row input area */}
                    <div className="flex items-end gap-2 px-3 py-2 min-h-[56px]">
                        {/* Left action buttons */}
                        <div className="flex items-center pb-1 gap-1">
                            {!hideAddDocButton && (
                                <AddDocButton
                                    onSelectDoc={handleAddDocFromProject}
                                    onBrowseAll={() => setDocSelectorOpen(true)}
                                    selectedDocIds={attachedDocs.map(
                                        (d) => d.id,
                                    )}
                                />
                            )}
                            {onProjectsClick && (
                                <button
                                    type="button"
                                    onClick={onProjectsClick}
                                    aria-label={t("openProjects")}
                                    className="flex items-center gap-1.5 rounded-lg px-2 h-8 text-sm text-gray-400 hover:bg-gray-100 hover:text-gray-700 transition-colors"
                                >
                                    <FolderOpen className="h-3.5 w-3.5" />
                                    <span className="hidden sm:inline">
                                        {t("openProjects")}
                                    </span>
                                </button>
                            )}
                            {!hideWorkflowButton && (
                                <button
                                    type="button"
                                    onClick={() => setWorkflowModalOpen(true)}
                                    aria-label={t("openWorkflows")}
                                    className={`flex items-center justify-center rounded-full w-8 h-8 transition-colors ${selectedWorkflow ? "bg-blue-50 text-blue-600" : "text-gray-400 hover:bg-gray-100 hover:text-gray-700"}`}
                                >
                                    {selectedWorkflow ? (
                                        <Check className="h-4 w-4" />
                                    ) : (
                                        <Library className="h-4 w-4" />
                                    )}
                                </button>
                            )}
                        </div>

                        {/* Textarea */}
                        <textarea
                            ref={textareaRef}
                            rows={1}
                            placeholder={t("placeholder")}
                            value={value}
                            onChange={handleChange}
                            onKeyDown={handleKeyDown}
                            className="flex-1 resize-none overflow-y-auto border-0 text-sm p-0 bg-transparent outline-none placeholder:text-gray-400 leading-[24px] py-1 max-h-48 self-center"
                        />

                        {/* Right action buttons */}
                        <div className="flex items-center gap-2 pb-1 pr-1">
                            <ModelToggle
                                value={model}
                                onChange={setModel}
                                apiKeys={apiKeys}
                            />
                            <button
                                type="button"
                                className="relative bg-gradient-to-b from-neutral-700 to-black text-white rounded-full h-8 w-8 flex items-center justify-center cursor-pointer disabled:cursor-default disabled:from-neutral-400 disabled:to-neutral-500 backdrop-blur-xl shadow-sm active:enabled:scale-95 transition-all duration-150"
                                onClick={handleActionClick}
                                disabled={!isLoading && !value.trim()}
                            >
                                {isLoading ? (
                                    <Square
                                        className="h-3 w-3"
                                        fill="currentColor"
                                        strokeWidth={0}
                                    />
                                ) : (
                                    <ArrowRight className="h-4 w-4" />
                                )}
                            </button>
                        </div>
                    </div>
                </div>
            </div>

            <AddDocumentsModal
                open={docSelectorOpen}
                onClose={() => setDocSelectorOpen(false)}
                onSelect={handleAddDocsFromSelector}
                breadcrumb={[t("title"), t("addDocument")]}
            />
            <AssistantWorkflowModal
                open={workflowModalOpen}
                onClose={() => setWorkflowModalOpen(false)}
                onSelect={(wf) => {
                    setSelectedWorkflow({ id: wf.id, title: wf.title });
                    setWorkflowModalOpen(false);
                }}
                projectName={projectName}
                projectCmNumber={projectCmNumber}
            />
            <ApiKeyMissingModal
                open={apiKeyModalProvider !== null}
                provider={apiKeyModalProvider}
                onClose={() => setApiKeyModalProvider(null)}
            />
        </>
    );
});
