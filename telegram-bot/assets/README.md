# Bot avatar

Telegram serves a bot's profile picture from its own servers, and it can only be
set via @BotFather — there is **no Bot API method** for a bot to set its own
avatar. So this is a one-time manual step per install:

1. Put the shared avatar here as `donna.png` (square, 512×512 recommended,
   under ~200 KB).
2. In Telegram, message @BotFather → `/setuserpic` → choose your bot → upload
   `donna.png`.

Every install that uploads the same file gets the same Donna face. The bot's
name blurb, "about" text, and command menu are set automatically from code at
startup (see `main()` in `../src/main.rs`), so only the avatar needs this step.
