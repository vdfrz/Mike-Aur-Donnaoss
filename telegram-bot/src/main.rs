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
use std::time::Duration;

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

/// Largest `.docx` the bot will accept for redlining. Sized to a real pleading
/// bundle plus headroom (a few-hundred-page brief is only a few MB), kept as the
/// single source of truth so the gate and the post-download check agree.
const MAX_DOC_BYTES: usize = 20 * 1024 * 1024;

/// The Word `.docx` MIME type, used both to gate uploads and to label the
/// multipart part we POST to the backend.
const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// Reply when the user sends a non-`.docx` document to redline. Caught bot-side
/// so the user isn't bounced through an upload first (mirrors the backend
/// `edit_document` guard in `builtin_tools.rs`).
const DOCX_ONLY_MSG: &str =
    "I can only redline Word `.docx` files for now — please re-send as `.docx`.";

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
    #[command(description = "teach Mike a lasting rule: /remember <text>")]
    Remember(String),
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
            "MIKE_SESSION_TOKEN is empty — calls will only succeed if the backend \
             runs with MIKE_BYPASS_AUTH=true."
        );
    }

    let bot = Bot::new(token);

    let history: History = Arc::new(Mutex::new(HashMap::new()));
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let warned: Warned = Arc::new(Mutex::new(HashSet::new()));
    let in_flight: InFlight = Arc::new(StdMutex::new(HashSet::new()));
    let redlines: Redlines = Arc::new(Mutex::new(HashMap::new()));

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(handle_command),
                )
                .branch(dptree::endpoint(handle_text)),
        )
        .branch(Update::filter_callback_query().endpoint(handle_callback));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![cfg, history, pending, warned, in_flight, redlines])
        // Process every update concurrently (not serialized per chat) so a
        // button tap can be handled *while* its `handle_text` is still parked
        // waiting for that tap — otherwise the two would deadlock.
        .distribution_function(|_| None::<()>)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

/// `/start`, `/help`, `/reset`, `/cancel` — static / state-only, no backend
/// round-trip (except `/cancel`, which wakes a waiting clarify flow).
// dptree injects every handler argument by type; allow the extra one.
#[allow(clippy::too_many_arguments)]
async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    cfg: BotConfig,
    history: History,
    pending: Pending,
    warned: Warned,
    redlines: Redlines,
) -> ResponseResult<()> {
    let text = match cmd {
        Command::Start => "👋 Mike aur Donna legal assistant.\n\n\
             Send me a question and I'll route it through your local backend \
             (research, drafting, and more). Use /help for details."
            .to_string(),
        Command::Help => "Send any message and I'll forward it to your Mike backend and stream \
             the answer back here. Long answers are split across messages.\n\n\
             If I need a decision before drafting, I'll show tappable buttons — \
             pick one (or several, then ✅ Done) and I'll continue.\n\n\
             /start — welcome message\n/help — this text\n\
             /reset — clear this chat's conversation history\n\
             /cancel — drop a pending clarifying question\n\
             /remember <text> — teach me a lasting rule I'll keep in mind"
            .to_string(),
        Command::Reset => {
            history.lock().await.remove(&msg.chat.id);
            warned.lock().await.remove(&msg.chat.id);
            redlines.lock().await.remove(&msg.chat.id);
            "🧹 Conversation history cleared.".to_string()
        }
        Command::Cancel => {
            let entry = pending.lock().await.remove(&msg.chat.id);
            match entry {
                Some(entry) => {
                    // Clear the dangling keyboard and wake the parked call_chat.
                    let _ = bot
                        .edit_message_text(msg.chat.id, entry.keyboard_msg, "🚫 Cancelled.")
                        .await;
                    let _ = entry.tx.send(ClarifyOutcome::Cancelled);
                    "🚫 Cancelled — tell me what you'd like to do instead.".to_string()
                }
                None => "Nothing to cancel right now.".to_string(),
            }
        }
        Command::Remember(rule) => {
            // Sends its own confirmation/usage/error reply, so return early
            // instead of falling through to the trailing send_message.
            remember_rule(&bot, msg.chat.id, &cfg, rule.trim()).await?;
            return Ok(());
        }
    };
    bot.send_message(msg.chat.id, text).await?;
    Ok(())
}

