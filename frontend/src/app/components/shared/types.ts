// Shared TypeScript types for Mike AI legal assistant

export interface MikeFolder {
  id: string;
  project_id: string;
  user_id: string;
  name: string;
  parent_folder_id: string | null;
  created_at: string;
  updated_at: string;
}

export interface MikeProject {
  id: string;
  user_id: string;
  is_owner?: boolean;
  name: string;
  cm_number: string | null;
  shared_with: string[];
  created_at: string;
  updated_at: string;
  documents?: MikeDocument[];
  folders?: MikeFolder[];
  document_count?: number;
  chat_count?: number;
  review_count?: number;
  /**
   * RAG isolation: 'shared' (default) means chats inside this project
   * see global pool + project pool; 'strict' means project pool only.
   * Used by the chat dispatcher's retrieve_kb_chunks() to pick the
   * right SearchScope on every turn.
   */
  isolation_mode?: "shared" | "strict";
}

export interface MikeDocument {
  id: string;
  user_id?: string;
  project_id: string | null;
  folder_id?: string | null;
  filename: string;
  file_type: string | null; // pdf | docx | doc
  storage_path: string | null;
  pdf_storage_path: string | null;
  size_bytes: number | null;
  page_count: number | null;
  structure_tree: StructureNode[] | null;
  status: "pending" | "processing" | "ready" | "error";
  created_at: string | null;
  updated_at?: string | null;
  /** Max version_number across assistant_edit rows, null if doc is unedited. */
  latest_version_number?: number | null;
}

export interface StructureNode {
  id: string;
  title: string;
  level: number;
  page_number: number | null;
  children: StructureNode[];
}

export interface MikeChat {
  id: string;
  project_id: string | null;
  user_id: string;
  title: string | null;
  created_at: string;
}

export interface MikeEditAnnotation {
  type?: "edit_data";
  kind?: "edit";
  edit_id: string;
  document_id: string;
  version_id: string;
  /** Per-document monotonic Vn for the edit's target version. */
  version_number?: number | null;
  change_id: string;
  del_w_id?: string;
  ins_w_id?: string;
  deleted_text: string;
  inserted_text: string;
  context_before?: string;
  context_after?: string;
  reason?: string;
  status: "pending" | "accepted" | "rejected";
}

export type AssistantEvent =
  | { type: "reasoning"; text: string; isStreaming?: boolean }
  | {
        type: "tool_call_start";
        name: string;
        isStreaming?: boolean;
        /** Updated in place by `tool_call_progress` SSE events the backend
         *  emits every 5 s while a tool is still in flight. The UI uses
         *  this to show "Sto eseguendo X (Ns)…" so the user knows the
         *  long wait is intentional (e.g. an MCP tool that requires a
         *  manual approval click on the server side). Absent on the
         *  initial event; populated thereafter. */
        elapsedSecs?: number;
        progressLabel?: string;
    }
  | { type: "thinking"; isStreaming?: boolean }
  | {
        type: "doc_read";
        filename: string;
        document_id?: string;
        isStreaming?: boolean;
    }
  | {
        type: "doc_find";
        filename: string;
        query: string;
        total_matches: number;
        isStreaming?: boolean;
    }
  | {
        type: "doc_created";
        filename: string;
        download_url: string;
        /** Set when the generated doc is persisted as a first-class document. */
        document_id?: string;
        version_id?: string;
        version_number?: number | null;
        /** Source markdown the model used to generate the .docx; seeds the inline AI editor. */
        body?: string;
        isStreaming?: boolean;
        /** Court-bundle compile progress: current stage label (e.g. "Preparing Annexure P-1"). */
        stage?: string;
        /** Item progress within a stage (e.g. 2 of 5). */
        stageCurrent?: number;
        stageTotal?: number;
        /** Epoch ms when compilation started, for the elapsed timer. */
        startedAt?: number;
    }
  | { type: "doc_download"; filename: string; download_url: string }
  | {
        type: "doc_replicated";
        /** Source document filename. */
        filename: string;
        /** How many copies were produced in this single tool call. */
        count: number;
        /** One entry per new copy. Empty while streaming. */
        copies?: {
            new_filename: string;
            document_id: string;
            version_id: string;
        }[];
        error?: string;
        isStreaming?: boolean;
    }
  | { type: "workflow_applied"; workflow_id: string; title: string }
  | {
        type: "doc_edited";
        filename: string;
        document_id: string;
        version_id: string;
        /** Per-document monotonic Vn written at emit time. */
        version_number?: number | null;
        download_url: string;
        annotations: MikeEditAnnotation[];
        error?: string;
        isStreaming?: boolean;
    }
  | { type: "content"; text: string; isStreaming?: boolean }
  | {
        type: "clarification";
        request_id?: string;
        questions: {
            header?: string;
            text: string;
            multiSelect?: boolean;
            options?: { label: string; description?: string }[];
            chips?: string[];
        }[];
    };

