/**
 * Mike API client — all requests to the Node.js backend.
 * Attaches the sovereign JWT token for user authentication.
 */

import type {
    AssistantEvent,
    CaseDetail,
    CaseOutput,
    MikeCase,
    MikeChat,
    MikeChatDetailOut,
    MikeCitationAnnotation,
    MikeDocument,
    MikeFolder,
    MikeMessage,
    MikeProject,
    MikeWorkflow,
    TabularReview,
    TabularReviewDetailOut,
} from "@/app/components/shared/types";

// Server-side shape before mapping
interface ServerMessage {
    id: string;
    chat_id: string;
    role: "user" | "assistant";
    content: string | AssistantEvent[] | null;
    files?: { filename: string; document_id?: string }[] | null;
    workflow?: { id: string; title: string } | null;
    annotations?: MikeCitationAnnotation[] | null;
    created_at: string;
}
interface ServerChatDetailOut {
    chat: MikeChat;
    messages: ServerMessage[];
}

const API_BASE =
    process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

// Mirror apiRequest's 401 self-heal for streaming responses (which bypass
// apiRequest): clear the stale token and bounce to /login so an expired
// session mid-chat doesn't strand the UI with a raw "HTTP 401" and leave a
// dead token that fails every later send. Returns the response unchanged so
// callers can keep checking response.ok for non-401 errors.
function handleStreamAuth(response: Response): Response {
    if (response.status === 401 && typeof window !== "undefined") {
        localStorage.removeItem("mike_auth_token");
        localStorage.removeItem("mike_auth_user");
        window.location.href = "/login";
        throw new Error("Session expired");
    }
    return response;
}

function getAuthHeader(): Record<string, string> {
    const token =
        typeof window !== "undefined"
            ? localStorage.getItem("mike_auth_token")
            : null;
    if (!token) return {};
    return { Authorization: `Bearer ${token}` };
}

async function apiRequest<T>(path: string, init?: RequestInit): Promise<T> {
    const authHeaders = getAuthHeader();
    const { headers: initHeaders, ...restInit } = init ?? {};
    const response = await fetch(`${API_BASE}${path}`, {
        cache: "no-store",
        ...restInit,
        headers: {
            Accept: "application/json",
            ...authHeaders,
            ...(initHeaders as Record<string, string> | undefined),
        },
    });

    // Stale token (typically: a session token left over from a previous
    // DB or process). Clear local auth and bounce to /login so the user
    // gets a fresh session instead of every API call silently failing.
    if (response.status === 401 && typeof window !== "undefined") {
        localStorage.removeItem("mike_auth_token");
        localStorage.removeItem("mike_auth_user");
        window.location.href = "/login";
        throw new Error("Session expired");
    }

    if (!response.ok) {
        const detail = await response.text();
        throw new Error(detail || `API error: ${response.status}`);
    }

    if (
        response.status === 204 ||
        response.headers.get("content-length") === "0"
    ) {
        return undefined as T;
    }

    return (await response.json()) as T;
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

export async function listProjects(): Promise<MikeProject[]> {
    const data = await apiRequest<{ projects: MikeProject[] } | MikeProject[]>("/project");
    return Array.isArray(data) ? data : (data as { projects: MikeProject[] }).projects ?? [];
}

export async function createProject(
    name: string,
    cm_number?: string,
    shared_with?: string[],
): Promise<MikeProject> {
    return apiRequest<MikeProject>("/project", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name, cm_number, shared_with }),
    });
}

export async function deleteAccount(): Promise<void> {
    return apiRequest<void>("/user/account", { method: "DELETE" });
}

export async function getProject(projectId: string): Promise<MikeProject> {
    return apiRequest<MikeProject>(`/project/${projectId}`);
}

