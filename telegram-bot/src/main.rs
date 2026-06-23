//! Mike aur Donna — Telegram bot scaffold (Loop D, Session 5 + 6).
//!
//! A *thin* Teloxide front-end over the existing axum `/chat` SSE endpoint.
//! It does NOT re-implement the agent: every message is forwarded to the
//! backend running on `localhost:3001`, the streamed reply is accumulated,
//! and the final text is sent back to the Telegram chat.
//!
//! Session 6b adds real clarifying-question handling: when the backend emits a
//! `client_tool_request` for `ask_clarifying_questions`, the bot renders each
//! question's options as a Telegram inline keyboard, collects the user's taps
//! via a callback handler, and POSTs `/chat/client-tool-result` to resume the
//! still-open SSE stream. The callback handler bridges to the in-flight
//! `call_chat` through a per-chat oneshot channel (`Pending`).
//!
//! Config (env vars):
//!   TELEGRAM_BOT_TOKEN  (required)  token from @BotFather
//!   MIKE_API_URL        (optional)  backend base URL, default http://localhost:3001
//!   MIKE_SESSION_TOKEN  (optional)  Bearer token for /chat; leave empty if the
//!                                   backend runs with MIKE_BYPASS_AUTH=true
//!   MIKE_MODEL          (optional)  model id to pass through; omitted → backend default

use std::collections::{HashMap, HashSet};
use std::env;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::{json, Value};
use teloxide::{
    net::Download,
    prelude::*,
    types::{
        CallbackQuery, ChatAction, FileId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile,
        MessageId,
    },
    utils::command::BotCommands,
};
use tokio::sync::{oneshot, Mutex};

/// Telegram rejects messages longer than 4096 characters.
const TELEGRAM_LIMIT: usize = 4096;

/// Keep at most this many messages (user + assistant) per chat, dropping the
/// oldest. ~20 turns of back-and-forth context handed to the backend each call.
const MAX_HISTORY_MESSAGES: usize = 40;

/// `result` payload telling the backend the user declined to answer and just
/// wants it to proceed with its best guess (mirrors the frontend's "proceed"
/// path). `format_clarifying_answers` treats `proceed:true` as "draft now".
const PROCEED_RESULT: &str = "{\"answers\":[],\"proceed\":true}";

/// How long to wait for the user to answer before giving up and proceeding.
/// Kept comfortably under the backend's 180s tool-call timeout so our
/// `client-tool-result` POST always wins the race.
const CLARIFY_WAIT: Duration = Duration::from_secs(170);

/// If the backend sends no data for this long mid-stream, give up instead of
/// hanging forever. It's an *idle* timeout — a normally-streaming answer resets
/// it on every chunk — so it only trips on a genuine stall.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Case Prep analysis runs several LLM agents in parallel and can stay quiet
/// between findings, so it gets a longer idle window than a plain chat turn.
const ANALYZE_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Most files a single `/analyze` run accepts. A realistic case bundle sent
/// over Telegram is a handful of documents; this caps an accidental dump so the
/// run (and the cloud-model bill) stays bounded. Over it → the user is told the
/// limit and asked to /analyze or /reset, never silently truncated.
const MAX_FILES: usize = 20;

/// Per-file size ceiling. This is the Telegram Bot API's own `getFile` download
/// limit (20 MB) — a bot literally cannot fetch a larger file — so it is the
/// real ceiling, not an arbitrary round number. Over it → the file is rejected
/// with its actual size, not dropped silently.
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// TL;DR length sent as each delivered .docx's caption (Telegram caps captions
/// at 1024 chars; we stay well under). The full text ships inside the .docx.
const TLDR_CHARS: usize = 600;

/// One turn in a chat's conversation history, mirroring the `/chat/` message shape.
#[derive(Clone)]
struct ChatMsg {
    role: String,
    content: String,
}

/// Per-chat conversation history, shared across all handler invocations.
type History = Arc<Mutex<HashMap<ChatId, Vec<ChatMsg>>>>;

/// Chats already told their history hit the cap, so we warn once per
/// conversation instead of on every message past the limit. Cleared by `/reset`.
type Warned = Arc<Mutex<HashSet<ChatId>>>;

/// In-flight clarifying-question wizards, keyed by chat. The callback handler
/// looks up the live wizard for a chat and, when the last question is answered,
/// hands the assembled result back to the waiting `call_chat` via `tx`.
type Pending = Arc<Mutex<HashMap<ChatId, PendingClarify>>>;

/// Chats with a `handle_text` turn currently running, so a second plain message
/// for the same chat is rejected instead of running a *concurrent* `call_chat`
/// that would interleave the shared history Vec. A plain `std::sync::Mutex` (not
/// the async one) so the RAII `InFlightGuard` can release it on `Drop` without
/// awaiting; the lock is only ever held for the insert/remove, never across an
/// `.await`.
type InFlight = Arc<StdMutex<HashSet<ChatId>>>;

/// RAII guard: while held, this chat is marked in-flight; dropping it (on any
/// exit path of `handle_text`, including `?` early returns) clears the mark.
struct InFlightGuard {
    set: InFlight,
    chat_id: ChatId,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.chat_id);
        }
    }
}

/// Try to mark `chat_id` in-flight. Returns the guard if this is the first turn
/// for the chat, or `None` if a turn is already running (caller should reject).
fn try_acquire_inflight(set: &InFlight, chat_id: ChatId) -> Option<InFlightGuard> {
    let mut guard = set.lock().expect("in-flight mutex poisoned");
    if guard.insert(chat_id) {
        Some(InFlightGuard {
            set: set.clone(),
            chat_id,
        })
    } else {
        None
    }
}

/// One generated draft surfaced by a `doc_created` event during a stream.
struct DocRef {
    filename: String,
    document_id: String,
}

/// Runtime configuration injected into every handler.
#[derive(Clone)]
struct BotConfig {
    api_url: String,
    session_token: String,
    model: Option<String>,
}

/// Sentinel error text meaning "the backend rejected our token (401)".
/// `handle_text` matches on it to drop the stale per-chat token and nudge the
/// user to `/login` again, instead of surfacing a raw "backend returned 401".
const AUTH_ERR: &str =
    "🔐 Session expired or you're not logged in — send /login <PIN> to continue.";

/// Per-Telegram-chat Mike session token, set by `/login <PIN>`. A newtype so
/// dptree injects it distinctly from the other `ChatId → String` maps below.
#[derive(Clone, Default)]
struct Tokens(Arc<Mutex<HashMap<ChatId, String>>>);

/// Telegram chat → the Mike `chat_id` it's attached to, so messages land in the
/// same persisted thread as the laptop. Set by resuming via `/chats`, or
/// captured from the stream's `chat_id` event when a fresh thread is created.
#[derive(Clone, Default)]
struct ActiveChats(Arc<Mutex<HashMap<ChatId, String>>>);

/// Per-chat cache of `document_id → filename` from the last `/docs` listing, so
/// a tapped document is delivered to the phone with its real name.
#[derive(Clone, Default)]
struct DocNames(Arc<Mutex<HashMap<ChatId, HashMap<String, String>>>>);

/// One file the user has sent and that is waiting to be run through Case Prep.
/// We hold only Telegram's `file_id` (and metadata) here — the bytes are fetched
/// from Telegram lazily when `/analyze` actually runs, so a big buffer of staged
/// files costs almost nothing.
#[derive(Clone)]
struct PendingFile {
    file_id: FileId,
    filename: String,
    size: u64,
}

/// Per-chat queue of files staged by document/photo messages, drained by
/// `/analyze`. Cleared by `/reset` (so a user can abandon a staged batch).
#[derive(Clone, Default)]
struct PendingFiles(Arc<Mutex<HashMap<ChatId, Vec<PendingFile>>>>);

/// Chats with a Case Prep run currently in flight. Used to reject a *second*
/// concurrent `/analyze` for the same chat without blocking plain chat messages
/// or other chats — the run itself executes on a spawned task. A plain
/// `std::sync::Mutex` so the insert/remove never crosses an `.await`.
#[derive(Clone, Default)]
struct AnalyzeJobs(Arc<StdMutex<HashSet<ChatId>>>);

// ---------------------------------------------------------------------------
// Clarifying-question model (mirrors the backend `ask_clarifying_questions`
// tool schema and the `/chat/client-tool-result` contract).
// ---------------------------------------------------------------------------

/// One selectable option for a clarifying question.
#[derive(Clone)]
struct ClarifyOption {
    label: String,
    description: Option<String>,
}

/// One clarifying question with its options.
#[derive(Clone)]
struct ClarifyQuestion {
    header: String,
    question: String,
    multi_select: bool,
    options: Vec<ClarifyOption>,
}

/// A parsed `client_tool_request` for `ask_clarifying_questions`.
struct ToolRequest {
    request_id: String,
    questions: Vec<ClarifyQuestion>,
}

/// What the callback (or `/cancel`) hands back to the waiting `call_chat`.
enum ClarifyOutcome {
    /// User answered: the JSON `result` string to POST back to the backend.
    Answered(String),
    /// User tapped "proceed anyway": let the model draft with its best guess.
    Proceed,
    /// User ran `/cancel`: abort this turn, don't resume the backend.
    Cancelled,
}

/// Result of handling a clarifying request inside `call_chat`.
enum ClarifyControl {
    /// Tool result posted — keep reading the (resumed) SSE stream.
    Continue,
    /// User cancelled — stop reading and discard the turn.
    Stop,
}

/// Live state of one chat's clarifying-question wizard.
struct PendingClarify {
    questions: Vec<ClarifyQuestion>,
    /// Index of the question currently being answered.
    current: usize,
    /// Selected option labels for each *completed* question, in order.
    collected: Vec<Vec<String>>,
    /// Toggled option indices for the *current* (multi-select) question.
    selected: Vec<usize>,
    /// The message holding the live inline keyboard (edited in place).
    keyboard_msg: MessageId,
    /// Wakes the in-flight `call_chat` once the wizard finishes.
    tx: oneshot::Sender<ClarifyOutcome>,
}

/// A decoded inline-button tap.
enum CbAction {
    /// Tap on option `o_idx` of question `q_idx`.
    Option { q_idx: usize, o_idx: usize },
    /// "Done" for a multi-select question `q_idx`.
    Done { q_idx: usize },
    /// "Skip / proceed anyway".
    Proceed,
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "Mike aur Donna assistant")]
enum Command {
    #[command(description = "show the welcome message")]
    Start,
    #[command(description = "show what this bot can do")]
    Help,
    #[command(description = "clear this chat's conversation history")]
    Reset,
    #[command(description = "cancel a pending clarifying question")]
    Cancel,
    #[command(description = "log in with your Mike PIN: /login <PIN>")]
    Login(String),
    #[command(description = "log out — clear this chat's saved session")]
    Logout,
    #[command(description = "list & resume one of your saved conversations")]
    Chats,
    #[command(description = "start a fresh conversation")]
    New,
    #[command(description = "list & download your documents")]
    Docs,
    #[command(description = "run Case Prep on the files you've sent: analysis + brief, strategy memo, list of dates & annexure index as .docx")]
    Analyze,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let token = match env::var("TELEGRAM_BOT_TOKEN") {
        Ok(t) if !t.trim().is_empty() => t,
        _ => {
            eprintln!(
                "TELEGRAM_BOT_TOKEN is not set. Create a bot with @BotFather and \
                 export the token, e.g.\n  export TELEGRAM_BOT_TOKEN=123456:ABC-..."
            );
            std::process::exit(1);
        }
    };

    let cfg = BotConfig {
        api_url: env::var("MIKE_API_URL").unwrap_or_else(|_| "http://localhost:3001".into()),
        session_token: env::var("MIKE_SESSION_TOKEN").unwrap_or_default(),
        model: env::var("MIKE_MODEL").ok().filter(|s| !s.trim().is_empty()),
    };

    tracing::info!("Mike telegram bot starting — backend at {}", cfg.api_url);
    if cfg.session_token.is_empty() {
        tracing::warn!(
            "MIKE_SESSION_TOKEN is empty — users must authenticate in-chat with \
             /login <PIN>, unless the backend runs with MIKE_BYPASS_AUTH=true."
        );
    }

    let bot = Bot::new(token);

    // Consistent branding across every self-hosted install: the slash-command
    // menu and the profile blurbs are set from code at startup, so each cloned
    // bot looks the same with no manual BotFather steps. (The avatar is the one
    // exception — it's BotFather-only; see telegram-bot/assets/README.md.)
    // Failures are non-fatal (e.g. Telegram rate limits), so log and carry on.
    if let Err(e) = bot.set_my_commands(Command::bot_commands()).await {
        tracing::warn!("could not set the command menu: {e}");
    }
    if let Err(e) = bot
        .set_my_short_description()
        .short_description("Donna, your Mike aur Donna legal assistant on Telegram.")
        .await
    {
        tracing::warn!("could not set the short description: {e}");
    }
    if let Err(e) = bot
        .set_my_description()
        .description(
            "Donna is the Telegram half of Mike aur Donna, your local legal assistant. \
             Send /login <PIN> to connect to your account, then ask a question, use /chats \
             to continue a conversation from your laptop, or /docs to download a draft.",
        )
        .await
    {
        tracing::warn!("could not set the description: {e}");
    }

