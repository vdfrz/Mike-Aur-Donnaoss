# Setup Guide: Mike aur Donna

## Fastest: one paste into your IDE agent

The goal of this path is a **native app you launch from Spotlight** (macOS) or your applications menu (Linux). No terminals to keep open, no ports, no `.env` to hand-edit. You set your API key inside the app after it opens.

If you use an AI coding assistant (Cursor, Claude Code, Windsurf, Copilot Chat, etc.), paste the block below into it and let it do the whole install. It checks for each prerequisite first.

> **Notice:** this prompt installs anything that is missing (Rust, Node, Ollama, and the build script then pulls Tauri, pdfium, and pandoc). If a prerequisite is already on your machine, the agent skips it and moves on. macOS (Apple Silicon) is the primary target; the prompt adapts itself for Linux.

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

Replace `<REPO_URL>` with this repository's clone URL and `<OLLAMA_USERNAME>` with the namespace the model was published under (see [finetune/README.md](finetune/README.md)).

No IDE agent? The same thing by hand is two commands:

```bash
git clone <REPO_URL> mike && cd mike
./scripts/setup.sh
```

Then launch "Mike" from Spotlight and set your DeepSeek key in Settings. See [Option C](#option-c-desktop-app-one-script) for the detail.

---

## Option A: GitHub Codespaces (Recommended)

No local install needed. Runs entirely in your browser.

### 1. Open the Codespace

- Go to the GitHub repo
- Click the green **Code** button → **Codespaces** tab → **Create codespace on main**
- Wait for it to build (first time takes 3-5 minutes: it installs Rust, Node, and pdfium automatically)

### 2. Add your API keys

When the Codespace opens, you'll see a message in the terminal saying `.env` was created. Open it:

1. Click `.env` in the file explorer (project root)
2. Fill in these three values:

```env
JWT_SECRET=paste-any-random-string-here-at-least-32-characters
DEEPSEEK_API_KEY=sk-your-deepseek-key
IK_API_KEY=your-indian-kanoon-key
```

- **JWT_SECRET**: optional, kept for compatibility with the upstream project. Any random string is fine; the current build does not require it.
- **DeepSeek key**: get from https://platform.deepseek.com/api_keys
- **Indian Kanoon key**: get from https://api.indiankanoon.org (or you can add it later in the app's Settings page)

Save the file.

### 3. Run the app

Open **two terminals** in VS Code (click the `+` icon in the terminal panel):

**Terminal 1: Backend:**

```bash
cargo run --features rag
```

First run compiles everything (~5-10 min on Codespaces). It will also:
- Create the SQLite database and run migrations
- Create the `data/storage/` directory
- Download the embedding model (~280 MB, one-time)

**Terminal 2: Frontend:**

```bash
cd frontend && npm run dev
```

### 4. Open the app

Codespaces auto-detects the ports. When the frontend starts, you'll see a popup, then click **Open in Browser**. Or go to the **Ports** tab at the bottom and click the globe icon next to port `3000`.

You'll be prompted to create a PIN on first launch. After that you're in.

### 5. Select DeepSeek as your model

Once logged in, go to **Account → Models** and select a DeepSeek model (e.g. `deepseek-chat` or `deepseek-reasoner`). The app uses your `DEEPSEEK_API_KEY` automatically.

---

## Option B: Local Setup (VS Code)

### Prerequisites

Install these once:

1. **Rust**: https://rustup.rs/
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   Close and reopen your terminal after installing.

2. **Node.js** (v18 or newer): https://nodejs.org/

3. **pdfium** (PDF extraction library)
   - Download the release for your OS from https://github.com/bblanchon/pdfium-binaries/releases
   - Place the library file (`pdfium.dll` on Windows, `libpdfium.dylib` on Mac, `libpdfium.so` on Linux) inside `libs/pdfium/` in the project root

### 1. Configure API keys

```bash
cp .env.example .env
```

Open `.env` and set:

```env
JWT_SECRET=paste-any-random-string-here-at-least-32-characters
DEEPSEEK_API_KEY=sk-your-deepseek-key
IK_API_KEY=your-indian-kanoon-key
```

- **JWT_SECRET**: optional, kept for upstream compatibility. Any random string is fine; the current build does not require it.
- **DeepSeek key**: https://platform.deepseek.com/api_keys
- **Indian Kanoon key**: https://api.indiankanoon.org (or set it later in-app under Settings)

### 2. Install frontend dependencies

```bash
cd frontend && npm install && cd ..
```

### 3. Run the app

**Terminal 1: Backend:**

```bash
cargo run --features rag
```

**Terminal 2: Frontend:**

```bash
cd frontend && npm run dev
```

### 4. Open the app

Go to **http://localhost:3000** in your browser. Create a PIN on first launch.

### 5. Select DeepSeek as your model

Go to **Account → Models** and pick `deepseek-chat` or `deepseek-reasoner`.

---

## Option C: desktop app (one script)

For a native macOS or Linux app with no terminals to keep open afterward. This builds Mike as a desktop application and installs it.

```bash
git clone <REPO_URL> mike && cd mike
./scripts/setup.sh
```

`scripts/setup.sh` installs the prerequisites (Rust, Tauri CLI, pdfium, and on macOS the Homebrew system libraries), builds the desktop bundle, and copies it to `/Applications` (macOS). After it finishes once, search "Mike" in Spotlight to open it any time. The desktop app needs no `.env`: on first launch create a PIN, then open Account -> Models and paste your DeepSeek API key (https://platform.deepseek.com/api_keys) to select a model. Indian Kanoon search is optional and its key goes in Settings too.

---

## Troubleshooting

| Problem | Fix |
|---|---|
| `cargo: command not found` | Restart terminal after Rust install |
| `npm: command not found` | Install Node.js from https://nodejs.org/ |
| Backend won't start (from source) | Make sure nothing else is using the port (default 3001), then re-run `cargo run --features rag` |
| "No Indian Kanoon API key" | Add it in Settings (desktop app), or set `IK_API_KEY` in `.env` when running from source |
| "Local model not configured" | Desktop app: paste your DeepSeek key in Account -> Models. From source: set `DEEPSEEK_API_KEY` in `.env` or in Account -> Models, then select a DeepSeek model |
| Slow first `cargo run` | Normal. Rust compiles from scratch first time (5-10 min). Rebuilds are fast. |
| Codespace port not opening | Go to **Ports** tab → right-click port 3000 → **Port Visibility** → **Public** |