export async function updateProject(
    projectId: string,
    payload: {
        name?: string;
        cm_number?: string;
        shared_with?: string[];
        /** RAG scope: see MikeProject.isolation_mode for semantics. */
        isolation_mode?: "shared" | "strict";
    },
): Promise<MikeProject> {
    return apiRequest<MikeProject>(`/project/${projectId}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function deleteProject(projectId: string): Promise<void> {
    await apiRequest(`/project/${projectId}`, { method: "DELETE" });
}

export interface ProjectPeople {
    owner: {
        user_id: string;
        email: string | null;
        display_name: string | null;
    };
    members: { email: string; display_name: string | null }[];
}

export async function getProjectPeople(
    projectId: string,
): Promise<ProjectPeople> {
    return apiRequest<ProjectPeople>(`/projects/${projectId}/people`);
}

// ---------------------------------------------------------------------------
// Documents
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Folders
// ---------------------------------------------------------------------------

export async function createProjectFolder(
    projectId: string,
    name: string,
    parentFolderId?: string | null,
): Promise<MikeFolder> {
    return apiRequest<MikeFolder>(`/projects/${projectId}/folders`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
            name,
            parent_folder_id: parentFolderId ?? null,
        }),
    });
}

export async function renameProjectFolder(
    projectId: string,
    folderId: string,
    name: string,
): Promise<MikeFolder> {
    return apiRequest<MikeFolder>(
        `/projects/${projectId}/folders/${folderId}`,
        {
            method: "PATCH",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ name }),
        },
    );
}

export async function deleteProjectFolder(
    projectId: string,
    folderId: string,
): Promise<void> {
    await apiRequest(`/projects/${projectId}/folders/${folderId}`, {
        method: "DELETE",
    });
}

export async function moveSubfolderToFolder(
    projectId: string,
    folderId: string,
    parentFolderId: string | null,
): Promise<MikeFolder> {
    return apiRequest<MikeFolder>(
        `/projects/${projectId}/folders/${folderId}`,
        {
            method: "PATCH",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ parent_folder_id: parentFolderId }),
        },
    );
}

export async function moveDocumentToFolder(
    projectId: string,
    documentId: string,
    folderId: string | null,
): Promise<MikeDocument> {
    return apiRequest<MikeDocument>(
        `/projects/${projectId}/documents/${documentId}/folder`,
        {
            method: "PATCH",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ folder_id: folderId }),
        },
    );
}

export async function addDocumentToProject(
    projectId: string,
    documentId: string,
): Promise<MikeDocument> {
    return apiRequest<MikeDocument>(
        `/projects/${projectId}/documents/${documentId}`,
        { method: "POST" },
    );
}

export interface MikeDocumentVersion {
    id: string;
    version_number: number | null;
    source: string;
    created_at: string;
    display_name: string | null;
}

export async function listDocumentVersions(
    documentId: string,
): Promise<{
    current_version_id: string | null;
    versions: MikeDocumentVersion[];
}> {
    return apiRequest(`/single-documents/${documentId}/versions`);
}

export async function uploadDocumentVersion(
    documentId: string,
    file: File,
    displayName?: string,
): Promise<MikeDocumentVersion> {
    const authHeaders = getAuthHeader();
    const form = new FormData();
    form.append("file", file);
    if (displayName) form.append("display_name", displayName);
    const response = await fetch(
        `${API_BASE}/single-documents/${documentId}/versions`,
        {
            method: "POST",
            headers: { ...authHeaders },
            body: form,
        },
    );
    if (!response.ok) throw new Error(await response.text());
    return response.json() as Promise<MikeDocumentVersion>;
}

export async function renameDocumentVersion(
    documentId: string,
    versionId: string,
    displayName: string | null,
): Promise<MikeDocumentVersion> {
    return apiRequest<MikeDocumentVersion>(
        `/single-documents/${documentId}/versions/${versionId}`,
        {
            method: "PATCH",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ display_name: displayName }),
        },
    );
}

export async function uploadProjectDocument(
    projectId: string,
    file: File,
): Promise<MikeDocument> {
    const authHeaders = getAuthHeader();
    const form = new FormData();
    form.append("file", file);
    const response = await fetch(
        `${API_BASE}/projects/${projectId}/documents`,
        {
            method: "POST",
            headers: { ...authHeaders },
            body: form,
        },
    );
    if (!response.ok) throw new Error(await response.text());
    return response.json() as Promise<MikeDocument>;
}

export async function uploadStandaloneDocument(
    file: File,
    options?: { cache?: boolean },
): Promise<MikeDocument> {
    const authHeaders = getAuthHeader();
    const form = new FormData();
    form.append("file", file);
    // `cache: true` tells the backend this is a chat-attached upload —
    // it lands under data/storage/cache/<doc_id> and gets cleaned up
    // when the chat it ends up linked to is deleted. Other call sites
    // (project libraries, tabular review setup) leave this off so the
    // upload stays in the long-lived documents/ tree.
    if (options?.cache) form.append("cache", "true");
    const response = await fetch(`${API_BASE}/single-documents`, {
        method: "POST",
        headers: { ...authHeaders },
        body: form,
    });
    if (!response.ok) throw new Error(await response.text());
    return response.json() as Promise<MikeDocument>;
}

export async function listStandaloneDocuments(): Promise<MikeDocument[]> {
    return apiRequest<MikeDocument[]>("/single-documents");
}

export async function deleteDocument(documentId: string): Promise<void> {
    await apiRequest(`/single-documents/${documentId}`, { method: "DELETE" });
}

export async function getDocumentUrl(
    documentId: string,
    versionId?: string | null,
): Promise<{ url: string; filename: string; version_id: string | null }> {
    const qs = versionId
        ? `?version_id=${encodeURIComponent(versionId)}`
        : "";
    return apiRequest(`/single-documents/${documentId}/url${qs}`);
}

export async function downloadDocumentsZip(
    documentIds: string[],
): Promise<Blob> {
    const authHeaders = getAuthHeader();
    const response = await fetch(`${API_BASE}/single-documents/download-zip`, {
        method: "POST",
        cache: "no-store",
        headers: {
            "Content-Type": "application/json",
            ...authHeaders,
        },
        body: JSON.stringify({ document_ids: documentIds }),
    });
    if (!response.ok) {
        const detail = await response.text();
        throw new Error(detail || `API error: ${response.status}`);
    }
    return response.blob();
}

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

export async function createChat(payload?: {
    project_id?: string;
}): Promise<{ id: string }> {
    return apiRequest<{ id: string }>("/chat", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload ?? {}),
    });
}

export async function listChats(): Promise<MikeChat[]> {
    const data = await apiRequest<{ chats: MikeChat[] } | MikeChat[]>("/chat");
    return Array.isArray(data) ? data : (data as { chats: MikeChat[] }).chats ?? [];
}

export async function listProjectChats(projectId: string): Promise<MikeChat[]> {
    return apiRequest<MikeChat[]>(`/projects/${projectId}/chats`);
}

export async function getChat(chatId: string): Promise<MikeChatDetailOut> {
    const raw = await apiRequest<ServerChatDetailOut>(`/chat/${chatId}`);
    const messages: MikeMessage[] = raw.messages.map((m) => {
        if (m.role === "user") {
            return {
                role: "user",
                content: typeof m.content === "string" ? m.content : "",
                files: m.files ?? undefined,
                workflow: m.workflow ?? undefined,
            };
        }
        const events = Array.isArray(m.content)
            ? (m.content as AssistantEvent[])
            : undefined;
        return {
            role: "assistant",
            content:
                events
                    ?.filter((e) => e.type === "content")
                    .map((e) => (e as { type: "content"; text: string }).text)
                    .join("") ?? "",
            annotations: m.annotations ?? undefined,
            events,
        };
    });
    return { chat: raw.chat, messages };
}

export async function renameChat(chatId: string, title: string): Promise<void> {
    await apiRequest(`/chat/${chatId}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ title }),
    });
}

