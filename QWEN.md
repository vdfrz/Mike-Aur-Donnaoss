# QWEN Agent Rules — MikeRust-main

## Dev server

- **ALWAYS** test backend changes using `cargo tauri dev`.
- **NEVER** start a standalone Next.js dev server (`npm run dev`, `next dev`, etc.) unless explicitly asked.
- The **Tauri app** is the primary interface. The webapp is secondary.

## Build

- **NEVER** run `cargo build --release` during development. Debug builds only.
