"use client";

import {
    use,
    useCallback,
    useEffect,
    useLayoutEffect,
    useMemo,
    useRef,
    useState,
} from "react";
import { useRouter } from "next/navigation";
import {
    ChevronLeft,
    ChevronRight,
    FileText,
    Loader2,
    Plus,
    Trash2,
    Upload,
    X,
} from "lucide-react";
import {
    deleteChat,
    deleteDocument,
    getChat,
    getProject,
    uploadProjectDocument,
    createProjectFolder,
    renameProjectFolder,
    deleteProjectFolder,
    moveDocumentToFolder,
    moveSubfolderToFolder,
} from "@/app/lib/mikeApi";
import { useAssistantChat } from "@/app/hooks/useAssistantChat";
import { useChatHistoryContext } from "@/app/contexts/ChatHistoryContext";
import { UserMessage } from "@/app/components/assistant/UserMessage";
import { AssistantMessage } from "@/app/components/assistant/AssistantMessage";
import { ChatInput } from "@/app/components/assistant/ChatInput";
import type { ChatInputHandle } from "@/app/components/assistant/ChatInput";
import { ProjectExplorer } from "@/app/components/projects/ProjectExplorer";
import { DocView } from "@/app/components/shared/DocView";
import { OwnerOnlyModal } from "@/app/components/shared/OwnerOnlyModal";
import { DocxView } from "@/app/components/shared/DocxView";
import { invalidateDocxBytes } from "@/app/hooks/useFetchDocxBytes";
import { MikeIcon } from "@/components/chat/mike-icon";
import { useAuth } from "@/contexts/AuthContext";
import { useUserProfile } from "@/contexts/UserProfileContext";
import { useSidebar } from "@/app/contexts/SidebarContext";
import type {
    CitationQuote,
    MikeCitationAnnotation,
    MikeDocument,
    MikeEditAnnotation,
    MikeMessage,
    MikeProject,
} from "@/app/components/shared/types";
import { expandCitationToEntries } from "@/app/components/shared/types";

interface Props {
    params: Promise<{ id: string; chatId: string }>;
}

type DocTab = {
    documentId: string;
    filename: string;
    quotes?: CitationQuote[];
    versionId?: string | null;
    refetchKey?: number;
    warning?: string | null;
    scrollTop?: number;
};

type EditScrollTarget = {
    key: string;
    documentId: string;
    inserted_text?: string;
    deleted_text?: string;
    ins_w_id?: string | null;
    del_w_id?: string | null;
};

function isDocxTab(filename: string) {
    const ext = filename.split(".").pop()?.toLowerCase();
    return ext === "docx" || ext === "doc";
}

const ICON_SIZE = 30;
const GAP = 14;
const EXPLORER_MIN = 160;
const EXPLORER_DEFAULT = 280;
const CHAT_MIN = 320;
const CHAT_DEFAULT = 420;

function AssistantGreeting({ username }: { username: string }) {
    const [loaded, setLoaded] = useState(false);
    const [iconOffset, setIconOffset] = useState(0);
    const [textOffset, setTextOffset] = useState(0);
    const textRef = useRef<HTMLHeadingElement>(null);

    useLayoutEffect(() => {
        if (!textRef.current) return;
        const h1Width = textRef.current.offsetWidth;
        setIconOffset((h1Width + GAP) / 2);
        setTextOffset((ICON_SIZE + GAP) / 2);
    }, [username]);

    useEffect(() => {
        if (!iconOffset) return;
        const t = setTimeout(() => setLoaded(true), 100);
        return () => clearTimeout(t);
    }, [iconOffset]);

    return (
        <div className="flex-1 flex items-center justify-center">
            <div className="relative flex items-center justify-center h-[30px]">
                <div
                    className="absolute h-[30px]"
                    style={{
                        left: "50%",
                        transform: loaded
                            ? `translateX(calc(-50% - ${iconOffset}px))`
                            : "translateX(-50%)",
                        transition:
                            "transform 900ms cubic-bezier(0.25, 0.46, 0.45, 0.94)",
                    }}
                >
                    <MikeIcon size={ICON_SIZE} />
                </div>
                <h1
                    ref={textRef}
                    className="absolute text-2xl font-serif font-light text-gray-900 whitespace-nowrap"
                    style={{
                        left: "50%",
                        transform: loaded
                            ? `translateX(calc(-50% + ${textOffset}px))`
                            : "translateX(-50%)",
                        opacity: loaded ? 1 : 0,
                        transition:
                            "transform 900ms cubic-bezier(0.25, 0.46, 0.45, 0.94), opacity 800ms ease-in-out 300ms",
                    }}
                >
                    Hi, {username}
                </h1>
            </div>
        </div>
    );
}