export async function deleteChat(chatId: string): Promise<void> {
    await apiRequest(`/chat/${chatId}`, { method: "DELETE" });
}

export async function generateChatTitle(
    chatId: string,
    message: string,
): Promise<{ title: string }> {
    return apiRequest<{ title: string }>(`/chat/${chatId}/generate-title`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ message }),
    });
}

export async function streamChat(payload: {
    messages: {
        role: string;
        content: string;
        files?: { filename: string; document_id?: string }[];
        workflow?: { id: string; title: string; prompt_md?: string | null };
        reasoning_content?: string;
    }[];
    chat_id?: string;
    project_id?: string;
    model?: string;
    /** When "court_bundle", the backend focuses the assistant on bundling. */
    intent?: string;
    signal?: AbortSignal;
}): Promise<Response> {
    const { signal, ...body } = payload;
    const authHeaders = getAuthHeader();
    const response = await fetch(`${API_BASE}/chat`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            Accept: "text/event-stream",
            ...authHeaders,
        },
        body: JSON.stringify(body),
        signal,
    });
    return handleStreamAuth(response);
}

type StreamChatMessage = {
    role: string;
    content: string;
    files?: { filename: string; document_id?: string }[];
    workflow?: { id: string; title: string; prompt_md?: string | null };
    reasoning_content?: string;
};

export async function streamProjectChat(payload: {
    projectId: string;
    messages: StreamChatMessage[];
    chat_id?: string;
    model?: string;
    displayed_doc?: { filename: string; document_id: string };
    attached_documents?: { filename: string; document_id: string }[];
    signal?: AbortSignal;
}): Promise<Response> {
    const { projectId, signal, ...body } = payload;
    const authHeaders = getAuthHeader();
    const response = await fetch(`${API_BASE}/projects/${projectId}/chat`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            Accept: "text/event-stream",
            ...authHeaders,
        },
        body: JSON.stringify(body),
        signal,
    });
    return handleStreamAuth(response);
}

// ---------------------------------------------------------------------------
// Tabular Review
// ---------------------------------------------------------------------------