    let history: History = Arc::new(Mutex::new(HashMap::new()));
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let warned: Warned = Arc::new(Mutex::new(HashSet::new()));
    let in_flight: InFlight = Arc::new(StdMutex::new(HashSet::new()));
    let tokens = Tokens::default();
    let active = ActiveChats::default();
    let docnames = DocNames::default();
    let pending_files = PendingFiles::default();
    let analyze_jobs = AnalyzeJobs::default();

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(handle_command),
                )
                // A document or photo (e.g. a scan) → stage it for /analyze.
                // Checked before the plain-text fallback so file uploads aren't
                // bounced as "text only".
                .branch(
                    dptree::filter(|m: Message| {
                        m.document().is_some() || m.photo().is_some()
                    })
                    .endpoint(handle_document),
                )
                .branch(dptree::endpoint(handle_text)),
        )
        .branch(Update::filter_callback_query().endpoint(handle_callback));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![
            cfg, history, pending, warned, in_flight, tokens, active, docnames,
            pending_files, analyze_jobs
        ])
        // Process every update concurrently (not serialized per chat) so a
        // button tap can be handled *while* its `handle_text` is still parked
        // waiting for that tap — otherwise the two would deadlock.
        .distribution_function(|_| None::<()>)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

/// Slash commands. Most are state-only; `/login`, `/chats` and `/docs` hit the
/// backend (delegated to helpers below).
async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    cfg: BotConfig,
    history: History,
    pending: Pending,
    warned: Warned,
    tokens: Tokens,
    active: ActiveChats,
    docnames: DocNames,
    pending_files: PendingFiles,
    analyze_jobs: AnalyzeJobs,
) -> ResponseResult<()> {
    match cmd {
        Command::Start => {
            bot.send_message(
                msg.chat.id,
                "👋 Mike aur Donna legal assistant.\n\n\
                 First, log in so I act as you and sync with your laptop:\n\
                 /login <your PIN>\n\n\
                 Then send a question, or use /chats to continue a laptop \
                 conversation and /docs to pull a draft to your phone. \
                 See /help for the full list.",
            )
            .await?;
        }
        Command::Help => {
            bot.send_message(
                msg.chat.id,
                "Send any message and I'll forward it to your Mike backend and \
                 stream the answer back. Long answers are split across messages. \
                 If I need a decision before drafting, I'll show tappable buttons.\n\n\
                 /login <PIN> — log in (syncs me with your laptop account)\n\
                 /logout — clear this chat's saved session\n\
                 /chats — list & resume a saved conversation\n\
                 /new — start a fresh conversation\n\
                 /docs — list & download your documents\n\
                 /reset — clear this chat's in-memory history\n\
                 /cancel — drop a pending clarifying question\n\
                 /start — welcome message",
            )
            .await?;
        }
        Command::Reset => {
            history.lock().await.remove(&msg.chat.id);
            warned.lock().await.remove(&msg.chat.id);
            let staged = pending_files.0.lock().await.remove(&msg.chat.id);
            let text = match staged {
                Some(files) if !files.is_empty() => format!(
                    "🧹 Cleared in-memory history and discarded {} staged file(s).",
                    files.len()
                ),
                _ => "🧹 In-memory history cleared.".to_string(),
            };
            bot.send_message(msg.chat.id, text).await?;
        }
        Command::Cancel => {
            let entry = pending.lock().await.remove(&msg.chat.id);
            let text = match entry {
                Some(entry) => {
                    // Clear the dangling keyboard and wake the parked call_chat.
                    let _ = bot
                        .edit_message_text(msg.chat.id, entry.keyboard_msg, "🚫 Cancelled.")
                        .await;
                    let _ = entry.tx.send(ClarifyOutcome::Cancelled);
                    "🚫 Cancelled — tell me what you'd like to do instead."
                }
                None => "Nothing to cancel right now.",
            };
            bot.send_message(msg.chat.id, text).await?;
        }
        Command::Login(pin) => cmd_login(&bot, &msg, &cfg, &tokens, &pin).await?,
        Command::Logout => {
            tokens.0.lock().await.remove(&msg.chat.id);
            active.0.lock().await.remove(&msg.chat.id);
            bot.send_message(
                msg.chat.id,
                "👋 Logged out. Send /login <PIN> when you want to continue.",
            )
            .await?;
        }
        Command::New => {
            history.lock().await.remove(&msg.chat.id);
            warned.lock().await.remove(&msg.chat.id);
            active.0.lock().await.remove(&msg.chat.id);
            bot.send_message(
                msg.chat.id,
                "🆕 Started a new conversation. It'll appear on your laptop once \
                 you send your first message.",
            )
            .await?;
        }
        Command::Chats => {
            let cfg2 = with_token(&cfg, resolve_token(&tokens, msg.chat.id, &cfg).await);
            cmd_chats(&bot, msg.chat.id, &cfg2).await?;
        }
        Command::Docs => {
            let cfg2 = with_token(&cfg, resolve_token(&tokens, msg.chat.id, &cfg).await);
            cmd_docs(&bot, msg.chat.id, &cfg2, &docnames).await?;
        }
        Command::Analyze => {
            let chat_id = msg.chat.id;
            // Nothing staged → guide the user instead of running an empty case.
            let count = pending_files
                .0
                .lock()
                .await
                .get(&chat_id)
                .map_or(0, Vec::len);
            if count == 0 {
                bot.send_message(
                    chat_id,
                    "📎 Send me the case files first (as documents, or photos of a scan), \
                     then run /analyze. I take .docx, .pdf and image scans.",
                )
                .await?;
                return Ok(());
            }
            // One Case Prep run per chat at a time. `insert` returns false if a
            // run is already claimed — reject without touching the staged files.
            let claimed = analyze_jobs
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(chat_id);
            if !claimed {
                bot.send_message(
                    chat_id,
                    "⏳ A Case Prep run is already going for this chat. I'll finish it first.",
                )
                .await?;
                return Ok(());
            }
            let files = pending_files
                .0
                .lock()
                .await
                .remove(&chat_id)
                .unwrap_or_default();
            let cfg2 = with_token(&cfg, resolve_token(&tokens, chat_id, &cfg).await);
            let bot2 = bot.clone();
            let jobs = analyze_jobs.clone();
            // Spawn the run so the dispatcher stays free for other messages
            // (and other chats). The slot is released when the job finishes.
            tokio::spawn(async move {
                run_analyze_job(&bot2, chat_id, &cfg2, files).await;
                // Always release the slot, even if the mutex was poisoned — a
                // panic elsewhere must not lock this chat out of /analyze forever.
                jobs.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&chat_id);
            });
        }
    }
    Ok(())
}

/// Any non-command text message → forward to `/chat` with this chat's running
/// history, reply with the answer (plus any drafts and a sources footnote).
async fn handle_text(
    bot: Bot,
    msg: Message,
    cfg: BotConfig,
    history: History,
    pending: Pending,
    warned: Warned,
    in_flight: InFlight,
    tokens: Tokens,
    active: ActiveChats,
) -> ResponseResult<()> {
    let Some(user_text) = msg.text().map(str::to_string) else {
        bot.send_message(msg.chat.id, "I can only handle text messages for now.")
            .await?;
        return Ok(());
    };

    // Don't start a new turn while this chat still owes an answer to a pending
    // clarifying question — it would clobber the live wizard's state.
    if pending.lock().await.contains_key(&msg.chat.id) {
        bot.send_message(
            msg.chat.id,
            "⏳ Please answer the question above (tap an option) or /cancel first.",
        )
        .await?;
        return Ok(());
    }

    // Serialize turns per chat: claim the in-flight slot for the WHOLE call_chat
    // round. Without this, two quick plain messages run two call_chat
    // concurrently and interleave the shared history Vec (user1, user2,
    // assistant2, assistant1). The guard clears the slot on every exit path.
    let Some(_in_flight) = try_acquire_inflight(&in_flight, msg.chat.id) else {
        bot.send_message(
            msg.chat.id,
            "⏳ Still working on your previous message — I'll get to this once it's done.",
        )
        .await?;
        return Ok(());
    };

    // Authenticate as the logged-in user (per-chat token from /login), falling
    // back to the static env token / bypass. Shadowing `cfg` means every backend
    // call below — chat stream, clarify POST, document fetch — acts as that user.
    let cfg = with_token(&cfg, resolve_token(&tokens, msg.chat.id, &cfg).await);

    // The persisted thread this chat is attached to (if any), so the turn lands
    // in the same conversation as the laptop.
    let active_chat_id = active.0.lock().await.get(&msg.chat.id).cloned();

    // "typing…" indicator + a visible "waiting on Mike" status the user can see.
    // We delete it once the answer is ready, or turn it into the error message
    // (e.g. the 120s 404) if the backend never responds.
    let _ = bot.send_chat_action(msg.chat.id, ChatAction::Typing).await;
    let status_msg = bot
        .send_message(msg.chat.id, "⏳ We are waiting for a response from Mike…")
        .await
        .ok()
        .map(|m| m.id);

    // Build the message list = this chat's prior turns + the new user message.
    // We push the user turn now so it persists even if the backend call fails.
    let (messages, mut dropped) = {
        let mut map = history.lock().await;
        let turns = map.entry(msg.chat.id).or_default();
        turns.push(ChatMsg {
            role: "user".to_string(),
            content: user_text.clone(),
        });
        let dropped = trim_history(turns);
        (turns.clone(), dropped)
    };

    match call_chat(&bot, msg.chat.id, &cfg, &pending, &messages, active_chat_id.clone()).await {
        Ok(outcome) => {
            let ChatOutcome {
                reply: model_reply,
                citations,
                docs,
                chat_id: new_chat_id,
                cancelled,
            } = outcome;

            // The user cancelled mid-clarification: the cancel handler already
            // acknowledged it. Don't send a half-finished answer or persist it.
            if cancelled {
                if let Some(id) = status_msg {
                    let _ = bot.delete_message(msg.chat.id, id).await;
                }
                return Ok(());
            }

            // Attach to the thread the backend used/created so the next message
            // continues it (and the conversation shows up on the laptop too).
            if let Some(cid) = new_chat_id {
                active.0.lock().await.insert(msg.chat.id, cid);
            }

            // Persist the assistant turn (the model's bare text, without the
            // appended sources footnote, to keep conversational history clean).
            {
                let mut map = history.lock().await;
                let turns = map.entry(msg.chat.id).or_default();
                turns.push(ChatMsg {
                    role: "assistant".to_string(),
                    content: model_reply.clone(),
                });
                dropped += trim_history(turns);
            }

            // Append a plain-text sources footnote after the model's answer.
            let mut reply = model_reply;
            if let Some(footnote) = build_citations_footnote(&citations) {
                reply.push_str(&footnote);
            }
            let reply = if reply.trim().is_empty() {
                "(the backend returned an empty reply)".to_string()
            } else {
                reply
            };

            // Answer is ready — clear the "waiting on Mike" status before sending.
            if let Some(id) = status_msg {
                let _ = bot.delete_message(msg.chat.id, id).await;
            }

            for chunk in split_for_telegram(&reply) {
                bot.send_message(msg.chat.id, chunk).await?;
            }

            // Deliver each generated draft as a downloadable Word file.
            for doc in &docs {
                match fetch_document(&cfg, &doc.document_id).await {
                    Ok(bytes) => {
                        let file = InputFile::memory(bytes).file_name(doc.filename.clone());
                        bot.send_document(msg.chat.id, file).await?;
                    }
                    Err(e) => {
                        tracing::error!("draft download failed for {}: {e:#}", doc.document_id);
                        bot.send_message(
                            msg.chat.id,
                            format!("⚠️ couldn't fetch the draft file ({})", doc.filename),
                        )
                        .await?;
                    }
                }
            }

            // The first time this chat's history overflows the cap, tell the
            // user plainly that older messages are now being forgotten (warn
            // once per conversation, not on every message past the limit).
            if dropped > 0 && warned.lock().await.insert(msg.chat.id) {
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "ℹ️ This conversation has reached my memory limit \
                         ({MAX_HISTORY_MESSAGES} messages). I'll keep going, but I'm now \
                         dropping the oldest messages and may forget earlier details. \
                         Send /reset to start a clean conversation."
                    ),
                )
                .await?;
            }
        }
        Err(e) => {
            // Surface the failure to the user instead of silently dropping it —
            // turn the "waiting on Mike" status into the error (e.g. the 404).
            tracing::error!("chat call failed: {e:#}");
            // A 401 means the per-chat token expired/invalid — drop it so the
            // next /login re-authenticates cleanly.
            if e.to_string() == AUTH_ERR {
                tokens.0.lock().await.remove(&msg.chat.id);
            }
            let text = format!("⚠️ {e}");
            match status_msg {
                Some(id) => {
                    let _ = bot.edit_message_text(msg.chat.id, id, text).await;
                }
                None => {
                    bot.send_message(msg.chat.id, text).await?;
                }
            }
        }
    }
    Ok(())
}