/// Any non-command text message → forward to `/chat` with this chat's running
/// history, reply with the answer (plus any drafts and a sources footnote).
// dptree injects every handler argument by type, so these can't be bundled into
// a struct without losing the injection — allow the extra arg.
#[allow(clippy::too_many_arguments)]
async fn handle_text(
    bot: Bot,
    msg: Message,
    cfg: BotConfig,
    history: History,
    pending: Pending,
    warned: Warned,
    in_flight: InFlight,
    redlines: Redlines,
) -> ResponseResult<()> {
    // A document upload → redline review flow (handled before the text-only
    // reject below, so a `.docx` no longer hits "I can only handle text…").
    if msg.document().is_some() {
        return handle_document(&bot, &msg, &cfg, &pending, &redlines).await;
    }

    let Some(user_text) = msg.text().map(str::to_string) else {
        bot.send_message(msg.chat.id, "I can only handle text messages for now.")
            .await?;
        return Ok(());
    };

    // If a document is parked awaiting "apply these as tracked changes?", an
    // affirmation runs the apply step instead of a normal chat turn. A
    // non-affirmation leaves it parked, so a later "yes" still works and an
    // ordinary question is answered normally.
    if is_affirmative(&user_text) {
        let parked = redlines.lock().await.remove(&msg.chat.id);
        if let Some(rl) = parked {
            return apply_redline(&bot, &msg, &cfg, &pending, rl).await;
        }
    }

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

    // Conservative "teach Mike a lasting rule" trigger: a message that clearly
    // states a standing instruction is sent to the Mike-listens harness instead
    // of being answered as a normal chat turn. Plain questions never match.
    // (After the in-flight guard so this teaching turn is serialized too.)
    if looks_like_a_rule(&user_text) {
        remember_rule(&bot, msg.chat.id, &cfg, &user_text).await?;
        return Ok(());
    }

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

    match call_chat(&bot, msg.chat.id, &cfg, &pending, &messages, None).await {
        Ok(outcome) => {
            let ChatOutcome {
                reply: model_reply,
                citations,
                docs,
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
async fn handle_callback(bot: Bot, q: CallbackQuery, pending: Pending) -> ResponseResult<()> {
    let Some(chat_id) = q.message.as_ref().map(|m| m.chat().id) else {
        bot.answer_callback_query(q.id).await?;
        return Ok(());
    };
    let Some(action) = q.data.as_deref().and_then(parse_callback) else {
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
    attach_doc: Option<&str>,
) -> Result<ChatOutcome> {
    let mut msg_json: Vec<Value> = messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    // Carry an uploaded document to the backend as `files[].document_id` on the
    // last message — the backend collects file ids across all messages and maps
    // them to chat-local labels (doc-0, …) at chat.rs:4420.
    if let (Some(doc_id), Some(last)) = (attach_doc, msg_json.last_mut()) {
        last["files"] = json!([{ "document_id": doc_id }]);
    }
    let mut body = json!({ "messages": msg_json });
    if let Some(model) = &cfg.model {
        body["model"] = json!(model);
    }

    let client = reqwest::Client::new();
    let url = format!("{}/chat/", cfg.api_url.trim_end_matches('/'));
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
async fn fetch_document(cfg: &BotConfig, document_id: &str) -> Result<Vec<u8>> {
    let url = format!(
        "{}/document/{}/docx",
        cfg.api_url.trim_end_matches('/'),
        document_id
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
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

// ---------------------------------------------------------------------------
// Memory (Loop M): teach Mike a lasting rule via the Mike-listens harness.
// ---------------------------------------------------------------------------

/// Conservative natural-language detector for a standing instruction. Matches a
/// small fixed set of phrases only, so an ordinary question never false-fires.
fn looks_like_a_rule(t: &str) -> bool {
    let l = t.to_lowercase();
    [
        "remember",
        "from now on",
        "in future",
        "in the future",
        "always",
        "never",
    ]
    .iter()
    .any(|p| l.contains(p))
}

/// POST a rule to the backend's Mike-listens harness (`/mike-feedback/chat`,
/// multipart field `message`). The harness learns it and auto-injects it into
/// every future `/chat`; the bot only triggers. Failures surface visibly.
async fn remember_rule(bot: &Bot, chat_id: ChatId, cfg: &BotConfig, text: &str) -> ResponseResult<()> {
    if text.is_empty() {
        bot.send_message(chat_id, "Usage: /remember <the rule you want me to keep>")
            .await?;
        return Ok(());
    }
    let url = format!("{}/mike-feedback/chat", cfg.api_url.trim_end_matches('/'));
    let form = reqwest::multipart::Form::new().text("message", text.to_string());
    let resp = reqwest::Client::new()
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .multipart(form)
        .send()
        .await;
    match resp {
        Ok(r) if r.status().is_success() => {
            // Drain the SSE so the harness commit completes server-side; the bot
            // doesn't need to parse it.
            let _ = r.bytes().await;
            bot.send_message(
                chat_id,
                "✅ Got it — I'll keep that in mind from now on. You can review or remove \
                 your saved rules on the Personalization page (Mike listens).",
            )
            .await?;
        }
        Ok(r) if r.status().as_u16() == 401 => {
            bot.send_message(
                chat_id,
                "⚠️ Couldn't save that — Mike rejected the request (auth). Check the bot's \
                 session token / backend bypass and try again.",
            )
            .await?;
        }
        _ => {
            bot.send_message(
                chat_id,
                "⚠️ Couldn't save that rule right now — please try again in a moment.",
            )
            .await?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Document redlining (Loop D): receive a `.docx` → rubric risk-review → confirm
// → `edit_document` tracked changes → deliver the redlined file back.
// ---------------------------------------------------------------------------

/// A document uploaded and risk-reviewed, parked until the user confirms with
/// "yes" so we can apply the tracked changes. Keyed by chat. Cleared on /reset.
struct RedlinePending {
    /// Backend document id (carried as `files[].document_id` into the apply turn).
    document_id: String,
    /// Original upload name, used to label the redlined file we send back.
    filename: String,
    /// The turn-1 review text, replayed as history so the apply turn has context.
    review: String,
}

/// Per-chat documents awaiting an "apply these changes?" confirmation.
type Redlines = Arc<Mutex<HashMap<ChatId, RedlinePending>>>;

/// The routing decision for an incoming document, computed from plain values so
/// it is unit-testable without teloxide or the network.
#[derive(Debug, PartialEq)]
enum DocGate {
    /// A `.docx` with an instruction caption — run the redline flow.
    Upload,
    /// Wrong type or too large — reply with `msg` and stop.
    Reject(String),
    /// A `.docx` but no caption — ask the user what to review/change.
    NeedCaption,
}

/// Gate an incoming document on (filename, mime, size, caption). Pure: no I/O.
/// `.docx`-only (by extension, plus the docx MIME when Telegram provides one),
/// under the size cap, and needs a caption (the instruction).
fn gate_document(
    file_name: Option<&str>,
    mime: Option<&str>,
    size: usize,
    caption: Option<&str>,
) -> DocGate {
    let is_docx_name = file_name
        .map(|n| n.to_ascii_lowercase().ends_with(".docx"))
        .unwrap_or(false);
    // When Telegram supplies a MIME, require the docx one; absence is allowed.
    let mime_ok = mime.map(|m| m == DOCX_MIME).unwrap_or(true);
    if !is_docx_name || !mime_ok {
        return DocGate::Reject(DOCX_ONLY_MSG.to_string());
    }
    if size > MAX_DOC_BYTES {
        return DocGate::Reject("That file is over the 20 MB limit.".to_string());
    }
    match caption {
        Some(c) if !c.trim().is_empty() => DocGate::Upload,
        _ => DocGate::NeedCaption,
    }
}

/// Conservative "yes, apply it" detector for the redline confirm step. A plain
/// follow-up question must NOT trigger an apply.
fn is_affirmative(t: &str) -> bool {
    let l = t.trim().to_lowercase();
    matches!(
        l.as_str(),
        "yes" | "y" | "yep" | "yeah" | "yes please" | "ok" | "okay" | "apply" | "apply it"
            | "go ahead" | "do it" | "sure"
    ) || l.starts_with("yes")
}

/// Stream a Telegram file into memory via teloxide (no `io-util` feature needed),
/// mirroring the byte-collection style already used elsewhere in the crate.
async fn download_doc(bot: &Bot, file_id: &FileId) -> Result<Vec<u8>> {
    let file = bot.get_file(file_id.clone()).await?;
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = bot.download_file_stream(&file.path);
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
    }
    Ok(buf)
}

/// Upload the `.docx` bytes to the backend as multipart (`file` part +
/// `cache=true` so `content_hash` is set and the chat-link survives) and return
/// the new document id.
async fn upload_doc(cfg: &BotConfig, bytes: Vec<u8>, filename: &str) -> Result<String> {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(DOCX_MIME)
        .map_err(|e| anyhow!("bad multipart part: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("cache", "true");
    let url = format!("{}/document", cfg.api_url.trim_end_matches('/'));
    let resp = reqwest::Client::new()
        .post(&url)
        .header("authorization", format!("Bearer {}", cfg.session_token))
        .multipart(form)
        .send()
        .await
        .map_err(|e| anyhow!("could not reach backend at {url}: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = resp.text().await.unwrap_or_default();
        return Err(anyhow!(
            "upload returned {status}: {}",
            detail.chars().take(200).collect::<String>()
        ));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| anyhow!("bad upload response: {e}"))?;
    v.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("upload response missing id"))
}

/// Handle a `.docx` upload: gate → download → upload → turn-1 rubric review →
/// ask to confirm, parking the doc for the follow-up "yes".
async fn handle_document(
    bot: &Bot,
    msg: &Message,
    cfg: &BotConfig,
    pending: &Pending,
    redlines: &Redlines,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let Some(doc) = msg.document() else {
        return Ok(());
    };

    let mime_owned = doc.mime_type.as_ref().map(|m| m.essence_str().to_string());
    match gate_document(
        doc.file_name.as_deref(),
        mime_owned.as_deref(),
        doc.file.size as usize,
        msg.caption(),
    ) {
        DocGate::Reject(m) => {
            bot.send_message(chat_id, m).await?;
            return Ok(());
        }
        DocGate::NeedCaption => {
            bot.send_message(
                chat_id,
                "Send the `.docx` again with a caption telling me what to review or \
                 change (e.g. 'risk-review this settlement').",
            )
            .await?;
            return Ok(());
        }
        DocGate::Upload => {}
    }

    let file_id = doc.file.id.clone();
    let filename = doc
        .file_name
        .clone()
        .unwrap_or_else(|| "document.docx".to_string());
    let caption = msg.caption().unwrap_or("").trim().to_string();

    let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;
    let status = bot
        .send_message(chat_id, "📄 Got the file — running a risk review…")
        .await
        .ok()
        .map(|m| m.id);

    // Download from Telegram (re-check the real byte length against the cap).
    let bytes = match download_doc(bot, &file_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("doc download failed: {e:#}");
            let t = "⚠️ Couldn't download that file from Telegram — please try again.";
            send_or_edit(bot, chat_id, status, t).await?;
            return Ok(());
        }
    };
    if bytes.len() > MAX_DOC_BYTES {
        send_or_edit(bot, chat_id, status, "That file is over the 20 MB limit.").await?;
        return Ok(());
    }

    // Upload to the backend.
    let document_id = match upload_doc(cfg, bytes, &filename).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("upload failed: {e:#}");
            send_or_edit(
                bot,
                chat_id,
                status,
                "⚠️ Couldn't upload that document to Mike — please try again.",
            )
            .await?;
            return Ok(());
        }
    };

    // Turn 1 — review only, no edits yet.
    let review_prompt = format!(
        "Risk-review this uploaded document against the litigation risk rubric. \
         Produce the risk table only — do NOT edit yet.\n\nUser instruction: {caption}"
    );
    let turns = vec![ChatMsg {
        role: "user".to_string(),
        content: review_prompt,
    }];
    let outcome = match call_chat(bot, chat_id, cfg, pending, &turns, Some(&document_id)).await {
        Ok(o) => o,
        Err(e) => {
            send_or_edit(bot, chat_id, status, &format!("⚠️ {e}")).await?;
            return Ok(());
        }
    };

    if let Some(id) = status {
        let _ = bot.delete_message(chat_id, id).await;
    }

    let review = if outcome.reply.trim().is_empty() {
        "(the backend returned an empty review)".to_string()
    } else {
        outcome.reply
    };
    for chunk in split_for_telegram(&review) {
        bot.send_message(chat_id, chunk).await?;
    }
    bot.send_message(chat_id, "Apply these as tracked changes? Reply 'yes'.")
        .await?;

    redlines.lock().await.insert(
        chat_id,
        RedlinePending {
            document_id,
            filename,
            review,
        },
    );
    Ok(())
}

/// Turn 2 — the user confirmed: instruct `edit_document` to apply the proposed
/// fixes as tracked changes, then deliver the redlined `.docx` back.
async fn apply_redline(
    bot: &Bot,
    msg: &Message,
    cfg: &BotConfig,
    pending: &Pending,
    rl: RedlinePending,
) -> ResponseResult<()> {
    let chat_id = msg.chat.id;
    let _ = bot.send_chat_action(chat_id, ChatAction::Typing).await;
    let status = bot
        .send_message(chat_id, "✍️ Applying tracked changes…")
        .await
        .ok()
        .map(|m| m.id);

    let turns = vec![
        ChatMsg {
            role: "user".to_string(),
            content: "Risk-review this uploaded document and propose tracked-change edits."
                .to_string(),
        },
        ChatMsg {
            role: "assistant".to_string(),
            content: rl.review.clone(),
        },
        ChatMsg {
            role: "user".to_string(),
            content: "Apply your proposed fixes to the uploaded Word file as tracked changes \
                      now using edit_document (doc_id \"doc-0\")."
                .to_string(),
        },
    ];
    let outcome = match call_chat(bot, chat_id, cfg, pending, &turns, Some(&rl.document_id)).await {
        Ok(o) => o,
        Err(e) => {
            send_or_edit(bot, chat_id, status, &format!("⚠️ {e}")).await?;
            return Ok(());
        }
    };

    if let Some(id) = status {
        let _ = bot.delete_message(chat_id, id).await;
    }

    // Relay the model's reply — it surfaces any `edit_document` error
    // (e.g. clause-not-found) in prose rather than dropping it silently.
    let reply = if outcome.reply.trim().is_empty() {
        "Done — here is the redlined document.".to_string()
    } else {
        outcome.reply
    };
    for chunk in split_for_telegram(&reply) {
        bot.send_message(chat_id, chunk).await?;
    }

    // Deliver the edited file (edit_document updated its storage_path).
    match fetch_document(cfg, &rl.document_id).await {
        Ok(bytes) => {
            let out_name = format!("redlined-{}", rl.filename);
            let file = InputFile::memory(bytes).file_name(out_name);
            bot.send_document(chat_id, file).await?;
        }
        Err(e) => {
            tracing::error!("redlined fetch failed for {}: {e:#}", rl.document_id);
            bot.send_message(
                chat_id,
                "⚠️ Applied the review, but couldn't fetch the edited file — please try again.",
            )
            .await?;
        }
    }
    Ok(())
}

/// Edit an existing status message in place, or send a fresh one if there isn't
/// one — used so an error replaces the "running…" placeholder instead of
/// leaving it dangling.
async fn send_or_edit(
    bot: &Bot,
    chat_id: ChatId,
    status: Option<MessageId>,
    text: &str,
) -> ResponseResult<()> {
    match status {
        Some(id) => {
            let _ = bot.edit_message_text(chat_id, id, text).await;
        }
        None => {
            bot.send_message(chat_id, text).await?;
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_docx_with_caption_uploads() {
        assert_eq!(
            gate_document(Some("Brief.docx"), Some(DOCX_MIME), 1024, Some("risk-review this")),
            DocGate::Upload
        );
    }

    #[test]
    fn gate_pdf_rejected_with_docx_message() {
        match gate_document(Some("scan.pdf"), Some("application/pdf"), 1024, Some("review")) {
            DocGate::Reject(m) => assert!(m.to_lowercase().contains("docx")),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn gate_oversize_docx_rejected() {
        match gate_document(Some("big.docx"), Some(DOCX_MIME), MAX_DOC_BYTES + 1, Some("review")) {
            DocGate::Reject(m) => assert!(m.contains("20 MB")),
            other => panic!("expected size Reject, got {other:?}"),
        }
    }

    #[test]
    fn gate_docx_without_caption_needs_caption() {
        assert_eq!(
            gate_document(Some("brief.docx"), None, 1024, Some("   ")),
            DocGate::NeedCaption
        );
        assert_eq!(
            gate_document(Some("brief.docx"), None, 1024, None),
            DocGate::NeedCaption
        );
    }

    #[test]
    fn affirmative_detects_yes_not_questions() {
        assert!(is_affirmative("yes"));
        assert!(is_affirmative("Yes, apply it"));
        assert!(is_affirmative("  OK  "));
        assert!(!is_affirmative("what risks did you find?"));
        assert!(!is_affirmative("no, leave it"));
    }

    #[test]
    fn looks_like_a_rule_ignores_questions_catches_instructions() {
        // The reproducible no-false-trigger guard (Loop M check 3).
        assert!(!looks_like_a_rule(
            "What is the limitation period to file a written statement?"
        ));
        assert!(looks_like_a_rule("From now on, never omit the verification clause."));
        assert!(looks_like_a_rule("Remember to cite the section number."));
        assert!(!looks_like_a_rule("I want to draft a contract."));
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