export async function listTabularReviews(
    projectId?: string,
): Promise<TabularReview[]> {
    const qs = projectId
        ? `?project_id=${encodeURIComponent(projectId)}`
        : "";
    return apiRequest<TabularReview[]>(`/tabular-review${qs}`);
}

export async function createTabularReview(payload: {
    title?: string;
    document_ids: string[];
    columns_config: { index: number; name: string; prompt: string }[];
    workflow_id?: string;
    project_id?: string;
}): Promise<TabularReview> {
    return apiRequest<TabularReview>("/tabular-review", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function getTabularReview(
    reviewId: string,
): Promise<TabularReviewDetailOut> {
    return apiRequest<TabularReviewDetailOut>(`/tabular-review/${reviewId}`);
}

export async function updateTabularReview(
    reviewId: string,
    payload: {
        title?: string;
        columns_config?: { index: number; name: string; prompt: string }[];
        document_ids?: string[];
        project_id?: string | null;
        shared_with?: string[];
    },
): Promise<TabularReview> {
    return apiRequest<TabularReview>(`/tabular-review/${reviewId}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function getTabularReviewPeople(
    reviewId: string,
): Promise<ProjectPeople> {
    return apiRequest<ProjectPeople>(`/tabular-review/${reviewId}/people`);
}

export async function generateTabularColumnPrompt(
    title: string,
    options?: { format?: string; documentName?: string; tags?: string[] },
): Promise<{ prompt: string; source: "preset" | "llm" | "fallback" }> {
    return apiRequest<{
        prompt: string;
        source: "preset" | "llm" | "fallback";
    }>("/tabular-review/prompt", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
            title,
            format: options?.format,
            documentName: options?.documentName,
            tags: options?.tags,
        }),
    });
}

export async function uploadReviewDocument(
    reviewId: string,
    file: File,
    options?: {
        projectId?: string;
        documentIds?: string[];
        columnsConfig?: { index: number; name: string; prompt: string }[];
    },
): Promise<MikeDocument> {
    const uploaded = options?.projectId
        ? await uploadProjectDocument(options.projectId, file)
        : await uploadStandaloneDocument(file);

    await updateTabularReview(reviewId, {
        columns_config: options?.columnsConfig,
        document_ids: [...(options?.documentIds ?? []), uploaded.id],
    });

    return uploaded;
}

export async function deleteTabularReview(reviewId: string): Promise<void> {
    await apiRequest(`/tabular-review/${reviewId}`, { method: "DELETE" });
}

export async function streamTabularGeneration(
    reviewId: string,
): Promise<Response> {
    const authHeaders = getAuthHeader();
    return fetch(`${API_BASE}/tabular-review/${reviewId}/generate`, {
        method: "POST",
        headers: { ...authHeaders },
    });
}

export async function streamTabularChat(
    reviewId: string,
    messages: { role: string; content: string }[],
    chat_id?: string | null,
    signal?: AbortSignal,
    context?: { reviewTitle?: string | null; projectName?: string | null },
): Promise<Response> {
    const authHeaders = getAuthHeader();
    return fetch(`${API_BASE}/tabular-review/${reviewId}/chat`, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...authHeaders },
        body: JSON.stringify({
            messages,
            chat_id: chat_id ?? undefined,
            review_title: context?.reviewTitle ?? undefined,
            project_name: context?.projectName ?? undefined,
        }),
        signal: signal ?? undefined,
    });
}

export interface TRCitationAnnotation {
    type: "tabular_citation";
    ref: number;
    col_index: number;
    row_index: number;
    col_name: string;
    doc_name: string;
    quote: string;
}

interface RawTRMessage {
    id: string;
    chat_id: string;
    role: "user" | "assistant";
    content: string | AssistantEvent[] | null;
    annotations?: TRCitationAnnotation[] | null;
    created_at: string;
}

export interface TRDisplayMessage {
    role: "user" | "assistant";
    content: string;
    events?: AssistantEvent[];
    annotations?: TRCitationAnnotation[];
}

export interface TRChat {
    id: string;
    title: string | null;
    created_at: string;
    updated_at: string;
}

export function mapTRMessages(raw: RawTRMessage[]): TRDisplayMessage[] {
    return raw.map((m) => {
        if (m.role === "user") {
            return {
                role: "user" as const,
                content: typeof m.content === "string" ? m.content : "",
            };
        }
        const events = Array.isArray(m.content)
            ? (m.content as AssistantEvent[])
            : undefined;
        const content =
            events
                ?.filter((e) => e.type === "content")
                .map((e) => (e as { type: "content"; text: string }).text)
                .join("") ?? "";
        return {
            role: "assistant" as const,
            content,
            events,
            annotations: m.annotations ?? undefined,
        };
    });
}

export async function getTabularChats(reviewId: string): Promise<TRChat[]> {
    return apiRequest<TRChat[]>(`/tabular-review/${reviewId}/chats`);
}

export async function getTabularChatMessages(
    reviewId: string,
    chatId: string,
): Promise<RawTRMessage[]> {
    return apiRequest<RawTRMessage[]>(
        `/tabular-review/${reviewId}/chats/${chatId}/messages`,
    );
}

export async function deleteTabularChat(
    reviewId: string,
    chatId: string,
): Promise<void> {
    await apiRequest(`/tabular-review/${reviewId}/chats/${chatId}`, {
        method: "DELETE",
    });
}

export async function regenerateTabularCell(
    reviewId: string,
    documentId: string,
    columnIndex: number,
): Promise<{
    summary: string;
    flag: "green" | "grey" | "yellow" | "red";
    reasoning: string;
}> {
    return apiRequest(`/tabular-review/${reviewId}/regenerate-cell`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
            document_id: documentId,
            column_index: columnIndex,
        }),
    });
}

export async function clearTabularCells(
    reviewId: string,
    documentIds: string[],
): Promise<void> {
    await apiRequest(`/tabular-review/${reviewId}/clear-cells`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ document_ids: documentIds }),
    });
}

// ---------------------------------------------------------------------------
// Workflows
// ---------------------------------------------------------------------------

type WorkflowType = MikeWorkflow["type"];

export async function listWorkflows(
    type: WorkflowType,
): Promise<MikeWorkflow[]> {
    const data = await apiRequest<{ workflows: MikeWorkflow[] } | MikeWorkflow[]>(`/workflow?type=${type}`);
    return Array.isArray(data) ? data : (data as { workflows: MikeWorkflow[] }).workflows ?? [];
}

export async function getWorkflow(workflowId: string): Promise<MikeWorkflow> {
    return apiRequest<MikeWorkflow>(`/workflow/${workflowId}`);
}

export async function createWorkflow(payload: {
    title: string;
    type: "assistant" | "tabular";
    prompt_md?: string;
    columns_config?: { index: number; name: string; prompt: string }[];
    practice?: string | null;
}): Promise<MikeWorkflow> {
    return apiRequest<MikeWorkflow>("/workflow", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function updateWorkflow(
    workflowId: string,
    payload: {
        title?: string;
        prompt_md?: string;
        columns_config?: { index: number; name: string; prompt: string }[];
        practice?: string | null;
    },
): Promise<MikeWorkflow> {
    return apiRequest<MikeWorkflow>(`/workflow/${workflowId}`, {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function deleteWorkflow(workflowId: string): Promise<void> {
    await apiRequest(`/workflow/${workflowId}`, { method: "DELETE" });
}

export async function listHiddenWorkflows(): Promise<string[]> {
    return apiRequest<string[]>("/workflow/hidden");
}

export async function hideWorkflow(workflowId: string): Promise<void> {
    await apiRequest("/workflow/hidden", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ workflow_id: workflowId }),
    });
}

export async function unhideWorkflow(workflowId: string): Promise<void> {
    await apiRequest(`/workflow/hidden/${workflowId}`, { method: "DELETE" });
}

export async function shareWorkflow(
    workflowId: string,
    payload: { emails: string[]; allow_edit: boolean },
): Promise<void> {
    await apiRequest<void>(`/workflows/${workflowId}/share`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function listWorkflowShares(
    workflowId: string,
): Promise<
    {
        id: string;
        shared_with_email: string;
        allow_edit: boolean;
        created_at: string;
    }[]
> {
    return apiRequest(`/workflows/${workflowId}/shares`);
}

export async function deleteWorkflowShare(
    workflowId: string,
    shareId: string,
): Promise<void> {
    await apiRequest(`/workflows/${workflowId}/shares/${shareId}`, {
        method: "DELETE",
    });
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

export async function listCases(): Promise<MikeCase[]> {
    const data = await apiRequest<{ cases: MikeCase[] } | MikeCase[]>("/cases");
    return Array.isArray(data) ? data : (data as { cases: MikeCase[] }).cases ?? [];
}

export async function createCase(payload: {
    title: string;
    court?: string;
    parties_json?: string;
}): Promise<MikeCase> {
    return apiRequest<MikeCase>("/cases", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function getCase(caseId: string): Promise<CaseDetail> {
    return apiRequest<CaseDetail>(`/cases/${caseId}`);
}

export async function updateCase(
    caseId: string,
    payload: { title?: string; court?: string; parties_json?: string; status?: string },
): Promise<MikeCase> {
    return apiRequest<MikeCase>(`/cases/${caseId}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
    });
}

export async function deleteCase(caseId: string): Promise<void> {
    await apiRequest<void>(`/cases/${caseId}`, { method: "DELETE" });
}

export interface ResolvedPrecedentCase {
    tid: number;
    title: string | null;
    court: string | null;
    decision_date: string | null;
    kanoon_url: string | null;
    snippet: string | null;
    relevant_paragraphs: string | null;
    relevance_score: number | null;
    confidence: number | null;
    reason: string | null;
}

export interface ResolvedPrecedent {
    point_of_law: string | null;
    suggested_search_query: string;
    target_court: string | null;
    grounding: { source_doc_id: string; exact_quote: string } | null;
    cases: ResolvedPrecedentCase[];
}

/** Resolve precedent_finder search suggestions into real Indian Kanoon cases. */
export async function resolveCasePrecedents(
    caseId: string,
    resultsPerQuery = 1,
): Promise<ResolvedPrecedent[]> {
    const data = await apiRequest<{ resolved_precedents: ResolvedPrecedent[] }>(
        `/cases/${caseId}/resolve-precedents`,
        {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ results_per_query: resultsPerQuery }),
        },
    );
    return data.resolved_precedents ?? [];
}

