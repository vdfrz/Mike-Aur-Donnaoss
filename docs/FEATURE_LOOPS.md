# Feature Loops — Redline + Risk Rubric + Memory (build + pressure-test)

> **Paste ONE loop per fresh Claude Code session and fan out.** Each loop is a self-driving session
> (obeys LOOP_PLAN.md §0.5: don't pause to ask, self-correct ≤3 rounds per check, commit green, never
> commit red). Build loops run in a **worktree-per-loop**; pressure-test loops run from the **MAIN repo**.
> Last authored + adversarially verified: 2026-06-21.

## What we're building (3 parts; backend already has every primitive)
1. **Shared risk rubric** → inject `docs/LITIGATION_RISK_RUBRIC_PROMPT_BLOCK.md` into `MIKE_SYSTEM_PROMPT` (src/routes/chat.rs) so the **Tauri app AND the bot** both run rubric-driven risk-triage (both call `/chat`).
2. **Bot redlining** → send a `.docx` to the bot → rubric risk-review → confirm → `edit_document` real Word tracked changes → delivered.
3. **Bot memory** → `/remember` / NL trigger → the existing "Mike listens" harness learns it → auto-applied to every future turn.

## CORRECT run-config (the stale LOOP_PLAN.md §0.6 says :3001 / ~/mikerust-data — IGNORE it)
- **Run from the MAIN repo** `/Users/vedantmishra/Desktop/mike aur donna main git` (worktrees have an empty DB).
- **Backend:** `set -a; source .env; set +a; cargo run --features rag` → binds `127.0.0.1:1514` (PORT=1514 in .env). A prebuilt `target/debug/mike` exists to skip a rebuild.
- **DB:** `sqlite:src-tauri/mike.db` (the REAL history DB — baseline today: real account `88a19121` = 449 docs / 5 chats / 0 lessons; `local-user` = 0/0/0).
- **LLM:** DeepSeek is **CLOUD** (`https://api.deepseek.com/v1`, key in .env); chat model id `local:deepseek-v4-flash`. **NOT ollama.**
- **Bot:** from `telegram-bot/` — `set -a; source .env; set +a; ./target/debug/mike-telegram-bot`. `telegram-bot/.env` has the token + `MIKE_API_URL=http://localhost:1514`.