/// A button was tapped on a clarifying-question keyboard. Advance the wizard
/// for this chat, editing the keyboard message in place, and — when the last
/// question is answered — wake the parked `call_chat` with the assembled result.
async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    cfg: BotConfig,
    tokens: Tokens,
    history: History,
    active: ActiveChats,
    docnames: DocNames,
    pending: Pending,
    in_flight: InFlight,
) -> ResponseResult<()> {
    let Some(chat_id) = q.message.as_ref().map(|m| m.chat().id) else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };
    let data = q.data.clone().unwrap_or_default();
    // `/chats` and `/docs` buttons route here too; their callback data is
    // namespaced so it never collides with the clarify keyboard (`q…`/`proceed`).
    if let Some(id) = data.strip_prefix("chat:") {
        return resume_chat(&bot, &q, chat_id, id, &cfg, &tokens, &history, &active, &in_flight)
            .await;
    }
    if let Some(id) = data.strip_prefix("doc:") {
        return deliver_doc(&bot, &q, chat_id, id, &cfg, &tokens, &docnames).await;
    }
    let Some(action) = parse_callback(&data) else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };

    // Decide the next UI step under the lock (no awaits held), then act on it.
    let (next, finish_entry) = {
        let mut map = pending.lock().await;
        let next = match map.get_mut(&chat_id) {
            None => Next::Expired,
            Some(st) => compute_next(st, &action),
        };
        // For a finishing step, take ownership of the wizard (and its sender)
        // now, before releasing the lock.
        let finish_entry = if matches!(next, Next::Finish { .. }) {
            map.remove(&chat_id)
        } else {
            None
        };
        (next, finish_entry)
    };

    match next {
        Next::Expired => {
            bot.answer_callback_query(q.id)
                .text("This question has expired.")
                .await?;
        }
        Next::Invalid(reason) => {
            bot.answer_callback_query(q.id).text(reason).await?;
        }
        Next::Stay { kb_msg, kb } => {
            let _ = bot
                .edit_message_reply_markup(chat_id, kb_msg)
                .reply_markup(kb)
                .await;
            bot.answer_callback_query(q.id).await?;
        }
        Next::Advance { kb_msg, text, kb } => {
            let _ = bot
                .edit_message_text(chat_id, kb_msg, text)
                .reply_markup(kb)
                .await;
            bot.answer_callback_query(q.id).await?;
        }
        Next::Finish {
            kb_msg,
            outcome,
            confirm,
        } => {
            // Editing without a reply_markup clears the keyboard.
            let _ = bot.edit_message_text(chat_id, kb_msg, confirm).await;
            if let Some(entry) = finish_entry {
                let _ = entry.tx.send(outcome);
            }
            bot.answer_callback_query(q.id).await?;
        }
    }
    Ok(())
}

/// The per-chat session token if the user has run `/login`, else the static
/// `MIKE_SESSION_TOKEN` fallback (which may be empty when the backend runs with
/// MIKE_BYPASS_AUTH=true).
async fn resolve_token(tokens: &Tokens, chat_id: ChatId, cfg: &BotConfig) -> String {
    tokens
        .0
        .lock()
        .await
        .get(&chat_id)
        .filter(|t| !t.is_empty())
        .cloned()
        .unwrap_or_else(|| cfg.session_token.clone())
}

/// A copy of `cfg` whose `session_token` is the caller's resolved per-chat
/// token, so the existing `cfg.session_token`-based helpers authenticate as
/// that user without any signature changes.
fn with_token(cfg: &BotConfig, token: String) -> BotConfig {
    BotConfig {
        session_token: token,
        ..cfg.clone()
    }
}

/// GET a JSON endpoint with the chat's bearer token. Maps a 401 to `AUTH_ERR`.
async fn api_get_json(cfg: &BotConfig, path: &str) -> Result<Value> {
    let url = format!(
        "{}/{}",
        cfg.api_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(anyhow!("{AUTH_ERR}"));
    }
    if !status.is_success() {
        return Err(anyhow!("backend returned {status}"));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| anyhow!("bad response from {url}: {e}"))
}

/// Truncate a button label so the keyboard stays tidy (char-safe).
fn truncate_label(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// `/login <PIN>` → unlock the backend, cache the returned session token for this
/// chat, and scrub the PIN message. A visible message on every outcome.
async fn cmd_login(
    bot: &Bot,
    msg: &Message,
    cfg: &BotConfig,
    tokens: &Tokens,
    pin: &str,
) -> ResponseResult<()> {
    let pin = pin.trim();
    if pin.is_empty() {
        bot.send_message(msg.chat.id, "Usage: /login <your PIN>")
            .await?;
        return Ok(());
    }
    let url = format!("{}/auth/unlock", cfg.api_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&json!({ "pin": pin }))
        .send()
        .await;

    // Scrub the PIN message regardless of outcome — it's already on Telegram's
    // servers, but don't leave it sitting in the visible chat history.
    let _ = bot.delete_message(msg.chat.id, msg.id).await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body = r.json::<Value>().await.unwrap_or(Value::Null);
            match body.get("token").and_then(Value::as_str) {
                Some(token) if !token.is_empty() => {
                    tokens.0.lock().await.insert(msg.chat.id, token.to_string());
                    bot.send_message(
                        msg.chat.id,
                        "✅ Logged in — I'm now acting as you and syncing with your \
                         laptop. (I deleted your PIN message for safety.)\n\n\
                         Try /chats to continue a laptop conversation, or just ask \
                         a question.",
                    )
                    .await?;
                }
                _ => {
                    bot.send_message(
                        msg.chat.id,
                        "⚠️ Login succeeded but the backend returned no token — \
                         please try /login again.",
                    )
                    .await?;
                }
            }
        }
        Ok(r) => {
            let code = r.status();
            bot.send_message(
                msg.chat.id,
                format!("❌ Login failed ({code}). Check your PIN and try /login again."),
            )
            .await?;
        }
        Err(e) => {
            bot.send_message(
                msg.chat.id,
                format!("⚠️ Couldn't reach the backend to log in: {e}"),
            )
            .await?;
        }
    }
    Ok(())
}

/// `/chats` → list the user's saved conversations as tappable buttons.
async fn cmd_chats(bot: &Bot, chat_id: ChatId, cfg: &BotConfig) -> ResponseResult<()> {
    const MAX: usize = 10;
    match api_get_json(cfg, "/chat").await {
        Ok(v) => {
            let chats = v
                .get("chats")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if chats.is_empty() {
                bot.send_message(
                    chat_id,
                    "You have no saved conversations yet — send a message to start one.",
                )
                .await?;
                return Ok(());
            }
            let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
            for c in chats.iter().take(MAX) {
                let Some(id) = c.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let title = c
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("Untitled conversation");
                rows.push(vec![InlineKeyboardButton::callback(
                    truncate_label(title, 60),
                    format!("chat:{id}"),
                )]);
            }
            let mut text = "📂 Pick a conversation to continue here:".to_string();
            if chats.len() > MAX {
                text.push_str(&format!(
                    "\n(showing the {MAX} most recent of {})",
                    chats.len()
                ));
            }
            bot.send_message(chat_id, text)
                .reply_markup(InlineKeyboardMarkup::new(rows))
                .await?;
        }
        Err(e) => {
            bot.send_message(chat_id, format!("⚠️ {e}")).await?;
        }
    }
    Ok(())
}

