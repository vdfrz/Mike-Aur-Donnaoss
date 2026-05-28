# Interactive Analysis Experience — Batch Prompt

**Goal:** Transform the case analysis wait from a silent spinner into a Lavern-style interactive experience where users see their agents working in real time. Users should never wonder "is it stuck?" — even on a 128-page scanned PDF that takes 3+ minutes.

**Inspiration:** Lavern (`/Users/vedantmishra/Downloads/lavern-main/viz/src/working/`) — study their `WorkingView.tsx`, `InsightFeed.tsx`, `ProgressSidebar.tsx`, `ReassuranceCard.tsx`, `useReassuranceInjector.ts`, and `useNarrativeStatus.ts` for patterns. We're adapting their architecture to Mike's simpler 7-agent parallel model.

---

## PART A: Backend Changes (Rust)

### A1. Add `page_count` to documents table

**Migration file:** Create `migrations/0032_document_page_count.sql`:
```sql
ALTER TABLE documents ADD COLUMN page_count INTEGER;
ALTER TABLE documents ADD COLUMN needs_ocr INTEGER DEFAULT 0;
```

**Populate at upload time:** In `src/routes/documents.rs`, after text extraction succeeds, write `page_count` and `needs_ocr` back to the documents row. The PDF extraction code (`src/sync/scanner.rs` / `src/pdf.rs`) already knows page count (pdfium loads all pages) and OCR status (the `OCR_FALLBACK_THRESHOLD` check). Just persist these values.

For the `extract_text_dispatch` function in `src/sync/scanner.rs`: after extraction, return a struct with `page_count: Option<u32>` and `needed_ocr: bool` alongside the extracted text. Then in the documents route, update:
```sql
UPDATE documents SET page_count = ?, needs_ocr = ? WHERE id = ?
```

For DOCX/XLSX, estimate page count as `word_count / 300` (or leave NULL — less critical than PDFs).

### A2. Return document metadata in case endpoints

In `src/routes/cases.rs`, update the `get_case` handler's documents query to JOIN and include `page_count`, `size_bytes`, `needs_ocr`, and `file_type` from the `documents` table:

```sql
SELECT cd.document_id, cd.document_type, cd.attached_at,
       d.filename, d.file_type, d.size_bytes, d.page_count, d.needs_ocr
FROM case_documents cd
JOIN documents d ON d.id = cd.document_id
WHERE cd.case_id = ?
```

Return these in the JSON response so the frontend has everything it needs.

### A3. Richer SSE events during analysis

The orchestrator at `src/agents/case_prep/orchestrator.rs` already has the infrastructure for parallel agents. We need to add a progress channel so it can emit events during execution, not just at the end.

**Add a progress callback to `analyze_case`:**

Currently the function signature is:
```rust
pub async fn analyze_case(case_id, user_id, db, llm_params) -> Result<Vec<Finding>>
```

Change to accept an optional progress sender:
```rust
pub async fn analyze_case(
    case_id: &str,
    user_id: &str, 
    db: &SqlitePool,
    llm_params: StreamParams,
    progress_tx: Option<tokio::sync::mpsc::Sender<ProgressEvent>>,
) -> Result<Vec<Finding>>
```

Where `ProgressEvent` is:
```rust
pub enum ProgressEvent {
    /// Text extraction starting for a document
    ExtractingDoc { filename: String, doc_index: usize, total_docs: usize },
    /// Text extraction complete for a document  
    ExtractedDoc { filename: String, doc_index: usize, total_docs: usize, page_count: usize, needed_ocr: bool },
    /// An agent has started running
    AgentStarted { agent_name: String },
    /// An agent emitted a thinking/content delta (throttled to ~1/sec)
    AgentThinking { agent_name: String, snippet: String },
    /// An agent completed successfully
    AgentDone { agent_name: String, finding_type: String },
    /// An agent failed
    AgentError { agent_name: String, error: String },
    /// Context compression happening (large documents)
    Compressing { original_tokens: usize, target_tokens: usize },
}
```

