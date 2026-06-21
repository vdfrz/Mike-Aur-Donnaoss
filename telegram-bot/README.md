# mike-telegram-bot

Thin Telegram front-end for the Mike aur Donna backend (Loop D · Session 5).

It does **not** re-implement the agent. Every message is forwarded to the
existing axum `/chat` SSE endpoint; the streamed reply is accumulated and sent
back to the Telegram chat. This is a standalone crate (its own `[workspace]`),
so it builds independently of the main `mike` crate and touches no shared files.

## Run

1. Start the Mike backend (listens on `127.0.0.1:3001`).
2. Create a bot with [@BotFather](https://t.me/BotFather) and get its token.
3. Run the bot:

   ```sh
   export TELEGRAM_BOT_TOKEN=123456:ABC-...        # required
   export MIKE_API_URL=http://localhost:3001       # optional (default)
   export MIKE_SESSION_TOKEN=                       # optional; static fallback token / bypass
   export MIKE_MODEL=                               # optional; omitted → backend default
   cargo run
   ```

4. In Telegram: send `/login <your PIN>` to authenticate **as your real
   account** (the bot deletes the PIN message afterwards). Then ask any
   question, or use `/chats` to continue a conversation started on your laptop
   and `/docs` to pull a draft to your phone.

   Auth options, in priority order: a per-chat token from `/login`; else the
   static `MIKE_SESSION_TOKEN`; else nothing (only works if the backend runs
   with `MIKE_BYPASS_AUTH=true`, which authenticates as `local-user`, **not**
   your real account — so `/chats` and `/docs` won't see your laptop data).
   Session tokens expire after 1 week; the bot drops an expired one and asks you
   to `/login` again.

## Verify

```sh
cargo test     # SSE-parser + message-chunking unit tests
cargo build    # compiles the bot binary
```

## Branding (make every install look like Donna)

The slash-command menu, the bot's name blurb, and its "about" text are set from
code at startup (`main()`), so they're identical on every clone with no manual
steps. The **profile picture** is the one exception — Telegram only lets you set
a bot's avatar via @BotFather, so it's a one-time manual step per install:

1. Drop the shared image at `assets/donna.png` (square, 512×512, <200 KB).
2. @BotFather → `/setuserpic` → choose your bot → upload it.

See `assets/README.md` for details.

## Scope

A thin Telegram client over the backend's `/chat` SSE endpoint, with cross-device
continuity: plain-text Q&A with history, clarifying-question buttons, `/login`
auth as your real account, `/chats` to resume a laptop conversation, and `/docs`
to download a draft to your phone. It re-implements none of the agent — every
turn is forwarded to the local backend.
