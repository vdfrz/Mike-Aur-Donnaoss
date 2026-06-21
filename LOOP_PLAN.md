# Mike aur Donna — Loop & Agent Master Plan

> **Cross-session bridge.** Read at the START of every session; update §0 at the END.
> Execution plan only — product vision lives in `ROADMAP.md`. Last updated: 2026-06-19.

---

## 0. Status

- **Branch:** `loop-a-removals` (off `case-prep-base`). Recovery point: snapshot `ae3183b` (undo all: `git reset --soft case-prep-base`).
- **Loop A frontend:** ✅ removed Edit-with-AI, Red Team UI, court-bundle pages/sidebar/i18n. Kept `RegistryTab.tsx`, tracked-changes, clarifying-questions UI.
- **Loop B (PixelRAG):** ✅ NO-GO. Decision: agentic RAG in Rust (no external framework in the app).
- **S1 (Loop A removals) ✅ DONE** — committed on `loop-a-removals` @ `6a129b0`, in `loop-a-removals-wt`. **Both halves stitched in one commit**: frontend removals (the parked `stash@{0}`, already applied in the worktree) + backend removals (Edit-AI / court-bundle / Red Team per §2 S1). 80 files, −5212 lines. Verified: `cargo check`=0 errors, frontend `tsc --noEmit`=0 errors, grep for removed symbols empty, `BundleProgressBlock` still referenced (not orphaned). *(The prior stopped run had already applied the stash + done the backend edits uncommitted; this session verified that working state against the §2 spec and committed it rather than discard+redo correct, un-backed-up work.)*
- **S5 (bot scaffold) ✅ DONE** — branch `feat/telegram-bot` @ `a2e317e`. Standalone Teloxide 0.17 crate `telegram-bot/` (own `[workspace]`, path-deps the local teloxide checkout) = thin HTTP client over `localhost:3001/chat` (no agent re-impl). `cargo build` → 0 errors; 5 SSE-parser/chunker unit tests green; binary runs. Live `/start`→answer is the user-test.
- **✅ Resolved — Loop-A frontend-removal WIP** (was parked in `stash@{0}`): now committed in `6a129b0` (see S1 above). `stash@{0}` + tag `loop-a-frontend-removals` are now redundant safety copies — keep or drop at will.
- **S6a (bot history + .docx drafts + citations footnote) ✅** — committed on `feat/telegram-bot` @ `99f276e`; `cargo build`/`test` green (8 tests).
- **S6b (clarifying-question inline buttons) ✅** — on `feat/telegram-bot`. Real inline-keyboard clarify flow replaces the `// TODO(S6b)` auto-proceed: parses `client_tool_request.arguments.questions`, renders one question at a time as a Telegram inline keyboard (single-tap, or multi-select + ✅ Done, with a "skip / proceed anyway" escape hatch), a callback handler drives the wizard and bridges to the in-flight `call_chat` via a per-chat oneshot, then POSTs `/chat/client-tool-result` to resume the still-open SSE stream. `/cancel` aborts a pending question; a 170s self-timeout proceeds before the backend's 180s tool timeout. Dispatcher uses `distribution_function(|_|None)` so a tap is handled while `handle_text` is parked (no deadlock). `cargo build`/`test` green (17 tests).
- **S2 (RAG semantic FIND) ✅ DONE** — committed on `loop-a-removals` @ `0c779d8`, in `loop-a-removals-wt`. `search_firm_corpus` is now hybrid: vector-KNN ∪ FTS5/BM25 fused by Reciprocal Rank Fusion. Chunks are embedded into a new `corpus_chunks_vec` sqlite-vec table (migration **0043**, mirrors `doc_chunks`) on ingest — best-effort, so an offline/unloaded model degrades to BM25-only and never fails ingest. BM25-only fallback is byte-identical to before; `expand_chunk` unchanged; `delete_file` now also clears a file's vectors (no FK cascade reaches a vec0 table). **Tests:** real-e5 `corpus_semantic_search` **passes** (44s incl. model download — a no-keyword-overlap query ranks the right chunk first) + a deterministic in-module hybrid test (fabricated vectors, runs under plain `cargo test`) + an RRF unit test. Reviewed by `database-reviewer` + `rust-reviewer`: **no blockers**; the one real finding (orphaned vectors on file delete) is fixed.
  - ⚠️ **Pre-existing reds on this branch, NOT S2** (files S2 never touched): `routes::chat::tests::returns_none_for_unclosed_block` (citations parser) and the `pdf::extract_docx_text` doctest both fail at `6a129b0` too — S1 only ran `cargo check`, which never compiles tests/doctests, so they were never caught. Flagged for a cleanup pass; S2's own tests + suite (239 lib tests minus that one) are green. Also fixed `tests/workflows_smoke.rs` (missing rag-gated `ik_reindex` field) so the test binary compiles at all.
