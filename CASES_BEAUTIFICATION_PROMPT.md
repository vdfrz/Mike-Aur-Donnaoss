# Cases Section UI Beautification — Batch Prompt

**Scope:** `frontend/src/app/cases/[id]/page.tsx` — the single-case detail page. Changes should be surgical and limited to this file plus any small shared component imports needed.

**Guiding principle:** Make the cases page feel like it belongs in the same app as the main chat (`frontend/src/app/components/assistant/AssistantMessage.tsx`). Right now it feels like a different app — flat, utilitarian, missing the personality and polish of the main interface. Every change below should match existing patterns, not invent new ones.

---

## Design System Reference (DO NOT DEVIATE)

Read these files before writing any code. They are the source of truth:

| What | File |
|---|---|
| Thinking snippets data | `frontend/src/app/data/thinkingSnippets.ts` — exports `THINKING_SNIPPETS` array and `getRandomSnippet()` |
| ReasoningBlock / ThinkingPlaceholder / DraftingPlaceholder | `frontend/src/app/components/assistant/AssistantMessage.tsx` lines 348-465 |
| PreResponseWrapper (collapsible "Working / Completed in N steps") | `frontend/src/app/components/shared/PreResponseWrapper.tsx` |
| MikeIcon (animated SVG windmill, spin/done/error states) | `frontend/src/components/chat/mike-icon.tsx` — `export function MikeIcon({ spin, done, error, size, style })` |
| ToolbarTabs (shared tab bar) | `frontend/src/app/components/shared/ToolbarTabs.tsx` |
| Global styles / CSS variables | `frontend/src/app/globals.css` — oklch color tokens |
| Fonts | Layout uses Inter (`font-sans`) for UI chrome, EB Garamond (`font-serif`) for body/content text |

**Key design tokens used across the app:**
- Primary buttons: `bg-gray-900 text-white hover:bg-gray-800`
- Cards: `rounded-lg border border-gray-200 bg-white`
- Section labels: `text-xs font-medium text-gray-500 uppercase tracking-wide`
- Body text: `text-sm font-serif text-gray-900 leading-relaxed`
- Muted meta: `text-xs text-gray-400`
- Tiny spinner: `w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin shrink-0`
- Completed dot: `w-1.5 h-1.5 rounded-full bg-gray-300 shrink-0`
- Bouncing dots ("Working..."): three `w-0.5 h-0.5 rounded-full bg-gray-400` with staggered `animate-[bounce_1.4s_infinite_Xs]`
- Chevron toggle: `transition-transform duration-200` with `-rotate-90` when collapsed

---

## Changes Required

### 1. Thinking Snippets in Analysis Progress

**Current behavior (lines ~1085-1113):** During analysis, the progress card shows per-agent status dots and, if the agent is streaming thinking, it dumps raw `ap.thinking.slice(-300)` in `font-mono text-[10px] text-gray-400`. This looks debug-like and completely different from the main chat's polished thinking UI.

**Required behavior:** Replace the raw thinking dump with the same cycling-snippet pattern used in `ReasoningBlock` (AssistantMessage.tsx lines 399-465):

- Import `getRandomSnippet` from `@/app/data/thinkingSnippets`.
- For each agent with `status === "running"`, show the tiny CSS spinner (`w-1.5 h-1.5 rounded-full border border-gray-400 border-t-transparent animate-spin`) followed by a cycling snippet in `text-sm font-serif text-gray-500` (NOT font-mono, NOT text-[10px]).
- Cycle snippets every 2-3 seconds using `setInterval` + `useState`, exactly like `ReasoningBlock` does.
- When an agent completes (`status === "done"`), show the completed dot (`w-1.5 h-1.5 rounded-full bg-gray-300`) instead of the spinner, and replace the snippet with just the agent label.
- When an agent errors (`status === "error"`), show a red dot (`w-1.5 h-1.5 rounded-full bg-red-400`) and a short error message in `text-xs text-red-600`.
- Each agent's thinking row should be its own component (or use a key-based state) so snippets cycle independently per agent.

**Additionally:** Replace the generic `Loader2` spinner next to running agents with the **MikeIcon** component:
- Import `MikeIcon` from `@/components/chat/mike-icon`.
- Use `<MikeIcon spin size={14} />` for running agents instead of `<Loader2 className="h-3 w-3 animate-spin" />`.
- Use `<MikeIcon done size={14} />` for completed agents.
- Use `<MikeIcon error size={14} />` for errored agents.
- This replaces the current `StatusDot` component entirely for the analysis progress section.

