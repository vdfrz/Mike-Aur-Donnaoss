# mike-telegram-bot

Thin Telegram front-end for the Mike aur Donna backend (Loop D · Session 5).

It does **not** re-implement the agent. Every message is forwarded to the
existing axum `/chat` SSE endpoint; the streamed reply is accumulated and sent
back to the Telegram chat. This is a standalone crate (its own `[workspace]`),
so it builds independently of the main `mike` crate and touches no shared files.

## Run

1. Start the Mike backend (listens on `127.0.0.1:3001`). The bot needs to
   authenticate, so either:
   - run the backend with `MIKE_BYPASS_AUTH=true` (local only), **or**
   - pass a real session token via `MIKE_SESSION_TOKEN` (see below).
2. Create a bot with [@BotFather](https://t.me/BotFather) and get its token.
3. Run the bot:

   ```sh
   export TELEGRAM_BOT_TOKEN=123456:ABC-...        # required
   export MIKE_API_URL=http://localhost:3001       # optional (default)
   export MIKE_SESSION_TOKEN=                       # optional; empty if backend uses MIKE_BYPASS_AUTH
   export MIKE_MODEL=                               # optional; omitted → backend default
   cargo run
   ```

4. In Telegram: send `/start`, then any question. The answer streams back.

## Verify

```sh
cargo test     # SSE-parser + message-chunking unit tests
cargo build    # compiles the bot binary
```

## Scope

Scaffold only: plain-text round-trip + `/start` / `/help`. Research results,
`.docx` drafting, and clarifying-question buttons land in Session 6.
