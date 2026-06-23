# Mike aur Donna

A private AI legal assistant for Indian lawyers that drafts, cites, and analyses your case files on your own machine. No cloud database, no telemetry.

It reads your uploaded documents, answers questions with citations you can click back to the source, runs a full case analysis, and drafts briefs and replies. Everything stays local except the LLM call you choose to make. You install it once and it becomes a native app you open from Spotlight.

## What you can do with it

- **Ask your case files anything** and get answers with citations. Click a citation to jump to the exact page and passage in the built-in viewer.
- **Run a full case analysis** with one click: summary, strengths and weaknesses, evidence gaps, opposition predictor, strategy, precedents, and risk, all tuned for Indian courts.
- **Draft and redline in place.** Ask for an edit to an uploaded document and accept or reject the change inline, like tracked changes.
- **Search Indian Kanoon** for judgments and statutes from inside the chat, with the agent verifying its own citations.
- **Keep client data private.** Built-in PII scrubbing, a fully on-device model option (mike-legal), and no data leaving your machine unless you call a remote model.

## Get it running in minutes

You end up with a native "Mike" app you launch from Spotlight (macOS) or your applications menu (Linux). No terminals to keep open, no ports, no config file to hand-edit. You paste your API key into the app's Settings after it opens. macOS on Apple Silicon is the primary target; Linux works too.

### One paste into your IDE agent

If you use an AI coding assistant (Cursor, Claude Code, Windsurf, Copilot Chat), paste the block below into it and let it install everything and build the app.

> **Notice:** this prompt installs anything that is missing (Rust, Node, Ollama, and the build script then pulls Tauri, pdfium, and pandoc). If a prerequisite is already on your machine, the agent skips it. If it is present, the agent adjusts and moves on.

```text
You are installing "Mike aur Donna", a local AI legal app (Rust + axum backend,
Next.js frontend, SQLite), as a NATIVE DESKTOP APP on this machine. The end
result must be an app the user can find and launch from Spotlight (macOS) or
their applications menu (Linux). Primary target is macOS on Apple Silicon; if
the host is Linux, adapt the commands.

PREREQUISITES: check each one. If it is already installed, skip it and move on.
If it is missing, install it:
  - git
  - Rust toolchain (rustup + cargo)        https://rustup.rs
  - Node.js 18 or newer (with npm)         https://nodejs.org
  - Ollama (for the on-device mike-legal model)   https://ollama.com
  - Homebrew (macOS only, for system libraries)   https://brew.sh
The build script (scripts/setup.sh) installs the rest itself: the Tauri CLI,
pdfium, tesseract, and pandoc. On macOS prefer Homebrew: brew install node ollama.
On Linux use the native package manager and the official rustup + Ollama scripts.

STEPS:
1. Clone the repo if it is not already here, then enter it:
     git clone <REPO_URL> mike && cd mike
2. Build and install the native app:
     ./scripts/setup.sh
   This installs the remaining prerequisites, downloads pdfium, builds the
   desktop bundle (first build takes several minutes), and on macOS copies
   "Mike" into /Applications and opens it. On Linux it produces an installer
   under src-tauri/target/release/bundle/. Run that to install.
3. (Optional on-device model) Pull mike-legal once Ollama is running:
     ollama pull <OLLAMA_USERNAME>/mike-legal
4. Launch the app: on macOS search "Mike" in Spotlight; on Linux open it from
   the applications menu.
5. On first launch, create a PIN. Then open Settings / Account -> Models, paste
   your DeepSeek API key (https://platform.deepseek.com/api_keys), and select a
   DeepSeek model. Indian Kanoon search is optional: add an IK_API_KEY in
   Settings. The on-device mike-legal model needs no key.

If any step fails, STOP and show me the exact failing command and its full
error output. Do not silently continue or fake success.
```

Replace `<REPO_URL>` with this repo's clone URL and `<OLLAMA_USERNAME>` with the namespace the model was published under (see [finetune/README.md](finetune/README.md)).

### No IDE agent? Two commands

```bash
git clone <REPO_URL> mike && cd mike
./scripts/setup.sh
```

Then launch "Mike" from Spotlight and set your DeepSeek key in Settings. `scripts/setup.sh` installs the prerequisites, builds the desktop app, and installs it for you.

### Other ways to install

- **GitHub Codespaces:** runs entirely in the browser, nothing to install locally.
- **Run from source:** two terminals (backend and frontend) for development.

Full instructions for every path, plus troubleshooting, are in [SETUP.md](SETUP.md).

## Related guides

- [SETUP.md](SETUP.md): every install path, prerequisites, and troubleshooting.
- [finetune/README.md](finetune/README.md): install or publish the on-device mike-legal model.
- [telegram-bot/README.md](telegram-bot/README.md): run the optional Telegram bot front-end for Mike on your phone.

## Under the hood

Rust and axum backend, Next.js frontend, SQLite for everything (schema, chat history, settings, and `sqlite-vec` vector search). RAG uses local ONNX embeddings (multilingual-e5-base via fastembed). Document extraction covers PDF (pdfium), DOCX with tracked-change detection, RTF, XLSX, and plain text. LLMs are pluggable: DeepSeek (recommended), Anthropic, Gemini, or local models via Ollama and vLLM.

| Provider | Where you set the key | Notes |
|---|---|---|
| **DeepSeek** (recommended) | Account -> Models | Best cost and quality for Indian legal text |
| Anthropic Claude | Account -> Models | |
| Google Gemini | Account -> Models | |
| Ollama / vLLM (local) | Account -> Models | `mike-legal` and `qwen2.5:3b` fit 8 GB RAM, no key needed |

See [.env.example](.env.example) for the optional environment variables used when running from source.

## Credits and license

Built on the open-source [Mike](https://github.com/willchen96/mike) project by Will Chen (AGPL-3.0). The backend is a full Rust rewrite; the frontend is adapted from the upstream Next.js app. Licensed [AGPL-3.0](https://www.gnu.org/licenses/agpl-3.0.html), inherited from upstream.