**The overall progress card structure should change from:**
```
┌─ "Analysing your case…" ──────────────────┐
│ ● case_summary         [Loader2 spinning]  │
│   ap.thinking.slice(-300) in mono font...  │
│ ● evidence_gap         [Loader2 spinning]  │
│   ap.thinking.slice(-300) in mono font...  │
└────────────────────────────────────────────┘
```
**To:**
```
┌─ Working ···  (bouncing dots, serif font) ─┐ ← Use PreResponseWrapper pattern
│ [MikeIcon spin] "Searching for precedents  │ ← cycling snippet, serif
│                  in the Allahabad HC…"      │
│ [MikeIcon spin] "Mike is having an         │ ← different snippet per agent
│                  existential crisis about   │
│                  Section 498A."             │
│ [MikeIcon done] Case Summary               │ ← completed agent
│ [MikeIcon done] Strengths & Weaknesses     │ ← completed agent
└────────────────────────────────────────────┘
```

Use the `PreResponseWrapper` component or replicate its exact header pattern ("Working" + bouncing dots while running, "Completed in N agents" when done). The wrapper should be collapsible like `PreResponseWrapper`.

### 2. Analysis Button — MikeIcon Instead of Loader2

The "Run Analysis" button (around line 850-863) uses `<Loader2>` while analysis runs. Replace with `<MikeIcon spin size={14} />` so it matches the main chat's streaming indicator. When not running, no icon needed.

### 3. Tab Bar — Match App Style

The current tab bar (lines ~870-884) uses a hand-rolled `border-b-2` underline style. The app has a shared `ToolbarTabs` component at `frontend/src/app/components/shared/ToolbarTabs.tsx` that uses a different, subtler pattern: `text-xs` with `font-medium text-gray-700` active / `font-normal text-gray-500` inactive, no underline border. This is the pattern used elsewhere in the app.

**Option A (preferred):** Replace the hand-rolled tab bar with the `ToolbarTabs` component. Import it and pass the tabs array + active + onChange. Note that `ToolbarTabs` uses `h-10 px-8` and `text-xs` sizing — this is the correct app style.

**Option B (if ToolbarTabs doesn't fit the layout):** Keep the hand-rolled bar but match the ToolbarTabs styling: remove `border-b-2`, use `text-xs`, use `font-medium text-gray-700` / `font-normal text-gray-500 hover:text-gray-700` without underline.

### 4. Chat Tab — Markdown Rendering + Proper Bubbles

**Current behavior (ChatTab, lines ~1428-1509):** Messages render as plain text with `whitespace-pre-wrap`. No markdown, no code blocks, no formatted tables. The assistant message uses `bg-white border border-gray-200 rounded-lg` which looks like a card, not a message.

**Required changes:**

a) **Add markdown rendering for assistant messages.** Import `ReactMarkdown` from `react-markdown` and `remarkGfm` from `remark-gfm` (both already installed — check `AssistantMessage.tsx` for the exact import pattern). Wrap assistant message content in:
```tsx
<ReactMarkdown remarkPlugins={[remarkGfm]} className="prose prose-sm max-w-none font-serif">
  {msg.content}
</ReactMarkdown>
```

b) **Match user bubble style to main chat.** The main chat uses `bg-gray-100 rounded-xl px-4 py-3` for user messages (`UserMessage` component). The cases chat uses `rounded-lg`. Change to `rounded-xl` to match.

c) **Assistant messages should NOT have a border.** In the main chat, assistant messages have no wrapper card — they're just rendered markdown text. Remove the `bg-white border border-gray-200` from assistant messages and just render the content directly with a `text-gray-900 font-serif text-sm` wrapper.

d) **Input area:** The textarea placeholder says hardcoded English "Ask about this case…" — use the `t()` function with a translation key instead. Also change `focus:ring-1 focus:ring-ring` to `focus-visible:ring-[3px] focus-visible:border-ring focus-visible:ring-ring/50` to match the shared Button component's focus pattern.

### 5. Finding Cards — Tighten Spacing + Typography

The finding cards are structurally good but have minor inconsistencies with the app's type system:

a) **Agent name labels in card headers:** Currently `text-xs text-gray-700`. The app uses `text-xs font-medium text-gray-700` for similar headers. Add `font-medium`.

b) **Card toggle chevrons:** Currently uses `ChevronDown`/`ChevronRight` swap on click. The app's standard pattern (PreResponseWrapper, ReasoningBlock) uses a single `ChevronDown` with `transition-transform duration-200` and `-rotate-90` when collapsed. Change to this pattern.