function Divider({ onDrag }: { onDrag: (dx: number) => void }) {
    const dragging = useRef(false);
    const lastX = useRef(0);
    const [isDragging, setIsDragging] = useState(false);

    const onMouseDown = (e: React.MouseEvent) => {
        dragging.current = true;
        setIsDragging(true);
        lastX.current = e.clientX;
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
    };

    useEffect(() => {
        function onMouseMove(e: MouseEvent) {
            if (!dragging.current) return;
            onDrag(e.clientX - lastX.current);
            lastX.current = e.clientX;
        }
        function onMouseUp() {
            if (!dragging.current) return;
            dragging.current = false;
            setIsDragging(false);
            document.body.style.cursor = "";
            document.body.style.userSelect = "";
        }
        window.addEventListener("mousemove", onMouseMove);
        window.addEventListener("mouseup", onMouseUp);
        return () => {
            window.removeEventListener("mousemove", onMouseMove);
            window.removeEventListener("mouseup", onMouseUp);
        };
    }, [onDrag]);

    return (
        <div className="relative w-0 shrink-0 z-10">
            <div
                onMouseDown={onMouseDown}
                className="absolute inset-y-0 -left-2 -right-2 cursor-col-resize flex items-stretch justify-center"
            >
                {isDragging && (
                    <div className="w-1 bg-blue-500 transition-colors" />
                )}
            </div>
        </div>
    );
}