export async function addCaseDocuments(
    caseId: string,
    documents: { document_id: string; document_type?: string }[],
): Promise<void> {
    const document_ids = documents.map((d) => d.document_id);
    const document_types: Record<string, string> = {};
    for (const d of documents) {
        if (d.document_type) document_types[d.document_id] = d.document_type;
    }
    await apiRequest<void>(`/cases/${caseId}/documents`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
            document_ids,
            document_types: Object.keys(document_types).length > 0 ? document_types : undefined,
        }),
    });
}

export async function removeCaseDocument(
    caseId: string,
    documentId: string,
): Promise<void> {
    await apiRequest<void>(`/cases/${caseId}/documents/${documentId}`, {
        method: "DELETE",
    });
}

export async function analyzeCaseStream(
    caseId: string,
    redactPii?: boolean,
    signal?: AbortSignal,
): Promise<Response> {
    const token =
        typeof window !== "undefined"
            ? localStorage.getItem("mike_auth_token")
            : null;
    return fetch(`${API_BASE}/cases/${caseId}/analyze`, {
        method: "POST",
        headers: {
            Accept: "text/event-stream",
            "Content-Type": "application/json",
            ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        body: JSON.stringify({ redact_pii: redactPii ?? false }),
        signal,
    });
}

export async function generateCaseOutput(
    caseId: string,
    outputType: string,
    redactPii?: boolean,
): Promise<CaseOutput> {
    const typeToEndpoint: Record<string, string> = {
        case_brief: "brief",
        strategy_memo: "strategy-memo",
        hearing_prep: "hearing-prep",
        list_of_dates: "list-of-dates",
        annexure_index: "annexure-index",
    };
    const endpoint = typeToEndpoint[outputType] ?? outputType;
    return apiRequest<CaseOutput>(`/cases/${caseId}/outputs/${endpoint}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ redact_pii: redactPii ?? false }),
    });
}

export async function streamCaseChat(payload: {
    caseId: string;
    messages: { role: string; content: string; workflow?: { id: string; title: string; prompt_md?: string | null } }[];
    chat_id?: string;
    model?: string;
    signal?: AbortSignal;
}): Promise<Response> {
    const token =
        typeof window !== "undefined"
            ? localStorage.getItem("mike_auth_token")
            : null;
    return fetch(`${API_BASE}/cases/${payload.caseId}/chat`, {
        method: "POST",
        headers: {
            "Content-Type": "application/json",
            Accept: "text/event-stream",
            ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        body: JSON.stringify({
            messages: payload.messages,
            chat_id: payload.chat_id,
            model: payload.model,
        }),
        signal: payload.signal,
    });
}

/**
 * Restore a case's conversation after a reload. Returns the most recent
 * case-scoped chat (its id, so new turns keep appending to it) and the prior
 * messages in order. `chat_id` is null when the case has never been chatted.
 */
export async function getCaseChat(
    caseId: string,
): Promise<{ chat_id: string | null; messages: { role: string; content: string }[] }> {
    return apiRequest<{ chat_id: string | null; messages: { role: string; content: string }[] }>(
        `/cases/${caseId}/chat`,
    );
}

// ---------------------------------------------------------------------------
// Personalization
// ---------------------------------------------------------------------------

export interface PersonalizationProfile {
    profile_text: string;
    updated_at: string | null;
}

export async function getPersonalization(): Promise<PersonalizationProfile> {
    return apiRequest<PersonalizationProfile>("/personalization");
}

export async function putPersonalization(
    profile_text: string,
): Promise<PersonalizationProfile> {
    return apiRequest<PersonalizationProfile>("/personalization", {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ profile_text }),
    });
}

export async function deletePersonalization(): Promise<void> {
    await apiRequest("/personalization", { method: "DELETE" });
}

// ---------------------------------------------------------------------------
// Drafting registry: parties, annexures, citations (cross-references)
// ---------------------------------------------------------------------------

export interface CasePartyRecord {
    id: string;
    case_id: string;
    slug: string;
    name: string;
    side: "petitioner" | "respondent";
    role_label: string | null;
    serial_no: number;
    source: "manual" | "ai";
    created_at: string;
    updated_at: string;
}

export interface CaseAnnexure {
    id: string;
    case_id: string;
    document_id: string;
    slug: string;
    description: string | null;
    doc_date: string | null;
    side: "P" | "R" | "C";
    serial_no: number;
    created_at: string;
    updated_at: string;
}

export interface CaseCitation {
    id: string;
    kind: "judgment" | "statute";
    status: "referred" | "cited";
    kanoon_tid?: number | null;
    title?: string | null;
    court?: string | null;
    decision_date?: string | null;
    kanoon_url?: string | null;
    canonical_citation?: string | null;
    pdf_document_id?: string | null;
    statute?: string | null;
    section_number?: string | null;
    times_cited: number;
    first_cited_at: string;
    last_cited_at: string;
}

export async function listCaseParties(caseId: string): Promise<{ parties: CasePartyRecord[] }> {
    return apiRequest(`/cases/${caseId}/parties`);
}

export async function createCaseParty(
    caseId: string,
    body: { name: string; side: "petitioner" | "respondent"; role_label?: string },
): Promise<CasePartyRecord> {
    return apiRequest(`/cases/${caseId}/parties`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
}

export async function updateCaseParty(
    caseId: string,
    partyId: string,
    body: { name?: string; role_label?: string; slug?: string },
): Promise<{ party: CasePartyRecord | null }> {
    return apiRequest(`/cases/${caseId}/parties/${partyId}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
}

export async function deleteCaseParty(caseId: string, partyId: string): Promise<void> {
    await apiRequest(`/cases/${caseId}/parties/${partyId}`, { method: "DELETE" });
}

export async function reorderCaseParties(
    caseId: string,
    side: string,
    orderedIds: string[],
): Promise<{ parties: CasePartyRecord[] }> {
    return apiRequest(`/cases/${caseId}/parties/reorder`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ side, ordered_ids: orderedIds }),
    });
}