export interface MikeMessage {
  role: "user" | "assistant";
  content: string;
  files?: { filename: string; document_id?: string }[];
  workflow?: { id: string; title: string; prompt_md?: string | null };
  model?: string;
  annotations?: MikeCitationAnnotation[];
  events?: AssistantEvent[];
  /** Set when streaming failed; rendered as a red error block. */
  error?: string;
  /** DeepSeek reasoning content — must be passed back to the API on subsequent turns. */
  reasoning_content?: string;
}

export interface CitationQuote {
  page: number;
  quote: string;
}

/**
 * A citation emitted by the assistant. Single-page citations have a numeric
 * `page` and a plain `quote`. A citation that spans a page break (one
 * continuous sentence cut by a page boundary) has `page` as a range string
 * like "41-42" and a `quote` containing the `[[PAGE_BREAK]]` sentinel at the
 * break point (text before is on page 41, text after is on page 42).
 */
export interface MikeCitationAnnotation {
  type: "citation_data";
  ref: number;
  doc_id: string;
  document_id: string;
  version_id?: string | null;
  version_number?: number | null;
  filename: string;
  page: number | string;
  quote: string;

  /**
   * Where this citation came from. Set by the backend at parse time so
   * the UI can render the right badge and pick the right opener:
   *  - "attached" (default) → user-attached document, opened via
   *    `/document/{id}/...` like before
   *  - "kb" → retrieved from the RAG knowledge base (auto-retrieval).
   *    `path` is the absolute filesystem path; `chunk_index` is the
   *    chunk number; `scope` is "global" or "project"
   *  - "tool"  → fetched by the model via `search_kb` tool
   *    (placeholder — tool not yet implemented)
   */
  source?: "attached" | "kb" | "tool" | "vanga";
  scope?: "global" | "project";
  path?: string;
  chunk_index?: number;
  pdf_url?: string;
  court_code?: string;
}

const PAGE_BREAK_SENTINEL = "[[PAGE_BREAK]]";

/**
 * Expand a citation into one or more (page, quote) entries suitable for
 * highlighting in the PDF viewer. A single-page citation yields one entry; a
 * cross-page citation with page "N-M" and a `[[PAGE_BREAK]]` split yields two.
 */
export function expandCitationToEntries(
  a: MikeCitationAnnotation,
): CitationQuote[] {
  const rangeMatch =
    typeof a.page === "string"
      ? a.page.match(/^(\d+)\s*-\s*(\d+)$/)
      : null;
  if (rangeMatch && a.quote.includes(PAGE_BREAK_SENTINEL)) {
    const startPage = parseInt(rangeMatch[1], 10);
    const endPage = parseInt(rangeMatch[2], 10);
    const [before, after] = a.quote.split(PAGE_BREAK_SENTINEL);
    return [
      { page: startPage, quote: before.trim() },
      { page: endPage, quote: after.trim() },
    ].filter((e) => e.quote.length > 0);
  }
  const pageNum =
    typeof a.page === "number" ? a.page : parseInt(String(a.page), 10);
  if (!Number.isFinite(pageNum)) return [];
  return [{ page: pageNum, quote: a.quote }];
}

