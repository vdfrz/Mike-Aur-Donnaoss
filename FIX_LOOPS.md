# Fix Loops — from the full-repo review (REVIEW_FINDINGS.md)

Each loop below is a **self-driving session** (obey `LOOP_PLAN.md` §0.5: don't pause to ask,
self-correct ≤3 rounds per check, commit on success, never commit red). Paste one loop per fresh
chat and fan out. Full detail + line-level fixes for every finding live in `REVIEW_FINDINGS.md`.

## Parallel-safety map
Disjoint file sets → run simultaneously: **A, B, D, E, F, G, H, J**.
Serialize with the RAG / case-prep streams (they edit these files): **C (chat.rs), I (case_prep)**.

| Loop | Area | Files | Skills / agents | Parallel? |
|---|---|---|---|---|
| A | PII (CRITICAL + privacy) | `src/pii/*` | `/ecc:rust-test`, `/ecc:rust-review`, `backend-builder` | ✅ |
| B | LLM utils: UTF-8 + truncation + stop_reason | `src/llm/builtin_tools.rs`, `kanoon_tool.rs`, `claude.rs`, `summarize.rs` | `/ecc:rust-test`, `/human-logic`, `claude-api`, `/ecc:rust-review` | ✅ |
| C | chat.rs streaming robustness | `src/routes/chat.rs` | `ecc:silent-failure-hunter`, `/ecc:rust-test`, `/ecc:rust-review` | ⚠️ serialize w/ RAG |
| D | Security: SSRF / IDOR / auth hang | `src/routes/user.rs`, `desktop.rs`, `auth.rs` | `/ecc:security-review`, `ecc:security-reviewer`, `/human-logic` | ✅ |
| E | Project-file crypto + integrity | `src/mikeprj/crypto.rs`, `io.rs` | `/ecc:security-review`, `/ecc:rust-test` | ✅ |
| F | Doc deliverable integrity | `src/routes/messy_doc.rs`, `documents.rs` | `/human-logic`, `/ecc:rust-test` | ✅ |
| G | PDF: OCR detection + docx runs | `src/pdf/mod.rs`, `docx_writer.rs` | `/ecc:rust-test`, `/human-logic` | ✅ |
| H | Sync scanner: blocking + partial-failure | `src/sync/scanner.rs` | `/ecc:rust-build`, `/human-logic` | ✅ |
| I | case-prep over-budget context | `src/agents/case_prep/orchestrator.rs` | `ecc:silent-failure-hunter`, `/human-logic` | ⚠️ serialize w/ case-prep |
| J | telegram-bot concurrency | `telegram-bot/src/main.rs` | `/human-logic`, `/ecc:rust-test` | ✅ |
| Z | MEDIUM cleanup (56) | mostly `src/routes`, `src/llm` | `/simplify`, `/ecc:rust-review` | ✅ low-priority |

---

## Loop A — PII byte-slice panics + silent privacy downgrade  ⭐ CRITICAL
Fix in `src/pii/`. Run `/ecc:rust-test` (write the failing tests FIRST), then implement, then `/ecc:rust-review`.
- **CRITICAL** `anonymizer.rs:413-423` — `anonymize()` byte-slices on GLiNER offsets that are Python *code-point* indices → panic or wrong redaction. Map codepoint offsets → byte offsets (`char_indices`) before slicing.
- **CRITICAL** `scrubber.rs:147` — `&text[..4000]` on a non-char boundary → panic on multibyte. Use a char-boundary-safe truncate.
- **HIGH** `anonymizer.rs:316-322` — person span end = `m.start() + name.len()` covers wrong bytes when `clean_name` dropped a leading word.
- **HIGH** `anonymizer.rs:176-182` — GLiNER parse failure silently becomes "no names/orgs/addresses found" (PII leak). Make parse failure a hard error / surfaced warning, not an empty result.
- **HIGH** `anonymizer.rs:478-483` — `anonymize_messages` replaces by unbounded substring regex → over-redaction. Anchor to detected spans.
- **CHECKS:** new tests with Devanagari + `₹` + smart-quote inputs don't panic and redact correctly; a malformed-GLiNER test surfaces an error not an empty set; `cargo test` green; `cargo clippy -D warnings` clean.