/// `/docs` → list the user's documents as tappable buttons, caching their names
/// so a tap can deliver the file under its real filename.
async fn cmd_docs(
    bot: &Bot,
    chat_id: ChatId,
    cfg: &BotConfig,
    docnames: &DocNames,
) -> ResponseResult<()> {
    const MAX: usize = 10;
    match api_get_json(cfg, "/document").await {
        Ok(v) => {
            let docs = v
                .get("documents")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if docs.is_empty() {
                bot.send_message(chat_id, "You have no documents yet.")
                    .await?;
                return Ok(());
            }
            let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
            let mut names: HashMap<String, String> = HashMap::new();
            for d in docs.iter().take(MAX) {
                let Some(id) = d.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let filename = d
                    .get("filename")
                    .and_then(Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or("document.docx");
                names.insert(id.to_string(), filename.to_string());
                rows.push(vec![InlineKeyboardButton::callback(
                    truncate_label(filename, 60),
                    format!("doc:{id}"),
                )]);
            }
            docnames.0.lock().await.insert(chat_id, names);
            let mut text = "📎 Tap a document to download it here:".to_string();
            if docs.len() > MAX {
                text.push_str(&format!(
                    "\n(showing the {MAX} most recent of {})",
                    docs.len()
                ));
            }
            bot.send_message(chat_id, text)
                .reply_markup(InlineKeyboardMarkup::new(rows))
                .await?;
        }
        Err(e) => {
            bot.send_message(chat_id, format!("⚠️ {e}")).await?;
        }
    }
    Ok(())
}

/// A `chat:<id>` button → load that thread's messages, seed this chat's history
/// with them, and attach to the thread so further messages continue it.
async fn resume_chat(
    bot: &Bot,
    q: &CallbackQuery,
    chat_id: ChatId,
    mike_chat_id: &str,
    cfg: &BotConfig,
    tokens: &Tokens,
    history: &History,
    active: &ActiveChats,
    in_flight: &InFlight,
) -> ResponseResult<()> {
    // Answer first so Telegram's button spinner clears within its ~10s deadline,
    // before we do any backend round-trip.
    bot.answer_callback_query(q.id.clone()).await?;

    // Reseeding history must be mutually exclusive with a turn that's mid-flight,
    // or we'd clobber the Vec that `call_chat` is appending to. Hold the same
    // per-chat slot `handle_text` uses; if a turn is running, ask them to retry.
    let Some(_guard) = try_acquire_inflight(in_flight, chat_id) else {
        bot.send_message(
            chat_id,
            "⏳ I'm still answering your last message — tap the conversation again once it's done.",
        )
        .await?;
        return Ok(());
    };

    let cfg2 = with_token(cfg, resolve_token(tokens, chat_id, cfg).await);
    let path = format!("/chat/{mike_chat_id}/messages");
    match api_get_json(&cfg2, &path).await {
        Ok(v) => {
            let mut turns: Vec<ChatMsg> = v
                .get("messages")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            let role = m.get("role").and_then(Value::as_str)?;
                            if role != "user" && role != "assistant" {
                                return None;
                            }
                            let content = m.get("content").and_then(Value::as_str)?;
                            Some(ChatMsg {
                                role: role.to_string(),
                                content: content.to_string(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let dropped = trim_history(&mut turns);
            let n = turns.len();
            history.lock().await.insert(chat_id, turns);
            active
                .0
                .lock()
                .await
                .insert(chat_id, mike_chat_id.to_string());
            // Be honest when the thread was longer than I can hold in context.
            let note = if dropped > 0 {
                format!(
                    "✅ Resumed — kept the most recent {n} message(s) (dropped {dropped} older \
                     to fit my memory limit). Keep going here; replies and new drafts also show \
                     up on your laptop."
                )
            } else {
                format!(
                    "✅ Resumed — loaded {n} earlier message(s). Keep going here; replies and \
                     new drafts also show up on your laptop."
                )
            };
            bot.send_message(chat_id, note).await?;
        }
        Err(e) => {
            bot.send_message(chat_id, format!("⚠️ Couldn't load that conversation: {e}"))
                .await?;
        }
    }
    Ok(())
}

/// A `doc:<id>` button → download the document and deliver it as a Word file.
async fn deliver_doc(
    bot: &Bot,
    q: &CallbackQuery,
    chat_id: ChatId,
    document_id: &str,
    cfg: &BotConfig,
    tokens: &Tokens,
    docnames: &DocNames,
) -> ResponseResult<()> {
    let cfg2 = with_token(cfg, resolve_token(tokens, chat_id, cfg).await);
    let filename = docnames
        .0
        .lock()
        .await
        .get(&chat_id)
        .and_then(|m| m.get(document_id))
        .cloned()
        .unwrap_or_else(|| "document.docx".to_string());
    bot.answer_callback_query(q.id.clone()).await?;
    match fetch_document(&cfg2, document_id).await {
        Ok(bytes) => {
            let file = InputFile::memory(bytes).file_name(filename.clone());
            bot.send_document(chat_id, file).await?;
        }
        Err(e) => {
            tracing::error!("doc download failed for {document_id}: {e:#}");
            bot.send_message(
                chat_id,
                format!("⚠️ Couldn't download {filename} — please try /docs again."),
            )
            .await?;
        }
    }
    Ok(())
}

/// The UI step to take after a button tap.
enum Next {
    /// No live wizard for this chat (stale keyboard).
    Expired,
    /// Tap ignored; tell the user why.
    Invalid(&'static str),
    /// Same question, refreshed keyboard (multi-select toggle).
    Stay {
        kb_msg: MessageId,
        kb: InlineKeyboardMarkup,
    },
    /// Move on to the next question.
    Advance {
        kb_msg: MessageId,
        text: String,
        kb: InlineKeyboardMarkup,
    },
    /// Wizard complete — wake `call_chat`.
    Finish {
        kb_msg: MessageId,
        outcome: ClarifyOutcome,
        confirm: &'static str,
    },
}

/// Apply a tap to the wizard state and return the resulting UI step. Mutates
/// `st` (records selections, advances `current`); never touches the network.
fn compute_next(st: &mut PendingClarify, action: &CbAction) -> Next {
    let kb_msg = st.keyboard_msg;
    match action {
        CbAction::Proceed => Next::Finish {
            kb_msg,
            outcome: ClarifyOutcome::Proceed,
            confirm: "⏭️ Proceeding with best guesses…",
        },
        CbAction::Done { q_idx } => {
            if *q_idx != st.current {
                return Next::Invalid("That question is no longer active.");
            }
            if !st.questions[st.current].multi_select {
                return Next::Invalid("Tap an option.");
            }
            if st.selected.is_empty() {
                return Next::Invalid("Pick at least one option first.");
            }
            let labels: Vec<String> = st
                .selected
                .iter()
                .map(|&i| st.questions[st.current].options[i].label.clone())
                .collect();
            st.collected.push(labels);
            st.selected.clear();
            advance(st, kb_msg)
        }
        CbAction::Option { q_idx, o_idx } => {
            if *q_idx != st.current {
                return Next::Invalid("That question is no longer active.");
            }
            if *o_idx >= st.questions[st.current].options.len() {
                return Next::Invalid("Unknown option.");
            }
            if st.questions[st.current].multi_select {
                if let Some(pos) = st.selected.iter().position(|x| x == o_idx) {
                    st.selected.remove(pos);
                } else {
                    st.selected.push(*o_idx);
                }
                let kb = render_question(&st.questions[st.current], st.current, &st.selected);
                Next::Stay { kb_msg, kb }
            } else {
                let label = st.questions[st.current].options[*o_idx].label.clone();
                st.collected.push(vec![label]);
                advance(st, kb_msg)
            }
        }
    }
}

/// Move to the next question, or finish the wizard if the last was just answered.
fn advance(st: &mut PendingClarify, kb_msg: MessageId) -> Next {
    st.current += 1;
    if st.current >= st.questions.len() {
        Next::Finish {
            kb_msg,
            outcome: ClarifyOutcome::Answered(build_clarify_result(&st.questions, &st.collected)),
            confirm: "✅ Got it — working on your request…",
        }
    } else {
        let total = st.questions.len();
        let q = &st.questions[st.current];
        Next::Advance {
            kb_msg,
            text: render_question_text(q, st.current, total),
            kb: render_question(q, st.current, &[]),
        }
    }
}

/// Decode a button's `callback_data` (`q{q}:o{o}`, `q{q}:done`, or `proceed`).
fn parse_callback(data: &str) -> Option<CbAction> {
    if data == "proceed" {
        return Some(CbAction::Proceed);
    }
    let rest = data.strip_prefix('q')?;
    let (q_str, tail) = rest.split_once(':')?;
    let q_idx: usize = q_str.parse().ok()?;
    if tail == "done" {
        return Some(CbAction::Done { q_idx });
    }
    let o_idx: usize = tail.strip_prefix('o')?.parse().ok()?;
    Some(CbAction::Option { q_idx, o_idx })
}

/// Build the message body for a question: a `Question n/N` header (when there's
/// more than one), the question text, any option descriptions as a legend, and
/// a multi-select hint.
fn render_question_text(q: &ClarifyQuestion, idx: usize, total: usize) -> String {
    let mut s = String::new();
    if total > 1 {
        s.push_str(&format!("❓ Question {}/{}\n", idx + 1, total));
    }
    if !q.header.trim().is_empty() {
        s.push_str(&format!("[{}] ", q.header.trim()));
    }
    s.push_str(&q.question);
    let legend: Vec<String> = q
        .options
        .iter()
        .filter_map(|o| {
            o.description
                .as_ref()
                .map(|d| format!("• {} — {}", o.label, d))
        })
        .collect();
    if !legend.is_empty() {
        s.push_str("\n\n");
        s.push_str(&legend.join("\n"));
    }
    if q.multi_select {
        s.push_str("\n\n(Select all that apply, then tap ✅ Done.)");
    }
    s
}

/// Build the inline keyboard for a question: one button per option (with a
/// checkbox prefix for multi-select), a ✅ Done button for multi-select, and a
/// "skip / proceed anyway" escape hatch on every question.
fn render_question(q: &ClarifyQuestion, q_idx: usize, selected: &[usize]) -> InlineKeyboardMarkup {
    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    for (o_idx, opt) in q.options.iter().enumerate() {
        let prefix = if q.multi_select {
            if selected.contains(&o_idx) {
                "☑️ "
            } else {
                "⬜ "
            }
        } else {
            ""
        };
        rows.push(vec![InlineKeyboardButton::callback(
            format!("{prefix}{}", opt.label),
            format!("q{q_idx}:o{o_idx}"),
        )]);
    }
    if q.multi_select {
        rows.push(vec![InlineKeyboardButton::callback(
            "✅ Done",
            format!("q{q_idx}:done"),
        )]);
    }
    rows.push(vec![InlineKeyboardButton::callback(
        "⏭️ Skip / proceed anyway",
        "proceed",
    )]);
    InlineKeyboardMarkup::new(rows)
}

/// Assemble the `client-tool-result` JSON the backend's
/// `format_clarifying_answers` parses: `{answers:[{question, selected}], proceed:false}`.
fn build_clarify_result(questions: &[ClarifyQuestion], collected: &[Vec<String>]) -> String {
    let answers: Vec<Value> = questions
        .iter()
        .zip(collected.iter())
        .map(|(q, sel)| json!({ "question": q.question, "selected": sel }))
        .collect();
    json!({ "answers": answers, "proceed": false }).to_string()
}

/// Drop the oldest messages so a chat never carries more than the cap.
/// Returns how many messages were dropped (0 if under the cap).
fn trim_history(turns: &mut Vec<ChatMsg>) -> usize {
    if turns.len() > MAX_HISTORY_MESSAGES {
        let overflow = turns.len() - MAX_HISTORY_MESSAGES;
        turns.drain(..overflow);
        overflow
    } else {
        0
    }
}

/// One source reference parsed from the stream's `citations` event.
struct Citation {
    filename: String,
    page: Option<i64>,
}

/// Everything we collect from a single `/chat/` stream.
#[derive(Default)]
struct ChatOutcome {
    reply: String,
    citations: Vec<Citation>,
    docs: Vec<DocRef>,
    /// The persisted `chat_id` the backend used/created for this turn (captured
    /// from the stream's `chat_id` event), so we can keep continuing the thread.
    chat_id: Option<String>,
    /// Set when the user `/cancel`led mid-clarification.
    cancelled: bool,
}

/// Mutable state threaded through SSE parsing for one stream.
#[derive(Default)]
struct StreamState {
    outcome: ChatOutcome,
    error: Option<String>,
    /// A `client_tool_request` awaiting an interactive answer.
    pending_tool: Option<ToolRequest>,
}

/// POST the conversation to the backend's `/chat/` SSE endpoint and accumulate
/// the streamed assistant reply, citations, and generated drafts. Returns the
/// outcome, or an error (transport, non-2xx, or an `error` event from the stream).
async fn call_chat(
    bot: &Bot,
    chat_id: ChatId,
    cfg: &BotConfig,
    pending: &Pending,
    messages: &[ChatMsg],
    active_chat_id: Option<String>,
) -> Result<ChatOutcome> {
    let msg_json: Vec<Value> = messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    let mut body = json!({ "messages": msg_json });
    if let Some(model) = &cfg.model {
        body["model"] = json!(model);
    }
    // Continue the attached thread (so it persists to the same conversation the
    // laptop sees); omit it to let the backend create a fresh one.
    if let Some(cid) = &active_chat_id {
        body["chat_id"] = json!(cid);
    }

    let client = reqwest::Client::new();
    // No trailing slash: axum's `nest("/chat", …)` matches `/chat`, and a
    // trailing-slash `/chat/` 404s. Same for the `/chat` and `/document` GETs.
    let url = format!("{}/chat", cfg.api_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(anyhow!("{AUTH_ERR}"));
        }
        let detail = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "backend returned {status}: {}",
            detail.chars().take(300).collect::<String>()
        ));
    }

    let mut buf = String::new();
    let mut state = StreamState::default();
    let mut stream = resp.bytes_stream();
    loop {
        // Drain every complete SSE event (separated by a blank line) from buf.
        while let Some(pos) = buf.find("\n\n") {
            let raw_event: String = buf.drain(..pos + 2).collect();
            apply_sse_event(&raw_event, &mut state);
        }
        // If the model is asking for clarification, run the interactive flow.
        // The open SSE stream (`stream`/`resp`) stays alive throughout, so the
        // backend's tool call keeps blocking until we POST the result.
        if let Some(req) = state.pending_tool.take() {
            match run_clarify_flow(bot, chat_id, cfg, &client, pending, req).await? {
                ClarifyControl::Continue => continue,
                ClarifyControl::Stop => {
                    state.outcome.cancelled = true;
                    break;
                }
            }
        }
        // Pull the next chunk; the stream closing ends the response. If the
        // backend goes silent for STREAM_IDLE_TIMEOUT, treat it as a dead
        // stream rather than hanging forever.
        match tokio::time::timeout(STREAM_IDLE_TIMEOUT, stream.next()).await {
            Err(_elapsed) => {
                return Err(anyhow!(
                    "404 — no response from Mike after {}s. Please try again.",
                    STREAM_IDLE_TIMEOUT.as_secs()
                ));
            }
            Ok(Some(chunk)) => {
                let chunk = chunk.map_err(|e| anyhow!("stream error: {e}"))?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
            }
            Ok(None) => break,
        }
    }
    // Flush any trailing event that wasn't terminated by a blank line.
    if !state.outcome.cancelled && !buf.trim().is_empty() {
        apply_sse_event(&buf, &mut state);
    }

    match state.error {
        Some(e) => Err(anyhow!("{e}")),
        None => Ok(state.outcome),
    }
}

/// Drive one clarifying-question round: show the first question's keyboard,
/// register the wizard, and park until the callback handler (or `/cancel`, or
/// the timeout) resolves it — then POST the result so the stream resumes.
async fn run_clarify_flow(
    bot: &Bot,
    chat_id: ChatId,
    cfg: &BotConfig,
    client: &reqwest::Client,
    pending: &Pending,
    req: ToolRequest,
) -> Result<ClarifyControl> {
    // No usable questions → just let the model proceed.
    if req.questions.is_empty() {
        post_tool_result(cfg, client, &req.request_id, PROCEED_RESULT).await?;
        return Ok(ClarifyControl::Continue);
    }

    let (tx, rx) = oneshot::channel();
    let total = req.questions.len();
    let q0 = &req.questions[0];
    // If we can't even show the keyboard, don't strand the backend on its tool
    // call (it would stall ~180s): log it and let the model proceed instead.
    let sent = match bot
        .send_message(chat_id, render_question_text(q0, 0, total))
        .reply_markup(render_question(q0, 0, &[]))
        .await
    {
        Ok(sent) => sent,
        Err(e) => {
            tracing::error!("could not show clarifying keyboard: {e}; proceeding");
            post_tool_result(cfg, client, &req.request_id, PROCEED_RESULT).await?;
            return Ok(ClarifyControl::Continue);
        }
    };
    {
        let mut map = pending.lock().await;
        map.insert(
            chat_id,
            PendingClarify {
                questions: req.questions,
                current: 0,
                collected: Vec::new(),
                selected: Vec::new(),
                keyboard_msg: sent.id,
                tx,
            },
        );
    }

    // Park here while the user taps. The stream stays open (we hold it in the
    // caller); the backend keeps blocking on its own oneshot until we POST.
    let outcome = match tokio::time::timeout(CLARIFY_WAIT, rx).await {
        Ok(Ok(o)) => o,
        // Sender dropped without a value (shouldn't happen) → treat as cancel.
        Ok(Err(_)) => ClarifyOutcome::Cancelled,
        // No answer in time → clean up and let the model proceed.
        Err(_) => {
            if let Some(entry) = pending.lock().await.remove(&chat_id) {
                let _ = bot
                    .edit_message_text(
                        chat_id,
                        entry.keyboard_msg,
                        "⌛ No answer — proceeding with best guesses.",
                    )
                    .await;
            }
            ClarifyOutcome::Proceed
        }
    };

    match outcome {
        ClarifyOutcome::Answered(result) => {
            // If the POST 404s, the backend already timed out this tool call and
            // resumed on its own — the user's tapped answer was discarded. Tell
            // them rather than letting it vanish silently.
            if !post_tool_result(cfg, client, &req.request_id, &result).await? {
                let _ = bot
                    .send_message(
                        chat_id,
                        "⚠️ Your answer arrived too late — Mike already moved on, \
                         so I'm proceeding with its best guess.",
                    )
                    .await;
            }
            Ok(ClarifyControl::Continue)
        }
        ClarifyOutcome::Proceed => {
            post_tool_result(cfg, client, &req.request_id, PROCEED_RESULT).await?;
            Ok(ClarifyControl::Continue)
        }
        // Don't resume: the backend will self-time-out on its tool call. We
        // can't signal a hard abort over this contract, so we just stop here.
        ClarifyOutcome::Cancelled => Ok(ClarifyControl::Stop),
    }
}

/// Fold one SSE event block into the stream state. The backend emits
/// `data: {json}` lines; we accumulate `content_delta.text`, collect
/// `doc_created` / `citations`, capture a `client_tool_request` for
/// `ask_clarifying_questions`, and capture the first `error.message`. Other
/// event types are ignored.
fn apply_sse_event(raw_event: &str, state: &mut StreamState) {
    for line in raw_event.lines() {
        let Some(data) = line.trim_end_matches('\r').strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("content_delta") => {
                if let Some(t) = value.get("text").and_then(Value::as_str) {
                    state.outcome.reply.push_str(t);
                }
            }
            Some("chat_id") => {
                if let Some(c) = value.get("chatId").and_then(Value::as_str) {
                    state.outcome.chat_id = Some(c.to_string());
                }
            }
            Some("doc_created") => {
                let filename = value.get("filename").and_then(Value::as_str);
                let document_id = value.get("document_id").and_then(Value::as_str);
                if let (Some(filename), Some(document_id)) = (filename, document_id) {
                    state.outcome.docs.push(DocRef {
                        filename: filename.to_string(),
                        document_id: document_id.to_string(),
                    });
                }
            }
            Some("citations") => {
                if let Some(arr) = value.get("citations").and_then(Value::as_array) {
                    for c in arr {
                        let Some(filename) = c.get("filename").and_then(Value::as_str) else {
                            continue;
                        };
                        state.outcome.citations.push(Citation {
                            filename: filename.to_string(),
                            page: c.get("page").and_then(Value::as_i64),
                        });
                    }
                }
            }
            Some("client_tool_request")
                if value.get("name").and_then(Value::as_str)
                    == Some("ask_clarifying_questions") =>
            {
                if let Some(req) = parse_tool_request(&value) {
                    state.pending_tool = Some(req);
                }
            }
            Some("error") if state.error.is_none() => {
                state.error = Some(
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_string(),
                );
            }
            _ => {}
        }
    }
}

/// Parse a `client_tool_request` event into a `ToolRequest`. Questions whose
/// options are missing/empty are dropped (they can't render a keyboard).
fn parse_tool_request(value: &Value) -> Option<ToolRequest> {
    let request_id = value.get("request_id")?.as_str()?.to_string();
    let questions = value
        .get("arguments")
        .and_then(|a| a.get("questions"))
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_question).collect())
        .unwrap_or_default();
    Some(ToolRequest {
        request_id,
        questions,
    })
}