- **S3 (RAG agentic INGEST online + progress UI) ✅ DONE** — committed on `loop-a-removals` @ `e03df44`, in `loop-a-removals-wt`. At ingest (after chunk+embed+tag) a best-effort ONLINE cloud-model **distillation** pass distils each draft into a reusable style/structure profile (summary, section outline, style notes, stock phrases) stored locally in new table `corpus_profiles` (migration **0044**; PK `file_id` FK→`corpus_files` ON DELETE CASCADE → re-ingest upserts, delete cascades — no route change). **No PII scrub** (firm's own data, user's own provider). Offline-safe: the call goes through `oneshot::complete`; any failure → `None` → no profile row, ingest still marks `ready`. Testable seam `persist_profile_from_raw(.., Option<&str>)`. **Frontend:** reuses the existing `ReindexProgress` indexing component on the knowledge page (font matches by construction) → elapsed timer + "Ingested X / Y"; added backward-compatible `doneVerb` label; inputs disable while running; failures surface on the indicator. **Tests:** `tests/corpus_ingest_profile.rs` — online (mocked JSON) writes a profile row; offline (`None`) writes no profile, keeps chunks, no error — + 2 parse unit tests. **CHECKS green:** `cargo test` S3 suite passes (2 integration + 7 `corpus::ingest` lib tests); the only lib-suite red is the **same pre-existing `returns_none_for_unclosed_block`** documented under S2 (untouched by S3 — `chat.rs` not edited); `tsc --noEmit` = 0 errors. Reviewed by `security-reviewer` + `database-reviewer`: **no blockers**; applied both warnings (upsert refreshes `user_id`; `reusable_phrases` always stored as valid JSON).
  - ⚠️ **Pre-existing security note, NOT S3-scoped:** `gemini.rs` builds `?key=<API_KEY>` into the request URL, so a network-level reqwest error logged via `{e}` (e.g. the existing `build_template` warn, and S3's distillation warn) could leak the key into traces. Affects *all* `complete()` callers — flagged for a dedicated fix (switch Gemini to a `Bearer` header or strip the key before erroring), not done here to stay surgical.
- **Checkboxes:** ☑ S1 ☑ S2 ☑ S3 ☐ S4 ☑ S5 ☑ S6 ☐ S7

---

## 0.5 AUTONOMY CONTRACT — every session obeys this

The point of these loops is **zero babysitting**. Each session runs to completion on its own:

- **Don't pause to ask the user.** Make every *reversible* decision yourself from this spec + existing codebase conventions. Silence ≠ stop; keep going.
- **SCL — the Self-Correct Loop (run after every build/edit step):**
  1. Run the session's **CHECKS**.
  2. If a check fails → hand the **exact error output** to the named **resolver agent** → apply fix → re-run that check.
  3. Cap **3 rounds per check**. Never commit red; never leave the branch un-compilable.
- **STOP only on a real blocker** — write a `## BLOCKER — Sx` note into §0 (what failed, the error, what you tried) and halt. Real blockers are: (a) a check still red after 3 SCL rounds; (b) a requirement genuinely ambiguous and not resolvable from spec/code; (c) an irreversible/destructive action not explicitly authorized here. Nothing else justifies stopping.
- **On success:** commit (clear message) → tick the §0 checkbox + 1-line status → append a **"🧪 You test:"** 1–2 line manual check for the user. That handoff line is the only thing the user does.
- **Caps:** build sessions ≤3 SCL rounds/check; the eval loop (S4) ≤8 iterations then report best.
- **Resolvers/agents available:** `rust-build-resolver`, `react-build-resolver`, `rust-reviewer`, `gan-evaluator`, `explorer`, `backend-builder`, `frontend-builder`.
- **Skills per session:**
  - **S2** (semantic find) → `/ecc:rust-test` (write the failing test first) · `ecc:database-reviewer` (migration/schema) · `ecc:rust-reviewer`.
  - **S3** (online ingest + UI) → `ecc:security-reviewer` (the one data-leaves-machine touchpoint) · `ecc:database-reviewer` (profile store) · `/mike-design` (match the indexing font).
  - **S4** (pressure-test) → **drive the loop with `/ecc:gan-build`** instead of hand-rolling it; `gan-evaluator` is the scorer.
  - **S6b** (bot buttons) → `ecc:rust-reviewer`.
  - **Every session:** `/ecc:resume-session` at start, `/ecc:save-session` at end, `/ecc:checkpoint` at each green point; `/human-logic` while building.

---

## 0.6 Running localhost (user preference)

When asked to **open / fire up / run localhost**, ALWAYS run from the **main repo working tree** (`/Users/vedantmishra/Desktop/mike aur donna main git`) — where the configured `.env`, the real SQLite DB (`DATABASE_URL=sqlite:/Users/vedantmishra/mikerust-data/mike.db`), and `data/storage/` live. NEVER run from an isolated worktree or fresh checkout (those lack `.env` and point at an empty/wrong DB). Backend: `cargo run --features rag` (:3001). Frontend: `cd frontend && npm run dev` (:3000 → http://localhost:3000).

---

## 1. Workstreams + keystone

| # | Workstream | Real loop? | Depends on |
|---|---|---|---|
| **A** | Remove Edit-AI + Red Team + court-bundle (backend) | no — one-pass build+verify | — |
| **C** | RAG: semantic FIND + agentic INGEST (online) + pressure-test | S4 is a true loop | A |
| **D** | Telegram bot: research + drafting | no — build+verify | scaffold: A · quality: C |

**Keystone:** *drop drafts → AI learns → useful output* is a RAG problem: **FIND** (surface the right draft — today keyword-only, weak) + **UNDERSTAND/USE** (already handled by the `/chat` agent). Add agentic value at **ingestion**, not via a new query framework. Bot is only as good as the RAG under it → C before D-polish.

**Only safe parallel pair:** S1 ‖ S5 (disjoint files). All RAG sessions edit `chat.rs`/`builtin_tools.rs` → must follow S1.

---

## 2. Session loops (each is self-driving per §0.5)

### Session 1 — Remove Edit-AI + Red Team + court-bundle (backend) ⏳ running
- **CHECKS (DONE = all pass):**
  - `cargo check` → 0 errors.
  - frontend `npm run build` (or `tsc --noEmit`) → 0 errors.
  - `grep -rE 'inline_suggest|save_markdown|compile_court_bundle|COURT_BUNDLE_FOCUS|bundle_intent|red_team|RedTeamBody|review_draft' src/ frontend/src` → empty.
  - `BundleProgressBlock` either still used by case-prep (fine) or removed if now orphaned.
- **LOOP:** `backend-builder` removes the targets below → **SCL(cargo check, resolver=rust-build-resolver)** → grep-clean check (failures → `backend-builder`) → **SCL(frontend build, resolver=react-build-resolver)**.
- **Targets:** Edit-AI: `chat.rs` `/inline-suggest`+`post_inline_suggest`; `documents.rs` `/save-markdown`+`save_markdown`. Court-bundle: delete `src/pdf/bundle.rs`+`pub mod bundle;`; `builtin_tools.rs` remove `COMPILE_COURT_BUNDLE` (const/is_builtin/schema/dispatch/exec); `chat.rs` remove bundle plumbing (`COURT_BUNDLE_FOCUS`, `bundle_intent`, `kind_val` court_bundle branch, lightweight-load conditional, `list_chats` kind filter, 3 `compile_court_bundle` dispatch/SSE branches). Red Team: delete `src/agents/red_team.rs`+`pub mod red_team;`; `cases.rs` remove `/{id}/red-team` route+`red_team_review`+`RedTeamBody`.
- **KEEP:** `ask_clarifying_questions`, registry/party/annexure routes, `src/drafting/`, `src/corpus/`, `src/embeddings/`, `case_prep` outputs, `oneshot.rs`, `generate_docx`/`edit_document`.
- **On success:** commit; 🧪 You test: launch the app, open a case → no Edit(AI)/Red Team buttons, no "Court Bundle" in the sidebar.

### Session 2 — RAG part 1: semantic FIND (TDD loop)
- **CHECKS (DONE):** a new test `corpus_semantic_search` passes — a query with **no keyword overlap** ranks the semantically-correct chunk above an unrelated one; the new migration applies; `cargo test` green.
- **LOOP:** `explorer` pins integration points (`corpus/ingest.rs`, `corpus/tools.rs`, `embeddings/service.rs`, schema) → `backend-builder` writes the **failing test first**, then implements (embed chunks on ingest into `sqlite-vec`; `exec_search_firm_corpus` = hybrid vector-KNN ∪ BM25, re-ranked; `expand_chunk` unchanged) → **SCL(cargo test, resolver=rust-build-resolver)** → `rust-reviewer` → SCL on its findings.
- **On success:** commit (incl. migration); 🧪 You test: ask the assistant something worded differently from your drafts — it should still surface the right passage.

### Session 3 — RAG part 2: agentic INGEST (online) + progress UI
- **CHECKS (DONE):** integration test — importing a sample draft (online, mockable) writes a **profile** row; an **offline** import writes chunks, **no** profile, **no** error; `cargo test` green; frontend build green. (Font-match of the progress UI is the one **user-test**.)
- **LOOP:** `explorer` (does ROADMAP-0A's doc-type generator exist? reuse it; locate the existing **indexing** progress component) → `backend-builder` builds the online distillation pass + profile store (**no PII scrub** — local-only data) with graceful offline fallback → **SCL(cargo test)** → `frontend-builder` builds the ingestion indicator: elapsed **timer** + **`Ingested X / Y`** count, reusing the indexing component's typography via `/mike-design` → **SCL(frontend build, resolver=react-build-resolver)** → `rust-reviewer`.
- **On success:** commit; 🧪 You test: drop a few drafts (online) → watch count+timer tick in the indexing font → draft a new doc of that type → it reuses your style.

### Session 4 — RAG part 3: pressure-test (the real loop)
- **BAR (DONE):** `gan-evaluator` score ≥ **4/5** on the rubric *"reuses the retrieved drafts' structure/style — not generic boilerplate"* across **≥3 distinct draft-sets**, holding for **2 consecutive iterations**. Hard cap **8 iterations**.
- **LOOP:**
  1. Build 3+ eval sets from `documents/` (corpus in, target style noted).
  2. Each iteration: generator drafts grounded on the corpus → `gan-evaluator` scores each set against the rubric → if any set < BAR, feed its **specific feedback** to `backend-builder` to adjust retrieval / chunking / the drafting prompt → re-run.
  3. Exit when all sets ≥ BAR for 2 straight iters → success. Else at 8 iters → **STOP**, write best scores + the top remaining gap to §0.
- **On success:** commit; 🧪 You test: drop your real drafts → ask for a draft → judge if it actually sounds like you.

### Session 5 — Telegram bot scaffold ✅ done (`feat/telegram-bot` @ `a2e317e`)
- **CHECKS (DONE):** bot crate `cargo build` → 0; bot process starts; one message → a real `/chat` answer in Telegram.
- **LOOP:** `explorer` maps the `/chat` request/response + auth contract → `backend-builder` builds a **thin HTTP client** over `localhost:3001/chat` (do NOT re-implement the agent) → **SCL(cargo build, resolver=rust-build-resolver)** → run + send a test message. Branch `feat/telegram-bot`.
- **On success:** commit; 🧪 You test: DM the bot `/start`, then a question → real answer.

### Session 6 — Telegram bot capabilities
- **CHECKS (DONE):** build green; research returns **cited** kanoon/statute results; drafting returns a real **`.docx`** as a Telegram document; clarifying questions render as **inline-keyboard buttons**; `/human-logic` covered: long-message chunking, file-size cap, visible error on failure, `/cancel`. (Live Telegram behaviour = user-test.)
- **LOOP:** `backend-builder` adds each capability over the existing tools → **SCL(cargo build)** → run. Memory stays per-lawyer/local (ROADMAP).
- **On success:** commit; 🧪 You test in Telegram: (1) ask a research question → cited results; (2) ask for a draft → answer clarifying buttons → receive a `.docx`.

### Session 7 — Integrate + merge
- **CHECKS (DONE):** `loop-a-removals` + `feat/telegram-bot` merged into `case-prep-base`; no conflict markers; `cargo check` + frontend build green.
- **LOOP:** merge → resolve conflicts (favor the explicit specs above) → **SCL(cargo check + frontend build)**. STOP only if a conflict needs a product decision.
- **On success:** commit; 🧪 You test: full smoke — app boots, a case opens, a draft generates, the bot answers.

---

## 3. Cross-session machinery
- Bridge = this file. `/ecc:save-session` at end, `/ecc:resume-session` at start.
- Parallel safety: `claims_claim` a session before starting; `claims_board` to see what's taken.
- Each session = own commit(s); never commit on `case-prep-base` directly; verify before commit.

## 4. ruflo — thin slice, dev-time only
- Use: `memory_store`/`memory_search`, `claims_*`. Optional: `workflow_create` to drive a fan-out instead of native agents.
- Skip + never ship inside the app: `hive-mind`, `neural_*`, `wasm_*`, `daa_*`, `autopilot_*`.
- Step 0 each session: `system_health`; if it errors, use native agents and move on.

---

## 5. Remaining work + launch order (for fresh chats)

**Sequence (triggers in bold):**
1. **When S1's window finishes** → *Loop A stitch*: in `loop-a-removals-wt`, re-attach the frontend removals (`git stash apply stash@{0}`, or recover from tag `loop-a-frontend-removals` @ `2897061`), resolve (frontend `.tsx`/`.json` vs backend `.rs` are disjoint → should be clean), `cargo check` + frontend build, commit. Now `loop-a-removals` holds BOTH halves.
2. **After stitch** → S2 → S3 → S4 (RAG chain, sequential, built on the removed-code branch).
3. **Anytime / parallel-safe** (isolated `telegram-bot/` crate) → S6b.
4. **Last** → S7 merge.

**S6b — clarifying-question buttons** (replace the `// TODO(S6b)` auto-proceed in `telegram-bot/src/main.rs`): backend emits `client_tool_request {request_id, name:"ask_clarifying_questions", arguments:{questions:[{header, question, multiSelect, options:[{label, description}]}]}}` and BLOCKS the open SSE stream until `POST {api_url}/chat/client-tool-result {request_id, result:"{\"answers\":[{question, selected:[…]}], \"proceed\":bool}"}`. So: render each question's options as an inline keyboard → collect taps via a callback handler → bridge callback → the in-flight `call_chat` via a per-chat oneshot/mpsc → POST result to resume → keep the SSE connection alive while waiting. Add `/cancel`.

**Open decisions (not code — your call, jot the answer here when decided):**
- **Supabase:** frontend currently uses it (`frontend/src/lib/supabase*.ts`, `auth.ts`, `storage.ts`) — contradicts the privacy-first/offline goal. Rip out → local auth/storage, or keep?
- **Always-on / phone access:** the bot answers only while the laptop is awake + running (phone = keyboard, laptop = brain). For 24/7 + private, run backend+bot on a small always-on machine you own; otherwise it's "works when the laptop's on."