## Loop B — LLM utils: UTF-8 panics, silent truncation, stop_reason
Fix in `src/llm/`. `/ecc:rust-test` first; consult `claude-api` for the model-contract item; finish with `/ecc:rust-review`.
- **HIGH** `builtin_tools.rs:499-501` — `find_in_document` snippet slice panics on multibyte. Char-boundary-safe slice.
- **HIGH** `kanoon_tool.rs:525` — `String::truncate(FRAGMENT_CHAR_CAP)` panics mid-char. Use `char_indices`/`floor_char_boundary`.
- **HIGH** `claude.rs:83-114` — stream never surfaces `stop_reason=="max_tokens"`; truncated answers look complete. Detect length-stop and surface/recover (raise tokens or mark truncated).
- **HIGH** `summarize.rs:152-216` — history summary silently truncated at 512 tokens (drops the tail). And `:43-51,107-112` — `should_summarize` ignores system prompt / RAG / attached docs (the real overflow source). Count the full payload; surface "truncated N of M".
- **CHECKS:** multibyte inputs don't panic; a `max_tokens` response is flagged not silently accepted; summarizer measures full context; `cargo test` + `clippy -D warnings` green.

## Loop C — chat.rs streaming robustness  ⚠️ serialize with the RAG loops (they edit chat.rs)
Fix in `src/routes/chat.rs`. Use `ecc:silent-failure-hunter` to confirm the dropped-error path, then `/ecc:rust-test` + `/ecc:rust-review`.
- **HIGH (was CRITICAL)** `chat.rs:5430-5431` — `&text[..3000]` on extracted DOCX text panics on non-ASCII inside the spawned stream task → SSE dies with no terminal event. Char-boundary-safe truncate (the file already uses `is_char_boundary` at ~4152 — reuse that).
- **HIGH** `chat.rs:5108,5153,5157-5163,5819-5820` — mid-stream provider `Err` is captured in `got_err`, only logged, never sent as an `error` event; `got_done = !errored` then records the partial generation as a *complete* success and persists it. On `Err`, emit `{type:"error"}` over the SSE and set `errored=true`.
- **CHECKS:** multibyte doc text streams without panic; a simulated mid-stream error reaches the client as an error event and is NOT persisted as complete; `cargo check` green.

## Loop D — Security: SSRF, IDOR, auth hang
Fix in `src/routes/`. Drive with `/ecc:security-review` (+ `ecc:security-reviewer` agent); use `/human-logic` for the destructive-action item.
- **HIGH** `user.rs:525-623` — `probe_mcp_server` is an SSRF primitive (server fetches an arbitrary user-supplied URL). Allowlist schemes/hosts, block private/loopback ranges, add a timeout.
- **HIGH** `desktop.rs:32-34` — `open_in_word` resolves *any* user's document by id (IDOR). Scope the lookup to the authenticated `user_id`.
- **HIGH** `auth.rs:341-355` — biometric verify via Tauri channel can hang forever. Add a timeout on the reply path with a clear failure.
- **HIGH** `user.rs:966-977` — `delete_account` is irreversible with no confirmation/snapshot and leaves a stale MCP cache. Require an explicit confirm token; clear the cache.
- **CHECKS:** SSRF test rejects internal/loopback URLs; IDOR test denies cross-user doc access; biometric timeout returns an error; `cargo test` green.

## Loop E — Project-file crypto + integrity
Fix in `src/mikeprj/`. `/ecc:security-review` + `/ecc:rust-test` (round-trip + tamper tests).
- **HIGH** `crypto.rs:104-106,140-144,151-153` — AES-GCM header isn't authenticated (no AAD) → flag-downgrade bypass. Bind the header as AAD.
- **HIGH** `io.rs:290-329,361-366` — stored sha256 is never verified on import (integrity field is dead on read). Verify on import; reject mismatch.
- **HIGH** `io.rs:87-104` — a storage read failure silently yields a 0-byte document hashed as empty. Propagate the read error.
- **CHECKS:** tamper test (flipped header / corrupted bytes) fails to decrypt/import; missing-blob test errors instead of producing an empty doc; `cargo test` green.