fn parse_question(v: &Value) -> Option<ClarifyQuestion> {
    let question = v.get("question")?.as_str()?.to_string();
    let options: Vec<ClarifyOption> = v
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_option).collect())
        .unwrap_or_default();
    if options.is_empty() {
        return None;
    }
    Some(ClarifyQuestion {
        header: v
            .get("header")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        question,
        multi_select: v
            .get("multiSelect")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        options,
    })
}

fn parse_option(v: &Value) -> Option<ClarifyOption> {
    let label = v.get("label")?.as_str()?.to_string();
    Some(ClarifyOption {
        label,
        description: v
            .get("description")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
    })
}

/// Build a plain-text `📚 Sources:` footnote from citations, de-duplicating
/// identical filename+page. Returns `None` when there are no citations.
fn build_citations_footnote(citations: &[Citation]) -> Option<String> {
    if citations.is_empty() {
        return None;
    }
    let mut seen: Vec<(String, Option<i64>)> = Vec::new();
    let mut lines = String::new();
    let mut n = 0;
    for c in citations {
        let key = (c.filename.clone(), c.page);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        n += 1;
        match c.page {
            Some(p) => lines.push_str(&format!("[{n}] {} (p.{p})\n", c.filename)),
            None => lines.push_str(&format!("[{n}] {}\n", c.filename)),
        }
    }
    if n == 0 {
        return None;
    }
    Some(format!("\n\n📚 Sources:\n{}", lines.trim_end_matches('\n')))
}

/// POST a `client-tool-result` so the backend's blocked tool call resolves and
/// the open SSE stream resumes. `result` is the JSON string the backend parses.
/// Returns `true` if the answer landed (2xx), `false` if the backend rejected it
/// (e.g. 404 = it already timed out this question) so the caller can tell the user.
async fn post_tool_result(
    cfg: &BotConfig,
    client: &reqwest::Client,
    request_id: &str,
    result: &str,
) -> Result<bool> {
    let url = format!(
        "{}/chat/client-tool-result",
        cfg.api_url.trim_end_matches('/')
    );
    let body = json!({ "request_id": request_id, "result": result });
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("could not post tool result to {url}: {e}"))?;
    // A non-2xx here (e.g. 404 = the backend already timed out this question)
    // means our answer didn't land. Don't hard-fail — the backend's own tool
    // timeout still resumes the stream — but surface it instead of swallowing it.
    if !resp.status().is_success() {
        tracing::warn!(
            "client-tool-result POST returned {} — the answer may not have been applied",
            resp.status()
        );
        return Ok(false);
    }
    Ok(true)
}

/// Download a generated draft as raw `.docx` bytes from the backend.
///
/// Drafts are markdown-first: `draft_document` persists only the markdown and
/// the `.docx` is rendered on demand. So we first ask the backend to render the
/// markdown to a stored `.docx` (`POST /document/:id/render-word`), then stream
/// it. A 409 means there is no markdown to render (an uploaded file that already
/// has stored bytes) — fine, we fall through to the download either way.
async fn fetch_document(cfg: &BotConfig, document_id: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::new();
    let base = cfg.api_url.trim_end_matches('/');
    let auth = format!("Bearer {}", cfg.session_token);

    let render_url = format!("{base}/document/{document_id}/render-word");
    match client
        .post(&render_url)
        .header("authorization", auth.as_str())
        .send()
        .await
    {
        Ok(r) if r.status().is_success() || r.status() == reqwest::StatusCode::CONFLICT => {}
        Ok(r) => tracing::warn!("render-word returned {} for {render_url}", r.status()),
        Err(e) => tracing::warn!("could not reach backend at {render_url}: {e}"),
    }

    let url = format!("{base}/document/{document_id}/docx");
    let resp = client
        .get(&url)
        .header("authorization", auth.as_str())
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("backend returned {} for {url}", resp.status()));
    }
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| anyhow!("could not read document bytes: {e}"))
}

/// Split a reply into <=4096-char chunks, preferring line boundaries so a long
/// legal answer still sends instead of being rejected by Telegram.
fn split_for_telegram(text: &str) -> Vec<String> {
    if text.chars().count() <= TELEGRAM_LIMIT {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if current.chars().count() + line.chars().count() > TELEGRAM_LIMIT {
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            // A single line longer than the limit must be hard-split by chars.
            if line.chars().count() > TELEGRAM_LIMIT {
                let mut piece = String::new();
                for ch in line.chars() {
                    if piece.chars().count() + 1 > TELEGRAM_LIMIT {
                        chunks.push(std::mem::take(&mut piece));
                    }
                    piece.push(ch);
                }
                current = piece;
                continue;
            }
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

// ===========================================================================
// Case Prep over Telegram: stage files → /analyze → backend pipeline → outputs.
// The bot never links the `mike` crate; it drives the documented HTTP contract
// (POST /cases, POST /document, POST /cases/:id/analyze (SSE), the output
// routes, GET /document/:id/docx) exactly as the laptop UI does.
// ===========================================================================

/// A document or photo message → stage the file for `/analyze`. We keep only
/// Telegram's `file_id` here; the bytes are fetched when `/analyze` runs. Over
/// the per-file size or per-run count cap, the file is rejected with a specific
/// message rather than silently dropped or truncated.
async fn handle_document(
    bot: Bot,
    msg: Message,
    pending_files: PendingFiles,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;

    let pf = if let Some(doc) = msg.document() {
        let filename = doc.file_name.clone().unwrap_or_else(|| {
            let ext = doc
                .mime_type
                .as_ref()
                .map(|m| ext_for_mime(m.essence_str()))
                .unwrap_or("bin");
            format!("document.{ext}")
        });
        PendingFile {
            file_id: doc.file.id.clone(),
            filename,
            size: doc.file.size as u64,
        }
    } else if let Some(sizes) = msg.photo() {
        // Telegram sends several resolutions; the last is the largest.
        let Some(p) = sizes.last() else {
            return Ok(());
        };
        let n = pending_files.0.lock().await.get(&chat_id).map_or(0, Vec::len) + 1;
        PendingFile {
            file_id: p.file.id.clone(),
            filename: format!("scan_{n}.jpg"),
            size: p.file.size as u64,
        }
    } else {
        return Ok(());
    };

    let current = pending_files.0.lock().await.get(&chat_id).map_or(0, Vec::len);
    if let Some(reason) = reject_file(current, pf.size) {
        bot.send_message(chat_id, reason).await?;
        return Ok(());
    }

    let total = {
        let mut map = pending_files.0.lock().await;
        let v = map.entry(chat_id).or_default();
        v.push(pf.clone());
        v.len()
    };
    // A command typed as a file's caption isn't routed to filter_command (that
    // only reads message text), so it would be silently lost — nudge the user.
    let caption_hint = msg
        .caption()
        .map(str::trim)
        .filter(|c| c.starts_with("/analyze"))
        .map(|_| {
            "\n\n(Heads up: I can't read a command typed as a file caption. \
             Send /analyze as its own message.)"
        })
        .unwrap_or("");
    bot.send_message(
        chat_id,
        format!(
            "📎 Staged {} ({} file{} ready). Send more, or /analyze to run Case Prep. \
             /reset to clear.{}",
            pf.filename,
            total,
            if total == 1 { "" } else { "s" },
            caption_hint
        ),
    )
    .await?;
    Ok(())
}

/// Run the whole Case Prep flow for one chat, editing a single status message
/// through each phase so the user always sees progress (never a silent wait).
/// Any failure is surfaced as a real message — there is no path that leaves a
/// dangling "typing…".
async fn run_analyze_job(bot: &Bot, chat_id: ChatId, cfg: &BotConfig, files: Vec<PendingFile>) {
    let status_id = match bot.send_message(chat_id, "🧷 Starting Case Prep…").await {
        Ok(m) => m.id,
        Err(e) => {
            tracing::error!("analyze: could not send status message: {e}");
            return;
        }
    };
    let mut status = Status::new(status_id);
    if let Err(e) = analyze_flow(bot, chat_id, cfg, files, &mut status).await {
        let text = if e.to_string() == AUTH_ERR {
            AUTH_ERR.to_string()
        } else {
            format!("⚠️ Case Prep couldn't finish: {e}")
        };
        status.force(bot, chat_id, text).await;
    }
}

/// The Case Prep pipeline as a fallible sequence; `run_analyze_job` renders the
/// `Err` to the user. Per-file/per-output failures are *reported inline* and the
/// run continues, so one bad document never sinks the whole batch.
async fn analyze_flow(
    bot: &Bot,
    chat_id: ChatId,
    cfg: &BotConfig,
    files: Vec<PendingFile>,
    status: &mut Status,
) -> Result<()> {
    if files.is_empty() {
        return Err(anyhow!("no files were staged"));
    }
    // Quick calls (create/upload/attach/generate/delete) get connect + total
    // timeouts so a hung or unreachable backend surfaces as a visible error
    // rather than parking the job (and the chat's slot) forever. The SSE analyze
    // call manages its own idle timeout, so it uses a separate client without a
    // total timeout (see run_analysis).
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(180))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    // 1) Pull the staged files down from Telegram.
    status
        .force(
            bot,
            chat_id,
            format!("📥 Fetching {} file(s) from Telegram…", files.len()),
        )
        .await;
    let mut downloaded: Vec<(String, Vec<u8>)> = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for f in &files {
        match download_telegram_file(bot, &f.file_id).await {
            Ok(bytes) if !bytes.is_empty() => downloaded.push((f.filename.clone(), bytes)),
            Ok(_) => problems.push(format!("{} (empty)", f.filename)),
            Err(e) => {
                tracing::error!("telegram download {}: {e:#}", f.filename);
                problems.push(f.filename.clone());
            }
        }
    }
    if downloaded.is_empty() {
        return Err(anyhow!("couldn't download any of your files from Telegram"));
    }

    // 2) Create a PTEST-scoped case (kept in the account so it can be reviewed
    //    and deleted from the app; the title prefix marks it as a test artifact).
    status
        .force(bot, chat_id, "🗂 Creating a case…".to_string())
        .await;
    let title = case_title(downloaded.len());
    let case_id = create_case(&client, cfg, &title).await?;

    // 3) Upload each file and attach it to the case.
    let mut doc_ids: Vec<String> = Vec::new();
    let total = downloaded.len();
    // Consume `downloaded` so each file body is moved into the upload, not cloned
    // (a 20 MB file × up to 20 files would otherwise duplicate ~400 MB).
    for (i, (name, bytes)) in downloaded.into_iter().enumerate() {
        status
            .set(
                bot,
                chat_id,
                format!("📤 Uploading {name} ({}/{total})…", i + 1),
                false,
            )
            .await;
        match upload_document(&client, cfg, &name, bytes).await {
            Ok(id) => doc_ids.push(id),
            Err(e) => {
                tracing::error!("upload {name}: {e:#}");
                problems.push(name);
            }
        }
    }
    if doc_ids.is_empty() {
        let _ = delete_case(&client, cfg, &case_id).await;
        return Err(anyhow!("couldn't upload any of your files to Mike"));
    }
    // Attach failed after a case was created → don't orphan the case.
    if let Err(e) = attach_documents(&client, cfg, &case_id, &doc_ids).await {
        let _ = delete_case(&client, cfg, &case_id).await;
        return Err(e);
    }

    // 4) Run the 7-agent analysis, reporting progress off the SSE stream.
    status
        .force(
            bot,
            chat_id,
            format!(
                "🔍 Analyzing {} document(s)… this can take a couple of minutes.",
                doc_ids.len()
            ),
        )
        .await;
    let progress = run_analysis(cfg, &case_id, bot, chat_id, status).await?;

    // Flag files with no readable text (e.g. a blank scan), but keep going if
    // some documents did extract — one unreadable file shouldn't sink the run.
    if !progress.unreadable.is_empty() {
        let _ = bot
            .send_message(
                chat_id,
                format!(
                    "⚠️ {} file(s) had no readable text{} and were skipped: {}.",
                    progress.unreadable.len(),
                    if progress.ocr_used { " (even after OCR)" } else { "" },
                    progress.unreadable.join(", ")
                ),
            )
            .await;
    }
    if !problems.is_empty() {
        let _ = bot
            .send_message(chat_id, format!("⚠️ Couldn't process: {}.", problems.join(", ")))
            .await;
    }
    if progress.unreadable.len() >= doc_ids.len() {
        let _ = delete_case(&client, cfg, &case_id).await;
        return Err(anyhow!(
            "none of your files had readable text. Send text-based .docx/.pdf or a clearer scan"
        ));
    }
    if !progress.agent_errors.is_empty() {
        let _ = bot
            .send_message(
                chat_id,
                format!(
                    "⚠️ Some analysis agents reported errors: {}.",
                    progress.agent_errors.join("; ")
                ),
            )
            .await;
    }

    // 5) Generate each output and deliver it as a .docx with a short TL;DR.
    status
        .force(
            bot,
            chat_id,
            "📝 Generating brief, strategy memo, list of dates & annexure index…".to_string(),
        )
        .await;
    const OUTPUTS: [(&str, &str); 4] = [
        ("brief", "Case Brief"),
        ("strategy-memo", "Strategy Memo"),
        ("list-of-dates", "List of Dates"),
        ("annexure-index", "Annexure Index"),
    ];
    let mut delivered = 0usize;
    for (slug, label) in OUTPUTS {
        status
            .set(bot, chat_id, format!("📝 Generating {label}…"), false)
            .await;
        match generate_output(&client, cfg, &case_id, slug).await {
            Ok((content_md, docx_id)) => match fetch_document(cfg, &docx_id).await {
                Ok(bytes) => {
                    let fname = format!("{}.docx", label.replace(' ', "_"));
                    let file = InputFile::memory(bytes).file_name(fname);
                    let caption = tldr(&content_md, TLDR_CHARS);
                    let req = bot.send_document(chat_id, file);
                    let req = if caption.trim().is_empty() {
                        req
                    } else {
                        req.caption(format!("📄 {label}\n\n{caption}"))
                    };
                    if let Err(e) = req.await {
                        tracing::error!("send {label}: {e:#}");
                        let _ = bot
                            .send_message(
                                chat_id,
                                format!("⚠️ Generated the {label} but couldn't send the file."),
                            )
                            .await;
                    } else {
                        delivered += 1;
                    }
                }
                Err(e) => {
                    tracing::error!("render {label}: {e:#}");
                    let _ = bot
                        .send_message(
                            chat_id,
                            format!("⚠️ Generated the {label} but couldn't render the .docx."),
                        )
                        .await;
                }
            },
            Err(e) => {
                tracing::error!("generate {label}: {e:#}");
                let _ = bot
                    .send_message(chat_id, format!("⚠️ Couldn't generate the {label}: {e}."))
                    .await;
            }
        }
    }

    // 6) Close out with a real summary (no dangling status).
    let summary = if delivered > 0 {
        format!(
            "✅ Case Prep done. Delivered {delivered} output(s) above.\n\
             Case \"{title}\" is saved in your account; open the app to review or delete it."
        )
    } else {
        format!(
            "⚠️ Case Prep ran but produced no deliverable outputs. \
             Case \"{title}\" is saved; open the app to inspect it."
        )
    };
    status.force(bot, chat_id, summary).await;
    Ok(())
}

/// Fetch a staged Telegram file's bytes via getFile + the file download stream.
async fn download_telegram_file(bot: &Bot, file_id: &FileId) -> Result<Vec<u8>> {
    let file = bot
        .get_file(file_id.clone())
        .await
        .map_err(|e| anyhow!("getFile failed: {e}"))?;
    let mut stream = bot.download_file_stream(&file.path);
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| anyhow!("download error: {e}"))?;
        bytes.extend_from_slice(&chunk);
        // Backstop the cap on the real byte count — Telegram reports size 0 for
        // some photos, which would slip past the staging-time `reject_file`.
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(anyhow!(
                "file exceeds the {} MB limit",
                MAX_FILE_BYTES / (1024 * 1024)
            ));
        }
    }
    Ok(bytes)
}