export default function ProjectAssistantChatPage({ params }: Props) {
    const { id: projectId, chatId } = use(params);
    const router = useRouter();

    const { setSidebarOpen } = useSidebar();
    const { user } = useAuth();
    const { profile } = useUserProfile();
    const username =
        profile?.displayName?.trim() || user?.email?.split("@")[0] || "there";

    const [project, setProject] = useState<MikeProject | null>(null);
    const [chatTitle, setChatTitle] = useState<string | null>(null);
    const [chatOwnerId, setChatOwnerId] = useState<string | null>(null);
    const [ownerOnlyAction, setOwnerOnlyAction] = useState<string | null>(null);
    const [chatLoaded, setChatLoaded] = useState(false);
    const [creatingChat, setCreatingChat] = useState(false);
    const [deletingChat, setDeletingChat] = useState(false);

    const [explorerWidth, setExplorerWidth] = useState(EXPLORER_DEFAULT);
    const [chatWidth, setChatWidth] = useState(CHAT_DEFAULT);
    const [explorerCollapsed, setExplorerCollapsed] = useState(false);

    const fileInputRef = useRef<HTMLInputElement>(null);
    const [uploading, setUploading] = useState(false);
    const [explorerDragOver, setExplorerDragOver] = useState(false);

    const [tabs, setTabs] = useState<DocTab[]>([]);
    const [activeTabId, setActiveTabId] = useState<string | null>(null);
    const [activeQuotes, setActiveQuotes] = useState<CitationQuote[] | null>(null);
    const [selectedDocId, setSelectedDocId] = useState<string | null>(null);
    const [editScrollTarget, setEditScrollTarget] = useState<EditScrollTarget | null>(null);
    const [reloadingDocIds, setReloadingDocIds] = useState<Set<string>>(() => new Set());

    const activeTab = tabs.find((t) => t.documentId === activeTabId) ?? null;
    const tabBarRef = useRef<HTMLDivElement | null>(null);
    const tabItemRefs = useRef<Record<string, HTMLDivElement | null>>({});

    const chatInputRef = useRef<ChatInputHandle | null>(null);
    const messagesEndRef = useRef<HTMLDivElement>(null);
    const messagesContainerRef = useRef<HTMLDivElement>(null);
    const latestUserMessageRef = useRef<HTMLDivElement>(null);
    const [minHeight, setMinHeight] = useState("0px");

    const {
        setCurrentChatId,
        newChatMessages,
        setNewChatMessages,
        chats,
        saveChat,
    } = useChatHistoryContext();
    const [initialMessages] = useState<MikeMessage[]>(newChatMessages ?? []);
    const { messages, isResponseLoading, handleChat, setMessages, cancel } =
        useAssistantChat({ initialMessages, chatId, projectId });

    const hasLoaded = useRef(false);
    const hasAutoSent = useRef(false);
    const hasInitialScrolled = useRef(false);
    // Bug #16: tracks whether a turn is already streaming/loading locally so
    // the async getChat loader (resolving after the auto-send fires) doesn't
    // clobber an in-progress streamed turn with the stale persisted history.
    const turnInProgressRef = useRef(false);
    useEffect(() => {
        if (isResponseLoading || messages.length > initialMessages.length) {
            turnInProgressRef.current = true;
        }
    }, [isResponseLoading, messages.length, initialMessages.length]);

    useEffect(() => {
        setSidebarOpen(false);
    }, []); // eslint-disable-line react-hooks/exhaustive-deps

    useEffect(() => {
        getProject(projectId)
            .then(setProject)
            .catch(() => {});
    }, [projectId]);

    const projectMutationSignature = useMemo(() => {
        const created: string[] = [];
        const replicated: string[] = [];
        const editedPerDoc: Record<string, number> = {};
        for (const msg of messages) {
            for (const ev of msg.events ?? []) {
                if ("isStreaming" in ev && ev.isStreaming) continue;
                if (ev.type === "doc_created" && ev.document_id) {
                    created.push(
                        `${ev.document_id}:${ev.version_id ?? ""}:${ev.filename}`,
                    );
                    continue;
                }
                if (ev.type === "doc_replicated") {
                    for (const c of ev.copies ?? []) {
                        replicated.push(
                            `${c.document_id}:${c.version_id}:${c.new_filename}`,
                        );
                    }
                    continue;
                }
                if (ev.type === "doc_edited") {
                    editedPerDoc[ev.document_id] = Math.max(
                        editedPerDoc[ev.document_id] ?? 0,
                        (ev.version_number as number | null | undefined) ?? 0,
                    );
                }
            }
        }
        return [
            `created=${created.sort().join(",")}`,
            `replicated=${replicated.sort().join(",")}`,
            `edited=${Object.entries(editedPerDoc)
            .map(([k, v]) => `${k}=${v}`)
            .sort()
            .join(",")}`,
        ].join("|");
    }, [messages]);

    useEffect(() => {
        if (!projectMutationSignature) return;
        getProject(projectId)
            .then(setProject)
            .catch(() => {});
    }, [projectMutationSignature, projectId]);

    useEffect(() => {
        setCurrentChatId(chatId);
    }, [chatId, setCurrentChatId]);

    useEffect(() => {
        if (hasLoaded.current) return;
        hasLoaded.current = true;
        getChat(chatId)
            .then(({ chat, messages: loaded }) => {
                setChatTitle(chat.title);
                setChatOwnerId(chat.user_id ?? null);
                // Bug #16: skip the overwrite if a turn started streaming
                // while this load was in flight — otherwise we'd replace the
                // live streamed messages with the older persisted history.
                if (loaded.length > 0 && !turnInProgressRef.current)
                    setMessages(loaded);
            })
            .catch(() =>
                router.replace(`/projects/view?id=${projectId}&tab=assistant`),
            )
            .finally(() => setChatLoaded(true));
    }, [chatId]); // eslint-disable-line react-hooks/exhaustive-deps

    useEffect(() => {
        const match = chats?.find((c) => c.id === chatId);
        if (match?.title) setChatTitle(match.title);
    }, [chats, chatId]);

    useEffect(() => {
        if (
            newChatMessages &&
            newChatMessages.length === 1 &&
            newChatMessages[0].role === "user" &&
            !hasAutoSent.current &&
            !isResponseLoading &&
            messages.length === 1
        ) {
            hasAutoSent.current = true;
            setNewChatMessages(null);
            void handleChat(newChatMessages[0]);
        }
    }, [newChatMessages, messages.length, isResponseLoading]); // eslint-disable-line react-hooks/exhaustive-deps

    const scrollLatestUserToTop = useCallback(() => {
        requestAnimationFrame(() => {
            requestAnimationFrame(() => {
                const container = messagesContainerRef.current;
                const element = latestUserMessageRef.current;
                if (!container || !element) return;
                container.scrollTo({
                    top: element.offsetTop - 24,
                    behavior: "smooth",
                });
            });
        });
    }, []);

    useEffect(() => {
        const last = messages[messages.length - 1];
        if (last?.role === "user") scrollLatestUserToTop();
    }, [messages, scrollLatestUserToTop]);

    useEffect(() => {
        if (!chatLoaded || hasInitialScrolled.current || messages.length === 0) return;
        const container = messagesContainerRef.current;
        const el = latestUserMessageRef.current;
        if (!container || !el) return;
        hasInitialScrolled.current = true;
        setTimeout(() => {
            container.scrollTo({
                top: el.offsetTop - 16,
                behavior: "auto",
            });
        }, 100);
    }, [chatLoaded, messages.length]); // eslint-disable-line react-hooks/exhaustive-deps

    useEffect(() => {
        if (isResponseLoading) scrollLatestUserToTop();
    }, [isResponseLoading, scrollLatestUserToTop]);

    useEffect(() => {
        const userEl = latestUserMessageRef.current;
        const containerEl = messagesContainerRef.current;
        if (!userEl || !containerEl) return;
        setMinHeight(
            `${Math.max(0, containerEl.clientHeight - 48 - userEl.offsetHeight - 16)}px`,
        );
    }, [messages.length]); // eslint-disable-line react-hooks/exhaustive-deps

    useEffect(() => {
        if (!activeTabId) return;
        const el = tabItemRefs.current[activeTabId];
        if (!el) return;
        el.scrollIntoView({
            behavior: "smooth",
            block: "nearest",
            inline: "nearest",
        });
    }, [activeTabId, tabs.length]);

    function openTab(
        docId: string,
        filename: string,
        quotes?: CitationQuote[],
        versionId?: string | null,
    ) {
        setTabs((prev) => {
            const existing = prev.find((t) => t.documentId === docId);
            if (existing) {
                if (versionId !== undefined && existing.versionId !== versionId) {
                    return prev.map((t) =>
                        t.documentId === docId ? { ...t, versionId } : t,
                    );
                }
                return prev;
            }
            return [
                ...prev,
                { documentId: docId, filename, quotes, versionId },
            ];
        });
        setActiveTabId(docId);
        setActiveQuotes(quotes && quotes.length ? quotes : null);
        setSelectedDocId(docId);
    }

    function closeTab(docId: string) {
        setTabs((prev) => {
            const next = prev.filter((t) => t.documentId !== docId);
            if (activeTabId === docId) {
                const idx = prev.findIndex((t) => t.documentId === docId);
                const fallback = next[idx] ?? next[idx - 1] ?? null;
                setActiveTabId(fallback?.documentId ?? null);
                setActiveQuotes(null);
                setSelectedDocId(fallback?.documentId ?? null);
            }
            return next;
        });
    }

    function switchTab(docId: string) {
        setActiveTabId(docId);
        setActiveQuotes(null);
        setSelectedDocId(docId);
    }

    const handleSubmit = useCallback(
        (message: MikeMessage) => {
            if (!activeTab) return handleChat(message);
            return handleChat(message, {
                displayedDoc: {
                    filename: activeTab.filename,
                    documentId: activeTab.documentId,
                },
            });
        },
        [activeTab, handleChat],
    );

    const handleDocClick = (doc: MikeDocument) => {
        openTab(doc.id, doc.filename);
    };

    const handleCitationClick = (citation: MikeCitationAnnotation) => {
        openTab(
            citation.document_id,
            citation.filename,
            expandCitationToEntries(citation),
        );
    };

    const handleOpenDocument = (args: {
        documentId: string;
        filename: string;
        versionId: string | null;
        versionNumber: number | null;
    }) => {
        openTab(args.documentId, args.filename, undefined, args.versionId);
    };

    const handleEditViewClick = (ann: MikeEditAnnotation, filename: string) => {
        openTab(ann.document_id, filename, undefined, ann.version_id ?? null);
        setEditScrollTarget({
            key: `${ann.edit_id}-${Date.now()}`,
            documentId: ann.document_id,
            inserted_text: ann.inserted_text,
            deleted_text: ann.deleted_text,
            ins_w_id: ann.ins_w_id ?? null,
            del_w_id: ann.del_w_id ?? null,
        });
    };

    const handleEditResolved = (args: {
        editId: string;
        documentId: string;
        status: "accepted" | "rejected";
        versionId: string | null;
        downloadUrl: string | null;
    }) => {
        // Bug #7: accept/reject rewrites the docx bytes in place. Drop the
        // cached bytes and bump the open tab's refetchKey so the mounted
        // DocxView refetches the post-resolve file instead of staying on the
        // stale redline markup until a tab switch/reload. Bump inside the
        // functional updater so back-to-back resolves on the same doc each
        // increment from the latest state (no stale-closure collision).
        invalidateDocxBytes(args.documentId);
        setTabs((prev) =>
            prev.map((t) =>
                t.documentId === args.documentId
                    ? { ...t, refetchKey: (t.refetchKey ?? 0) + 1 }
                    : t,
            ),
        );
    };

    const patchTab = (documentId: string, patch: Partial<DocTab>) => {
        setTabs((prev) =>
            prev.map((t) =>
                t.documentId === documentId ? { ...t, ...patch } : t,
            ),
        );
    };

    const handleEditError = (args: { documentId: string; message: string }) => {
        patchTab(args.documentId, { warning: args.message });
    };

    const dismissTabWarning = (documentId: string) => {
        patchTab(documentId, { warning: null });
    };

    const handleTabScrollChange = (documentId: string, scrollTop: number) => {
        patchTab(documentId, { scrollTop });
    };

    const handleDocxReady = (documentId: string) => {
        setReloadingDocIds((prev) => {
            if (!prev.has(documentId)) return prev;
            const next = new Set(prev);
            next.delete(documentId);
            return next;
        });
    };

    const handleChatDrop = (e: React.DragEvent) => {
        e.preventDefault();
        const docId = e.dataTransfer.getData("application/mike-doc");
        if (!docId) return;
        const doc = project?.documents?.find((d) => d.id === docId);
        if (doc) chatInputRef.current?.addDoc(doc);
    };

    async function handleNewChat() {
        setCreatingChat(true);
        try {
            const id = await saveChat(projectId);
            if (id)
                router.push(
                    `/projects/view/assistant/chat?id=${projectId}&chatId=${id}`,
                );
        } finally {
            setCreatingChat(false);
        }
    }

    async function handleDeleteChat() {
        if (chatOwnerId && user?.id && chatOwnerId !== user.id) {
            setOwnerOnlyAction("delete this chat");
            return;
        }
        setDeletingChat(true);
        try {
            await deleteChat(chatId);
            router.push(`/projects/view?id=${projectId}&tab=assistant`);
        } finally {
            setDeletingChat(false);
        }
    }

    async function uploadFiles(files: File[]) {
        if (!files.length) return;
        setUploading(true);
        try {
            const uploaded = await Promise.all(
                files.map((f) => uploadProjectDocument(projectId, f)),
            );
            setProject((prev) => {
                if (!prev) return prev;
                return {
                    ...prev,
                    documents: [...(prev.documents ?? []), ...uploaded],
                };
            });
        } catch (err) {
            console.error("Upload failed:", err);
        } finally {
            setUploading(false);
            if (fileInputRef.current) fileInputRef.current.value = "";
        }
    }

    const handleExplorerFileDrop = async (e: React.DragEvent) => {
        e.preventDefault();
        setExplorerDragOver(false);
        const files = Array.from(e.dataTransfer.files);
        if (files.length) {
            await uploadFiles(files);
        }
    };

    const handleCreateFolder = async (
        parentId: string | null,
        name: string,
    ) => {
        const folder = await createProjectFolder(
            projectId,
            name,
            parentId ?? undefined,
        );
        setProject((prev) =>
            prev
                ? { ...prev, folders: [...(prev.folders ?? []), folder] }
                : prev,
        );
    };

    const handleRenameFolder = async (folderId: string, name: string) => {
        await renameProjectFolder(projectId, folderId, name);
        setProject((prev) =>
            prev
                ? {
                      ...prev,
                      folders: (prev.folders ?? []).map((f) =>
                          f.id === folderId ? { ...f, name } : f,
                      ),
                  }
                : prev,
        );
    };

    const handleDeleteFolder = async (folderId: string) => {
        const toDelete = new Set<string>();
        function collectIds(id: string) {
            toDelete.add(id);
            (project?.folders ?? [])
                .filter((f) => f.parent_folder_id === id)
                .forEach((f) => collectIds(f.id));
        }
        collectIds(folderId);
        await deleteProjectFolder(projectId, folderId);
        setProject((prev) =>
            prev
                ? {
                      ...prev,
                      folders: (prev.folders ?? []).filter(
                          (f) => !toDelete.has(f.id),
                      ),
                      documents: (prev.documents ?? []).map((d) =>
                          d.folder_id && toDelete.has(d.folder_id)
                              ? { ...d, folder_id: null }
                              : d,
                      ),
                  }
                : prev,
        );
    };

    const handleMoveDoc = async (
        docId: string,
        targetFolderId: string | null,
    ) => {
        setProject((prev) =>
            prev
                ? {
                      ...prev,
                      documents: (prev.documents ?? []).map((d) =>
                          d.id === docId
                              ? { ...d, folder_id: targetFolderId }
                              : d,
                      ),
                  }
                : prev,
        );
        await moveDocumentToFolder(projectId, docId, targetFolderId);
    };

    const handleMoveFolder = async (
        folderId: string,
        targetFolderId: string | null,
    ) => {
        setProject((prev) =>
            prev
                ? {
                      ...prev,
                      folders: (prev.folders ?? []).map((f) =>
                          f.id === folderId
                              ? { ...f, parent_folder_id: targetFolderId }
                              : f,
                      ),
                  }
                : prev,
        );
        await moveSubfolderToFolder(projectId, folderId, targetFolderId);
    };

    const handleDeleteDoc = async (docId: string) => {
        await deleteDocument(docId);
        setProject((prev) =>
            prev
                ? {
                      ...prev,
                      documents: (prev.documents ?? []).filter(
                          (d) => d.id !== docId,
                      ),
                  }
                : prev,
        );
        setTabs((prev) => prev.filter((t) => t.documentId !== docId));
        if (activeTabId === docId) {
            setActiveTabId(null);
            setActiveQuotes(null);
            setSelectedDocId(null);
        }
    };

    const handleResizeExplorer = (dx: number) => {
        setExplorerWidth((prev) =>
            Math.max(EXPLORER_MIN, Math.min(600, prev + dx)),
        );
    };

    const handleResizeChat = (dx: number) => {
        setChatWidth((prev) =>
            Math.max(CHAT_MIN, Math.min(800, prev + dx)),
        );
    };

    const messagesCount = messages.length;
    const showGreeting = messagesCount === 0 && chatLoaded;
    const showChat = messagesCount > 0 || !chatLoaded;

    return (
        <div className="flex-1 flex h-full overflow-hidden">
            {/* File input for uploads */}
            <input
                ref={fileInputRef}
                type="file"
                multiple
                className="hidden"
                onChange={(e) => {
                    const files = Array.from(e.target.files ?? []);
                    if (files.length) uploadFiles(files);
                }}
            />

            {/* Project Explorer */}
            {!explorerCollapsed && (
                <div
                    style={{ width: explorerWidth }}
                    className="shrink-0 flex flex-col bg-white overflow-hidden"
                    onDragOver={(e) => {
                        e.preventDefault();
                        setExplorerDragOver(true);
                    }}
                    onDragLeave={() => setExplorerDragOver(false)}
                    onDrop={handleExplorerFileDrop}
                >
                    <div className="flex items-center justify-between px-4 h-12 border-b shrink-0">
                        <span className="text-sm font-medium text-gray-700">
                            Documents
                        </span>
                        <div className="flex items-center gap-1">
                            <button
                                onClick={() => fileInputRef.current?.click()}
                                disabled={uploading}
                                className="p-1.5 text-gray-400 hover:text-gray-600 transition-colors"
                                title="Upload documents"
                            >
                                {uploading ? (
                                    <Loader2 className="h-4 w-4 animate-spin" />
                                ) : (
                                    <Upload className="h-4 w-4" />
                                )}
                            </button>
                            <button
                                onClick={() => setExplorerCollapsed(true)}
                                className="p-1.5 text-gray-400 hover:text-gray-600 transition-colors"
                                title="Collapse"
                            >
                                <ChevronLeft className="h-4 w-4" />
                            </button>
                        </div>
                    </div>
                    <div className="flex-1 overflow-auto">
                        {project && (
                            <ProjectExplorer
                                projectName={project?.name}
                                documents={project?.documents ?? []}
                                folders={project?.folders}
                                selectedDocId={selectedDocId}
                                onDocClick={handleDocClick}
                                onCreateFolder={handleCreateFolder}
                                onRenameFolder={handleRenameFolder}
                                onDeleteFolder={handleDeleteFolder}
                                onMoveDoc={handleMoveDoc}
                                onMoveFolder={handleMoveFolder}
                                onDeleteDoc={handleDeleteDoc}
                            />
                        )}
                    </div>
                    {explorerDragOver && (
                        <div className="absolute inset-0 bg-blue-50/50 border-2 border-dashed border-blue-300 z-50 pointer-events-none" />
                    )}
                </div>
            )}

            {explorerCollapsed && (
                <button
                    onClick={() => setExplorerCollapsed(false)}
                    className="w-6 shrink-0 flex items-center justify-center text-gray-400 hover:text-gray-600 hover:bg-gray-50 transition-colors border-r"
                    title="Show explorer"
                >
                    <ChevronRight className="h-4 w-4" />
                </button>
            )}

            <Divider onDrag={handleResizeExplorer} />

            {/* Chat Panel */}
            <div
                className="flex flex-col min-w-0"
                style={{ flex: showChat ? `1 1 ${chatWidth}px` : "1 1 420px" }}
                onDrop={handleChatDrop}
                onDragOver={(e) => e.preventDefault()}
            >
                {/* Chat header */}
                <div className="flex items-center justify-between px-4 h-12 border-b shrink-0 bg-white">
                    <div className="flex items-center gap-2 min-w-0">
                        <button
                            onClick={() =>
                                router.push(
                                    `/projects/view?id=${projectId}&tab=assistant`,
                                )
                            }
                            className="text-gray-400 hover:text-gray-600 transition-colors shrink-0"
                        >
                            <ChevronLeft className="h-5 w-5" />
                        </button>
                        {chatTitle ? (
                            <span className="text-sm font-medium text-gray-700 truncate">
                                {chatTitle}
                            </span>
                        ) : (
                            !chatLoaded && (
                                <div className="h-4 w-32 rounded bg-gray-100 animate-pulse" />
                            )
                        )}
                    </div>
                    <div className="flex items-center gap-1">
                        <button
                            onClick={handleNewChat}
                            disabled={creatingChat}
                            className="p-1.5 text-gray-400 hover:text-gray-600 transition-colors"
                            title="New chat"
                        >
                            {creatingChat ? (
                                <Loader2 className="h-4 w-4 animate-spin" />
                            ) : (
                                <Plus className="h-4 w-4" />
                            )}
                        </button>
                        <button
                            onClick={handleDeleteChat}
                            disabled={deletingChat}
                            className="p-1.5 text-gray-400 hover:text-red-500 transition-colors"
                            title="Delete chat"
                        >
                            {deletingChat ? (
                                <Loader2 className="h-4 w-4 animate-spin" />
                            ) : (
                                <Trash2 className="h-4 w-4" />
                            )}
                        </button>
                    </div>
                </div>

                {/* Messages area */}
                <div
                    ref={messagesContainerRef}
                    className="flex-1 overflow-y-auto bg-white"
                >
                    {showGreeting && (
                        <div className="h-full flex flex-col">
                            <AssistantGreeting username={username} />
                            <div className="px-4 pb-4">
                                <ChatInput
                                    ref={chatInputRef}
                                    onSubmit={handleSubmit}
                                    onCancel={cancel}
                                    isLoading={isResponseLoading}
                                />
                            </div>
                        </div>
                    )}
                    {showChat && (
                        <>
                            {messages.map((msg, idx) =>
                                msg.role === "user" ? (
                                    <div
                                        key={`user-${idx}`}
                                        ref={
                                            idx === messages.length - 1
                                                ? latestUserMessageRef
                                                : undefined
                                        }
                                    >
                                        <UserMessage
                                            content={msg.content}
                                            files={msg.files}
                                            workflow={msg.workflow}
                                        />
                                    </div>
                                ) : (
                                    <AssistantMessage
                                        key={`asst-${idx}`}
                                        content={msg.content}
                                        events={msg.events}
                                        annotations={msg.annotations}
                                        isError={!!msg.error}
                                        errorMessage={msg.error}
                                        onCitationClick={handleCitationClick}
                                        onOpenDocument={handleOpenDocument}
                                        onEditViewClick={handleEditViewClick}
                                        onEditResolved={handleEditResolved}
                                        onEditError={handleEditError}
                                        isStreaming={
                                            msg.events?.some(
                                                (e) =>
                                                    "isStreaming" in e &&
                                                    e.isStreaming,
                                            ) ?? false
                                        }
                                    />
                                ),
                            )}
                            {isResponseLoading && (
                                <div className="px-4 py-2">
                                    <Loader2 className="h-5 w-5 animate-spin text-gray-400" />
                                </div>
                            )}
                            <div
                                ref={messagesEndRef}
                                style={{ minHeight }}
                            />
                        </>
                    )}
                </div>

                {/* Chat input */}
                {showChat && (
                    <div className="border-t shrink-0">
                        <ChatInput
                            ref={chatInputRef}
                            onSubmit={handleSubmit}
                            onCancel={cancel}
                            isLoading={isResponseLoading}
                        />
                    </div>
                )}
            </div>

            <Divider onDrag={handleResizeChat} />

            {/* Document View Panel */}
            {activeTab && (
                <div className="flex-1 flex flex-col min-w-0 bg-white border-l">
                    {/* Tab bar */}
                    <div
                        ref={tabBarRef}
                        className="flex items-center h-10 border-b shrink-0 overflow-x-auto bg-gray-50"
                    >
                        {tabs.map((tab) => {
                            const isActive = tab.documentId === activeTabId;
                            const isLoading = reloadingDocIds.has(
                                tab.documentId,
                            );
                            return (
                                <div
                                    key={tab.documentId}
                                    ref={(el) => {
                                        tabItemRefs.current[tab.documentId] =
                                            el;
                                    }}
                                    onClick={() => switchTab(tab.documentId)}
                                    className={`group flex items-center gap-1.5 px-3 h-full cursor-pointer border-r text-xs shrink-0 transition-colors ${
                                        isActive
                                            ? "bg-white text-gray-900 border-b-2 border-b-gray-900"
                                            : "text-gray-500 hover:text-gray-700 hover:bg-gray-100"
                                    }`}
                                >
                                    <FileText className="h-3.5 w-3.5 shrink-0" />
                                    <span className="truncate max-w-[150px]">
                                        {tab.filename}
                                    </span>
                                    {isLoading && (
                                        <Loader2 className="h-3 w-3 animate-spin shrink-0" />
                                    )}
                                    <button
                                        onClick={(e) => {
                                            e.stopPropagation();
                                            closeTab(tab.documentId);
                                        }}
                                        className="ml-0.5 p-0.5 rounded hover:bg-gray-200 text-gray-400 hover:text-gray-600 opacity-0 group-hover:opacity-100 transition-opacity"
                                    >
                                        <X className="h-3 w-3" />
                                    </button>
                                </div>
                            );
                        })}
                    </div>

                    {/* Document view */}
                    <div className="flex-1 overflow-hidden">
                        {activeTab.warning && (
                            <div className="flex items-center gap-2 px-4 py-2 bg-amber-50 border-b text-xs text-amber-700">
                                <span>{activeTab.warning}</span>
                                <button
                                    onClick={() =>
                                        dismissTabWarning(
                                            activeTab.documentId,
                                        )
                                    }
                                    className="ml-auto p-0.5 hover:bg-amber-100 rounded"
                                >
                                    <X className="h-3 w-3" />
                                </button>
                            </div>
                        )}
                        {isDocxTab(activeTab.filename) ? (
                            <DocxView
                                key={`${activeTab.documentId}-${activeTab.versionId ?? ""}`}
                                documentId={activeTab.documentId}
                                versionId={activeTab.versionId ?? undefined}
                                refetchKey={activeTab.refetchKey}
                                quotes={activeTab.quotes ?? undefined}
                                onReady={() =>
                                    handleDocxReady(activeTab.documentId)
                                }
                            />
                        ) : (
                            <DocView
                                doc={{
                                    document_id: activeTab.documentId,
                                    version_id: activeTab.versionId ?? null,
                                }}
                                quotes={activeTab.quotes ?? undefined}
                            />
                        )}
                    </div>
                </div>
            )}

            {/* Owner-only modal */}
            {ownerOnlyAction && (
                <OwnerOnlyModal
                    open={true}
                    action={ownerOnlyAction}
                    onClose={() => setOwnerOnlyAction(null)}
                />
            )}
        </div>
    );
}