## Loop F — Document deliverable integrity
Fix in `src/routes/messy_doc.rs` + `documents.rs`. `/human-logic` + `/ecc:rust-test`.
- **HIGH** `messy_doc.rs:188-206` — cleaned legal document silently truncated to 512 tokens and handed over as the deliverable; and the LLM cleanup call has no idle/total timeout. Chunk/stream the full doc; add a timeout; surface truncation.
- **HIGH** `messy_doc.rs:30-32` — upload silently capped at axum's 2 MB default while the documents route allows ~50 GB. Reconcile to one constant; reject with the actual limit.
- **HIGH** `documents.rs:553-559` — `save-markdown` swallows the storage_path pointer-flip failure and serves stale bytes as "saved". Propagate the failure; don't report success.
- **CHECKS:** a >2 MB messy-doc upload is accepted (or rejected with the real number); a large doc round-trips without silent 512-token truncation; save failure surfaces an error; `cargo test` green.

## Loop G — PDF: OCR detection + docx multi-run replace
Fix in `src/pdf/`. `/ecc:rust-test` + `/human-logic`.
- **HIGH** `mod.rs:186-220` — `needs_ocr` hardcoded `false` on every page → scanned PDFs never detected. Implement real detection (no text layer → needs OCR) and surface an actionable empty-state.
- **HIGH** `docx_writer.rs:698-757` — `tolerant_replace_in_runs` corrupts multi-run matches yet reports success (byte-mapping computed then discarded). Use the byte-mapping or fail loudly on multi-run.
- **CHECKS:** an image-only PDF reports needs_ocr=true; a multi-run replace either succeeds correctly or returns an error (never silent corruption); `cargo test` green.

## Loop H — Sync scanner: blocking + partial-failure visibility
Fix in `src/sync/scanner.rs`. `/ecc:rust-build` (blocking-in-async) + `/human-logic`.
- **HIGH** `scanner.rs:195-225` — blocking `std::fs::metadata`/`std::fs::read` inside the async scan task. Use `tokio::fs` or `spawn_blocking`.
- **HIGH** `scanner.rs:160-173` — scan reports `Done` even when some files failed; no partial-failure signal. Carry per-file failures into the terminal status.
- **CHECKS:** no blocking std::fs on the async path; a forced per-file error shows up in the terminal status; `cargo test` green.

## Loop I — case-prep over-budget context  ⚠️ serialize with the case-prep stream
Fix in `src/agents/case_prep/orchestrator.rs`. `ecc:silent-failure-hunter` + `/human-logic`.
- **HIGH** `orchestrator.rs:173-178` — compression failure silently sends the over-budget context to all 7 agents (cost blow-up / truncation downstream). On compression failure, surface it and either retry or fail the run — don't fan out the over-budget payload.
- **CHECKS:** a forced compression failure does NOT silently dispatch over-budget context; `cargo test` green.

## Loop J — telegram-bot concurrency + the two MEDIUMs
Fix in `telegram-bot/src/main.rs`. `/human-logic` + `/ecc:rust-test`.
- **HIGH** `main.rs:299-326` — `distribution_function(|_| None)` runs same-chat messages concurrently; two quick messages interleave the shared history Vec. Add a per-chat in-flight guard (like `pending`) covering the whole `call_chat`; reject/queue a 2nd message with "⏳ still working…".
- **MEDIUM** the idle-timeout message says "404" (misleading — it's a stall, not an HTTP 404). **DECISION NEEDED:** the human asked for "say 404"; keep as-is or switch to a plain "backend stalled" message.
- **MEDIUM** `main.rs` Answered path — if the client-tool-result POST 404s, the tapped answer is discarded with no user-facing note. Surface "⚠️ your answer arrived too late — proceeding".
- **CHECKS:** concurrent-message test doesn't interleave history; `cargo test` + `clippy -D warnings` + `fmt --check` green.

## Loop Z — MEDIUM cleanup (56 findings)
Low priority. `/simplify` + `/ecc:rust-review` over the MEDIUM section of `REVIEW_FINDINGS.md`, area by area (most are in `src/routes` (25) and `src/llm` (13)). Batch by file; commit per area.