/// Parse a backend JSON response, mapping 401 → AUTH_ERR and other non-2xx to a
/// readable error (so the user sees a real message, not a raw status code).
async fn json_or_err(resp: reqwest::Response, what: &str) -> Result<Value> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(anyhow!("{AUTH_ERR}"));
    }
    if !status.is_success() {
        let detail = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "{what}: backend returned {status} {}",
            detail.chars().take(200).collect::<String>()
        ));
    }
    resp.json::<Value>()
        .await
        .map_err(|e| anyhow!("{what}: bad response: {e}"))
}

/// `POST /cases` → the new case id.
async fn create_case(client: &reqwest::Client, cfg: &BotConfig, title: &str) -> Result<String> {
    let url = format!("{}/cases", cfg.api_url.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .json(&json!({ "title": title }))
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    let v = json_or_err(resp, "create case").await?;
    v.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("create case: no id in response"))
}

/// `POST /document` (multipart) → the new document id. The filename's extension
/// lets the backend type the file and route scans/images through OCR.
async fn upload_document(
    client: &reqwest::Client,
    cfg: &BotConfig,
    filename: &str,
    bytes: Vec<u8>,
) -> Result<String> {
    let url = format!("{}/document", cfg.api_url.trim_end_matches('/'));
    let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string());
    let form = reqwest::multipart::Form::new().part("file", part);
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .multipart(form)
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    let v = json_or_err(resp, "upload").await?;
    v.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("upload: no document id in response"))
}

/// `POST /cases/:id/documents` — link uploaded documents to the case.
async fn attach_documents(
    client: &reqwest::Client,
    cfg: &BotConfig,
    case_id: &str,
    doc_ids: &[String],
) -> Result<()> {
    let url = format!(
        "{}/cases/{}/documents",
        cfg.api_url.trim_end_matches('/'),
        case_id
    );
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .json(&json!({ "document_ids": doc_ids }))
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    json_or_err(resp, "attach").await?;
    Ok(())
}

/// `POST /cases/:id/analyze` — consume the SSE stream to completion, surfacing
/// progress through `status`, and return what we observed (unreadable files,
/// agent errors, OCR usage).
async fn run_analysis(
    cfg: &BotConfig,
    case_id: &str,
    bot: &Bot,
    chat_id: ChatId,
    status: &mut Status,
) -> Result<AnalyzeProgress> {
    let url = format!(
        "{}/cases/{}/analyze",
        cfg.api_url.trim_end_matches('/'),
        case_id
    );
    // No total timeout here — the analysis legitimately streams for minutes; the
    // loop below enforces an *idle* timeout instead. A connect timeout still
    // bounds an unreachable backend.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .header("accept", "text/event-stream")
        .json(&json!({ "redact_pii": false }))
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    let s = resp.status();
    if s == reqwest::StatusCode::UNAUTHORIZED {
        return Err(anyhow!("{AUTH_ERR}"));
    }
    if !s.is_success() {
        let detail = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "analyze: backend returned {s} {}",
            detail.chars().take(200).collect::<String>()
        ));
    }

    let mut buf = String::new();
    let mut prog = AnalyzeProgress::default();
    let mut stream = resp.bytes_stream();
    loop {
        while let Some(pos) = buf.find("\n\n") {
            let raw: String = buf.drain(..pos + 2).collect();
            apply_analyze_event(&raw, &mut prog);
        }
        report_progress(bot, chat_id, status, &prog).await;
        if prog.done {
            break;
        }
        match tokio::time::timeout(ANALYZE_IDLE_TIMEOUT, stream.next()).await {
            Err(_elapsed) => {
                return Err(anyhow!(
                    "analysis stalled (no update for {}s)",
                    ANALYZE_IDLE_TIMEOUT.as_secs()
                ));
            }
            Ok(Some(chunk)) => {
                let chunk = chunk.map_err(|e| anyhow!("stream error: {e}"))?;
                buf.push_str(&String::from_utf8_lossy(&chunk));
            }
            Ok(None) => break,
        }
    }
    if !buf.trim().is_empty() {
        apply_analyze_event(&buf, &mut prog);
    }
    if let Some(e) = prog.fatal_error.clone() {
        return Err(anyhow!("{e}"));
    }
    // Stream closed without a terminal "done" → the backend dropped mid-analysis.
    // Bail rather than generate outputs off an incomplete run.
    if !prog.done {
        return Err(anyhow!(
            "analysis ended early; the connection closed before it finished"
        ));
    }
    Ok(prog)
}

/// Build a human progress line from the current analysis state and (throttled)
/// edit the status message in place.
async fn report_progress(bot: &Bot, chat_id: ChatId, status: &mut Status, prog: &AnalyzeProgress) {
    let text = if !prog.agents_seen.is_empty() {
        let total = prog.agents_seen.len().max(prog.agents_done);
        format!("🧠 Running analysis agents… {}/{} done", prog.agents_done, total)
    } else if prog.extracted > 0 {
        let total = if prog.total_docs == 0 {
            prog.extracted
        } else {
            prog.total_docs
        };
        format!("🔍 Extracting text… {}/{} document(s)", prog.extracted, total)
    } else if let Some(stage) = &prog.stage {
        format!("🔍 {stage}")
    } else {
        return;
    };
    status.set(bot, chat_id, text, false).await;
}

/// `POST /cases/:id/outputs/:slug` → (markdown, docx document id). A 422 here
/// means analysis produced no findings (surfaced to the user by the caller).
async fn generate_output(
    client: &reqwest::Client,
    cfg: &BotConfig,
    case_id: &str,
    slug: &str,
) -> Result<(String, String)> {
    let url = format!(
        "{}/cases/{}/outputs/{}",
        cfg.api_url.trim_end_matches('/'),
        case_id,
        slug
    );
    let resp = client
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .json(&json!({ "redact_pii": false }))
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    let v = json_or_err(resp, slug).await?;
    let content = v
        .get("content_md")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let docx = v
        .get("docx_document_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{slug}: response had no docx_document_id"))?;
    Ok((content, docx))
}

/// `DELETE /cases/:id` — best-effort cleanup when a run aborts before producing
/// anything; ignored on error (the PTEST- case is harmless and deletable later).
async fn delete_case(client: &reqwest::Client, cfg: &BotConfig, case_id: &str) -> Result<()> {
    let url = format!("{}/cases/{}", cfg.api_url.trim_end_matches('/'), case_id);
    client
        .delete(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .send()
        .await
        .map_err(|e| anyhow!("delete case: {e}"))?;
    Ok(())
}

/// What we learn from the analyze SSE stream.
#[derive(Default)]
struct AnalyzeProgress {
    stage: Option<String>,
    extracted: usize,
    total_docs: usize,
    /// Filenames whose extracted text was empty (char_count == 0).
    unreadable: Vec<String>,
    ocr_used: bool,
    agents_seen: HashSet<String>,
    agents_done: usize,
    agent_errors: Vec<String>,
    done: bool,
    fatal_error: Option<String>,
}

/// One status message edited in place through the run.
struct Status {
    id: MessageId,
    last: String,
    last_edit: Instant,
}

impl Status {
    fn new(id: MessageId) -> Self {
        Status {
            id,
            last: String::new(),
            last_edit: Instant::now(),
        }
    }

    /// Edit the status, unless the text is unchanged. ponytail: a 1.5s throttle
    /// dodges Telegram's ~1/s edit-rate limit when agent events arrive in a
    /// burst; phase boundaries call `force` to always land. Edit errors are
    /// cosmetic, so they're swallowed.
    async fn set(&mut self, bot: &Bot, chat_id: ChatId, text: String, force: bool) {
        if text == self.last {
            return;
        }
        if !force && self.last_edit.elapsed() < Duration::from_millis(1500) {
            return;
        }
        self.last = text.clone();
        self.last_edit = Instant::now();
        let _ = bot.edit_message_text(chat_id, self.id, text).await;
    }

    async fn force(&mut self, bot: &Bot, chat_id: ChatId, text: String) {
        self.set(bot, chat_id, text, true).await;
    }
}

/// Reject a staged file that breaks a cap, returning a message naming the
/// offending value and the limit. `None` means the file is within limits.
fn reject_file(current_count: usize, incoming_bytes: u64) -> Option<String> {
    if incoming_bytes > MAX_FILE_BYTES {
        return Some(format!(
            "⚠️ That file is {:.1} MB, over the {} MB per-file limit (Telegram won't let a bot \
             download anything larger). Skipped it.",
            incoming_bytes as f64 / (1024.0 * 1024.0),
            MAX_FILE_BYTES / (1024 * 1024),
        ));
    }
    if current_count >= MAX_FILES {
        return Some(format!(
            "⚠️ You've already staged the maximum of {MAX_FILES} files for one Case Prep run. \
             Run /analyze on these, or /reset to start over."
        ));
    }
    None
}

/// PTEST-prefixed case title so the bot-created case is isolated and easy to
/// find and delete; the prefix is what the test harness asserts on.
fn case_title(file_count: usize) -> String {
    format!(
        "PTEST-Case Prep ({file_count} doc{})",
        if file_count == 1 { "" } else { "s" }
    )
}

/// A short, char-safe preview of an output's markdown for the .docx caption.
fn tldr(content_md: &str, max: usize) -> String {
    let trimmed = content_md.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let kept: String = trimmed.chars().take(max).collect();
    format!("{kept}…")
}

/// Map a MIME essence to a file extension so an extension-less upload still
/// types correctly on the backend. Unknown → "bin" (backend falls back to sniff).
fn ext_for_mime(essence: &str) -> &'static str {
    match essence {
        "application/pdf" => "pdf",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/tiff" => "tiff",
        _ => "bin",
    }
}