c) **Citation pills at the bottom of findings:** Currently `rounded-full text-[10px] bg-blue-50 text-blue-700 px-2 py-0.5`. The main chat's doc citation pills use `bg-gray-100 text-gray-900 hover:bg-gray-200`. Match that palette for document citations. Keep the `rounded-full` shape.

d) **Finding body text:** Ensure all prose content in findings uses `font-serif` consistently. Some sections might be missing it.

### 6. Left Sidebar Polish

a) **Case title area:** The title editing interaction is fine but the back button `← Back` should be an icon-only `ArrowLeft` button to save space, matching sidebar patterns elsewhere in the app (like AppSidebar). Use `p-1 rounded hover:bg-gray-100 transition-colors` for the button, no text label.

b) **Document list:** Each document row currently shows the filename in `text-xs text-gray-700`. Add a small file-type badge: `text-[10px] uppercase tracking-wide text-gray-400` (e.g., "PDF", "DOCX") to the right of the filename, similar to how document types are shown in the main document list.

c) **"Add Documents" button:** Currently uses `<Plus>` icon with a text label. Make it match the pattern of other secondary actions in the app: `border border-gray-200 bg-white text-gray-700 hover:bg-gray-100 rounded-md text-xs` (if it doesn't already).

### 7. Empty States

a) **No findings empty state (lines ~1054-1065):** Currently plain text centered. Add a subtle illustration or icon — use `<Search className="h-8 w-8 text-gray-300 mb-3" />` above the text, matching the empty-state pattern used in the main doc list.

b) **No outputs empty state:** Same treatment.

c) **No chat messages:** Currently says hardcoded "Ask questions about this case. All attached documents are available as context." — use `t()` for i18n, and add a MikeIcon: `<MikeIcon size={28} />` above the text to give it personality.

### 8. Overall Layout Spacing

a) The main content area uses `px-6 py-6` padding. The sidebar uses `p-4`. This is fine, but the tab content area should have consistent max-width. Currently `max-w-3xl` is applied to FindingsTab but not to OverviewTab or OutputsTab. Apply `max-w-3xl` consistently to all tab content wrappers.

b) The left sidebar is `w-72` (288px). This is fine. But ensure the border between sidebar and content uses `border-r border-gray-200` (not `border-gray-100` which is lighter than the app standard).

### 9. Transitions and Micro-interactions

a) **Tab content switching:** Add a subtle fade transition when switching tabs. Use `transition-opacity duration-150` on the tab content wrapper, or a simple CSS `@keyframes fadeIn { from { opacity: 0 } to { opacity: 1 } }` animation.

b) **Finding card expand/collapse:** Add `transition-all duration-200` on the content area so it doesn't pop in/out abruptly. Even a simple opacity transition helps.

c) **Document row hover:** Currently the delete icon fades in on hover (`opacity-0 group-hover:opacity-100`). This is good and matches the app pattern. Keep it.

---

## Files to Read Before Starting

1. `frontend/src/app/cases/[id]/page.tsx` — THE file being modified
2. `frontend/src/app/components/assistant/AssistantMessage.tsx` — source of truth for thinking UI, ReasoningBlock, message styling
3. `frontend/src/app/components/shared/PreResponseWrapper.tsx` — collapsible "Working" wrapper
4. `frontend/src/app/data/thinkingSnippets.ts` — snippet data + getRandomSnippet
5. `frontend/src/components/chat/mike-icon.tsx` — MikeIcon component
6. `frontend/src/app/components/shared/ToolbarTabs.tsx` — shared tab component
7. `frontend/src/app/globals.css` — CSS variables and global styles
8. `frontend/src/app/components/shared/types.ts` — CaseFinding, AnalysisProgress types

## Constraints

- **Do NOT create new files** unless absolutely necessary. All changes should fit in `page.tsx` with existing imports.
- **Do NOT install new packages.** `react-markdown`, `remark-gfm`, `lucide-react`, and all other needed packages are already installed.
- **Do NOT change the Rust backend.** This is frontend-only.
- **Match existing style exactly.** Use the same Tailwind classes, same fonts (`font-serif` for content, `font-sans` for UI), same color tokens. Do not introduce new colors.
- **Keep the existing structured finding renderers** (`renderFindingContent` for case_summary, strengths_weaknesses, evidence_gap, risk_assessor). They are correct and should be preserved.
- **Preserve all existing functionality.** Analysis running, SSE streaming, doc panel, chat, output generation — none of this should break.
- **Keep all existing translation key usage (`t(...)`)** and add `t()` calls for any remaining hardcoded strings where translation keys exist.
- **CLAUDE.md rules apply:** Surgical changes only. No unrelated refactors. Match existing style. Every changed line traces to this spec.