**In `run_agent`:** Modify `call_llm` to accept the progress sender. While streaming the LLM response (`StreamEvent::ContentDelta`), buffer deltas and flush an `AgentThinking` event every ~1 second (use a `tokio::time::Instant` to throttle). This is the key to showing live thinking.

**In the document extraction loop** (lines 64-74 of orchestrator.rs): Emit `ExtractingDoc` before each document and `ExtractedDoc` after. This gives the frontend per-document progress.

**In `run_case_analysis`** (cases.rs): Forward `ProgressEvent`s to SSE as they arrive:
```rust
let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(128);

// Spawn a forwarder that converts ProgressEvents to SSE Events
let sse_tx_clone = tx.clone();
tokio::spawn(async move {
    while let Some(evt) = progress_rx.recv().await {
        let json = match evt {
            ProgressEvent::ExtractingDoc { filename, doc_index, total_docs } => json!({
                "type": "extracting_doc",
                "filename": filename,
                "doc_index": doc_index,
                "total_docs": total_docs,
            }),
            ProgressEvent::AgentStarted { agent_name } => json!({
                "type": "agent_status",
                "agent_name": agent_name,
                "status": "running",
            }),
            ProgressEvent::AgentThinking { agent_name, snippet } => json!({
                "type": "agent_thinking",
                "agent_name": agent_name,
                "snippet": snippet,
            }),
            // ... map all variants to JSON
        };
        let _ = sse_tx_clone.send(Ok(Event::default().data(json.to_string()))).await;
    }
});
```

### A4. Time estimation

Add a simple estimation function based on total page count and document types:

```rust
fn estimate_analysis_seconds(docs: &[(String, String, String, String)], page_counts: &[Option<u32>]) -> u32 {
    let total_pages: u32 = page_counts.iter().filter_map(|p| *p).sum();
    let has_ocr = docs.iter().any(|(_, _, file_type, _)| file_type == "pdf"); // simplified
    
    let base = 30; // minimum 30s for LLM calls
    let per_page = if has_ocr { 1 } else { 0 }; // ~1s per OCR page
    let llm_time = 60; // 7 agents in parallel, ~60s for LLM
    
    base + (total_pages * per_page) + llm_time
}
```

Emit this as an early SSE event:
```json
{"type": "estimate", "total_pages": 187, "estimated_seconds": 150, "has_ocr": true}
```

---

## PART B: Frontend Changes

All changes in `frontend/src/app/cases/[id]/page.tsx` unless noted.

### B1. Document metadata in sidebar

**Currently:** Each document shows just filename + type badge.

**After:** Each document shows filename, page count, file size, OCR badge (if applicable). Below the document list, show an aggregate stats card:

```
┌──────────────────────────┐
│ 3 docs, 187 pages        │  ← aggregate
│ 14.2 MB                  │
│ ▓▓▓▓▓▓▓▓▓▓░░░░░  65%    │  ← progress bar during analysis
│ ~3 min estimated          │
└──────────────────────────┘
```

Also show an amber warning banner when a document likely needs OCR:
```
⚠ 128p chargesheet may need OCR (~2 min)
```

Heuristic: if `file_type === "pdf"` and `page_count > 50` and `needs_ocr` is true (from the new DB column), show the warning.

**Update the `CaseDocument` type** in `frontend/src/app/components/shared/types.ts` to include:
```typescript
export interface CaseDocument {
    document_id: string;
    document_type: string | null;
    attached_at: string | null;
    filename?: string;
    file_type?: string;
    size_bytes?: number;
    page_count?: number;
    needs_ocr?: boolean;
}
```

### B2. Stats bar above tab content

During analysis, show a horizontal stats bar above the tab bar (or directly below it):

```
┌─ 187 pages ─┬─ 14.2 MB ─┬─ 1:42 elapsed ─┬─ ~2 min remaining ─┐
```

Four metric cards in a row. Use the app's metric card pattern: `background: var(--color-background-secondary)`, no border.

- **Pages:** total from all attached documents
- **Size:** sum of `size_bytes`, formatted as KB/MB
- **Elapsed:** live-ticking timer (update every 1 second via `setInterval`), format `M:SS`
- **Remaining:** computed from the `estimate` SSE event minus elapsed. When elapsed exceeds estimate, show "Almost done…" instead of a negative number.