/// Extract the JSON payload from one SSE event block (`data: {…}` line). Skips
/// the `[DONE]` sentinel and any non-JSON keepalive lines.
fn sse_data_json(raw: &str) -> Option<Value> {
    for line in raw.lines() {
        let line = line.trim_start();
        if let Some(rest) = line.strip_prefix("data:") {
            let payload = rest.trim();
            if payload.is_empty() || payload == "[DONE]" {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(payload) {
                return Some(v);
            }
        }
    }
    None
}

/// Fold one analyze SSE event into the running `AnalyzeProgress`.
fn apply_analyze_event(raw: &str, p: &mut AnalyzeProgress) {
    let Some(v) = sse_data_json(raw) else {
        return;
    };
    match v.get("type").and_then(Value::as_str) {
        Some("stage") => {
            if let Some(m) = v.get("message").and_then(Value::as_str) {
                p.stage = Some(m.to_string());
            }
        }
        Some("extracting_doc") => {
            if let Some(t) = v.get("total_docs").and_then(Value::as_u64) {
                p.total_docs = t as usize;
            }
        }
        Some("extracted_doc") => {
            p.extracted += 1;
            if let Some(t) = v.get("total_docs").and_then(Value::as_u64) {
                p.total_docs = t as usize;
            }
            if v.get("needed_ocr").and_then(Value::as_bool) == Some(true) {
                p.ocr_used = true;
            }
            // char_count == 0 is the backend's "no readable text" signal (it
            // stores an empty marker when extraction/OCR yields nothing).
            let chars = v.get("char_count").and_then(Value::as_u64).unwrap_or(0);
            if chars == 0 {
                let name = v
                    .get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or("a file")
                    .to_string();
                p.unreadable.push(name);
            }
        }
        Some("agent_status") => {
            let name = v
                .get("agent_name")
                .and_then(Value::as_str)
                .unwrap_or("agent")
                .to_string();
            let st = v.get("status").and_then(Value::as_str).unwrap_or("");
            p.agents_seen.insert(name.clone());
            match st {
                "done" => p.agents_done += 1,
                "error" => {
                    let e = v.get("error").and_then(Value::as_str).unwrap_or("failed");
                    p.agent_errors.push(format!("{name}: {e}"));
                }
                _ => {}
            }
        }
        Some("done") => p.done = true,
        Some("error") => {
            let e = v
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| v.get("error").and_then(Value::as_str))
                .unwrap_or("analysis failed");
            p.fatal_error = Some(e.to_string());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversize_file_with_its_size() {
        let over = MAX_FILE_BYTES + 1;
        let msg = reject_file(0, over).expect("oversize file must be rejected");
        assert!(msg.contains("MB"), "message should name the size: {msg}");
    }

    #[test]
    fn rejects_over_count_with_the_limit_number() {
        let msg = reject_file(MAX_FILES, 1024).expect("over-count must be rejected");
        assert!(
            msg.contains(&MAX_FILES.to_string()),
            "message should name the {MAX_FILES} limit: {msg}"
        );
    }

    #[test]
    fn accepts_file_within_limits() {
        assert!(reject_file(0, 1024).is_none());
        assert!(reject_file(MAX_FILES - 1, MAX_FILE_BYTES).is_none());
    }

    #[test]
    fn case_title_is_ptest_scoped() {
        assert!(case_title(3).starts_with("PTEST-"), "{}", case_title(3));
        assert!(case_title(1).contains("1 doc"));
        assert!(case_title(2).contains("2 docs"));
    }

    #[test]
    fn tldr_truncates_long_text_and_keeps_short() {
        let short = "A brief summary.";
        assert_eq!(tldr(short, 600), short);
        let long = "x".repeat(2000);
        let out = tldr(&long, 600);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 601); // 600 kept + ellipsis
    }

    #[test]
    fn flags_doc_with_no_readable_text() {
        let mut p = AnalyzeProgress::default();
        apply_analyze_event(
            "data: {\"type\":\"extracted_doc\",\"filename\":\"blank_scan.png\",\"doc_index\":0,\"total_docs\":2,\"page_count\":1,\"needed_ocr\":true,\"char_count\":0}\n\n",
            &mut p,
        );
        apply_analyze_event(
            "data: {\"type\":\"extracted_doc\",\"filename\":\"petition.pdf\",\"doc_index\":1,\"total_docs\":2,\"page_count\":12,\"needed_ocr\":false,\"char_count\":8421}\n\n",
            &mut p,
        );
        assert_eq!(p.extracted, 2);
        assert_eq!(p.total_docs, 2);
        assert!(p.ocr_used);
        assert_eq!(p.unreadable, vec!["blank_scan.png".to_string()]);
    }

    #[test]
    fn tracks_agents_and_terminal_events() {
        let mut p = AnalyzeProgress::default();
        for ev in [
            "data: {\"type\":\"agent_status\",\"agent_name\":\"case_summary\",\"status\":\"running\"}\n\n",
            "data: {\"type\":\"agent_status\",\"agent_name\":\"case_summary\",\"status\":\"done\"}\n\n",
            "data: {\"type\":\"agent_status\",\"agent_name\":\"risk_assessor\",\"status\":\"error\",\"error\":\"timeout\"}\n\n",
            "data: {\"type\":\"done\"}\n\n",
        ] {
            apply_analyze_event(ev, &mut p);
        }
        assert_eq!(p.agents_seen.len(), 2);
        assert_eq!(p.agents_done, 1);
        assert_eq!(p.agent_errors, vec!["risk_assessor: timeout".to_string()]);
        assert!(p.done);
        assert!(p.fatal_error.is_none());
    }

    #[test]
    fn captures_fatal_analyze_error() {
        let mut p = AnalyzeProgress::default();
        apply_analyze_event(
            "data: {\"type\":\"error\",\"message\":\"model unavailable\"}\n\n",
            &mut p,
        );
        assert_eq!(p.fatal_error.as_deref(), Some("model unavailable"));
    }

    #[test]
    fn sse_skips_done_sentinel_and_keepalives() {
        assert!(sse_data_json("data: [DONE]\n\n").is_none());
        assert!(sse_data_json(": keepalive\n\n").is_none());
        assert!(sse_data_json("data: {\"type\":\"done\"}\n\n").is_some());
    }

    #[test]
    fn accumulates_content_delta_in_order() {
        let sse = "data: {\"type\":\"chat_id\",\"chatId\":\"abc\"}\n\n\
                   data: {\"type\":\"content_delta\",\"text\":\"Hello \"}\n\n\
                   data: {\"type\":\"content_delta\",\"text\":\"world\"}\n\n\
                   data: {\"type\":\"citations\",\"citations\":[]}\n\n";
        let mut state = StreamState::default();
        for block in sse.split_inclusive("\n\n") {
            apply_sse_event(block, &mut state);
        }
        assert_eq!(state.outcome.reply, "Hello world");
        assert!(state.error.is_none());
    }

    #[test]
    fn captures_first_error_event() {
        let mut state = StreamState::default();
        apply_sse_event(
            "data: {\"type\":\"error\",\"message\":\"model unavailable\"}\n\n",
            &mut state,
        );
        assert_eq!(state.error.as_deref(), Some("model unavailable"));
    }

    #[test]
    fn ignores_unknown_and_malformed_events() {
        let mut state = StreamState::default();
        apply_sse_event("data: not-json\n\n", &mut state);
        apply_sse_event(
            "data: {\"type\":\"reasoning_delta\",\"text\":\"x\"}\n\n",
            &mut state,
        );
        assert!(state.outcome.reply.is_empty());
        assert!(state.error.is_none());
    }

    #[test]
    fn collects_doc_created_and_citations() {
        let sse = "data: {\"type\":\"content_delta\",\"text\":\"draft ready\"}\n\n\
                   data: {\"type\":\"doc_created\",\"filename\":\"Notice.docx\",\"download_url\":\"/document/abc/docx\",\"document_id\":\"abc\",\"body\":\"...\"}\n\n\
                   data: {\"type\":\"citations\",\"citations\":[{\"ref\":1,\"filename\":\"Act.pdf\",\"page\":42},{\"ref\":2,\"filename\":\"Act.pdf\",\"page\":42},{\"ref\":3,\"filename\":\"Rule.pdf\",\"page\":3}]}\n\n";
        let mut state = StreamState::default();
        for block in sse.split_inclusive("\n\n") {
            apply_sse_event(block, &mut state);
        }
        assert_eq!(state.outcome.docs.len(), 1);
        assert_eq!(state.outcome.docs[0].filename, "Notice.docx");
        assert_eq!(state.outcome.docs[0].document_id, "abc");
        // 3 citations in, but Act.pdf p.42 is duplicated → 2 footnote lines.
        let footnote = build_citations_footnote(&state.outcome.citations).unwrap();
        assert_eq!(
            footnote,
            "\n\n📚 Sources:\n[1] Act.pdf (p.42)\n[2] Rule.pdf (p.3)"
        );
    }

    #[test]
    fn parses_clarifying_tool_request_with_questions() {
        let mut state = StreamState::default();
        apply_sse_event(
            "data: {\"type\":\"client_tool_request\",\"request_id\":\"req-1\",\"name\":\"ask_clarifying_questions\",\"arguments\":{\"questions\":[{\"header\":\"Forum\",\"question\":\"Which track?\",\"multiSelect\":false,\"options\":[{\"label\":\"FIR\",\"description\":\"police\"},{\"label\":\"Private complaint\"}]}]}}\n\n",
            &mut state,
        );
        let req = state.pending_tool.expect("a tool request was parsed");
        assert_eq!(req.request_id, "req-1");
        assert_eq!(req.questions.len(), 1);
        assert_eq!(req.questions[0].question, "Which track?");
        assert!(!req.questions[0].multi_select);
        assert_eq!(req.questions[0].options.len(), 2);
        assert_eq!(req.questions[0].options[0].label, "FIR");
        assert_eq!(
            req.questions[0].options[0].description.as_deref(),
            Some("police")
        );
        assert_eq!(req.questions[0].options[1].description, None);
    }

    #[test]
    fn tool_request_drops_question_without_options() {
        let mut state = StreamState::default();
        apply_sse_event(
            "data: {\"type\":\"client_tool_request\",\"request_id\":\"r\",\"name\":\"ask_clarifying_questions\",\"arguments\":{\"questions\":[{\"header\":\"h\",\"question\":\"no opts?\",\"options\":[]}]}}\n\n",
            &mut state,
        );
        let req = state.pending_tool.expect("request present");
        assert!(req.questions.is_empty());
    }

    #[test]
    fn empty_citations_have_no_footnote() {
        assert!(build_citations_footnote(&[]).is_none());
    }

    #[test]
    fn short_reply_is_single_chunk() {
        assert_eq!(split_for_telegram("hello"), vec!["hello".to_string()]);
    }

    #[test]
    fn long_reply_is_chunked_under_limit() {
        let text = "a".repeat(10_000);
        let chunks = split_for_telegram(&text);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= TELEGRAM_LIMIT));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn parse_callback_round_trips() {
        assert!(matches!(
            parse_callback("q0:o2"),
            Some(CbAction::Option { q_idx: 0, o_idx: 2 })
        ));
        assert!(matches!(
            parse_callback("q1:done"),
            Some(CbAction::Done { q_idx: 1 })
        ));
        assert!(matches!(parse_callback("proceed"), Some(CbAction::Proceed)));
        assert!(parse_callback("garbage").is_none());
        assert!(parse_callback("q0:oX").is_none());
        assert!(parse_callback("qX:o0").is_none());
    }

    #[test]
    fn build_clarify_result_matches_contract() {
        let questions = vec![ClarifyQuestion {
            header: "Forum".into(),
            question: "Which track?".into(),
            multi_select: false,
            options: vec![
                ClarifyOption {
                    label: "FIR".into(),
                    description: None,
                },
                ClarifyOption {
                    label: "Private".into(),
                    description: None,
                },
            ],
        }];
        let collected = vec![vec!["FIR".to_string()]];
        let result = build_clarify_result(&questions, &collected);
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["proceed"], json!(false));
        assert_eq!(v["answers"][0]["question"], json!("Which track?"));
        assert_eq!(v["answers"][0]["selected"], json!(["FIR"]));
    }

    #[test]
    fn single_select_keyboard_has_options_and_proceed() {
        let q = ClarifyQuestion {
            header: "H".into(),
            question: "Q?".into(),
            multi_select: false,
            options: vec![
                ClarifyOption {
                    label: "A".into(),
                    description: None,
                },
                ClarifyOption {
                    label: "B".into(),
                    description: None,
                },
            ],
        };
        let kb = render_question(&q, 0, &[]);
        // 2 option rows + 1 proceed row; no Done row for single-select.
        assert_eq!(kb.inline_keyboard.len(), 3);
        assert_eq!(kb.inline_keyboard[0][0].text, "A");
        assert_eq!(kb.inline_keyboard[2][0].text, "⏭️ Skip / proceed anyway");
    }

    #[test]
    fn multi_select_keyboard_marks_selected_and_has_done() {
        let q = ClarifyQuestion {
            header: "H".into(),
            question: "Q?".into(),
            multi_select: true,
            options: vec![
                ClarifyOption {
                    label: "A".into(),
                    description: None,
                },
                ClarifyOption {
                    label: "B".into(),
                    description: None,
                },
            ],
        };
        let kb = render_question(&q, 0, &[1]);
        // 2 options + Done + proceed = 4 rows; option B (index 1) is checked.
        assert_eq!(kb.inline_keyboard.len(), 4);
        assert!(kb.inline_keyboard[0][0].text.starts_with('⬜'));
        assert!(kb.inline_keyboard[1][0].text.starts_with('☑'));
        assert_eq!(kb.inline_keyboard[2][0].text, "✅ Done");
    }

    #[test]
    fn single_select_tap_finishes_and_records_label() {
        let (tx, _rx) = oneshot::channel();
        let mut st = PendingClarify {
            questions: vec![ClarifyQuestion {
                header: String::new(),
                question: "Q?".into(),
                multi_select: false,
                options: vec![
                    ClarifyOption {
                        label: "A".into(),
                        description: None,
                    },
                    ClarifyOption {
                        label: "B".into(),
                        description: None,
                    },
                ],
            }],
            current: 0,
            collected: Vec::new(),
            selected: Vec::new(),
            keyboard_msg: MessageId(1),
            tx,
        };
        match compute_next(&mut st, &CbAction::Option { q_idx: 0, o_idx: 1 }) {
            Next::Finish {
                outcome: ClarifyOutcome::Answered(result),
                ..
            } => {
                let v: Value = serde_json::from_str(&result).unwrap();
                assert_eq!(v["answers"][0]["selected"], json!(["B"]));
                assert_eq!(v["proceed"], json!(false));
            }
            _ => panic!("expected a finishing step"),
        }
    }

    #[test]
    fn multi_select_toggles_then_done_advances() {
        let (tx, _rx) = oneshot::channel();
        let mut st = PendingClarify {
            questions: vec![
                ClarifyQuestion {
                    header: String::new(),
                    question: "Q1?".into(),
                    multi_select: true,
                    options: vec![
                        ClarifyOption {
                            label: "A".into(),
                            description: None,
                        },
                        ClarifyOption {
                            label: "B".into(),
                            description: None,
                        },
                    ],
                },
                ClarifyQuestion {
                    header: String::new(),
                    question: "Q2?".into(),
                    multi_select: false,
                    options: vec![ClarifyOption {
                        label: "X".into(),
                        description: None,
                    }],
                },
            ],
            current: 0,
            collected: Vec::new(),
            selected: Vec::new(),
            keyboard_msg: MessageId(1),
            tx,
        };
        // Tap A then B → both selected.
        assert!(matches!(
            compute_next(&mut st, &CbAction::Option { q_idx: 0, o_idx: 0 }),
            Next::Stay { .. }
        ));
        assert!(matches!(
            compute_next(&mut st, &CbAction::Option { q_idx: 0, o_idx: 1 }),
            Next::Stay { .. }
        ));
        // Done → records [A, B] and advances to the second question.
        assert!(matches!(
            compute_next(&mut st, &CbAction::Done { q_idx: 0 }),
            Next::Advance { .. }
        ));
        assert_eq!(st.current, 1);
        assert_eq!(st.collected, vec![vec!["A".to_string(), "B".to_string()]]);
    }

    #[test]
    fn done_with_nothing_selected_is_invalid() {
        let (tx, _rx) = oneshot::channel();
        let mut st = PendingClarify {
            questions: vec![ClarifyQuestion {
                header: String::new(),
                question: "Q?".into(),
                multi_select: true,
                options: vec![ClarifyOption {
                    label: "A".into(),
                    description: None,
                }],
            }],
            current: 0,
            collected: Vec::new(),
            selected: Vec::new(),
            keyboard_msg: MessageId(1),
            tx,
        };
        assert!(matches!(
            compute_next(&mut st, &CbAction::Done { q_idx: 0 }),
            Next::Invalid(_)
        ));
    }

    #[test]
    fn stale_question_index_is_invalid() {
        let (tx, _rx) = oneshot::channel();
        let mut st = PendingClarify {
            questions: vec![ClarifyQuestion {
                header: String::new(),
                question: "Q?".into(),
                multi_select: false,
                options: vec![ClarifyOption {
                    label: "A".into(),
                    description: None,
                }],
            }],
            current: 0,
            collected: Vec::new(),
            selected: Vec::new(),
            keyboard_msg: MessageId(1),
            tx,
        };
        // A tap on question 5 while we're on question 0 must be rejected.
        assert!(matches!(
            compute_next(&mut st, &CbAction::Option { q_idx: 5, o_idx: 0 }),
            Next::Invalid(_)
        ));
    }

    // ---- additional coverage: pure helpers + uncovered branches ----

    /// A throwaway wizard for `compute_next` tests (its sender is unused here).
    fn pending_with(questions: Vec<ClarifyQuestion>) -> PendingClarify {
        let (tx, _rx) = oneshot::channel();
        PendingClarify {
            questions,
            current: 0,
            collected: Vec::new(),
            selected: Vec::new(),
            keyboard_msg: MessageId(1),
            tx,
        }
    }

    /// Concise `ClarifyQuestion` builder for tests.
    fn q(
        header: &str,
        question: &str,
        multi_select: bool,
        opts: &[(&str, Option<&str>)],
    ) -> ClarifyQuestion {
        ClarifyQuestion {
            header: header.into(),
            question: question.into(),
            multi_select,
            options: opts
                .iter()
                .map(|(l, d)| ClarifyOption {
                    label: (*l).into(),
                    description: d.map(str::to_string),
                })
                .collect(),
        }
    }

    #[test]
    fn trim_history_drops_oldest_over_cap() {
        let mut turns: Vec<ChatMsg> = (0..MAX_HISTORY_MESSAGES + 5)
            .map(|i| ChatMsg {
                role: "user".into(),
                content: i.to_string(),
            })
            .collect();
        let dropped = trim_history(&mut turns);
        assert_eq!(dropped, 5);
        assert_eq!(turns.len(), MAX_HISTORY_MESSAGES);
        // The 5 oldest turns were dropped; the first survivor is "5".
        assert_eq!(turns[0].content, "5");
        // A second trim at the cap drops nothing.
        assert_eq!(trim_history(&mut turns), 0);
    }

    #[test]
    fn trim_history_leaves_short_history_untouched() {
        let mut turns = vec![ChatMsg {
            role: "user".into(),
            content: "a".into(),
        }];
        assert_eq!(trim_history(&mut turns), 0);
        assert_eq!(turns.len(), 1);
    }

    #[test]
    fn render_question_text_includes_number_header_legend_and_hint() {
        let question = q(
            "Forum",
            "Which track?",
            true,
            &[("FIR", Some("police")), ("Private", None)],
        );
        let text = render_question_text(&question, 0, 2);
        assert!(text.contains("Question 1/2"));
        assert!(text.contains("[Forum]"));
        assert!(text.contains("Which track?"));
        assert!(text.contains("• FIR — police"));
        // An option without a description contributes no legend line.
        assert!(!text.contains("• Private"));
        assert!(text.contains("Select all that apply"));
    }

    #[test]
    fn render_question_text_minimal_single_question() {
        let question = q("", "Only one?", false, &[("A", None), ("B", None)]);
        let text = render_question_text(&question, 0, 1);
        // total==1 → no "Question n/N"; empty header → no "[...]"; no
        // descriptions → no legend; single-select → no multi-select hint.
        assert_eq!(text, "Only one?");
    }

    #[test]
    fn proceed_button_finishes_with_proceed_outcome() {
        let mut st = pending_with(vec![q("", "Q?", false, &[("A", None), ("B", None)])]);
        assert!(matches!(
            compute_next(&mut st, &CbAction::Proceed),
            Next::Finish {
                outcome: ClarifyOutcome::Proceed,
                ..
            }
        ));
    }

    #[test]
    fn done_on_single_select_question_is_invalid() {
        let mut st = pending_with(vec![q("", "Q?", false, &[("A", None), ("B", None)])]);
        assert!(matches!(
            compute_next(&mut st, &CbAction::Done { q_idx: 0 }),
            Next::Invalid(_)
        ));
    }

    #[test]
    fn out_of_range_option_is_invalid() {
        let mut st = pending_with(vec![q("", "Q?", false, &[("A", None)])]);
        assert!(matches!(
            compute_next(&mut st, &CbAction::Option { q_idx: 0, o_idx: 9 }),
            Next::Invalid(_)
        ));
    }

    #[test]
    fn single_select_first_of_two_advances_and_records() {
        let mut st = pending_with(vec![
            q("", "Q1?", false, &[("A", None), ("B", None)]),
            q("", "Q2?", false, &[("X", None)]),
        ]);
        assert!(matches!(
            compute_next(&mut st, &CbAction::Option { q_idx: 0, o_idx: 0 }),
            Next::Advance { .. }
        ));
        assert_eq!(st.current, 1);
        assert_eq!(st.collected, vec![vec!["A".to_string()]]);
    }

    #[test]
    fn split_for_telegram_splits_multiline_on_boundaries() {
        // 200 lines of 100 chars each (~20k chars) → several chunks, each a
        // run of whole lines (the line-boundary accumulation path).
        let text: String = (0..200)
            .map(|i| format!("line-{i:03}-{}\n", "x".repeat(90)))
            .collect();
        let chunks = split_for_telegram(&text);
        assert!(chunks.len() >= 2);
        assert!(chunks.iter().all(|c| c.chars().count() <= TELEGRAM_LIMIT));
        assert_eq!(chunks.concat(), text);
        // No chunk starts mid-line: boundary splitting kept whole lines together.
        assert!(chunks.iter().all(|c| c.starts_with("line-")));
    }

    // ---- in-flight per-chat guard (HIGH: concurrent same-chat history race) ----

    #[test]
    fn second_concurrent_message_for_same_chat_is_rejected() {
        let set: InFlight = Arc::new(StdMutex::new(HashSet::new()));
        let chat = ChatId(1);
        // First turn claims the slot.
        let g1 = try_acquire_inflight(&set, chat);
        assert!(
            g1.is_some(),
            "first message must acquire the in-flight slot"
        );
        // A second message for the SAME chat while the first runs is rejected.
        assert!(
            try_acquire_inflight(&set, chat).is_none(),
            "second concurrent message for the same chat must be rejected"
        );
        // A different chat is unaffected — turns are serialized per chat, not globally.
        let g_other = try_acquire_inflight(&set, ChatId(2));
        assert!(g_other.is_some(), "a different chat must not be blocked");
    }

    #[test]
    fn in_flight_slot_is_released_on_drop() {
        let set: InFlight = Arc::new(StdMutex::new(HashSet::new()));
        let chat = ChatId(7);
        {
            let _g = try_acquire_inflight(&set, chat).expect("acquire");
            assert!(set.lock().unwrap().contains(&chat));
        } // guard dropped here (mirrors handle_text returning / `?` early-exit)
        assert!(
            !set.lock().unwrap().contains(&chat),
            "dropping the guard must clear the slot so the next turn can run"
        );
        // The chat can be claimed again now that the previous turn finished.
        assert!(try_acquire_inflight(&set, chat).is_some());
    }

    /// Reproduces the HIGH finding's interleaving and proves the in-flight guard
    /// serializes same-chat turns so history is NOT corrupted.
    ///
    /// Models `handle_text`'s exact pattern: lock briefly to push the user turn,
    /// release across an `.await` (the `call_chat` round), lock again to push the
    /// assistant turn. Without serialization two concurrent turns interleave as
    /// `[user1, user2, assistant2, assistant1]`. With the guard the 2nd turn is
    /// rejected, so history stays a clean `[user1, assistant1]`.
    #[tokio::test]
    async fn concurrent_same_chat_turns_do_not_interleave_history() {
        let history: History = Arc::new(Mutex::new(HashMap::new()));
        let in_flight: InFlight = Arc::new(StdMutex::new(HashSet::new()));
        let chat = ChatId(42);

        // One simulated turn, mirroring handle_text's lock/await/lock shape.
        async fn one_turn(
            history: History,
            in_flight: InFlight,
            chat: ChatId,
            user: &str,
            assistant: &str,
            backend_delay: Duration,
        ) -> bool {
            let Some(_guard) = try_acquire_inflight(&in_flight, chat) else {
                return false; // rejected: a turn for this chat is already running
            };
            {
                let mut map = history.lock().await;
                map.entry(chat).or_default().push(ChatMsg {
                    role: "user".into(),
                    content: user.into(),
                });
            }
            // Stand-in for the `call_chat(...).await` round — the history lock is
            // NOT held here, which is exactly what let the race happen.
            tokio::time::sleep(backend_delay).await;
            {
                let mut map = history.lock().await;
                map.entry(chat).or_default().push(ChatMsg {
                    role: "assistant".into(),
                    content: assistant.into(),
                });
            }
            true
        }

        // Fire two turns for the same chat "at once". The first is slow; the
        // second arrives while the first is still in its backend round.
        let t1 = tokio::spawn(one_turn(
            history.clone(),
            in_flight.clone(),
            chat,
            "user1",
            "assistant1",
            Duration::from_millis(80),
        ));
        tokio::time::sleep(Duration::from_millis(10)).await;
        let t2 = tokio::spawn(one_turn(
            history.clone(),
            in_flight.clone(),
            chat,
            "user2",
            "assistant2",
            Duration::from_millis(0),
        ));

        let ran1 = t1.await.unwrap();
        let ran2 = t2.await.unwrap();
        assert!(ran1, "the first turn runs");
        assert!(
            !ran2,
            "the second concurrent turn for the same chat is rejected"
        );

        let map = history.lock().await;
        let turns = map.get(&chat).expect("history for the chat");
        let seq: Vec<&str> = turns.iter().map(|m| m.content.as_str()).collect();
        // Clean, non-interleaved order — never [user1, user2, assistant2, assistant1].
        assert_eq!(seq, vec!["user1", "assistant1"]);
    }

    // ---- Answered path: a too-late tool-result is surfaced (MEDIUM) ----

    /// A one-shot HTTP server on a fresh port that replies with `status` to a
    /// single request, then exits. Returns the bound `http://127.0.0.1:PORT`.
    fn one_shot_server(status_line: &'static str) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let resp = format!("{status_line}\r\nContent-Length: 0\r\n\r\n");
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        url
    }

    fn test_cfg(api_url: String) -> BotConfig {
        BotConfig {
            api_url,
            session_token: String::new(),
            model: None,
        }
    }

    #[tokio::test]
    async fn post_tool_result_returns_true_when_answer_lands() {
        let url = one_shot_server("HTTP/1.1 200 OK");
        let cfg = test_cfg(url);
        let client = reqwest::Client::new();
        let landed = post_tool_result(&cfg, &client, "req-1", PROCEED_RESULT)
            .await
            .expect("transport ok");
        assert!(landed, "a 2xx means the answer landed");
    }

    #[tokio::test]
    async fn post_tool_result_returns_false_when_answer_is_too_late() {
        // 404 = the backend already timed out this tool call. The tapped answer
        // did NOT land, so the Answered path must tell the user instead of
        // discarding it silently.
        let url = one_shot_server("HTTP/1.1 404 Not Found");
        let cfg = test_cfg(url);
        let client = reqwest::Client::new();
        let landed = post_tool_result(&cfg, &client, "req-1", "{\"answers\":[]}")
            .await
            .expect("transport ok");
        assert!(!landed, "a 404 means the answer arrived too late");
    }
}