export async function aiPopulateParties(
    caseId: string,
): Promise<{ seeded: number; parties: CasePartyRecord[] }> {
    return apiRequest(`/cases/${caseId}/parties/ai-populate`, { method: "POST" });
}

export async function listCaseAnnexures(caseId: string): Promise<{ annexures: CaseAnnexure[] }> {
    return apiRequest(`/cases/${caseId}/annexures`);
}

export async function createCaseAnnexure(
    caseId: string,
    body: { document_id: string; side?: string; description?: string; doc_date?: string },
): Promise<CaseAnnexure> {
    return apiRequest(`/cases/${caseId}/annexures`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
}

export async function updateCaseAnnexure(
    caseId: string,
    annexureId: string,
    body: { description?: string; doc_date?: string; side?: string; slug?: string },
): Promise<{ annexure: CaseAnnexure | null }> {
    return apiRequest(`/cases/${caseId}/annexures/${annexureId}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
}

export async function deleteCaseAnnexure(caseId: string, annexureId: string): Promise<void> {
    await apiRequest(`/cases/${caseId}/annexures/${annexureId}`, { method: "DELETE" });
}

export async function reorderCaseAnnexures(
    caseId: string,
    side: string,
    orderedIds: string[],
): Promise<{ annexures: CaseAnnexure[] }> {
    return apiRequest(`/cases/${caseId}/annexures/reorder`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ side, ordered_ids: orderedIds }),
    });
}

export async function aiPopulateAnnexures(
    caseId: string,
): Promise<{ seeded: number; annexures: CaseAnnexure[] }> {
    return apiRequest(`/cases/${caseId}/annexures/ai-populate`, { method: "POST" });
}

export async function listCaseCitations(
    caseId: string,
): Promise<{ judgments: CaseCitation[]; statutes: CaseCitation[] }> {
    return apiRequest(`/cases/${caseId}/citations`);
}

export async function deleteCaseCitation(caseId: string, citationId: string): Promise<void> {
    await apiRequest(`/cases/${caseId}/citations/${citationId}`, { method: "DELETE" });
}

export async function resolveCaseRefs(
    caseId: string,
    markdown: string,
): Promise<{ markdown: string; unresolved: string[] }> {
    return apiRequest(`/cases/${caseId}/resolve-refs`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ markdown }),
    });
}

export async function generateCasesReferred(
    caseId: string,
): Promise<{ output_id: string; doc_id: string; content_md: string }> {
    return apiRequest(`/cases/${caseId}/outputs/cases-referred`, { method: "POST" });
}

export async function generateAuthorities(
    caseId: string,
): Promise<{ output_id: string; doc_id: string; content_md: string }> {
    return apiRequest(`/cases/${caseId}/outputs/authorities`, { method: "POST" });
}


// ---------------------------------------------------------------------------
// Firm knowledge corpus
// ---------------------------------------------------------------------------

export interface CorpusFile {
    id: string;
    filename: string;
    doc_type: string | null;
    case_type: string | null;
    court: string | null;
    status: "pending" | "extracting" | "chunking" | "tagging" | "ready" | "failed" | "unsupported";
    chunk_count: number;
    is_template: boolean;
    created_at: string;
    error: string | null;
    batch_id: string | null;
    batch_label: string | null;
}

export interface CorpusLimits {
    max_docs: number;
    max_file_bytes: number;
    supported_exts: string[];
}

export async function getCorpusLimits(): Promise<CorpusLimits> {
    return apiRequest("/corpus/limits");
}

export async function listCorpusFiles(batchId?: string): Promise<{ files: CorpusFile[] }> {
    return apiRequest(`/corpus/files${batchId ? "?batch_id=" + encodeURIComponent(batchId) : ""}`);
}

export async function uploadCorpusFiles(
    files: File[],
    isTemplate = false,
    batchId?: string,
    batchLabel?: string,
): Promise<{ accepted: string[]; duplicates: string[]; skipped: { filename: string; reason: string }[] }> {
    const form = new FormData();
    for (const f of files) form.append("file", f);
    if (isTemplate) form.append("is_template", "true");
    if (batchId) form.append("batch_id", batchId);
    if (batchLabel) form.append("batch_label", batchLabel);
    const response = await fetch(`${API_BASE}/corpus/files`, {
        method: "POST",
        headers: { ...getAuthHeader() },
        body: form,
    });
    if (!response.ok) throw new Error(await response.text());
    return response.json();
}

/** Returns the raw streaming Response; the caller reads the SSE body. */
export async function processCorpusFiles(fileIds: string[]): Promise<Response> {
    return fetch(`${API_BASE}/corpus/process`, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...getAuthHeader() },
        body: JSON.stringify({ file_ids: fileIds }),
    });
}

export async function updateCorpusFile(
    id: string,
    body: { is_template?: boolean; doc_type?: string; case_type?: string },
): Promise<{ ok: boolean }> {
    return apiRequest(`/corpus/files/${id}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
}

export async function deleteCorpusFile(id: string): Promise<void> {
    await apiRequest(`/corpus/files/${id}`, { method: "DELETE" });
}

export async function deleteCorpusBatch(batchId: string): Promise<{ ok: boolean; deleted: number }> {
    return apiRequest(`/corpus/batches/${encodeURIComponent(batchId)}`, { method: "DELETE" });
}

// ---- Statutes (Indian statute database) ----

export interface StatuteAct {
    id: number;
    short_name: string;
    full_title: string;
    year: number | null;
    status: string;
    replaced_by: string | null;
    category: string | null;
    language: string | null;
    section_count: number;
}

export async function listStatuteActs(): Promise<StatuteAct[]> {
    return apiRequest(`/statutes/acts`);
}

/** Returns the raw streaming Response; the caller reads the SSE body. */
export async function ingestStatute(url: string): Promise<Response> {
    return fetch(`${API_BASE}/statutes/ingest`, {
        method: "POST",
        headers: { "Content-Type": "application/json", ...getAuthHeader() },
        body: JSON.stringify({ url }),
    });
}

export async function deleteStatuteAct(shortName: string): Promise<void> {
    await apiRequest(`/statutes/acts/${encodeURIComponent(shortName)}`, {
        method: "DELETE",
    });
}

// ---- Indian Kanoon BYOK API ----

export interface IndianKanoonConfig {
    enabled: boolean;
    has_key: boolean;
}

export async function getIndianKanoonConfig(): Promise<IndianKanoonConfig> {
    return apiRequest(`/indian-kanoon/config`);
}

export async function putIndianKanoonConfig(body: {
    enabled: boolean;
    ik_api_key?: string | null;
}): Promise<void> {
    await apiRequest(`/indian-kanoon/config`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
    });
}