## ⚠️ MANDATORY bootstrap for EVERY pressure-test loop (read before running PT1–PT4)
The test loops drive the backend headless as the bypass user `local-user`. Two traps the adversarial pass found — apply ALL of these in every test loop:
1. **Isolated sandbox, not :1514.** The live `:1514` instance is auth-ON and `MIKE_BYPASS_AUTH` is read at process start, so exporting it in your shell does nothing to it. Boot your OWN instance on a **free port :1599** from the prebuilt binary with `MIKE_BYPASS_AUTH=true` → writes land as `local-user`; the real account is untouchable. Capture `$!` and `trap '... kill' EXIT`.
2. **Seed DeepSeek for local-user, or you test the WRONG model.** `local-user` has no `user_settings` row → `/chat` falls through to `VLLM_BASE_URL` (local ollama), NOT DeepSeek. Before any chat turn, seed a settings row for `local-user` with `active_provider='deepseek'` + the DeepSeek key/model (copy read-only from the real user's row), and DELETE it in teardown.
3. **Tag + teardown.** Every artifact gets a `PTEST-` prefix; scope every `sqlite3` sweep to `user_id='local-user'` + the `PTEST-` tag; run teardown in a `trap … EXIT`. **Never** an unscoped DELETE; never touch `88a19121`. Assert real-account baseline (449/5/0) unchanged at the end.
4. **Bounded readiness + `--max-time` on streaming curls** so a wedged backend or a stalled DeepSeek stream can't spin until morning — emit a `## BLOCKER` and exit instead.

## Parallel-safety map
| Loop | Type | Owns | Run with |
|---|---|---|---|
| **R** | build | `src/routes/chat.rs` + rubric docs | ✅ parallel with the bot loops |
| **D** | build | `telegram-bot/src/main.rs` + `Cargo.toml` | ✅ parallel with R; **must precede M** (same file) |
| **M** | build | `telegram-bot/src/main.rs` | ⚠️ **after D**, same bot worktree |
| **PT1–PT4** | test | run from MAIN repo (sandbox :1599) | **after the build loops merge**; run serially (shared sandbox port + DB) |

**Disjoint build tracks → safe to run together:** **R** (backend) ‖ **D→M** (bot). Optional Part 4 (per-change risk tag in `document_edits.reason`) serializes after R (also edits chat.rs/builtin_tools.rs) — not included below; ask if you want it as Loop T.

---

# BUILD LOOPS

## Loop R — Shared risk rubric into the backend prompt (serves app + bot)

**Goal:** Inject the litigation risk rubric into `MIKE_SYSTEM_PROMPT` so every `/chat` call — Tauri app AND Telegram bot — runs rubric-driven risk-triage, with an explicit "beyond the rubric → disclose" fallback rule.

**AUTONOMY CONTRACT (embedded — do not pause to ask):**
- Make every reversible decision from this spec + existing code conventions. **Silence ≠ stop.**
- **SCL (Self-Correct Loop)** after each edit/build: run CHECKS → on a red, hand the EXACT error to **rust-build-resolver** → fix → re-run. Cap **3 rounds per check.** Never commit red; never leave the branch un-compilable.
- **STOP only on a real blocker** (a check still red after 3 rounds / a genuinely ambiguous requirement / an unauthorized destructive action). Write a `## BLOCKER` note (what failed, the exact error, what you tried) and halt.
- **On success:** commit (clear message) → append the `🧪 You test:` line below.
- Skills to name where apt: `/human-logic`, `backend-builder`, `/ecc:rust-build`; resolver `rust-build-resolver`.

**SETUP (build loop → worktree-per-loop off the current base; NEVER edit in main):**
```bash
set -euo pipefail
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"
cd "$ROOT"
BASE=$(git branch --show-current)          # reconcile/full (verified)
git worktree add -b loop-r-rubric ../mike-loops/r-rubric "$BASE"
cd ../mike-loops/r-rubric
```
All edits/builds happen in `../mike-loops/r-rubric`. **Builds run SERIAL with other loops (8GB/18GB memory limit) — and note the smoke step below does a FULL `cargo build` of the ~79MB worktree binary, which is the expensive part of this loop; expect it to be slow and to contend with any other building loop.** The smoke-RUN reads the REAL DB `src-tauri/mike.db` from the MAIN repo path but runs an **isolated backend instance** (see RUN-CONFIG) — it does NOT touch the worktree's empty DB and does NOT collide with the always-on `:1514` instance.

**TASK** — apply `/human-logic` (layer, don't duplicate; tell the user when going past the rubric). Owns `src/routes/chat.rs` → **serialize vs all other chat.rs loops**; parallel-safe vs the bot loop.

1. **Read first** (verify anchors before editing): `docs/LITIGATION_RISK_RUBRIC_PROMPT_BLOCK.md` (**7803 bytes — ~7.8k, NOT the "~5.4k" in its own header comment; budget prompt size accordingly**) and `docs/LITIGATION_RISK_RUBRIC.md` (full reference, 31569 bytes). The block's first content line is `LITIGATION DRAFTING & REVIEW RISK RUBRIC — ...` — that exact string is your post-build grep anchor.

2. **Inject the block into the const.** In `src/routes/chat.rs`, `const MIKE_SYSTEM_PROMPT: &str = r#"..."#;` **opens at line 1089, closes at the `"#;` on line 1465** (the line right after the `LEGACY TOOLS:` section at line 1463 — verified). Insert the rubric content as new layered text **before** that closing `"#;`, after the existing redline/draft/citation instructions — do NOT touch or duplicate the existing body. Paste the prompt-block content verbatim (drop only its leading `<!-- ... -->` HTML comment — raw-string-safe plain text). The const is injected verbatim as the base layer at `chat.rs:4670` (`sections.push(MIKE_SYSTEM_PROMPT.trim()...)` — verified), so editing the const is the whole job — no call-site change. **Raw-string safety:** the block must contain no `"#` sequence; if any slipped in, the `r#"..."#;` delimiter breaks the build — the resolver handles that via `cargo check`.

3. **Add the NEW redlining + fallback-with-disclosure directive** to BOTH `docs/LITIGATION_RISK_RUBRIC.md` (reference) and the injected const text, one short paragraph, verbatim wording:
   > REDLINING IS RUBRIC-DRIVEN. Review and redline against THIS rubric first. If an issue is genuinely not covered by the rubric, you MAY draw on general legal training knowledge — but you MUST tell the user when you go beyond the rubric, prefixed exactly: `⚠️ Beyond the rubric — general principle:`.

   Keep the two copies textually identical so the doc reference matches what the model actually sees.

**RUN-CONFIG (correct, current — the stale LOOP_PLAN §0.6 says 3001/`~/mikerust-data`; IGNORE it):**
- `DATABASE_URL=sqlite:src-tauri/mike.db` (the REAL history DB — verified: 449 docs / 5 chats / 0 lessons baseline).
- DeepSeek is **CLOUD** (`https://api.deepseek.com/v1`, `DEEPSEEK_API_KEY` in `.env`); chat model id `local:deepseek-v4-flash`. NOT ollama.
- The always-on app backend is on **:1514 and is bypass-OFF** (writes land under the REAL user). The smoke step below does **NOT** use it — it boots its OWN isolated instance on **:1599 with `MIKE_BYPASS_AUTH=true`** so every smoke write authenticates as the synthetic `local-user` (`src/auth/middleware.rs:27`, verified) and the real account is never touched. `POST /chat` requires auth (`post_chat_root` takes `AuthUser`, chat.rs:4304) — without bypass or a Bearer token it 401s, which is why bypass is mandatory here.

**BUILD (in the worktree):**
```bash
cargo check                      # SCL: red → rust-build-resolver → fix → re-run (≤3)
```

**SMOKE RUN (isolated bypass instance on :1599, real DB, worktree-built binary — proves the SHIPPED prompt):**
```bash
cd "/Users/vedantmishra/Desktop/mike aur donna main git"
set -a; source .env; set +a            # DEEPSEEK_API_KEY, etc.
# Build the WORKTREE binary so we smoke-test THIS loop's edited prompt (not the stale :1514 binary):
( cd ../mike-loops/r-rubric && cargo build --features rag )   # SCL on red → rust-build-resolver
BIN="../mike-loops/r-rubric/target/debug/mike"

# Boot an isolated, auth-bypassed instance on a free port (:1514 is already held by the app backend):
export PORT=1599 MIKE_BYPASS_AUTH=true DATABASE_URL="sqlite:src-tauri/mike.db"
BASE="http://127.0.0.1:1599"
nohup "$BIN" > /tmp/ptest-rubricR.log 2>&1 &
SMOKE_PID=$!
# Always clean up the smoke backend + temp files, even on early abort:
trap '
  for cid in $(sqlite3 src-tauri/mike.db "SELECT id FROM chats WHERE user_id='"'"'local-user'"'"' AND title LIKE '"'"'PTEST-rubricR%'"'"';"); do
    sqlite3 src-tauri/mike.db "DELETE FROM chats WHERE id='"'"'$cid'"'"' AND user_id='"'"'local-user'"'"';"
  done
  kill "$SMOKE_PID" 2>/dev/null
  rm -f /tmp/ptest_rubricR.sse /tmp/ptest-rubricR.log
' EXIT
# Readiness: 404 on / means up; 000 means down.
until [ "$(curl -s -m2 -o /dev/null -w '%{http_code}' "$BASE/")" != "000" ]; do sleep 1; done

# CONST-OK: the rubric string is compiled into the worktree binary we just built+booted.
strings "$BIN" 2>/dev/null | grep -q "LITIGATION DRAFTING & REVIEW RISK RUBRIC" && echo "CONST-OK"

# /chat smoke (bypass → local-user, no header needed): a clearly time-barred pleading must surface a rubric risk.
curl -s -N "$BASE/chat" -H 'Content-Type: application/json' -d \
 '{"title":"PTEST-rubricR","model":"local:deepseek-v4-flash","messages":[{"role":"user","content":"Review this for filing risks: an appeal filed 2 years after the impugned order, with no condonation application. What is the single most serious defect?"}]}' \
 > /tmp/ptest_rubricR.sse
grep -iE 'limitation|condon|time-barred' /tmp/ptest_rubricR.sse && echo "TRIAGE-OK"
```
The `trap … EXIT` above guarantees the smoke backend is killed and the single `PTEST-rubricR`-tagged chat (written as `local-user`) is swept on ANY exit path. **Do NOT touch any non-`PTEST-` row and never delete unscoped.**

**CHECKS (done-criteria — all must pass):**
1. `cargo check` (and the smoke `cargo build`) green in the worktree (zero errors).
2. `grep -c "LITIGATION DRAFTING & REVIEW RISK RUBRIC" src/routes/chat.rs` ≥ 1 (block is inside the const, before line ~1465's `"#;`).
3. `grep -c "Beyond the rubric — general principle" src/routes/chat.rs` ≥ 1 AND the identical string present in `docs/LITIGATION_RISK_RUBRIC.md` (`grep -c` ≥ 1 there too).
4. The existing const body is intact — `grep -q "LEGACY TOOLS:" src/routes/chat.rs` succeeds and nothing is duplicated (`grep -c "LEGACY TOOLS:"` stays 1).
5. Smoke: `CONST-OK` (rubric string is in the worktree binary) AND `TRIAGE-OK` (the `/chat` reply surfaces `limitation`/`condon`/`time-barred`). A single TRIAGE miss is model flake — re-run the `/chat` curl once; fail only on **2/2** misses. (If `TRIAGE-OK` is absent because no `content_delta` arrived, check `/tmp/ptest-rubricR.log` for a panic / DeepSeek auth error before blaming the prompt.)
6. Real DB clean after teardown: `sqlite3 src-tauri/mike.db "SELECT count(*) FROM chats WHERE title LIKE 'PTEST-%';"` returns `0`, AND the real account is pristine: `sqlite3 src-tauri/mike.db "SELECT (SELECT count(*) FROM documents WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491'),(SELECT count(*) FROM chats WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491');"` still returns `449|5`.

**STOP/BLOCKER path:** if check 1 stays red after 3 resolver rounds, or `:1599` will not bind (port in use — pick another free port and retry once), or the worktree binary won't build, write a `## BLOCKER` note (the exact error + what you tried) and halt. Do NOT fall back to writing against the real-user `:1514` instance.

**On success:** commit on `loop-r-rubric` — `feat(chat): inject litigation risk rubric into MIKE_SYSTEM_PROMPT + beyond-rubric disclosure rule (serves app + bot)`.

🧪 **You test:** open the app (or DM the bot) and paste a clearly time-barred pleading for review — Mike should flag the limitation/condonation defect as HIGH, and if it ever reaches past the rubric it should say `⚠️ Beyond the rubric — general principle:`.

---

## Loop D — Bot redlining (receive .docx → risk-review → tracked-change redline)

**Goal:** Make the Telegram bot accept an uploaded `.docx`, upload it to the backend, run a two-turn rubric review→apply flow that produces real tracked changes (`w:ins`/`w:del`), and deliver the redlined `.docx` back — while rejecting PDFs with a clear `.docx`-only message.

### SETUP (build loop — worktree off the current base, NEVER the main tree)
```bash
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"
cd "$ROOT"
BASE="$(git rev-parse --abbrev-ref HEAD)"          # shared base branch (e.g. reconcile/full)
# Keep the worktree a SIBLING under ~/Desktop — teloxide is a path dep
# (../../teloxide-master → /Users/vedantmishra/Desktop/teloxide-master) and only
# resolves if the worktree sits next to the main repo. Do not relocate it.
git worktree add -b loop-d-bot-redline "../mike-loopD" "$BASE"
cd "../mike-loopD"
# Build/edit happen HERE. (Localhost pressure-testing happens later from $ROOT only — worktree DB is empty.)
# Sanity-check the path dep survived the worktree before coding:
test -d "../../teloxide-master/crates/teloxide" || echo "## BLOCKER: teloxide path dep missing from worktree — do not proceed"
```

**Build-loop discipline:** `telegram-bot/` is a **standalone crate** (its `Cargo.toml` has an empty `[workspace]` table, detaching it from the parent `mike` workspace) — so it builds in isolation and does **not** rebuild the backend. Builds across parallel loops are **SERIAL** (8GB/18GB memory limit); do not kick off a `cargo build` while another loop's build is running.

### AUTONOMY (full contract)
Don't pause to ask — make every reversible call from this spec + existing bot conventions; silence ≠ stop. After each build/edit run CHECKS as a **SCL (Self-Correct Loop)**: on red, hand the EXACT error to **rust-build-resolver**, fix, re-run; cap **3 rounds/check**. **Never commit red; never leave the crate un-compilable.** **STOP only on a real blocker** (a check still red after 3 rounds / a genuinely ambiguous requirement / an unauthorized destructive action) — write a `## BLOCKER` note (what failed, the exact error, what you tried) and halt. On success: commit (clear message) → append the `🧪 You test:` handoff below.

### TASK (Part 2 — owns `telegram-bot/src/main.rs` + `telegram-bot/Cargo.toml`)
Reuse backend primitives; reinvent nothing. **Verified anchors (this worktree = same tree):**

1. **`telegram-bot/Cargo.toml:18`** — add the `multipart` feature (currently `["json", "stream"]`; required for the upload POST):
   `reqwest = { version = "0.12", features = ["json", "stream", "multipart"] }`

2. **One config constant** (top of `main.rs`, near `const TELEGRAM_LIMIT: usize = 4096;` at **line 40**) — `const MAX_DOC_BYTES: usize = 20 * 1024 * 1024;` (20 MB cap, single source of truth — human-logic: not an arbitrary round wall, sized to a real pleading + headroom).

3. **Document arm in `handle_text` — extend the reject branch at `main.rs:449-453`.** Today the `let Some(user_text) = msg.text() … else { "I can only handle text messages for now." }` rejects *everything* non-text. Before that reject fires, branch on `msg.document()`:
   - Thread it through the EXISTING `handle_text` (fn at `main.rs:438`; it already owns the in-flight guard + per-chat `cfg = with_token(&cfg, resolve_token(&tokens, msg.chat.id, &cfg).await)` at `main.rs:482`); **do not add a parallel dispatch branch** (dispatch is at `main.rs:312-320`).
   - **teloxide `Document` field shapes (verified in `../../teloxide-master`):** `doc.file` is a `FileMeta` → `doc.file.id` (`FileId`, pass to `get_file`), `doc.file.size` (`u32`, the byte size). `doc.mime_type` is `Option<Mime>` (NOT a string). The original filename is `doc.file_name` (`Option<String>`) — use that for the `.docx` extension check. (There is **no** `doc.file_size` and `doc.mime_type` is not a string — do not compare it with `.ends_with`.)
   - **human-logic guards (all reachable, surface every failure visibly):**
     - **`.docx`-only:** if `doc.file_name` does not end in `.docx` (case-insensitive) — or `doc.mime_type` is present and is not the docx mime — reply *"I can only redline Word `.docx` files for now — please re-send as `.docx`."* and `return Ok(())`. (Mirrors the backend guard at `builtin_tools.rs:835-836`; catch it bot-side so the user isn't bounced through an upload first.)
     - **size cap:** if `doc.file.size as usize > MAX_DOC_BYTES` (or the downloaded len exceeds it) → reply *"That file is over the 20 MB limit."* and return.
     - **no-caption:** if `msg.caption()` is empty/whitespace → reply *"Send the `.docx` again with a caption telling me what to review or change (e.g. 'risk-review this settlement')."* and return. (Caption = the instruction.)
   - **Download into memory** via teloxide. Verified present: `Requester::get_file` (teloxide-core, takes a `FileId`) and the `Download` impl on `Bot` (teloxide-core `bot/download.rs:17`, available with default features — no extra Cargo feature needed). **Watch the writer type:** `download_file(path: &str, destination: &mut (dyn tokio::io::AsyncWrite + Unpin + Send))`. A bare `Vec<u8>` is **not** `AsyncWrite` unless tokio's `io-util` feature is on (the bot's tokio is `["macros","rt-multi-thread"]` only). Two correct options:
     - **Preferred (no new feature):** use the streaming variant and collect, mirroring the existing `fetch_document` byte-collection at `main.rs:1690`:
       ```rust
       use futures::StreamExt;                 // already used elsewhere in the crate
       let file = bot.get_file(doc.file.id.clone()).await?;
       let mut buf: Vec<u8> = Vec::new();
       let mut stream = bot.download_file_stream(&file.path);
       while let Some(chunk) = stream.next().await {
           buf.extend_from_slice(&chunk?);     // buf = the .docx bytes
       }
       ```
     - **Or:** add `io-util` to tokio's features and `use tokio::io::AsyncWriteExt;`, then `download_file(&file.path, &mut buf)` into a writer (e.g. wrap a `Vec` appropriately). Pick whichever compiles cleanly; let **rust-build-resolver** settle any type error.
   - **Upload:** `POST {api}/document` multipart, **part name `file`**, **plus a `cache=true` text part** (the upload handler reads the `cache` field at `documents.rs:137-139`; without it `content_hash` is NULL and the chat-link UPDATE no-ops — see the comment at `chat.rs:4440` — so the model never sees the file, a silent false-negative). Parse `{ "id": ... }` from the response: the `upload_document` handler (`documents.rs:93`) returns `{"id":doc_id,"filename":…,"file_type":…,"size_bytes":…,"status":"ready"}` at **`documents.rs:273-279`**.
   - **Two-step UX** (carry the doc as `messages[].files[].document_id`, collected at `chat.rs:4419-4427` → mapped to `doc-0`):
     - **Turn 1 (review only):** caption + system framing *"Risk-review this uploaded document against the litigation rubric. Produce the risk table only — do NOT edit yet."* Reply with the streamed review, then ask *"Apply these as tracked changes? Reply 'yes'."* Stash the `document_id` + chat id for this `msg.chat.id` so the follow-up can find it (reuse the existing per-chat state maps; no new global if an existing `HashMap<ChatId, …>` already fits the pattern).
     - **Turn 2 (on "yes"):** send a follow-up `/chat` turn (same `document_id`, full history) instructing *"Apply your proposed fixes to the uploaded Word file as tracked changes now using edit_document."* This elicits `edit_document` (schema at `builtin_tools.rs:134-156`: `{doc_id:"doc-0", edits:[{find,replace}]}`) → `apply_tracked_edits` (`docx_writer.rs:837`) → `w:ins`/`w:del` with `w:author="Mike"`.
   - **Deliver** the edited `.docx` via the EXISTING `fetch_document(cfg, document_id)` at `main.rs:1690` (render-then-download), sent back as a Telegram document.
   - **clause-not-found surfacing:** if `edit_document` returns a JSON `{"error":…}` (e.g. find-text not present), relay that text to the user verbatim instead of silently dropping it.

4. **Auth:** entirely via the existing `resolve_token`/`with_token` path (`main.rs:482, 717, 731`) — `/login <PIN>` sets the per-chat token; no new auth code.

### SKILLS / AGENTS / RESOLVERS
`/ecc:rust-test` (write the failing test FIRST), `/human-logic` (walk the guards above), `/ecc:rust-review` (final pass), resolver **rust-build-resolver** for any red build/clippy.

### BUILD / TEST (this worktree — telegram-bot is its own standalone crate; builds SERIAL — see Build-loop discipline)
```bash
cd "../mike-loopD/telegram-bot"
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```
Write a unit test FIRST (`/ecc:rust-test`) for the **pure routing decision** — factor the mime/caption/size gate into a small helper that takes plain values (`file_name: &str`, `size: usize`, `caption: Option<&str>`) and returns an enum (`Reject{msg} | NeedCaption | Upload{…}`), so it's testable without teloxide or the network. Assert:
- a `.docx` name + non-empty caption → `Upload`
- a `.pdf` name → `Reject` carrying the `.docx`-only message
- a `.docx` whose size > `MAX_DOC_BYTES` → `Reject` (size)
- a `.docx`, empty/whitespace caption → `NeedCaption`
Then make them pass. Keep all teloxide/`get_file`/`download_file`/network calls OUT of the testable unit.

### OPTIONAL live smoke (from MAIN repo $ROOT only — NEVER the worktree; real DB)
Only if a manual sanity check is wanted; **the loop is DONE on CHECKS alone.** Note: port `1514` runs bypass-OFF, so anything you upload here lands under the **real** user — tag every test doc `PTEST-` and clean it up (scoped) afterward.
```bash
cd "$ROOT"; set -a; source .env; set +a            # PORT=1514, DATABASE_URL=sqlite:src-tauri/mike.db, DeepSeek = CLOUD
cargo run --features rag                            # binds 127.0.0.1:1514  (or reuse target/debug/mike to skip a rebuild)
# new shell:
cd "$ROOT/telegram-bot"; set -a; source .env; set +a   # MIKE_API_URL=http://localhost:1514
cargo run                                          # then in Telegram: /login <PIN>, send a PTEST- .docx with a caption
```
Afterward, sweep ONLY your test rows under the real user — never a blanket prefix delete:
```bash
cd "$ROOT"
REAL=88a19121-c6b1-4cc7-a421-5608ea4f0491
sqlite3 src-tauri/mike.db "DELETE FROM documents WHERE user_id='$REAL' AND filename LIKE 'PTEST-%';"
```

### CHECKS (done-criteria — all must be green)
- [ ] `cargo build` clean in `telegram-bot/`.
- [ ] `cargo test` green (the 4 routing-gate tests above pass).
- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] A `.docx` message no longer hits *"I can only handle text messages for now."* — it routes into the upload/review path.
- [ ] A PDF message gets the explicit `.docx`-only reply (not the generic text reject, not a silent drop).
- [ ] `reqwest` has the `multipart` feature; upload sends part `file` + `cache=true`; doc is carried as `messages[].files[].document_id`.
- [ ] human-logic: 20 MB cap from one const; no-caption asks for one; clause-not-found `{"error"}` surfaced to the user; auth via `/login`.

### On success
Commit (e.g. `feat(bot): redline uploaded .docx via two-step rubric review → tracked changes`).
🧪 **You test:** `/login` in Telegram, send a `.docx` with caption "risk-review this" → you get a risk table + "Apply as tracked changes?"; reply "yes" → you get the same file back with Mike's tracked insertions/deletions. Send a PDF → you get the clear "`.docx` only" message.

### If blocked
Write `## BLOCKER` with: the failing CHECK, the exact compiler/clippy error, and what you tried (the 3 SCL rounds). Do not commit red. Do not touch the main tree or the real DB.

---

## Loop M — Bot memory ("remember this" via the Mike-listens harness)

**Goal:** Give the Telegram bot a memory trigger — `/remember <text>` plus conservative natural-language detection ("remember", "from now on", "in future", "always", "never") POSTs to the backend's Mike-listens harness (`POST /mike-feedback/chat`) so the rule is learned once and auto-applied to every future `/chat`; a normal sentence must NOT false-trigger. Spec = Part 3. File = `telegram-bot/src/main.rs` (+ confirm-message copy). The backend route, learning, and auto-injection already exist — the bot only TRIGGERS.

**AUTONOMY CONTRACT (every reversible call is yours to make):**
- Don't pause to ask. Make every reversible decision from this spec + existing bot conventions. Silence ≠ stop.
- After each edit run CHECKS via SCL: on red, hand the EXACT error to `rust-build-resolver` → fix → re-run. Cap 3 rounds per check. Never commit red; never leave the branch un-compilable.
- STOP only on a real blocker (red after 3 rounds / genuinely ambiguous requirement / an unauthorized destructive action). Write a `## BLOCKER` note (what failed, the exact error, what you tried) and halt.
- Building happens in the WORKTREE (parallel-safe across loops, but builds run SERIAL on this machine due to 8GB/18GB memory limits — do not start a second cargo build while one is running). Pressure-TESTING localhost happens ONLY from the MAIN repo `$ROOT` against the real DB.
- On success: commit (clear message) → append the "🧪 You test:" line at the bottom.
- Skills to name where apt: `/human-logic` (no-arg guard, 401 surfacing, mention-the-undo-page, no false-trigger), `/ecc:rust-review` (idiomatic teloxide/reqwest, error paths), `/ecc:rust-test` (failing test FIRST for the detector). Resolver on red build = `rust-build-resolver`. Reuse `explorer` only if an anchor drifts.

**SETUP (build loop → worktree off the current HEAD; EDITS + cargo build happen HERE only):**
```bash
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"
cd "$ROOT"
# Loop M shares the bot worktree with Loop D (both edit telegram-bot/src/main.rs).
# Idempotent: reuse the worktree if it already exists, else cut it off the current HEAD.
# NOTE: a worktree captures COMMITTED state only — the main tree's uncommitted files
# (the repo currently has ~38) do NOT leak into the worktree. Build isolation is preserved.
WT="$ROOT/../mike-loop-bot"
if [ -d "$WT/.git" ] || git -C "$ROOT" worktree list | grep -q "$WT"; then
  cd "$WT"
  git checkout bot/loop-m 2>/dev/null || git checkout -b bot/loop-m
else
  git -C "$ROOT" worktree add "$WT" -b bot/loop-m
  cd "$WT"
fi
pwd   # MUST print .../mike-loop-bot — all EDITS + cargo build happen HERE.
# Pressure-testing happens from $ROOT (real DB) — see RUN / PRESSURE-TEST below.
```

**TASK (real anchors — verified against current code; re-verify with Read/Grep before editing):**

1. **Add the `/remember` command.** `telegram-bot/src/main.rs:225` — `enum Command` (derive `BotCommands`); the enum body runs lines 225–244 (last arm `Docs`). Add a new arm:
   ```rust
   #[command(description = "teach Mike a lasting rule: /remember <text>")]
   Remember(String),
   ```
   Add a `/remember` line to the `/help` body (the `Command::Help` arm in `handle_command`, lines ~365–381).

2. **Wire the command** in `handle_command` (`main.rs:340`; `match cmd { … }` ends ~`main.rs:432`). Resolve the per-chat token exactly like `Command::Chats` (lines 424–427) / `Command::Docs` (428–431):
   ```rust
   Command::Remember(text) => {
       let cfg2 = with_token(&cfg, resolve_token(&tokens, msg.chat.id, &cfg).await);
       remember_rule(&bot, msg.chat.id, &cfg2, text.trim()).await?;
   }
   ```
   `human-logic`: if `text.trim().is_empty()` the helper replies the usage hint and returns (mirror the `/login` empty-arg guard in `cmd_login` at `main.rs:784-788`). Do NOT silently no-op.

3. **Conservative NL detection** inside `handle_text` (`main.rs:438`). Place it AFTER the per-chat token is resolved — i.e. AFTER `let cfg = with_token(&cfg, resolve_token(...).await);` at **`main.rs:482`** — and BEFORE the messages Vec is built (~`main.rs:485`). This ordering matters: the in-flight guard (lines 470–478) and token resolution (482) must already have run so you reuse the shadowed, authenticated `cfg`. (Placing detection earlier would use an unresolved token.) One small helper, lowercase match on the 6 fixed phrases only:
   ```rust
   fn looks_like_a_rule(t: &str) -> bool {
       let l = t.to_lowercase();
       ["remember", "from now on", "in future", "in the future", "always", "never"]
           .iter().any(|p| l.contains(p))
   }
   ```
   When it matches: call `remember_rule(&bot, msg.chat.id, &cfg, &user_text).await?` (reuse the resolved `cfg` from line 482), THEN `return Ok(())` so the message is treated as teaching, not a normal chat turn. Conservative = these 6 phrases only; do NOT add greedy single words like "want" or "should".

4. **`remember_rule` helper** (new `async fn`, place near `cmd_login`/`fetch_document`). POST multipart to `{api}/mike-feedback/chat` with a single text field `message` (handler reads field `"message"` — confirmed `src/routes/mike_feedback.rs:58`). The bot only triggers; learning + auto-injection are backend-side (`harness::triage`/`evolve` ~`mike_feedback.rs:193-203`; auto-inject per user_id at `src/routes/chat.rs:4623` via `crate::harness::active_lessons_prompt`). Match `fetch_document`'s client/auth style (`main.rs:1690-1700`):
   ```rust
   async fn remember_rule(bot: &Bot, chat_id: ChatId, cfg: &BotConfig, text: &str) -> ResponseResult<()> {
       if text.is_empty() {
           bot.send_message(chat_id, "Usage: /remember <the rule you want me to keep>").await?;
           return Ok(());
       }
       let client = reqwest::Client::new();
       let url = format!("{}/mike-feedback/chat", cfg.api_url.trim_end_matches('/'));
       let form = reqwest::multipart::Form::new().text("message", text.to_string());
       let resp = client.post(&url)
           .header("authorization", format!("Bearer {}", cfg.session_token))
           .multipart(form).send().await;
       match resp {
           Ok(r) if r.status().is_success() => {
               // Drain the SSE so the harness commit completes server-side; we don't parse it.
               let _ = r.bytes().await;
               bot.send_message(chat_id,
                   "✅ Got it — I'll keep that in mind from now on. \
                    See & manage your saved rules on the Personalization page (Mike listens).").await?;
           }
           Ok(r) if r.status().as_u16() == 401 => { bot.send_message(chat_id, AUTH_ERR).await?; } // AUTH_ERR is the const at main.rs:135 (there is NO "AUTH_HINT")
           _ => { bot.send_message(chat_id, "⚠️ Couldn't save that rule right now — try again in a moment.").await?; }
       }
       Ok(())
   }
   ```
   **`AUTH_ERR`, not `AUTH_HINT`:** the login-nudge constant in this file is named `AUTH_ERR` (`main.rs:135`). There is no `AUTH_HINT` — referencing it will not compile.
   **Reqwest `multipart` feature:** `telegram-bot/Cargo.toml:18` currently reads `reqwest = { version = "0.12", features = ["json", "stream"] }` — **no `"multipart"`** (verified). Part 2 (Loop D, same worktree) is supposed to add it; if Loop D has NOT landed in this worktree, add `"multipart"` yourself — `Form::new()` needs it.
   `human-logic`: 401 → re-`/login` hint (not a raw error); network/5xx → visible actionable message (no silent catch); mention the Personalization page so the user can review/undo (`GET /mike-feedback/lessons`, doc at `mike_feedback.rs:10`).

**BUILD (in the worktree — serial; do not run a second cargo build concurrently):**
```bash
cd "$ROOT/../mike-loop-bot/telegram-bot"
cargo build            # SCL: on red → rust-build-resolver → fix → re-run (≤3 rounds)
cargo test looks_like_a_rule 2>/dev/null || cargo test 2>/dev/null  # green or "0 tests" both pass
```

**RUN / PRESSURE-TEST (from MAIN repo `$ROOT`, real DB — but a DEDICATED bypass-ON sandbox so the real account is never touched):**

> Why a dedicated instance, not the live `:1514`: the running `:1514` may be auth-ON (it returns 401 on `/mike-feedback/lessons`), and `MIKE_BYPASS_AUTH` is read per-request from the process env at `src/auth/middleware.rs:27` — it is fixed at process start, so `export`ing it in *this* shell does NOT change an already-running backend. Hitting `:1514` unauthenticated would just 401 and the assertions would silently fail (false BLOCKER, wasted run). So boot our own bypass-ON instance on a free port `:1599` from the prebuilt binary, sharing the same real `mike.db`. All writes land as `local-user`; the real account `88a19121` is untouchable.

```bash
cd "$ROOT"
set -a; source .env; set +a                 # loads DEEPSEEK_API_KEY etc. (DeepSeek is CLOUD, not ollama)
export PORT=1599 MIKE_BYPASS_AUTH=true DATABASE_URL="sqlite:src-tauri/mike.db"
BASE="http://127.0.0.1:1599"
nohup ./target/debug/mike > /tmp/ptest-m-backend.log 2>&1 &   # prebuilt binary, skips a rebuild
PT_PID=$!
trap 'kill "$PT_PID" 2>/dev/null' EXIT       # always stop our sandbox backend on exit
until [ "$(curl -s -m2 -o /dev/null -w '%{http_code}' "$BASE/")" != "000" ]; do sleep 1; done

# Headless API check of the exact surface /remember drives (no Bearer needed in bypass mode):
B=$(curl -s "$BASE/mike-feedback/lessons" | jq '.stats.total')
curl -s -N "$BASE/mike-feedback/chat" \
  -F "message=PTEST rule: from now on, always end every affidavit you draft with a verification clause." \
  > /tmp/ptest_m_learn.sse
A=$(curl -s "$BASE/mike-feedback/lessons" | tee /tmp/ptest_m_lessons.json | jq '.stats.total')
```

Bot smoke (OPTIONAL — only if a tester is physically on the device; real Telegram):
```bash
cd "$ROOT/telegram-bot"; set -a; source .env; set +a   # telegram-bot/.env sets MIKE_API_URL=http://localhost:1514 + TELEGRAM_BOT_TOKEN
# (NB: main.rs:262 defaults MIKE_API_URL to http://localhost:3001 only if UNSET; telegram-bot/.env overrides it to 1514.)
"$ROOT/../mike-loop-bot/telegram-bot/target/debug/mike-telegram-bot"  # then on phone: /login <PIN> → /remember … → a plain question
```

**CHECKS (done-criteria — all must hold):**
1. `cargo build` green in the worktree; `cargo test` green (or 0 tests). Branch compiles; never committed red.
2. `/remember` path: `grep -q '"phase":"done"' /tmp/ptest_m_learn.sse` (rule committed — **NOT** `"phase":"skipped"`) AND `[ "$A" -gt "$B" ]` AND `jq -r '.selected[].rule' /tmp/ptest_m_lessons.json | grep -qi verif`. No `"type":"error"` in the SSE. *(Triage is model-driven; if `"skipped"`, retry once with a more imperative phrasing — a soft finding, not a red bar.)*
3. No-false-trigger (the §4 reproducible guard): a `#[cfg(test)]` unit test asserts `looks_like_a_rule("What is the limitation period to file a written statement?") == false` AND `looks_like_a_rule("From now on, never omit the verification clause.") == true`. Run `cargo test looks_like_a_rule` — green.
4. Empty `/remember` (`/remember` alone) replies the usage hint and does NOT POST (read-through of the guard suffices if no device).

**PTEST cleanup / teardown (real DB — scoped to `local-user`, runs even on early exit):**
```bash
cd "$ROOT"
# local-user had 0 lessons pre-test (verified) → a blanket local-user wipe of the harness
# tables is exact and CANNOT touch the real account. No DROP, no unscoped DELETE.
sqlite3 src-tauri/mike.db "DELETE FROM harness_lessons  WHERE user_id='local-user';
  DELETE FROM harness_feedback WHERE user_id='local-user';
  DELETE FROM harness_features WHERE user_id='local-user';
  DELETE FROM harness_state    WHERE user_id='local-user';"
# Assert zero residue AND the real account (88a19121) untouched — expect: 0  0
sqlite3 src-tauri/mike.db "SELECT
  (SELECT count(*) FROM harness_lessons WHERE user_id='local-user'),
  (SELECT count(*) FROM harness_lessons WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491');"
rm -f /tmp/ptest_m_learn.sse /tmp/ptest_m_lessons.json /tmp/ptest-m-backend.log
kill "$PT_PID" 2>/dev/null   # stop our sandbox backend (the trap also covers early exit)
```

**On success:** commit on `bot/loop-m`:
```bash
cd "$ROOT/../mike-loop-bot"
git add telegram-bot/src/main.rs telegram-bot/Cargo.toml
git commit -m "feat(bot): /remember + conservative NL trigger → Mike-listens harness"
```

🧪 **You test:** From your phone, `/login <PIN>`, send `/remember from now on always sign off drafts as 'Yours faithfully'`, get the ✅ confirm — then ask Mike to draft a short letter and confirm it signs off that way; finally send a plain question (no trigger words) and confirm it answers normally instead of saying "got it".

> ℹ️ The verifier marked Loop M cautious-only (`autonomous_safe=false`) because the harness's learn-vs-skip triage is model-driven — handled here as a soft retry, not a hard fail. The code itself is complete and compiles. Watch this one if you fan it out.

---

# PRESSURE-TEST LOOPS  (run from the MAIN repo, after the build loops merge — apply the MANDATORY bootstrap above)

## Pressure-test PT1 — Redline end-to-end (real DB, via API on :1599 sandbox)

> Run interpreter: **bash** (`bash pt1.sh`). The loop mixes heredocs and `"${AUTH[@]}"` array expansion; bash gives stable semantics. Run from the MAIN repo only.

**GOAL:** Drive the full redline path over the live HTTP API — upload a defective `.docx` → rubric risk-review chat turn → `edit_document` → download tracked-changes `.docx` — and assert tracked changes + the right HIGH risks fire, plus the edge cases (pdf reject, oversized-doc no-panic, no caption/instruction, clause-not-found, multi-edit, off-rubric disclosure). Real DB → every artifact tagged `PTEST-` and DELETEd at teardown.

### SETUP — run from MAIN repo only (NEVER a worktree; worktree DB is empty)
This is a TEST loop, not a build loop: **no branch, no worktree.** Work in place from the main tree against the real DB on its own bypass-auth port.
```bash
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"
cd "$ROOT" || { echo "## BLOCKER: repo not found at $ROOT"; exit 1; }
git rev-parse --show-toplevel    # sanity: must print $ROOT, NOT a .../worktrees/... path
set -a; source .env; set +a      # loads DEEPSEEK_API_KEY (cloud) + base config (sets PORT=1514)
# Isolated sandbox instance: same real mike.db, but bypass-auth so ALL writes land under
# synthetic user 'local-user' (middleware.rs:27-30) — the real account 88a19121-... is never touched.
export PORT=1599 MIKE_BYPASS_AUTH=true DATABASE_URL="sqlite:src-tauri/mike.db"  # overrides .env's PORT=1514
BASE="http://127.0.0.1:1599"; AUTH=()   # bypass → no Bearer header needed
```
*(Live :1514 is bypass-OFF — confirmed 401 — so it would write to the REAL user. Do NOT test against :1514. PORT is env-overridable and binds `127.0.0.1:{PORT}` at lib.rs:157; booting our own on :1599 gives a clean `local-user` namespace on the same corpus. DeepSeek is **cloud** — `https://api.deepseek.com/v1`, `DEEPSEEK_API_KEY` from `.env` — not ollama; ignore the `local:` prefix in the model id.)*

### AUTONOMY (one line)
Don't pause to ask — make every reversible call from this spec + the code. SCL after each step: run the CHECKS → on red hand the EXACT error/output to the resolver → fix → re-run, cap **3 rounds** per check. **Model-driven content greps are flaky (DeepSeek): run twice, fail only on 2/2 misses; a single miss is a soft finding, never a red bar.** STOP only on a real blocker (a hard check red after 3 rounds, the backend won't boot, or an unauthorized destructive action) — write a `## BLOCKER` note (what failed, exact error, what you tried) and halt. Never leave the sandbox process or DB residue behind: teardown runs via `trap … EXIT` AND kills the backend before sweeping the DB.

### TASK — exact steps with real file:line anchors

**0. Boot the sandbox + arm teardown (do this first, before any artifact is created).** The prebuilt binary (`target/debug/mike`, 79MB, present) skips a rebuild.
```bash
mkdir -p /tmp/ptest
PT_PID=""
: > /tmp/ptest/docids.txt   # every doc id we create gets appended here for exact teardown

teardown() {
  # 1) API deletes (best-effort). FK is ON in the app pool, so these cascade
  #    document_versions/document_edits (migrations 0001/0017 ON DELETE CASCADE).
  if [ -s /tmp/ptest/docids.txt ]; then
    while read -r id; do
      [ -n "$id" ] && [ "$id" != null ] && curl -s -X DELETE "${AUTH[@]}" "$BASE/document/$id" >/dev/null 2>&1
    done < /tmp/ptest/docids.txt
  fi
  for cid in $(sqlite3 "$ROOT/src-tauri/mike.db" "SELECT id FROM chats WHERE user_id='local-user' AND title LIKE 'PTEST-%';" 2>/dev/null); do
    curl -s -X DELETE "${AUTH[@]}" "$BASE/chat/$cid" >/dev/null 2>&1
  done
  # 2) KILL THE BACKEND FIRST so the sqlite3 CLI is not racing the WAL writer.
  [ -n "$PT_PID" ] && kill "$PT_PID" 2>/dev/null
  for _ in 1 2 3 4 5; do kill -0 "$PT_PID" 2>/dev/null || break; sleep 1; done
  # 3) Belt-and-suspenders DB sweep — child tables FIRST (by explicit id list AND by
  #    local-user+PTEST scope), then parents. Scoped so the real account is untouchable.
  if [ -s /tmp/ptest/docids.txt ]; then
    IDS=$(paste -sd, /tmp/ptest/docids.txt | sed "s/[^,]*/'&'/g")
    sqlite3 "$ROOT/src-tauri/mike.db" "DELETE FROM document_edits    WHERE document_id IN ($IDS);
      DELETE FROM document_versions WHERE document_id IN ($IDS);" 2>/dev/null
  fi
  sqlite3 "$ROOT/src-tauri/mike.db" <<'SQL' 2>/dev/null
DELETE FROM document_edits    WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%');
DELETE FROM document_versions WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%');
DELETE FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%';
DELETE FROM chats     WHERE user_id='local-user' AND title    LIKE 'PTEST-%';
SQL
  rm -rf /tmp/ptest /tmp/ptest_mkdocx.py /tmp/ptest-pt1.log
}
trap teardown EXIT

nohup ./target/debug/mike > /tmp/ptest-pt1.log 2>&1 &
PT_PID=$!
# readiness: 404 on / means "up"; 000 means "down". cap ~40s.
code=000
for i in $(seq 1 40); do
  code=$(curl -s -m2 -o /dev/null -w '%{http_code}' "$BASE/")
  [ "$code" != "000" ] && break; sleep 1
done
[ "$code" != "000" ] || { echo "## BLOCKER: backend never bound :1599 in 40s"; tail -40 /tmp/ptest-pt1.log; exit 1; }
```

**1. The `.docx` synth helper** (corpus `raw/*.docx.txt` are text dumps — must build real OOXML; `generate_test_doc.js` does NOT exist on disk, so provide it). python-docx 1.2.0 confirmed present.
```bash
cat > /tmp/ptest_mkdocx.py <<'PY'
import sys
from docx import Document
d = Document()
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.rstrip("\n")
    d.add_paragraph(line if line.strip() else "")
d.save(sys.argv[2])
PY
```

**2. Happy path — upload → review → apply → download.**
Anchors (all verified): upload field is **`file`**, `cache=true` is **mandatory** — only cache uploads set `content_hash` (documents.rs:176), and chat links a doc for cleanup only when `content_hash IS NOT NULL` (chat.rs:4450). Attach via `messages[].files[].document_id` → mapped to `doc-0` (chat.rs:4839). `edit_document` UPDATEs `documents.storage_path` to the edited version (builtin_tools.rs:879), and `GET /document/:id/docx` (display_document, documents.rs:323) streams `documents.storage_path` — so the download serves the edited bytes. Tracked changes are `w:ins`/`w:del` with `w:author="Mike"` (docx_writer.rs:1015/1023). `edit_document` is `.docx`-ONLY (builtin_tools.rs:836). Model id `local:deepseek-v4-flash` (DeepSeek cloud).
```bash
python3 /tmp/ptest_mkdocx.py \
  "/Users/vedantmishra/Desktop/Legal Training Data/pipeline/raw/SETTLEMENT AGREEMENT(1).docx.txt" \
  /tmp/ptest/PTEST-settlement.docx

UP=$(curl -s "${AUTH[@]}" -w '\n%{http_code}' -F "file=@/tmp/ptest/PTEST-settlement.docx;filename=PTEST-settlement.docx" -F "cache=true" "$BASE/document")
DOC=$(echo "$UP" | head -1 | jq -r .id); UPCODE=$(echo "$UP" | tail -1)
UPSTATUS=$(echo "$UP" | head -1 | jq -r .status)
[ -n "$DOC" ] && [ "$DOC" != null ] && echo "$DOC" >> /tmp/ptest/docids.txt

curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l1_review.sse <<JSON
{"title":"PTEST-redline","model":"local:deepseek-v4-flash",
 "messages":[{"role":"user",
   "content":"Risk-review this uploaded settlement agreement against the litigation risk rubric. Flag every HIGH risk (limitation, jurisdiction, verification, payment-trigger/withdrawal sequencing). Then propose tracked-change edits — do not apply yet.",
   "files":[{"document_id":"$DOC"}]}]}
JSON
CHAT=$(grep -o '"chatId":"[^"]*"' /tmp/ptest/l1_review.sse | head -1 | cut -d'"' -f4)

curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l1_edit.sse <<JSON
{"chat_id":"$CHAT","model":"local:deepseek-v4-flash",
 "messages":[
   {"role":"user","content":"Risk-review this uploaded settlement agreement and propose tracked-change edits.","files":[{"document_id":"$DOC"}]},
   {"role":"assistant","content":"(prior review listing HIGH risks and proposed edits)"},
   {"role":"user","content":"Apply your proposed fixes to the uploaded Word file as tracked changes now using edit_document.","files":[{"document_id":"$DOC"}]}]}
JSON

# document_edits is inserted via a fire-and-forget tokio::spawn (builtin_tools.rs ~924),
# so give the async writes a moment to land before the download + DB checks.
for _ in $(seq 1 5); do
  n=$(sqlite3 "$ROOT/src-tauri/mike.db" "SELECT count(*) FROM document_edits WHERE document_id='$DOC';" 2>/dev/null)
  [ "${n:-0}" -ge 1 ] && break; sleep 1
done

curl -s "${AUTH[@]}" -w '\n%{http_code}' "$BASE/document/$DOC/docx" -o /tmp/ptest/l1_out.docx | true
DLCODE=$(curl -s "${AUTH[@]}" -o /tmp/ptest/l1_out.docx -w '%{http_code}' "$BASE/document/$DOC/docx")
# guard: download must be a non-empty valid zip before we assert tracked changes
if [ "$DLCODE" = "200" ] && unzip -l /tmp/ptest/l1_out.docx >/dev/null 2>&1; then
  unzip -p /tmp/ptest/l1_out.docx word/document.xml > /tmp/ptest/l1_doc.xml 2>/dev/null
else
  : > /tmp/ptest/l1_doc.xml   # leaves CHECK 4 to fail loudly as a download problem, not a phantom "no changes"
fi
```

**3. Edge cases (human-logic completeness — the obvious next things a user does).**
```bash
# 3a. PDF reject — edit_document is .docx-only (builtin_tools.rs:836); upload a PDF then try to apply.
printf '%%PDF-1.4\n1 0 obj<</Type/Catalog>>endobj\ntrailer<</Root 1 0 R>>\n%%%%EOF' > /tmp/ptest/PTEST-fake.pdf
PDF=$(curl -s "${AUTH[@]}" -F "file=@/tmp/ptest/PTEST-fake.pdf;filename=PTEST-fake.pdf" -F "cache=true" "$BASE/document" | jq -r .id)
[ -n "$PDF" ] && [ "$PDF" != null ] && echo "$PDF" >> /tmp/ptest/docids.txt
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l1_pdf.sse <<JSON
{"title":"PTEST-pdf-reject","model":"local:deepseek-v4-flash",
 "messages":[{"role":"user","content":"Apply tracked changes to this uploaded file using edit_document — change anything you can.","files":[{"document_id":"$PDF"}]}]}
JSON

# 3b. Oversized-doc no-panic — NOTE: backend body limit is 50 GiB (documents.rs:42), so a 413 is
#     essentially impossible here; the 20MB human-logic cap is a BOT concern (Part 2), NOT this API.
#     This sub-test only proves the API ingests a multi-MB doc WITHOUT panic/hang/500 (it should 200).
python3 - <<'PY'
from docx import Document
d=Document(); p=("That the parties hereto agree and reiterate the foregoing recitals. ")*30
for i in range(9000): d.add_paragraph(f"{i+1}. {p}")
d.save("/tmp/ptest/PTEST-big.docx")
PY
ls -l /tmp/ptest/PTEST-big.docx   # ~few MB — far under the 50 GiB limit
BIG=$(curl -s "${AUTH[@]}" -w '\n%{http_code}' -F "file=@/tmp/ptest/PTEST-big.docx;filename=PTEST-big.docx" -F "cache=true" "$BASE/document")
BIGCODE=$(echo "$BIG" | tail -1); BIGID=$(echo "$BIG" | head -1 | jq -r .id 2>/dev/null)
[ -n "$BIGID" ] && [ "$BIGID" != null ] && echo "$BIGID" >> /tmp/ptest/docids.txt

# 3c. No instruction/caption — upload + attach with EMPTY user content. The model must ASK what to do,
#     not silently no-op (human-logic: no-caption → ask).
DOC_NC=$(curl -s "${AUTH[@]}" -F "file=@/tmp/ptest/PTEST-settlement.docx;filename=PTEST-nocap.docx" -F "cache=true" "$BASE/document" | jq -r .id)
[ -n "$DOC_NC" ] && [ "$DOC_NC" != null ] && echo "$DOC_NC" >> /tmp/ptest/docids.txt
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l1_nocap.sse <<JSON
{"title":"PTEST-nocap","model":"local:deepseek-v4-flash",
 "messages":[{"role":"user","content":"","files":[{"document_id":"$DOC_NC"}]}]}
JSON

# 3d. Clause-not-found — force an edit_document whose `find` is absent. apply_tracked_edits
#     (docx_writer.rs:874) SILENTLY SKIPS a non-matching find and returns Ok with the doc unchanged
#     (no backend "not found" error). So the HARD assertion is "no bogus w:ins/w:del applied"; the
#     "model says not found" part is a SOFT finding only.
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l1_notfound.sse <<JSON
{"chat_id":"$CHAT","model":"local:deepseek-v4-flash",
 "messages":[
   {"role":"user","content":"Using edit_document on doc-0, find the exact text 'ZZQ_NONEXISTENT_CLAUSE_9f3a' and replace it with 'X'. That string is definitely in the file.","files":[{"document_id":"$DOC"}]}]}
JSON

# 3e. Multi-edit + off-rubric disclosure — ask for 2 edits at once and an off-rubric point.
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l1_multi.sse <<JSON
{"chat_id":"$CHAT","model":"local:deepseek-v4-flash",
 "messages":[
   {"role":"user","content":"Apply at least two tracked-change edits to doc-0 in one edit_document call (e.g. tighten two different clauses). Also flag one drafting risk NOT in the rubric, prefixing it exactly with the off-rubric disclosure marker.","files":[{"document_id":"$DOC"}]}]}
JSON
```

### Named skills / agents / resolvers
- `/human-logic` — the edge-case coverage in step 3 (pdf reject, oversized-doc no-panic, no-caption→ask, clause-not-found surfaced, multi-edit, off-rubric disclosure) is exactly its checklist; apply it while reading the assertions, not after.
- `explorer` — if any anchor (SSE event name, route, doc-link gate) doesn't match at runtime, dispatch explorer to re-locate it in `src/routes/chat.rs` / `documents.rs` / `src/pdf/docx_writer.rs` before editing the assertion.
- Resolver on a red boot: this is a test loop (no code edits expected) — if the **binary** won't boot or a route 404s unexpectedly, hand the exact `tail -40 /tmp/ptest-pt1.log` + failing curl to **`rust-build-resolver`**; cap 3 rounds.

### BUILD / RUN / TEST (correct run-config)
- **No build** — reuse prebuilt `target/debug/mike` (booted in step 0 with `.env` sourced, PORT=1599, `MIKE_BYPASS_AUTH=true`, `DATABASE_URL=sqlite:src-tauri/mike.db`). DeepSeek is **cloud** (`https://api.deepseek.com/v1`, `DEEPSEEK_API_KEY` from `.env`) — not ollama; ignore any `local:`/`11434` label.
- *(If the binary is stale/missing, rebuild from repo root: `set -a; source .env; set +a; cargo build --features rag` — then re-boot. Live runs use `cargo run --features rag` → :1514, but THIS loop uses the :1599 sandbox.)*
- Run the batteries (steps 2–3) top-to-bottom, then evaluate CHECKS.

### CHECKS — crisp done-criteria (each PASS/FAIL unless marked SOFT)
Run twice for every model-driven grep (CHECKS 2, 3c, 3d-soft, 3e, and the model parts of 7) and fail only on 2/2.
1. **Upload (hard):** `UPCODE`==200, `$DOC` non-null, `UPSTATUS`=="ready".
2. **Review fired & on-rubric (model):** `l1_review.sse` has a `"type":"chat_id"` and ≥1 `"type":"content_delta"`; concatenated delta text matches ≥2 of `limitation|jurisdiction|verification|withdrawal|tranche|payment` (case-insensitive).
3. **edit_document fired (hard):** `l1_edit.sse` contains `"type":"tool_call_start"` with `"name":"edit_document"` **or** the pair `"type":"doc_edited_start"` … `"type":"doc_edited"`. Absence ⇒ FAIL (model refused the apply step).
4. **Real tracked changes (hard):** `l1_doc.xml` is non-empty AND `grep -c '<w:ins' /tmp/ptest/l1_doc.xml` ≥1 **and** `grep -c '<w:del' /tmp/ptest/l1_doc.xml` ≥1, **and** `grep -q 'w:author="Mike"' /tmp/ptest/l1_doc.xml`. (If `l1_doc.xml` is empty, report it as a DOWNLOAD failure with `DLCODE`, not as "no changes".)
5. **DB persisted (hard, after the settle loop):** `sqlite3 "$ROOT/src-tauri/mike.db" "SELECT count(*) FROM document_versions WHERE document_id='$DOC'"` ≥1 **and** `SELECT count(*) FROM document_edits WHERE document_id='$DOC' AND status='pending'` ≥1.
6. **No errors in happy path (hard):** neither `l1_review.sse` nor `l1_edit.sse` contains `"type":"error"`.
7. **3a PDF reject (hard part + model part):** HARD — `l1_pdf.sse` does NOT contain a `doc_edited` event with applied `w:ins`/`w:del`. MODEL — it surfaces a refusal/`error` mentioning `docx`/`Word`/`only supports`. No silent success on a PDF.
8. **3b oversized no-panic (hard):** `BIGCODE` is `200` (accepted) or — only theoretically — `413`; **never** a hang/`500`/panic. Record which (expect 200). Backend still answers after: `curl -s -o /dev/null -w '%{http_code}' "$BASE/"` ≠ 000.
9. **3c no-caption → ask (model):** `l1_nocap.sse` concatenated deltas contain an interrogative / clarifying ask (matches `\?|what would you like|how can i|which|clarify`); NO `doc_edited` event (the model asks rather than silently editing).
10. **3d clause-not-found (hard + SOFT):** HARD — `l1_notfound.sse` contains NO new applied `doc_edited` carrying `w:ins`/`w:del` for the bogus find (the backend silently skips a non-matching find, so nothing should be claimed applied). SOFT — if the model also says it couldn't find the text (`not found|could not find|no match|wasn't|unable`), good; its absence is a soft finding only (the backend emits no not-found signal, so this is purely model behaviour — never a red bar).
11. **3e multi-edit + off-rubric (model):** `l1_multi.sse` shows a `doc_edited`/`tool_call_start:edit_document`, AND the text carries an explicit off-rubric disclosure (matches `beyond the rubric|general principle|⚠️`). (Multi-edit renumbering is best-effort: PASS if ≥1 edit applied + disclosure present.)
12. **No panic / process alive (hard):** `grep -iE 'panic|panicked|RUST_BACKTRACE|SIGABRT' /tmp/ptest-pt1.log` is empty, and `kill -0 $PT_PID` succeeds.

Any HARD check red after 3 SCL rounds → `## BLOCKER` (the check, the exact SSE/log excerpt, what you tried) and halt — **do not** delete real-account data, **do not** skip teardown (the `trap` still runs on exit).

### PTEST- tagging + teardown (real-DB hygiene — runs even on early exit via the `trap … EXIT` in step 0)
Every chat `title` and every uploaded `filename` is `PTEST-…`; all writes are `user_id='local-user'`. The `trap` (a) API-deletes every captured doc id + every `PTEST-` chat, (b) **kills `$PT_PID` first**, then (c) sweeps `document_edits`/`document_versions` by explicit id list AND by `local-user`+`PTEST-%` scope, then `documents`/`chats`, then `rm -rf`. After it runs, verify zero residue + real account pristine:
```bash
sqlite3 "$ROOT/src-tauri/mike.db" "SELECT
 (SELECT count(*) FROM documents       WHERE user_id='local-user' AND filename LIKE 'PTEST-%'),
 (SELECT count(*) FROM chats           WHERE user_id='local-user' AND title    LIKE 'PTEST-%'),
 (SELECT count(*) FROM document_edits  WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%'));"   # expect: 0|0|0
sqlite3 "$ROOT/src-tauri/mike.db" "SELECT
 (SELECT count(*) FROM documents WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491'),
 (SELECT count(*) FROM chats     WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491');"   # real account untouched
```
If residue ≠ `0|0|0` → re-run the trap's sweep block; never widen the `WHERE` beyond `local-user` + `PTEST-%` (and the captured id list).

### On success
This loop adds **no source changes**. Only commit if something is actually staged (a flaky-assertion tweak to a committed harness script, say); never run a bare `git commit` on an empty index:
```bash
if ! git diff --cached --quiet; then
  git commit -m "test(pt1): redline end-to-end pressure-test passing on :1599 sandbox"
else
  echo "PT1 green, no code change — verification-only run."
fi
```

🧪 **You test:** Open the Tauri app (`cd frontend && npm run dev` → http://localhost:3000, backend `set -a; source .env; set +a; cargo run --features rag` on :1514), upload any real `.docx`, ask "risk-review this and apply your fixes as tracked changes" — confirm the downloaded Word file opens with visible Mike-authored tracked insertions/deletions and the review named the same HIGH risks (limitation / jurisdiction / verification).

---

## Pressure-test PT2 — Memory learn/retract/apply + false-trigger resistance

**Goal:** Prove the "Mike listens" harness end-to-end against the REAL DB on an isolated `local-user` sandbox: a `/mike-feedback/chat` turn LEARNS a rule, `/lessons` CONFIRMS it, a fresh `/chat` turn APPLIES it, a retraction DEPRECATES it (and a later draft stops applying it), a normal question does NOT learn (false-trigger resistance), and writes are per-account scoped — then wipe every artifact. This loop edits NO source code; it is a black-box HTTP pressure test.

> **OPERATOR NOTE — shared SQLite file (verified):** `mike.db` is WAL mode (`src/db/mod.rs:173`) with **no `busy_timeout`** and `max_connections=5`. A real-account backend is already listening on `:1514` and writes the SAME `mike.db`. This harness boots a SECOND backend on `:1599` (bypass-ON, `local-user`). WAL tolerates concurrent readers + one serialized writer, but with no `busy_timeout` a momentary write collision surfaces as `SQLITE_BUSY` / "database is locked" / a sporadic 5xx. **Treat any "database is locked" or single 5xx as a KNOWN-RISK SOFT finding (retry once), NOT a panic** — it is write contention with the live :1514 instance, not a code bug. The real account is never targeted by any write here (bypass → `local-user` only).

---

### SETUP (test loop → MAIN repo, NEVER a worktree — worktree DB is empty)
```bash
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"
cd "$ROOT" || { echo "## BLOCKER: repo missing at $ROOT"; exit 1; }
[ "$(git rev-parse --show-toplevel)" = "$ROOT" ] || { echo "## BLOCKER: not in the MAIN worktree (DB would be empty)"; exit 1; }
set -a; source .env; set +a       # loads DEEPSEEK_API_KEY etc. (.env also sets PORT=1514 — we override below)
# Isolated sandbox: our OWN instance on :1599 with bypass ON → all writes land under
# synthetic user_id "local-user" (auth/middleware.rs:27), NEVER the real account.
# Same mike.db file (real corpus for RAG) but a clean user namespace.
export PORT=1599 MIKE_BYPASS_AUTH=true DATABASE_URL="sqlite:src-tauri/mike.db"
BASE="http://127.0.0.1:1599"
REAL_ACCT="88a19121-c6b1-4cc7-a421-5608ea4f0491"
mkdir -p /tmp/ptest

# Prefer the prebuilt binary (skips a rebuild); else build with rag.
if [ -x ./target/debug/mike ]; then BIN=./target/debug/mike;
else cargo build --features rag 2>/tmp/ptest/build.log || { echo "## BLOCKER: cargo build --features rag failed"; tail -40 /tmp/ptest/build.log; exit 1; }; BIN=./target/debug/mike; fi
nohup "$BIN" > /tmp/ptest-pt2.log 2>&1 &
PT_PID=$!

# Define teardown NOW (uses $PT_PID, captured by value) and register the trap so an
# early exit ALWAYS cleans up. (Original bug: trap referenced $TEARDOWN before it was set.)
TEARDOWN="$(cat <<TD
cd "$ROOT" 2>/dev/null || exit 0
# 1. API-level chat deletes first (PTEST-titled chats only, local-user namespace).
for cid in \$(sqlite3 src-tauri/mike.db "SELECT id FROM chats WHERE user_id='local-user' AND title LIKE 'PTEST-%';" 2>/dev/null); do
  curl -s -X DELETE "http://127.0.0.1:1599/chat/\$cid" >/dev/null 2>&1; done
# 2. Belt-and-suspenders DB sweep, scoped to local-user (0 lessons pre-run → blanket wipe is exact today).
sqlite3 src-tauri/mike.db "
  DELETE FROM chats           WHERE user_id='local-user' AND title LIKE 'PTEST-%';
  DELETE FROM harness_lessons  WHERE user_id='local-user';
  DELETE FROM harness_feedback WHERE user_id='local-user';
  DELETE FROM harness_features WHERE user_id='local-user';
  DELETE FROM harness_state    WHERE user_id='local-user';" 2>/dev/null
# 3. Verify zero residue + real account pristine.
echo "residue (want 0 0): \$(sqlite3 src-tauri/mike.db "SELECT (SELECT count(*) FROM harness_lessons WHERE user_id='local-user'), (SELECT count(*) FROM chats WHERE user_id='local-user' AND title LIKE 'PTEST-%');" 2>/dev/null)"
echo "real acct lessons (want 0): \$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM harness_lessons WHERE user_id='$REAL_ACCT';" 2>/dev/null)"
echo "real acct docs/chats (want 449 5): \$(sqlite3 src-tauri/mike.db "SELECT (SELECT count(*) FROM documents WHERE user_id='$REAL_ACCT'), (SELECT count(*) FROM chats WHERE user_id='$REAL_ACCT');" 2>/dev/null)"
# 4. Stop OUR isolated backend (by PID — never the :1514 instance) + remove temp fixtures.
kill $PT_PID 2>/dev/null
rm -rf /tmp/ptest /tmp/ptest-pt2.log
TD
)"
trap 'bash -c "$TEARDOWN"' EXIT

# Readiness: any HTTP code (incl. 404 on /) means up; 000 means down. Hard 60s cap so a
# wedged binary can't spin until morning.
boot_deadline=$(( $(date +%s) + 60 ))
until [ "$(curl -s -m2 -o /dev/null -w '%{http_code}' "$BASE/")" != "000" ]; do
  kill -0 "$PT_PID" 2>/dev/null || { echo "## BLOCKER: backend died on boot"; tail -40 /tmp/ptest-pt2.log; exit 1; }
  [ "$(date +%s)" -lt "$boot_deadline" ] || { echo "## BLOCKER: backend never became ready in 60s"; tail -40 /tmp/ptest-pt2.log; exit 1; }
  sleep 1
done
AUTH=()   # bypass mode → no Authorization header needed (AuthUser extractor resolves local-user).

# Guard: PROVE bypass actually took effect BEFORE any destructive sweep can target local-user.
# Send a throwaway PTEST chat and confirm it landed under local-user (not the real account).
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' \
  -d '{"title":"PTEST-bypass-guard","model":"local:deepseek-v4-flash","messages":[{"role":"user","content":"ping"}]}' \
  > /tmp/ptest/pt2_guard.sse
GUARD=$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM chats WHERE user_id='local-user' AND title='PTEST-bypass-guard';" 2>/dev/null)
[ "${GUARD:-0}" -ge 1 ] || { echo "## BLOCKER: bypass NOT in effect — writes are NOT landing under local-user; refusing to run destructive sweeps. Got GUARD=$GUARD"; exit 1; }
WHO=$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM harness_lessons WHERE user_id='local-user';")
echo "bypass confirmed (local-user). baseline local-user lessons: ${WHO:-?}"
```

### AUTONOMY
Don't pause to ask — make every reversible call from the spec + live code. After each step run its CHECK; on red, hand the EXACT curl/sqlite output to the resolver, fix, re-run (SCL cap 3 rounds/check). Harness triage is model-driven (DeepSeek CLOUD) → a single content/phase miss is a SOFT finding: retry ONCE with more imperative phrasing before failing. A `SQLITE_BUSY` / "database is locked" / lone 5xx is a SOFT, KNOWN-RISK finding (shared `mike.db` with the live :1514 instance, no `busy_timeout`) → retry ONCE, then note it; do NOT treat it as a panic. STOP only on a REAL blocker (a check still red after 3 rounds, an ambiguous requirement, an unauthorized destructive action, or a per-account SCOPING breach) — write a `## BLOCKER` note (what failed, exact error, what you tried) and halt. On any blocker, the EXIT trap still stops the :1599 backend and sweeps local-user; note PT_PID in the blocker so the operator can confirm.

### TASK — verified anchors (read before relying on them)
- Learn/retract route: `POST /mike-feedback/chat` — **multipart**, field `message` (optional `history` JSON) — `src/routes/mike_feedback.rs:36,50,56-66`. Runs `harness::triage` (`mike_feedback.rs:162`) → `harness::evolve` (`:194`). SSE: `{"type":"harness","phase":"start|edit|done|skipped",...}` then a final `{"type":"complete","lessons":[...]}` (`:193-237`). A pure question with no signal short-circuits to `{"type":"complete","answered":true}` (`:167-169`) and writes NO rule.
- Retraction is real: triage extracts `retract[]` (`src/harness/mod.rs:58,241`); `evolve` matches each against current lessons and sets `deprecated=1` (`mod.rs:378-396`). Emits a `phase:"edit","kind":"retract"` then `phase:"done"`.
- Confirm: `GET /mike-feedback/lessons` → `{selected:[{rule,kind,effectiveness}], stats:{total,active,deprecated}}` where `active` counts `deprecated=0` and `deprecated = total - active` (`mike_feedback.rs:297-312`). Router nested at `src/lib.rs:150` (`.nest("/mike-feedback", ...)`).
- Apply: `POST /chat` injects every active lesson into the system prompt via `crate::harness::active_lessons_prompt(&state.db, &auth.user_id)` (`src/routes/chat.rs:4623`; fn `src/harness/mod.rs:517`) — automatic, per `user_id`, on every chat turn. Chat `chat_id` arrives as `{"type":"chat_id","chatId":"…"}` (`chat.rs:4959`). Chat model id is `"local:deepseek-v4-flash"` (DeepSeek CLOUD `https://api.deepseek.com/v1`, key `DEEPSEEK_API_KEY` — NOT ollama).
- Scoping: every harness table has a `user_id` column (verified). Our sandbox is `user_id='local-user'`; the real account is `88a19121-c6b1-4cc7-a421-5608ea4f0491` (baseline 449 docs / 5 chats / 0 lessons, verified live) — must stay byte-for-byte untouched.

Run these sub-tests in order. Tag every chat `title` with `PTEST-`.

**T1 — LEARN.** Teach one concrete, checkable rule.
```bash
B=$(curl -s "${AUTH[@]}" "$BASE/mike-feedback/lessons" | jq '.stats.active')
curl -s -N "${AUTH[@]}" "$BASE/mike-feedback/chat" \
  -F "message=PTEST rule: from now on, every affidavit you draft MUST end with the exact line 'VERIFIED at Mumbai on this day.' — never omit this verification line." \
  > /tmp/ptest/pt2_learn.sse
A=$(curl -s "${AUTH[@]}" "$BASE/mike-feedback/lessons" > /tmp/ptest/pt2_lessons1.json; jq '.stats.active' /tmp/ptest/pt2_lessons1.json)
```
CHECK T1: `pt2_learn.sse` has `"phase":"done"` (NOT only `"skipped"`) and a final `"type":"complete"`; `A > B`; `pt2_lessons1.json .selected[].rule` matches `verif`/`VERIFIED` (case-insensitive). A `"phase":"skipped"` ⇒ triage saw no lesson → retry ONCE with more imperative phrasing, then SOFT-finding.

**T2 — APPLY.** Fresh chat must reflect the learned rule with no reminder.
```bash
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/pt2_apply.sse <<'JSON'
{"title":"PTEST-pt2-apply","model":"local:deepseek-v4-flash",
 "messages":[{"role":"user","content":"Draft a minimal affidavit for one Rahul Verma confirming his residential address. Keep it short."}]}
JSON
CHAT_APPLY=$(grep -o '"chatId":"[^"]*"' /tmp/ptest/pt2_apply.sse | head -1 | cut -d'"' -f4)
```
CHECK T2: concatenated `content_delta` (plus any `doc_created` body) matches `verified at mumbai` (case-insensitive). Proves the persisted lesson influenced a fresh draft. Miss ⇒ retry ONCE; still miss ⇒ SOFT finding (model non-determinism).

**T3 — RETRACT.** Roll the rule back; it must deprecate.
```bash
curl -s -N "${AUTH[@]}" "$BASE/mike-feedback/chat" \
  -F "message=PTEST: forget that rule about the Mumbai verification line — stop adding it to affidavits from now on." \
  > /tmp/ptest/pt2_retract.sse
curl -s "${AUTH[@]}" "$BASE/mike-feedback/lessons" > /tmp/ptest/pt2_lessons2.json
A2=$(jq '.stats.active' /tmp/ptest/pt2_lessons2.json)
DEP=$(jq '.stats.deprecated' /tmp/ptest/pt2_lessons2.json)
```
CHECK T3: `pt2_retract.sse` has a `"phase":"edit"` with `"kind":"retract"` then `"phase":"done"`; `A2 < A` (active count dropped) and `DEP >= 1`; the verification rule is GONE from `.selected[]` (which lists only `deprecated=0`). DB cross-check: `sqlite3 src-tauri/mike.db "SELECT deprecated FROM harness_lessons WHERE user_id='local-user' AND rule LIKE '%erif%';"` returns at least one `1`. If triage classifies the retract as a question (no `retract[]`) ⇒ retry ONCE with "retract / roll back the rule"; still nothing ⇒ SOFT finding.

**T4 — RETRACT APPLIES.** A new draft after retraction must NOT re-add the line.
```bash
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/pt2_postret.sse <<'JSON'
{"title":"PTEST-pt2-postret","model":"local:deepseek-v4-flash",
 "messages":[{"role":"user","content":"Draft a minimal affidavit for one Priya Nair confirming her residential address. Keep it short."}]}
JSON
CHAT_POSTRET=$(grep -o '"chatId":"[^"]*"' /tmp/ptest/pt2_postret.sse | head -1 | cut -d'"' -f4)
```
CHECK T4: concatenated text does NOT contain `verified at mumbai` (the deprecated rule no longer injected via `active_lessons_prompt`). A generic verification clause is fine; the specific Mumbai line must be absent. Present ⇒ retry ONCE; still present ⇒ SOFT finding (the deprecated rule is leaking → flag as a real regression candidate, note it).

**T5 — FALSE-TRIGGER RESISTANCE.** A plain legal question must NOT learn a rule.
```bash
Bq=$(curl -s "${AUTH[@]}" "$BASE/mike-feedback/lessons" | jq '.stats.total')
curl -s -N "${AUTH[@]}" "$BASE/mike-feedback/chat" \
  -F "message=What is the limitation period to file a written statement under the CPC?" \
  > /tmp/ptest/pt2_noise.sse
Aq=$(curl -s "${AUTH[@]}" "$BASE/mike-feedback/lessons" | jq '.stats.total')
```
CHECK T5: `pt2_noise.sse` ends `{"type":"complete","answered":true}` OR a `"phase":"skipped"` — NOT `"phase":"done"`; `Aq == Bq` (total lesson count UNCHANGED — no spurious rule from a question). A new rule from a pure question ⇒ false trigger ⇒ HARD finding (over-eager triage), note it.

**T6 — PER-ACCOUNT SCOPING.** The real account never saw any of this.
```bash
REAL_LESSONS=$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM harness_lessons WHERE user_id='$REAL_ACCT';")
REAL_DOCS=$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM documents WHERE user_id='$REAL_ACCT';")
REAL_CHATS=$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM chats WHERE user_id='$REAL_ACCT';")
echo "real-account during test → lessons:${REAL_LESSONS:-?} docs:${REAL_DOCS:-?} chats:${REAL_CHATS:-?}"
```
CHECK T6: `REAL_LESSONS == 0` AND `REAL_DOCS == 449` AND `REAL_CHATS == 5` (the verified baseline; our run wrote exclusively as `local-user`). Any drift ⇒ HARD finding (scoping breach) → STOP with a `## BLOCKER` (a real account was written). The PTEST chats we create are all `local-user`, so the real `chats` count must stay 5.

No `"type":"error"` event in any SSE file (`grep -l '"type":"error"' /tmp/ptest/pt2_*.sse` must be empty). Backend still alive after the loop: `kill -0 "$PT_PID"` succeeds and `curl -s -o /dev/null -w '%{http_code}' "$BASE/"` ≠ `000`.

### SKILLS / AGENTS / RESOLVERS
- `explorer` — re-grep any anchor that drifts (`mike_feedback.rs`, `harness/mod.rs`, `chat.rs:4623/4959`).
- `/human-logic` — completeness pass: retraction is reversible (deprecate, not delete), the false-trigger case is exercised, scoping is asserted both directions, the bypass guard runs BEFORE any destructive sweep, teardown runs on early exit via `trap`.
- `/ecc:rust-review` — only if a CHECK reveals a backend behaviour bug (e.g. T4 leak, T5 false trigger) worth filing; this loop does NOT edit code.
- Resolver: **rust-build-resolver** — only if the optional `cargo build --features rag` fallback in SETUP fails; hand it `/tmp/ptest/build.log` verbatim.

### BUILD / RUN / TEST (correct run-config)
- Run: from the MAIN repo, `set -a; source .env; set +a` then the isolated instance on `PORT=1599 MIKE_BYPASS_AUTH=true` against `DATABASE_URL=sqlite:src-tauri/mike.db` (prebuilt `./target/debug/mike`, else `cargo run --features rag`). LLM is DeepSeek CLOUD (`https://api.deepseek.com/v1`, `DEEPSEEK_API_KEY`), model `local:deepseek-v4-flash` — NOT ollama. NEVER run from a worktree (worktree DB is empty).
- This is a backend-API pressure test; no frontend/bot needed. (For reference only — the bot path is `telegram-bot/` with `MIKE_API_URL=http://localhost:1514`; PT2 hits the API directly on :1599.)
- Test: the six sub-test batteries above, in order, each with its CHECK + ONE imperative-rephrase retry on model-driven misses and ONE retry on a `SQLITE_BUSY`/lock blip.

### CHECKS (done-criteria — all must pass; SOFT findings recorded, not auto-fail)
1. T1 LEARN: `phase:"done"`, active count `+≥1`, a `verif` rule present.
2. T2 APPLY: fresh affidavit contains `verified at mumbai`.
3. T3 RETRACT: `kind:"retract"` + `phase:"done"`; active count drops; DB shows `deprecated=1` for the verif rule; rule absent from `.selected[]`.
4. T4 POST-RETRACT: new affidavit omits the Mumbai line.
5. T5 FALSE-TRIGGER: a pure question leaves total lesson count UNCHANGED (`answered:true`/`skipped`, never `done`).
6. T6 SCOPING: real account `88a19121…` stays lessons=0, docs=449, chats=5; every write was `local-user`.
7. Hygiene: zero `"type":"error"` events; backend alive at the end; any `SQLITE_BUSY` was a noted SOFT finding, not a panic.

### TEARDOWN (real DB — ALWAYS runs via the `trap` set in SETUP)
The teardown body and the EXIT trap were registered in SETUP (after `PT_PID` was known), so it fires on success, on `## BLOCKER`, and on any early exit. Run it explicitly once at the end too:
```bash
bash -c "$TEARDOWN"
```
CHECK teardown: `residue (want 0 0)` prints `0 0`; `real acct lessons (want 0)` prints `0`; `real acct docs/chats (want 449 5)` prints `449 5`; `kill -0 "$PT_PID"` now fails (instance stopped). Any non-zero residue ⇒ re-run the sweep; persistent residue ⇒ `## BLOCKER`. Any drift in the real-account counts ⇒ `## BLOCKER` (scoping breach).

### On success
This loop edits NO source, so DON'T touch the dirty `reconcile/full` working tree. Record the result on a dedicated branch so nothing lands on the user's active reconcile branch:
```bash
if git rev-parse --verify -q ptest/pt2-results >/dev/null; then
  git checkout ptest/pt2-results 2>/dev/null
else
  git checkout -b ptest/pt2-results 2>/dev/null
fi \
  && git commit --allow-empty -m "test(harness): PT2 memory learn/retract/apply + false-trigger pressure-test — all CHECKS green on local-user sandbox" \
  && git checkout - 2>/dev/null \
  || echo "NOTE: could not create ptest/pt2-results branch (dirty tree / detached HEAD) — skipping commit, results stand: PT2 all CHECKS green on local-user sandbox."
```

🧪 **You test:** From the MAIN repo run `set -a; source .env; set +a` then the bot (`cd telegram-bot && cargo run`, its `.env` has `MIKE_API_URL=http://localhost:1514`) or the app (`cd frontend && npm run dev`, http://localhost:3000) against your real account on `:1514`; tell Mike "from now on, end every affidavit with 'Verified at Mumbai'", ask for an affidavit (it should appear), then say "forget that rule" and ask again — the line should disappear, and the rule should toggle on the Personalization page (`GET /mike-feedback/lessons`).

---

## Pressure-test PT3 — Rubric behaviour on real defective pleadings + regression

**Goal:** Feed four known-defective Indian pleadings (each broken on exactly one axis) at the live backend and assert the rubric fires the matching HIGH risk in the streamed answer; prove normal chat / drafting / render-word still work; then wipe every artifact from the real DB.

> **Autonomy contract (read first).** Don't pause to ask — make every reversible call from this spec. SCL after each step: run CHECKS → on a red, hand the EXACT error to the resolver (`rust-build-resolver` for a server boot/compile fault, `explorer` to re-anchor a moved line), fix, re-run; cap **3 rounds/check**. STOP only on a real blocker (check red after 3 rounds / a genuinely ambiguous requirement / an unauthorized destructive action / **the sandbox backend won't boot or the cloud LLM is unreachable**) — write a `## BLOCKER` note (what failed, the exact error, what you tried) and halt. Never leave the sandbox backend running on exit. On success: empty marker commit + the `🧪 You test:` line.

---

### SETUP (test loop — MAIN repo, NEVER a worktree)

This loop runs entirely from the **main repo working tree** and exercises a **second, isolated backend** on its own port with `MIKE_BYPASS_AUTH=true`, so every write lands under `user_id='local-user'` and the real `88a19121` account is never touched. It shares the same `mike.db` (real corpus available for RAG). It changes **no source** — it only drives the running server.

```bash
set -u
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"
cd "$ROOT" || { echo "## BLOCKER: repo path missing"; exit 1; }
# Sanity: this MUST be the main working tree (worktrees have an empty DB).
case "$(git rev-parse --show-toplevel)" in
  "$ROOT") : ;;
  *) echo "## BLOCKER: not in the main repo working tree (worktree DB is empty)"; exit 1 ;;
esac
set -a; source .env; set +a            # DEEPSEEK_API_KEY, etc.

mkdir -p /tmp/ptest
: > /tmp/ptest/l3_docids.txt           # every created doc id is appended here for cleanup
REAL="88a19121-c6b1-4cc7-a421-5608ea4f0491"

# Pick a free port in 1599..1610 (000 == nothing listening). NEVER reuse :1514 (real-user, bypass-OFF).
PORT=""
for p in $(seq 1599 1610); do
  if [ "$(curl -s -m2 -o /dev/null -w '%{http_code}' "http://127.0.0.1:$p/")" = "000" ]; then PORT="$p"; break; fi
done
[ -n "$PORT" ] || { echo "## BLOCKER: no free port in 1599..1610"; exit 1; }
export PORT MIKE_BYPASS_AUTH=true RUST_BACKTRACE=1 DATABASE_URL="sqlite:src-tauri/mike.db"
BASE="http://127.0.0.1:$PORT"
AUTH=()                                # bypass mode → no Bearer header needed

# Capture the real-account baseline NOW (today = 449 docs / 5 chats); compared verbatim at teardown.
read REAL_DOCS REAL_CHATS < <(sqlite3 "$ROOT/src-tauri/mike.db" \
  "SELECT (SELECT count(*) FROM documents WHERE user_id='$REAL'), (SELECT count(*) FROM chats WHERE user_id='$REAL');" | tr '|' ' ')
echo "[setup] real-account baseline: docs=$REAL_DOCS chats=$REAL_CHATS (expected today: 449 / 5)"

# Write teardown.sh BEFORE the trap references it. It reads ids from disk, not from baked-in vars.
cat > /tmp/ptest/teardown.sh <<EOF
#!/usr/bin/env bash
BASE="$BASE"; ROOT="$ROOT"
# 1. API deletes for every doc id we recorded (defects + draft + render targets), dedup'd.
sort -u /tmp/ptest/l3_docids.txt 2>/dev/null | while read -r f did; do
  [ -n "\$did" ] && [ "\$did" != null ] && curl -s -X DELETE "\$BASE/document/\$did" >/dev/null 2>&1
done
# 2. Delete every PTEST- chat (regression chats included).
for cid in \$(sqlite3 "\$ROOT/src-tauri/mike.db" "SELECT id FROM chats WHERE user_id='local-user' AND title LIKE 'PTEST-%';" 2>/dev/null); do
  curl -s -X DELETE "\$BASE/chat/\$cid" >/dev/null 2>&1
done
# 3. Belt-and-suspenders DB sweep — scoped to local-user + PTEST tag; real account is unmatchable.
sqlite3 "\$ROOT/src-tauri/mike.db" <<'SQL' 2>/dev/null
DELETE FROM document_edits    WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%');
DELETE FROM document_versions WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%');
DELETE FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%';
DELETE FROM chats     WHERE user_id='local-user' AND title    LIKE 'PTEST-%';
SQL
rm -rf /tmp/ptest /tmp/ptest_mkdocx.py /tmp/ptest-backend.log
EOF
chmod +x /tmp/ptest/teardown.sh

# Boot the isolated sandbox backend. Prebuilt binary skips a rebuild.
if [ ! -x ./target/debug/mike ]; then
  echo "[setup] ./target/debug/mike missing — building from \$ROOT (cargo run --features rag binds 127.0.0.1:\$PORT)"
  # Gate any forced build through /ecc:rust-build; resolver rust-build-resolver on a red.
  cargo build --features rag || { echo "## BLOCKER: backend build failed"; exit 1; }
fi
nohup ./target/debug/mike > /tmp/ptest-backend.log 2>&1 & PT_PID=$!

# Trap kills the backend DIRECTLY (so it dies even if teardown.sh is gone) AND runs cleanup.
trap 'kill "$PT_PID" 2>/dev/null; bash /tmp/ptest/teardown.sh 2>/dev/null' EXIT

# BOUNDED readiness wait (≤60s). On timeout → dump log, BLOCKER, exit. NEVER an infinite spin.
ready=0
for _ in $(seq 1 60); do
  if [ "$(curl -s -m2 -o /dev/null -w '%{http_code}' "$BASE/")" != "000" ]; then ready=1; break; fi
  kill -0 "$PT_PID" 2>/dev/null || { echo "## BLOCKER: backend exited during boot"; tail -40 /tmp/ptest-backend.log; exit 1; }
  sleep 1
done
[ "$ready" = 1 ] || { echo "## BLOCKER: backend not ready on $BASE after 60s"; tail -40 /tmp/ptest-backend.log; exit 1; }

# Bypass-auth sanity gate: 200 ⇒ writing as local-user; 401 ⇒ NOT bypass → abort before any write.
LCODE=$(curl -s -m5 -o /dev/null -w '%{http_code}' "$BASE/mike-feedback/lessons")
[ "$LCODE" = 200 ] || { echo "## BLOCKER: bypass not active (GET /mike-feedback/lessons → $LCODE, want 200). Writes could hit the real account."; exit 1; }

# Cloud-LLM pre-flight: one cheap chat. error/zero-delta ⇒ LLM unreachable ⇒ BLOCKER (not 4 soft findings).
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' \
  -d '{"title":"PTEST-preflight","model":"local:deepseek-v4-flash","messages":[{"role":"user","content":"Reply with the single word: ok"}]}' \
  > /tmp/ptest/preflight.sse
if grep -q '"type": *"error"' /tmp/ptest/preflight.sse || ! grep -q '"type": *"content_delta"' /tmp/ptest/preflight.sse; then
  echo "## BLOCKER: DeepSeek cloud (https://api.deepseek.com/v1) unreachable or erroring — check DEEPSEEK_API_KEY / network."
  head -20 /tmp/ptest/preflight.sse; exit 1
fi
echo "[setup] ready on $BASE — bypass ✓, cloud LLM ✓"
```

> DeepSeek is **cloud** (`https://api.deepseek.com/v1`), model id `local:deepseek-v4-flash` — **not Ollama** (ignore the `local:` label). This loop changes no source; it only exercises the running server.

---

### TASK — verified anchors

The rubric is **prompt-embedded behaviour**, not a scorer endpoint: risk knowledge lives in `src/routes/chat.rs` (limitation / Ch.XXXVI CrPC→BNSS ~L1759, territorial jurisdiction ~L1397, Order VI r.15 verification ~L1691, S.138 NI Act 30-day window ~L1179, all under `const MIKE_SYSTEM_PROMPT` 1089–1465, injected verbatim at `chat.rs:4670`). So assertions are **content greps over streamed `content_delta` text**.

Hard gates confirmed in source:
- `POST /document` upload **must** send `cache=true` — `stream_chat_root` links a doc to the chat only when `content_hash IS NOT NULL` (`chat.rs:4440`), and `content_hash` is set only on cache uploads (`documents.rs`). Without it the model silently never sees the file.
- Attach via `messages[].files[].document_id` (`chat.rs:4425`), mapped to `doc-0`.
- Upload returns `{ "id": ... }` (`documents.rs:81`).
- SSE event names (exact, verified): `{"type":"chat_id","chatId":...}`, `{"type":"content_delta","text":...}`, `{"type":"doc_created"|"doc_created_start","document_id":...}`, `{"type":"doc_edited"|"doc_edited_start"}`, `{"type":"error","message":...}`.
- render-word returns `{"download_url":...}` like `/document/<id>/docx` (`documents.rs:512`).

Apply the `human-logic` skill while running: each defect→risk pair is an **independent named sub-test** (a miss is a *named soft finding*, not a crash); because DeepSeek is non-deterministic, **run each defect twice and fail only on 2/2 misses**; surface every miss explicitly (no silent skips); `local-user` + `PTEST-` tags are the exact cleanup boundary.

> **Known grep limitation (accepted):** `grep -o '"text":"[^"]*"'` truncates a delta that contains an escaped `\"`. This can only drop the *tail* of one delta; deltas are concatenated and each defect runs twice, so a real risk term still surfaces. Treat a single soft miss as model nondeterminism, **not** a code bug.

**Step 0 — .docx helper** (corpus `raw/*.docx.txt` are text dumps; upload needs real OOXML). Write `/tmp/ptest_mkdocx.py`:
```python
# python3 ptest_mkdocx.py <src.txt> <out.docx>
import sys
from docx import Document
d = Document()
for line in open(sys.argv[1], encoding="utf-8"):
    d.add_paragraph(line.rstrip("\n"))
d.save(sys.argv[2])
```

**Step 1 — synthesize 4 single-defect fixtures** into `/tmp/ptest/src_3{a,b,c,d}.txt`, each grounded on a real corpus form, with exactly one injected defect (keep all other axes clean so a miss is unambiguous):

| # | Defect injected (one axis only) | Base exemplar (`…/Legal Training Data/pipeline/raw/`) | Required term(s), case-insensitive |
|---|---|---|---|
| 3a | S.138 NI Act demand notice dated **45 days** after cheque dishonour | synth notice (tone from `affidavit condonation.docx.txt`) | `138` AND (`30 day` OR `limitation`) |
| 3b | Plaint filed in a court with **no territorial nexus** to cause/defendant | `Memorandum of Appeal.docx.txt` | `jurisdiction` |
| 3c | Affidavit with the **verification clause removed** | `REPLY ON BEHALF OF RESPONDENT.docx.txt` | `verif` (verification/verify) OR `order vi` |
| 3d | Appeal filed **2 years late, no condonation prayer** | `affidavit condonation.docx.txt` | `limitation` OR `condon` |

**Step 2 — defect battery** (upload `cache=true` → single review turn → grep streamed deltas; twice each):
```bash
declare -A REQ=( [3a]='138' [3b]='jurisdiction' [3c]='verif\|order vi' [3d]='limitation\|condon' )
declare -A REQ2=( [3a]='30 day\|limitation' )   # 3a needs both terms
FINDINGS=""
for f in 3a 3b 3c 3d; do
  python3 /tmp/ptest_mkdocx.py /tmp/ptest/src_$f.txt /tmp/ptest/PTEST-$f.docx
  hits=0
  for run in 1 2; do
    DID=$(curl -s "${AUTH[@]}" -F "file=@/tmp/ptest/PTEST-$f.docx;filename=PTEST-$f.docx" -F "cache=true" "$BASE/document" | jq -r .id)
    [ -n "$DID" ] && [ "$DID" != null ] && echo "$f $DID" >> /tmp/ptest/l3_docids.txt
    curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l3_${f}_r${run}.sse <<JSON
{"title":"PTEST-rubric-$f","model":"local:deepseek-v4-flash",
 "messages":[{"role":"user","content":"Review this pleading for filing risks. What is the single most serious defect?","files":[{"document_id":"$DID"}]}]}
JSON
    txt=$(grep -o '"text":"[^"]*"' /tmp/ptest/l3_${f}_r${run}.sse | tr 'A-Z' 'a-z')
    ok=1; echo "$txt" | grep -q "${REQ[$f]}" || ok=0
    [ -n "${REQ2[$f]:-}" ] && { echo "$txt" | grep -q "${REQ2[$f]}" || ok=0; }
    grep -q '"type": *"error"' /tmp/ptest/l3_${f}_r${run}.sse && ok=0
    hits=$((hits+ok))
  done
  if [ "$hits" -ge 1 ]; then echo "PASS $f ($hits/2)"
  else FINDINGS="$FINDINGS\n  - rubric: defect $f (${REQ[$f]}) NOT surfaced (0/2)"; fi
done
```

**Step 3 — regression triplet** (normal flows must still work):
```bash
# R1 plain Q&A
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d '{"title":"PTEST-reg-qa","model":"local:deepseek-v4-flash","messages":[{"role":"user","content":"In one paragraph, what is the limitation period to file a written statement under the CPC?"}]}' > /tmp/ptest/l3_reg_qa.sse
# R2 drafting → draft_document
curl -s -N "${AUTH[@]}" "$BASE/chat" -H 'Content-Type: application/json' -d '{"title":"PTEST-reg-draft","model":"local:deepseek-v4-flash","messages":[{"role":"user","content":"Draft a simple legal notice for recovery of Rs 50,000 unpaid invoice."}]}' > /tmp/ptest/l3_reg_draft.sse
DRAFTDOC=$(grep -o '"document_id":"[^"]*"' /tmp/ptest/l3_reg_draft.sse | head -1 | cut -d'"' -f4)
[ -n "$DRAFTDOC" ] && [ "$DRAFTDOC" != null ] && echo "draft $DRAFTDOC" >> /tmp/ptest/l3_docids.txt   # record for cleanup
# R3 render the draft to Word
[ -n "$DRAFTDOC" ] && curl -s "${AUTH[@]}" -X POST "$BASE/document/$DRAFTDOC/render-word" > /tmp/ptest/l3_render.json
RURL=$(jq -r .download_url /tmp/ptest/l3_render.json 2>/dev/null)
[ -n "$RURL" ] && [ "$RURL" != null ] && curl -s "${AUTH[@]}" "$BASE$RURL" -o /tmp/ptest/l3_render.docx
```

### SKILLS / AGENTS
`human-logic` (named-soft-findings, run-twice, visible misses) · `explorer` (re-anchor any moved chat.rs line) · resolver `rust-build-resolver` (only if the sandbox backend won't boot/compile). No source edits expected — if a CHECK forces one, gate it through `/ecc:rust-build`.

### CHECKS (crisp done-criteria)
1. **Backend up** the whole loop: `kill -0 $PT_PID` succeeds AND `grep -iE 'panic|panicked' /tmp/ptest-backend.log` is empty.
2. **Defects 3a–3d:** each ≥1/2 runs surfaces its required term(s) with no `"type":"error"`. A 0/2 is a recorded soft FINDING (printed below), not a hard fail — the loop still commits, but the finding is named.
3. **Each SSE well-formed:** every defect file contains `"type":"chat_id"` (with `"chatId"`) and ≥1 `"type":"content_delta"`.
4. **R1:** `l3_reg_qa.sse` has ≥1 `content_delta`, no `error`, mentions a period (`day`/`days`/`30`/`90`).
5. **R2:** `l3_reg_draft.sse` contains `"type":"doc_created"` (or `doc_created_start`); `DRAFTDOC` non-empty.
6. **R3:** `l3_render.json` has a `.download_url` matching `/document/.*/docx`; `l3_render.docx` is ≥1 byte and a valid zip (`unzip -l /tmp/ptest/l3_render.docx` exits 0).
7. **Print** the FINDINGS block (named misses) verbatim, then the regression PASS/FAIL line:
```bash
echo "=== FINDINGS ==="; printf "%b\n" "${FINDINGS:-  (none — all defects surfaced)}"
echo "=== REGRESSION ==="
grep -q '"type": *"content_delta"' /tmp/ptest/l3_reg_qa.sse && echo "R1 PASS" || echo "R1 FAIL"
grep -q '"type": *"doc_created' /tmp/ptest/l3_reg_draft.sse && [ -n "$DRAFTDOC" ] && echo "R2 PASS" || echo "R2 FAIL"
{ [ -s /tmp/ptest/l3_render.docx ] && unzip -l /tmp/ptest/l3_render.docx >/dev/null 2>&1 && echo "R3 PASS"; } || echo "R3 FAIL"
```

### RESIDUE + REAL-ACCOUNT ASSERTIONS (run BEFORE exit; teardown then sweeps)
```bash
# local-user PTEST residue must be 0 after the API deletes below; the trap re-sweeps belt-and-suspenders.
sort -u /tmp/ptest/l3_docids.txt | while read -r _ did; do [ -n "$did" ] && [ "$did" != null ] && curl -s -X DELETE "${AUTH[@]}" "$BASE/document/$did" >/dev/null; done
for cid in $(sqlite3 "$ROOT/src-tauri/mike.db" "SELECT id FROM chats WHERE user_id='local-user' AND title LIKE 'PTEST-%';"); do curl -s -X DELETE "${AUTH[@]}" "$BASE/chat/$cid" >/dev/null; done

echo "=== local-user residue (want 0 0) ==="
sqlite3 "$ROOT/src-tauri/mike.db" "SELECT
 (SELECT count(*) FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%'),
 (SELECT count(*) FROM chats     WHERE user_id='local-user' AND title    LIKE 'PTEST-%');"

echo "=== real account (must equal setup baseline $REAL_DOCS docs / $REAL_CHATS chats — expected 449 / 5) ==="
sqlite3 "$ROOT/src-tauri/mike.db" "SELECT
 (SELECT count(*) FROM documents WHERE user_id='$REAL'),
 (SELECT count(*) FROM chats     WHERE user_id='$REAL');"
```
If real-account counts drifted from `$REAL_DOCS`/`$REAL_CHATS`, that is a `## BLOCKER` (a write escaped the sandbox) — halt and report; do NOT run further sweeps.

### CLEANUP / TEARDOWN
Runs automatically via the `trap … EXIT` set in SETUP: kills `$PT_PID` directly, then `/tmp/ptest/teardown.sh` (idempotent; API deletes from `l3_docids.txt` → PTEST chat sweep → `local-user`+`PTEST-` scoped DB sweep → `rm -rf /tmp/ptest …`). The real `88a19121` account is never matched by any DELETE.

### On success
No source changed → an empty marker commit on the current branch (`reconcile/full`, **not** main — non-destructive) records the run:
```bash
git commit --allow-empty -m "test(pt3): rubric-behaviour pressure-test on defective pleadings + regression — gates green, real DB clean

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

🧪 **You test:** Open `http://localhost:3000`, upload a pleading missing its verification clause, and ask "what's the single most serious filing risk?" — Mike should flag the Order VI r.15 / verification defect; then ask the same about a 45-day-late S.138 notice and confirm it calls out the 30-day window.

---

## Pressure-test PT4 — Stability / load (no panics, SSE integrity)

**Goal:** Hammer the backend with concurrency + a large attached doc + repeated redline/memory turns; prove it never panics, every SSE stream stays valid JSON and terminates cleanly, multibyte/Devanagari is byte-safe, and all real-DB artifacts are scrubbed.

### SETUP — run from MAIN repo, NEVER a worktree
```bash
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"
cd "$ROOT"                              # test loop → MAIN tree (real DB). NO worktree, NO branch.
[ "$(git rev-parse --show-toplevel)" = "$ROOT" ] || { echo "BLOCKER: not in MAIN tree (worktree DB is empty → false greens). STOP."; exit 1; }
set -a; source .env; set +a             # loads .env: PORT=1514, DATABASE_URL=sqlite:src-tauri/mike.db, DEEPSEEK_API_KEY (DeepSeek = CLOUD https://api.deepseek.com/v1, NOT ollama)
mkdir -p /tmp/ptest
```
This is a pure pressure test: no code edits, no commits to product files. The only "branch" concern is that you are in the MAIN tree (its `src-tauri/mike.db` is the real history DB; a worktree's is empty → false greens).

### Autonomy
Don't pause to ask — make every reversible call from this spec + existing conventions; silence ≠ stop. SCL after each step: run CHECKS → on red hand the EXACT error to the resolver (**rust-build-resolver** for backend/build failures, **explorer** for "anchor moved / endpoint shape / SSE event name differs") → fix → re-run, cap **3 rounds/check**. STOP only on a real blocker (still red after 3 rounds / genuinely ambiguous / an unauthorized destructive action) → write a `## BLOCKER` note (what failed, exact error, what you tried) and halt. Never leave the isolated backend running or test rows in the DB on exit. Never commit red; never run a destructive SQL/`rm`/git command outside the `local-user`+`PTEST-` boundary defined below.

### TASK

**Why a second isolated instance (load-bearing decision):** live `:1514` is bypass-OFF — it returns 401 on `/mike-feedback/lessons`, so every write would land under the **real** user `88a19121-c6b1-4cc7-a421-5608ea4f0491` (currently 449 docs / 5 chats). Unacceptable for an unattended run. Boot **our own** instance on `:1599` with `MIKE_BYPASS_AUTH=true` (`src/auth/middleware.rs:27` → synthetic `user_id="local-user"` at :29), sharing the same `mike.db` (real corpus for RAG) but an isolated user namespace. `local-user` holds 0 docs / 0 lessons today → every artifact is `local-user` + `PTEST-` tagged → exact cleanup boundary.

**0 — Capture the real-account baseline, boot the sandbox backend, install a trap-EXIT teardown (set FIRST so even an early STOP cleans up):**
```bash
# Real-account baseline — CHECK 7 asserts this is never decremented (expected: 449).
BASELINE_DOCS=$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM documents WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491';")
echo "BASELINE_DOCS=$BASELINE_DOCS"   # sanity: should print 449

export PORT=1599 MIKE_BYPASS_AUTH=true DATABASE_URL="sqlite:src-tauri/mike.db"
BASE="http://127.0.0.1:1599"; AUTH=()          # bypass → no auth header needed
# Prebuilt binary (target/debug/mike, 79MB, present) skips a rebuild:
nohup ./target/debug/mike > /tmp/ptest-backend.log 2>&1 &
PT_PID=$!
echo "$PT_PID" > /tmp/ptest/pt.pid             # pidfile: teardown subshell reads this (env vars don't cross into `bash script.sh`)
trap 'bash /tmp/ptest_teardown.sh' EXIT         # teardown script written in step 0b
until [ "$(curl -s -m2 -o /dev/null -w '%{http_code}' "$BASE/")" != "000" ]; do
  kill -0 "$PT_PID" 2>/dev/null || { echo "BLOCKER: backend died on boot"; tail -40 /tmp/ptest-backend.log; exit 1; }
  sleep 1
done
```
If the prebuilt binary is stale/missing, build from the MAIN repo with `cargo build --features rag` (resolver: **rust-build-resolver**) and use `target/debug/mike`. Do **not** run `cargo run` for this loop — a foreground build complicates the panic-watch on the log.

**0a — `.docx` synth helper** (corpus `raw/*.docx.txt` are text dumps; `edit_document`/upload need real OOXML — `python-docx` 1.2.0 confirmed present). The README's `generate_test_doc.js` does NOT exist on disk; provide it:
```bash
cat > /tmp/ptest_mkdocx.py <<'PY'
import sys
from docx import Document
src, out = sys.argv[1], sys.argv[2]
d = Document()
for line in open(src, encoding="utf-8"):
    line = line.rstrip("\n")
    d.add_paragraph(line) if line.strip() else d.add_paragraph("")
d.save(out)
PY
python3 /tmp/ptest_mkdocx.py \
  "/Users/vedantmishra/Desktop/Legal Training Data/pipeline/raw/SETTLEMENT AGREEMENT(1).docx.txt" \
  /tmp/ptest/PTEST-settlement.docx
```

**0b — Teardown script** (referenced by the trap; runs on every exit incl. early STOP). The `$PT_PID` is interpolated NOW (heredoc is unquoted only for that one line via a sentinel) so the fresh subshell can kill the right process:
```bash
PT_PID_VAL="$(cat /tmp/ptest/pt.pid 2>/dev/null)"
cat > /tmp/ptest_teardown.sh <<SH
ROOT="/Users/vedantmishra/Desktop/mike aur donna main git"; cd "\$ROOT"
BASE="http://127.0.0.1:1599"
# API deletes for any captured ids (best-effort)
for j in /tmp/ptest/l4_up_*.json /tmp/ptest/l4_large.json; do
  id=\$(jq -r '.id // empty' "\$j" 2>/dev/null); [ -n "\$id" ] && curl -s -X DELETE "\$BASE/document/\$id" >/dev/null 2>&1; done
for cid in \$(sqlite3 src-tauri/mike.db "SELECT id FROM chats WHERE user_id='local-user' AND title LIKE 'PTEST-%';" 2>/dev/null); do
  curl -s -X DELETE "\$BASE/chat/\$cid" >/dev/null 2>&1; done
# Belt-and-suspenders DB sweep, scoped to local-user + PTEST so the real account is untouchable:
sqlite3 src-tauri/mike.db 2>/dev/null <<'SQL'
DELETE FROM document_edits    WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%');
DELETE FROM document_versions WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%');
DELETE FROM document_markdown_versions WHERE document_id IN (SELECT id FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%');
DELETE FROM documents      WHERE user_id='local-user' AND filename LIKE 'PTEST-%';
DELETE FROM chats          WHERE user_id='local-user' AND title    LIKE 'PTEST-%';
DELETE FROM harness_lessons  WHERE user_id='local-user';
DELETE FROM harness_feedback WHERE user_id='local-user';
DELETE FROM harness_features WHERE user_id='local-user';
DELETE FROM harness_state    WHERE user_id='local-user';
SQL
# Stop the sandbox backend (env vars don't cross into this subshell → kill by interpolated PID + pidfile):
kill "${PT_PID_VAL:-0}" 2>/dev/null
kill "\$(cat /tmp/ptest/pt.pid 2>/dev/null)" 2>/dev/null
SH
```

**1 — Battery 4a: concurrency** (12 chat SSE streams + 4 uploads, all in flight at once). Includes a **Devanagari/multibyte** probe to catch UTF-8 boundary slicing in the SSE chunker. `--max-time 120` on every stream so a stalled cloud-DeepSeek connection can't wedge the unattended run:
```bash
for i in $(seq 1 12); do
 ( curl -s -N --max-time 120 "$BASE/chat" -H 'Content-Type: application/json' \
    -d "{\"title\":\"PTEST-conc-$i\",\"model\":\"local:deepseek-v4-flash\",\"messages\":[{\"role\":\"user\",\"content\":\"Concurrency probe $i — जमानत के तीन आधार बताइए (anticipatory bail). Reply in Hindi + English.\"}]}" \
    > /tmp/ptest/l4_conc_$i.sse ) &
done
for i in $(seq 1 4); do
 ( curl -s --max-time 120 -F "file=@/tmp/ptest/PTEST-settlement.docx;filename=PTEST-conc-$i.docx" -F "cache=true" \
    "$BASE/document" > /tmp/ptest/l4_up_$i.json ) &
done
wait
```
(Model id `local:deepseek-v4-flash` is the chat label for the DeepSeek **cloud** backend — ignore the `local:` prefix; it is NOT ollama. `cache=true` is mandatory: without it `content_hash` is NULL and the chat-link UPDATE silently no-ops at `chat.rs:~4440`, so the model never sees the file — the upload field is `file`, the cache flag is `cache`, both per `documents.rs:113-137`.)

**2 — Battery 4b: large doc.** NOTE — the body limit is **50 GB** (`DefaultBodyLimit::max(50_usize * 1024 * 1024 * 1024)`, `documents.rs:42`), NOT a small cap. A ~5 MB doc therefore MUST be accepted (200); a 413 here would be a real regression in the limit. The HTTP code goes to a separate `-w` capture and the body to `-o`, so the body file is clean JSON (no GNU `head -c -1` newline-strip — that flag errors on macOS and isn't needed here):
```bash
python3 - <<'PY'
from docx import Document
d=Document()
para=("That the Plaintiff most respectfully submits the following grounds, "
      "which are repeated and reiterated as material and relevant. "
      "उक्त वादी सादर निवेदन करता है। ")*40
for i in range(1500): d.add_paragraph(f"{i+1}. {para}")
d.save("/tmp/ptest/PTEST-large.docx")
PY
ls -l /tmp/ptest/PTEST-large.docx
LCODE=$(curl -s --max-time 180 -o /tmp/ptest/l4_large.json -w '%{http_code}' \
  -F "file=@/tmp/ptest/PTEST-large.docx;filename=PTEST-large.docx" -F "cache=true" "$BASE/document")
LID=$(jq -r '.id // empty' /tmp/ptest/l4_large.json 2>/dev/null)
echo "LCODE=$LCODE LID=$LID"
```

**3 — Battery 4c: repeated redline+memory churn** (5 redline turns that elicit `edit_document` against the large doc + 5 `/mike-feedback` memory writes — back-to-back to surface state/lock/connection-leak issues). All streams `--max-time` bounded:
```bash
# Fall back to a concurrency upload id if 4b returned non-200 so churn still runs.
[ -n "$LID" ] || LID=$(jq -r '.id // empty' /tmp/ptest/l4_up_1.json 2>/dev/null)
for n in $(seq 1 5); do
  # redline turn against the large doc (forces edit_document; .docx-only tool, builtin_tools.rs:836)
  curl -s -N --max-time 180 "$BASE/chat" -H 'Content-Type: application/json' -d @- > /tmp/ptest/l4_churn_red_$n.sse <<JSON
{"title":"PTEST-churn-red-$n","model":"local:deepseek-v4-flash",
 "messages":[{"role":"user","content":"Risk-review this Word file and apply your top fix as tracked changes using edit_document now.","files":[{"document_id":"$LID"}]}]}
JSON
  # memory write (multipart, field 'message' — mike_feedback.rs:58)
  curl -s -N --max-time 120 "$BASE/mike-feedback/chat" \
    -F "message=PTEST churn $n: from now on always close affidavits with a verification clause." \
    > /tmp/ptest/l4_churn_mem_$n.sse
done
```

**4 — Battery 4d: SSE integrity probe** (long stream; every `data:` line must be valid JSON and the stream must terminate cleanly, not get `--max-time`-killed):
```bash
curl -s -N --max-time 120 "$BASE/chat" -H 'Content-Type: application/json' \
  -d '{"title":"PTEST-sse","model":"local:deepseek-v4-flash","messages":[{"role":"user","content":"Write a 6-paragraph note on Section 138 NI Act, alternating English and Hindi (हिंदी) paragraphs, so the stream is long and multibyte."}]}' \
  > /tmp/ptest/l4_sse.sse
SSE_EXIT=$?
```

**5 — Panic watch** (whole-loop window):
```bash
grep -inE 'panic|thread .main. panicked|RUST_BACKTRACE|SIGABRT|fatal runtime' /tmp/ptest-backend.log > /tmp/ptest/l4_panics.txt || true
```

### Skills / agents
- **explorer** — if any endpoint shape/SSE event name differs from this spec (anchors verified, but the model is non-deterministic).
- **rust-build-resolver** — only if the prebuilt binary is missing and `cargo build --features rag` goes red.
- **/human-logic** — applied to the assertions below: every failure surfaces a real message (no silent drop / infinite spinner; every stream is `--max-time` bounded), the large-doc cap is read from the real `DefaultBodyLimit` (50 GB) not an arbitrary number, and teardown runs on EVERY exit path (trap + pidfile), so no test rows and no orphaned backend leak past the session.

### CHECKS (crisp done-criteria — each is PASS/FAIL on the captured files)

> SSE NOTE (verified against `src/routes/chat.rs`): the **`/chat`** SSE stream has **no terminal `"type":"complete"` or `"type":"done"` event** — it ends by closing the channel. So a healthy `/chat` stream is recognised by *clean curl exit (0) + ≥1 `"type":"content_delta"` + no `"type":"error"` after the last delta*, NOT by a terminal token. The **`/mike-feedback/chat`** stream DOES emit `"type":"complete"` (and `"type":"harness"` phases) — its checks keep the complete-token assertion.

1. **No panic:** `/tmp/ptest/l4_panics.txt` is **empty** AND `kill -0 "$(cat /tmp/ptest/pt.pid)"` succeeds (process still alive after the whole battery). Any hit ⇒ FAIL, attach the matching log lines to the BLOCKER.
2. **Concurrency holds:** all 12 `l4_conc_*.sse` are non-empty, each contains ≥1 `"type":"content_delta"`, and **none** contains `"type":"error"`. (Terminal = stream closed; an empty file ⇒ FAIL = dropped/stalled connection under load. Spot-check one file ends mid-JSON-line ⇒ FAIL = truncation.) All 4 `l4_up_*.json` carry a non-null `.id`.
3. **Large doc:** `LCODE == 200` and `LID` non-null (50 GB limit ⇒ 5 MB is well within bounds; this is the expected path). A `413` here ⇒ FAIL (limit regressed); a `500`/hang/empty body ⇒ FAIL. Record the observed code.
4. **Churn stable:** all 5 `l4_churn_red_*.sse` are non-empty with ≥1 `content_delta` and no `"type":"error"`; ≥3/5 emit a `"type":"tool_call_start"` or `"type":"doc_edited"`/`"type":"doc_edited_start"` event (model-driven, so allow misses — 0/5 ⇒ FAIL, the apply path is broken under churn). All 5 `l4_churn_mem_*.sse` are non-empty, contain a final `"type":"complete"`, and no `"type":"error"`.
5. **SSE integrity + multibyte:** `SSE_EXIT == 0` (stream terminated, not `--max-time`-killed) and **every** `data:` line in `l4_sse.sse` parses as JSON:
   ```bash
   BAD=0; while IFS= read -r line; do
     case "$line" in "data: "*) echo "${line#data: }" | jq . >/dev/null 2>&1 || BAD=$((BAD+1));; esac
   done < /tmp/ptest/l4_sse.sse; echo "BAD=$BAD"
   ```
   `BAD == 0` required (a non-zero count means a multibyte char was sliced across an SSE frame → UTF-8 corruption). Devanagari present in `l4_sse.sse` (`grep -aq $'ऀ' /tmp/ptest/l4_sse.sse` or visually) confirms the multibyte path was exercised.
6. **Backend still answers post-load:** `curl -s -o /dev/null -w '%{http_code}' "$BASE/"` ≠ `000`.
7. **Cleanup verified (real DB untouched):**
   ```bash
   sqlite3 src-tauri/mike.db "SELECT
     (SELECT count(*) FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%'),
     (SELECT count(*) FROM chats     WHERE user_id='local-user' AND title    LIKE 'PTEST-%'),
     (SELECT count(*) FROM harness_lessons WHERE user_id='local-user');"
   # all three MUST be 0 after teardown
   REAL_NOW=$(sqlite3 src-tauri/mike.db "SELECT count(*) FROM documents WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491';")
   [ "$REAL_NOW" = "$BASELINE_DOCS" ] || echo "BLOCKER: real account changed ($BASELINE_DOCS → $REAL_NOW) — DB boundary breached, STOP."
   # REAL_NOW MUST equal BASELINE_DOCS (=449); never decremented.
   ```

### PTEST tagging + teardown
Every chat `title` and uploaded `filename` is prefixed `PTEST-`; all fixtures live under `/tmp/ptest/`; all writes are `local-user`. Teardown (step 0b) runs via `trap … EXIT` on **every** exit — success, STOP, or crash — doing API deletes + a `local-user`+`PTEST-`-scoped DB sweep (the real `88a19121…` account is never in any `WHERE`) and killing the sandbox backend by recorded PID/pidfile. Final residue + real-account check is CHECK 7. After CHECK 7 passes, remove temp files:
```bash
rm -rf /tmp/ptest /tmp/ptest_mkdocx.py /tmp/ptest-backend.log /tmp/ptest_teardown.sh
```

### On success
This is a test-only loop — no product files changed, so **no commit**. If you authored a helper or a tiny fixture you want to keep, branch off the current base first (`git switch -c ptest/pt4-stability`) and commit only the harness scripts with a clear message; otherwise leave the tree clean. Never commit the prebuilt binary or `mike.db`.

🧪 **You test:** boot the sandbox (`cd "/Users/vedantmishra/Desktop/mike aur donna main git"; set -a; source .env; set +a; PORT=1599 MIKE_BYPASS_AUTH=true ./target/debug/mike`), fire ~12 concurrent `/chat` curls with Hindi prompts + the 5 MB upload, then `grep -i panic /tmp/ptest-backend.log` (should be empty) and confirm `sqlite3 src-tauri/mike.db "SELECT count(*) FROM documents WHERE user_id='local-user' AND filename LIKE 'PTEST-%';"` returns `0` after teardown — and that `SELECT count(*) … WHERE user_id='88a19121-c6b1-4cc7-a421-5608ea4f0491'` is still `449`.
