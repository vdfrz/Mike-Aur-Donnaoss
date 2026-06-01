# Setup Guide — Mike aur Donna

## Option A — GitHub Codespaces (Recommended)

No local install needed. Runs entirely in your browser.

### 1. Open the Codespace

- Go to the GitHub repo
- Click the green **Code** button → **Codespaces** tab → **Create codespace on main**
- Wait for it to build (first time takes 3-5 minutes — it installs Rust, Node, and pdfium automatically)

### 2. Add your API keys

When the Codespace opens, you'll see a message in the terminal saying `.env` was created. Open it:

1. Click `.env` in the file explorer (project root)
2. Fill in these three values:

```env
JWT_SECRET=paste-any-random-string-here-at-least-32-characters
DEEPSEEK_API_KEY=sk-your-deepseek-key
IK_API_KEY=your-indian-kanoon-key
```

- **JWT_SECRET** — any random string, 32+ chars. Generate one by running `openssl rand -base64 48` in the terminal.
- **DeepSeek key** — get from https://platform.deepseek.com/api_keys
- **Indian Kanoon key** — get from https://api.indiankanoon.org (or you can add it later in the app's Settings page)

Save the file.

### 3. Run the app

Open **two terminals** in VS Code (click the `+` icon in the terminal panel):

**Terminal 1 — Backend:**

```bash
cargo run --features rag
```

First run compiles everything (~5-10 min on Codespaces). It will also:
- Create the SQLite database and run migrations
- Create the `data/storage/` directory
- Download the embedding model (~280 MB, one-time)

**Terminal 2 — Frontend:**

```bash
cd frontend && npm run dev
```

### 4. Open the app

Codespaces auto-detects the ports. When the frontend starts, you'll see a popup — click **Open in Browser**. Or go to the **Ports** tab at the bottom and click the globe icon next to port `3000`.

You'll be prompted to create a PIN on first launch. After that you're in.

### 5. Select DeepSeek as your model

Once logged in, go to **Account → Models** and select a DeepSeek model (e.g. `deepseek-chat` or `deepseek-reasoner`). The app uses your `DEEPSEEK_API_KEY` automatically.

---

## Option B — Local Setup (VS Code)

### Prerequisites

Install these once:

1. **Rust** — https://rustup.rs/
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
   Close and reopen your terminal after installing.

2. **Node.js** (v18 or newer) — https://nodejs.org/

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

- **JWT_SECRET** — required, any random string 32+ chars
- **DeepSeek key** — https://platform.deepseek.com/api_keys
- **Indian Kanoon key** — https://api.indiankanoon.org (or set it later in-app under Settings)

### 2. Install frontend dependencies

```bash
cd frontend && npm install && cd ..
```

### 3. Run the app

**Terminal 1 — Backend:**

```bash
cargo run --features rag
```

**Terminal 2 — Frontend:**

```bash
cd frontend && npm run dev
```

### 4. Open the app

Go to **http://localhost:3000** in your browser. Create a PIN on first launch.

### 5. Select DeepSeek as your model

Go to **Account → Models** and pick `deepseek-chat` or `deepseek-reasoner`.

---

## Troubleshooting

| Problem | Fix |
|---|---|
| `cargo: command not found` | Restart terminal after Rust install |
| `npm: command not found` | Install Node.js from https://nodejs.org/ |
| Backend won't start | Check `JWT_SECRET` is set in `.env` |
| "No Indian Kanoon API key" | Add `IK_API_KEY` to `.env` or set in Settings |
| "Local model not configured" | Set `DEEPSEEK_API_KEY` in `.env` + select DeepSeek model in Account → Models |
| Slow first `cargo run` | Normal — Rust compiles from scratch first time (5-10 min). Rebuilds are fast. |
| Codespace port not opening | Go to **Ports** tab → right-click port 3000 → **Port Visibility** → **Public** |