Hide this bar when analysis is not running.

### B3. Heartbeat band (phase progress dots)

Below the stats bar, add a slim strip showing phase progress:

```
● ● ◉ ○ ○  |  3 insights  |  5 of 7 agents  |  ⏸
```

- **Phase dots:** 5 phases: Extract → Summarize → Analyze → Validate → Report
  - Green filled = completed
  - Amber pulsing = in progress
  - Empty = pending
- **Insight count:** number of findings received so far
- **Agent count:** "N of 7 agents" completed
- **Pause/halt button:** allows user to cancel (sends abort signal)

Map SSE events to phases:
- `extracting_doc` events → Extract phase
- `agent_status: running` for `case_summary` → Summarize phase
- `agent_status: running` for other agents → Analyze phase
- All 7 agents done → Validate phase (brief pause for cross-referencing)
- Final findings emitted → Report phase

### B4. Two-column layout during analysis: Checklist + Live Feed

This is the core change. When `analysisRunning` is true, the Findings tab splits into two columns:

**Left column (200px): Progress Checklist** (inspired by Lavern's `ProgressSidebar.tsx`)

A vertical checklist showing every step with real-time sub-items:

```
✓ Extract text
    ✓ FIR_Copy.pdf — 47 pages
    ✓ Chargesheet.pdf — OCR, 128 pages
    ✓ Bail_Order.docx — 12 pages

✓ Case summary
    ✓ Identified parties + court
    ✓ Built timeline (8 events)

◉ Deep analysis                    ← current phase
    ✓ Strengths & weaknesses
    ✓ Evidence gaps
    ⟳ Opposition predictor         ← spinner
    ⟳ Strategy recommender         ← spinner
    ○ Precedent finder
    ○ Risk assessor

○ Cross-validate
○ Final report
```

Build this from SSE events:
- `extracting_doc` / `extracted_doc` → populate the Extract step
- `agent_status: running/done/error` → populate agent steps
- `finding` events → add sub-items like "Found 3 strengths, 2 weaknesses"

**Footer:** "~X min remaining" + reassurance text "Everything is working normally"

**Right column (flex: 1): Live Insight Feed** (inspired by Lavern's `InsightFeed.tsx`)

A scrolling feed that shows events as chat-style cards. This is where the "interactive af" feeling comes from — the user watches agents work in real time.

**Card types to implement:**

1. **ActivityCard** (lightweight): Agent started/stopped. Show colored avatar circle with agent initials + serif message.
   - Start: "Analyzing document structure and extracting key facts…"
   - Done: "✓ Done — found 3 strengths, 2 weaknesses (4.2s)"

2. **FindingCard** (rich, bordered): When a finding SSE event arrives, render it immediately in the feed with:
   - Agent avatar (colored circle)
   - Severity badge (Strength=green, Weakness=red, Gap=amber, Risk=orange)
   - The finding text (serif font)
   - Evidence quote with left-border styling (if `exact_quote` present in grounding)
   - Confidence bar (if the finding JSON includes a confidence score)

3. **PhaseTransitionCard** (divider): "Text extraction complete — beginning analysis" styled as centered italic text between horizontal rules.

4. **ThinkingBubble** (ephemeral, at feed bottom): For agents with `status: "running"`, show avatar + bouncing dots + description text. Use the cycling `getRandomSnippet()` pattern here. These disappear when the agent completes.

5. **ReassuranceCard** (centered, serif, italic): When the feed has been quiet for 20+ seconds with no findings, inject a reassurance message. Use phase-specific messages:
   - During extraction: "OCR on scanned documents takes a moment — Mike is reading carefully"
   - During analysis: "Complex analysis requires deep thinking — this is expected"  
   - During long waits: "Your case is safe. Mike is thorough, not stuck."
   - Generic: "Adjournment? Mike just heard his favorite word." (pull from thinkingSnippets)

Implement a `useReassuranceInjector` hook (similar to Lavern's):
- Track the last time a "high-value" event (finding, agent_done, phase_transition) was received
- Every 5 seconds, check if 20+ seconds have elapsed since the last high-value event
- If so, inject a reassurance message into the feed
- Cap at 20 reassurance messages total

**Animation:** Each card enters with a slide-from-left animation:
```css
@keyframes slideIn { 
    from { opacity: 0; transform: translateX(-12px); } 
    to { opacity: 1; transform: translateX(0); } 
}
```
Duration: 0.3s ease-out.

**Auto-scroll:** The feed should auto-scroll to the bottom when new cards appear, unless the user has manually scrolled up (use an `isAtBottom` ref to detect).

### B5. When analysis completes

When the `done` SSE event arrives:
1. The two-column layout collapses back to the normal single-column findings view
2. The stats bar shows final stats: "Completed in 2:34 — 187 pages, 7 agents, 12 findings"
3. The checklist disappears (or collapses into a "Completed in 5 steps" collapsible, matching `PreResponseWrapper`)
4. Findings render in the existing structured card format (the `renderFindingContent` per-agent renderers already built)

### B6. OCR timeout warning

If elapsed time exceeds the estimate by more than 30 seconds, show a gentle amber banner at the top of the feed:

```
⏱ Taking longer than expected — large documents with OCR can take up to 3 minutes. 
  Your analysis is still running.
```

If elapsed exceeds 150 seconds (2.5 minutes), show a more prominent rescue card (inspired by Lavern's `StuckStateRescue`):

```
┌────────────────────────────────────────────┐
│  ⏱ This is taking a while                  │
│                                            │
│  Your documents are large (187 pages) and  │
│  include scanned PDFs requiring OCR.       │
│                                            │
│  Your work is safe — agents sometimes      │
│  take longer on complex documents.         │
│                                            │
│  [Keep waiting]  [Stop and try again]      │
└────────────────────────────────────────────┘
```

The "Stop and try again" button should abort the SSE stream (signal the AbortController) and reset `analysisRunning`.

### B7. Agent avatar colors

Assign consistent colors to each agent for avatars and accent borders:

```typescript
const AGENT_COLORS: Record<string, { bg: string; text: string; border: string }> = {
    case_summary:         { bg: "#EEEDFE", text: "#3C3489", border: "#534AB7" },  // purple
    strengths_weaknesses: { bg: "#EAF3DE", text: "#27500A", border: "#3B6D11" },  // green
    evidence_gap:         { bg: "#FAEEDA", text: "#633806", border: "#854F0B" },  // amber
    opposition_predictor: { bg: "#E6F1FB", text: "#0C447C", border: "#185FA5" },  // blue
    strategy_recommender: { bg: "#FBEAF0", text: "#72243E", border: "#993556" },  // pink
    precedent_finder:     { bg: "#E1F5EE", text: "#085041", border: "#0F6E56" },  // teal
    risk_assessor:        { bg: "#FAECE7", text: "#712B13", border: "#993C1D" },  // coral
};
```

Agent initials for avatars: CS, SW, EG, OP, SR, PF, RA.

---

## PART C: New SSE Events Summary

The backend should emit these events in order:

```
{"type": "estimate", "total_pages": 187, "estimated_seconds": 150, "has_ocr": true}

// Per document:
{"type": "extracting_doc", "filename": "FIR_Copy.pdf", "doc_index": 0, "total_docs": 3}
{"type": "extracted_doc", "filename": "FIR_Copy.pdf", "doc_index": 0, "total_docs": 3, "page_count": 47, "needed_ocr": false}

// Per agent (7 agents in parallel):
{"type": "agent_status", "agent_name": "case_summary", "status": "running"}
{"type": "agent_thinking", "agent_name": "case_summary", "snippet": "Identifying parties from FIR..."}
{"type": "agent_status", "agent_name": "case_summary", "status": "done"}
{"type": "finding", "finding": { ... }}

// On error:
{"type": "agent_status", "agent_name": "...", "status": "error", "error": "..."}

// Completion:
{"type": "done"}
data: [DONE]
```

---

## Files to Read Before Starting

### Mike codebase:
1. `src/routes/cases.rs` — `run_case_analysis`, SSE event emission (lines ~496-617)
2. `src/agents/case_prep/orchestrator.rs` — `analyze_case`, `run_agent`, `call_llm` (full file)
3. `src/sync/scanner.rs` — `extract_text_dispatch`, page counting, OCR detection
4. `src/routes/documents.rs` — upload handler, text extraction at upload time
5. `src/storage/mod.rs` — `make_storage()` for understanding the storage path
6. `frontend/src/app/cases/[id]/page.tsx` — the full file, especially `handleRunAnalysis` (~line 399), `FindingsTab` (~line 1026)
7. `frontend/src/app/components/shared/types.ts` — `CaseDocument`, `CaseFinding`, `AnalysisProgress`
8. `frontend/src/app/data/thinkingSnippets.ts` — `getRandomSnippet()`
9. `frontend/src/app/components/assistant/AssistantMessage.tsx` — `ReasoningBlock` pattern (lines 399-465)
10. `frontend/src/app/components/shared/PreResponseWrapper.tsx` — collapsible wrapper pattern
11. `migrations/0031_cases.sql` — current case tables schema
12. `migrations/0001_initial.sql` and `0014_*.sql` — documents table schema

### Lavern codebase (for inspiration only — do NOT copy code):
1. `/Users/vedantmishra/Downloads/lavern-main/viz/src/working/WorkingView.tsx` — overall layout
2. `/Users/vedantmishra/Downloads/lavern-main/viz/src/working/components/ProgressSidebar.tsx` — checklist pattern
3. `/Users/vedantmishra/Downloads/lavern-main/viz/src/working/components/InsightFeed.tsx` — live feed architecture
4. `/Users/vedantmishra/Downloads/lavern-main/viz/src/working/components/ReassuranceCard.tsx` — reassurance messages
5. `/Users/vedantmishra/Downloads/lavern-main/viz/src/working/hooks/useReassuranceInjector.ts` — silence detection + injection
6. `/Users/vedantmishra/Downloads/lavern-main/viz/src/working/components/StuckStateRescue.tsx` — stuck state rescue
7. `/Users/vedantmishra/Downloads/lavern-main/viz/src/working/data/phase-descriptions.ts` — phase metadata

---

## Constraints

- **CLAUDE.md rules apply.** Surgical changes, match existing style, no unrelated refactors.
- **Do NOT copy Lavern code.** Study the patterns, implement them in Mike's style (serif fonts, gray palette, MikeIcon, thinkingSnippets).
- **The existing 7-agent orchestrator structure stays.** Don't add new agents or change the analysis pipeline. We're adding visibility, not changing the engine.
- **The per-agent LLM timeout stays at 180 seconds.** Don't change it.
- **Backward compatibility.** If `page_count` is NULL (old documents), don't show page counts — degrade gracefully.
- **Match Mike's design language.** Gray palette, serif for content, sans for UI, no gradients, no shadows. Use MikeIcon, thinkingSnippets, PreResponseWrapper patterns.
- **The two-column layout is ONLY during analysis.** When analysis is not running, the Findings tab renders normally as a single column of finding cards (the current behavior).
- **Keep the existing structured finding renderers** (`renderFindingContent` for case_summary, strengths_weaknesses, evidence_gap, risk_assessor). They are correct.
- **New files are OK** for new hooks (e.g., `useReassuranceInjector.ts`) and new components if they stay in the cases directory.
- **Run `cargo check` after Rust changes** and `npm run build` after frontend changes.

---

## Priority Order

If this is too large for one pass, implement in this order:

1. **Backend: richer SSE events** (A3) — this unblocks everything else
2. **Frontend: live feed + checklist** (B4) — the core interactive experience
3. **Frontend: stats bar + heartbeat** (B2, B3) — time awareness
4. **Backend: page_count migration + metadata** (A1, A2) — document awareness
5. **Frontend: document stats in sidebar** (B1) — visual polish
6. **Frontend: reassurance system** (B4 reassurance cards) — prevents "is it stuck?"
7. **Frontend: stuck state rescue** (B6) — graceful timeout handling
8. **Frontend: completion transition** (B5) — smooth end state