/** Format the page(s) of a citation for display, e.g. "Page 3" or "Page 41-42". */
export function formatCitationPage(a: MikeCitationAnnotation): string {
  if (typeof a.page === "string") return `Page ${a.page}`;
  return `Page ${a.page}`;
}

/** Produce a reader-friendly version of the quote (replaces [[PAGE_BREAK]] with "..."). */
export function displayCitationQuote(a: MikeCitationAnnotation): string {
  return a.quote.replaceAll(PAGE_BREAK_SENTINEL, "...");
}

// Tabular Review

export type ColumnFormat =
    | "text"
    | "bulleted_list"
    | "number"
    | "currency"
    | "yes_no"
    | "date"
    | "tag"
    | "percentage"
    | "monetary_amount";

export interface ColumnConfig {
    index: number;
    name: string;
    prompt: string;
    format?: ColumnFormat;
    tags?: string[];
}

export interface TabularReview {
  id: string;
  project_id: string | null;
  user_id: string;
  title: string | null;
  columns_config: ColumnConfig[] | null;
  workflow_id: string | null;
  practice?: string | null;
  /** Per-review email list. Used so standalone (project_id null) reviews can be shared directly. */
  shared_with?: string[];
  /** Server-set: true when the requesting user is the review's creator. */
  is_owner?: boolean;
  created_at: string;
  updated_at: string;
  document_count?: number;
}

export interface TabularCell {
  id: string;
  review_id: string;
  document_id: string;
  column_index: number;
  content: {
    summary: string;
    flag?: "green" | "grey" | "yellow" | "red";
    reasoning?: string;
  } | null;
  status: "pending" | "generating" | "done" | "error";
  created_at: string;
}

// Workflows

export interface MikeWorkflow {
  id: string;
  user_id: string | null;
  title: string;
  type: "assistant" | "tabular";
  prompt_md: string | null;
  columns_config: ColumnConfig[] | null;
  is_system: boolean;
  created_at: string;
  practice?: string | null;
  shared_by_name?: string | null;
  allow_edit?: boolean;
  is_owner?: boolean;
}

// API helpers

export interface MikeChatDetailOut {
  chat: MikeChat;
  messages: MikeMessage[];
}

export interface TabularReviewDetailOut {
  review: TabularReview;
  cells: TabularCell[];
  documents: MikeDocument[];
}

// Cases

export interface MikeCase {
  id: string;
  user_id: string;
  title: string;
  court: string | null;
  parties_json: string | null;
  status: string;
  created_at: string;
  updated_at: string;
  document_count?: number;
}

export interface CaseParty {
  name: string;
  role: "petitioner" | "respondent" | "appellant" | "other";
}

export interface CaseDocument {
  case_id: string;
  document_id: string;
  document_type: string | null;
  attached_at: string | null;
  filename?: string;
  file_type?: string | null;
  status?: string;
  size_bytes?: number;
  page_count?: number;
  needs_ocr?: boolean;
}

export interface CaseFinding {
  id: string;
  case_id: string;
  agent_name: string;
  finding_type: string;
  content_json: string;
  grounding_json: string | null;
  created_at: string;
}

export interface CaseOutput {
  id: string;
  case_id: string;
  output_type: string;
  content_md: string;
  docx_document_id: string | null;
  created_at: string;
}

export interface CaseDetail {
  case_info: MikeCase;
  documents: CaseDocument[];
  findings: CaseFinding[];
  outputs: CaseOutput[];
}

export type AnalysisAgentStatus = "pending" | "running" | "done" | "error";

export interface AnalysisProgress {
  agent_name: string;
  status: AnalysisAgentStatus;
  error?: string;
  thinking?: string;
}
