# Mike aur Donna

AI legal drafting and case analysis platform for Indian lawyers. Rust+axum backend, Next.js frontend, SQLite — runs on 8 GB RAM, no GPU required.

Built on the open-source [Mike](https://github.com/willchen96/mike) project by Will Chen (AGPL-3.0). The backend is a full Rust rewrite; the frontend is adapted from the upstream Next.js app.

## What it does

- **AI chat with citations** — ask questions about uploaded case files. The assistant cites specific documents, pages, and passages. Click a citation pill to jump to the source in the built-in PDF/DOCX viewer.
- **Case prep agents** — upload a case file bundle and run 7 parallel AI agents: case summary, strengths & weaknesses, evidence gaps, opposition predictor, strategy recommender, precedent finder, and risk assessor. All tuned for Indian courts (IPC/BNS dual-era, CPC, CrPC/BNSS, consumer forums, MACT, family courts, tribunals).
- **Indian Kanoon integration** — search and cite case law from Indian Kanoon directly inside the chat. The agent verifies its own citations against source documents.
- **Case search** — search Indian Kanoon for judgments, orders, and statutes from the app's sidebar.
- **PII anonymization** — GLiNER-based named entity recognition to scrub party names and sensitive details from documents before sharing.
- **Statute lookup** — built-in tool for the agent to look up Indian statutes by section number.
- **Document extraction** — PDF (pdfium), DOCX (with tracked-change detection), RTF, XLSX, TXT, MD, CSV. Scanned PDFs are flagged; vision-capable models can read them.
- **RAG** — local folder sync with chunking + ONNX embeddings (multilingual-e5-base via fastembed). Embeddings stored in sqlite-vec, partitioned per user/project.
- **Sovereign data** — everything stays on your machine. No telemetry, no cloud database. Outbound traffic only when you call a remote LLM or Indian Kanoon.

## Quick start

### Option A — GitHub Codespaces (easiest)

1. Click **Code → Codespaces → Create codespace on main**
2. Wait for setup (~5 min first time)
3. Add your API keys to `.env` (created automatically):
   ```env
   JWT_SECRET=any-random-string-32-chars-min
   DEEPSEEK_API_KEY=sk-your-key
   IK_API_KEY=your-indian-kanoon-key
   ```
4. Terminal 1: `cargo run --features rag`
5. Terminal 2: `cd frontend && npm run dev`
6. Open port 3000 when prompted

See [SETUP.md](SETUP.md) for full details including local setup.

### Option B — Local

```bash
# Prerequisites: Rust (rustup.rs), Node.js 18+, pdfium in libs/pdfium/

cp .env.example .env
# Edit .env: set JWT_SECRET, DEEPSEEK_API_KEY, IK_API_KEY

cd frontend && npm install && cd ..

# Terminal 1 — backend
cargo run --features rag

# Terminal 2 — frontend
cd frontend && npm run dev
```

Open http://localhost:3000. Create a PIN on first launch. Select DeepSeek under Account → Models.

## Architecture

```
Browser (Next.js :3000)
       │  HTTP + SSE
       ▼
axum backend (:3001)
   ├── SQLite           mike.db           (schema, vectors, settings, chats, docs)
   ├── sqlite-vec       doc_chunks        (768-dim embeddings, partition-keyed)
   ├── fastembed/ort    multilingual-e5-base ONNX  (CPU / DirectML / QNN)
   ├── pdfium-render    PDF text extraction
   ├── quick-xml+zip    DOCX extraction (incl. redline detection)
   ├── calamine         XLSX/XLS/XLSB/ODS extraction
   ├── Local storage    ./data/storage/{documents,cache}
   ├── LLM              DeepSeek / Anthropic / Gemini / OpenAI / vLLM / Ollama
   ├── Indian Kanoon    Case law search + citation verification
   ├── PII              GLiNER NER service (services/gliner/)
   └── MCP              any HTTP/SSE MCP server
```

## LLM providers

| Provider | Env var | Notes |
|---|---|---|
| **DeepSeek** (recommended) | `DEEPSEEK_API_KEY` | Best cost/quality for Indian legal text |
| Anthropic Claude | `ANTHROPIC_API_KEY` | |
| Google Gemini | `GEMINI_API_KEY` | |
| OpenAI | via vLLM config | |
| Ollama / vLLM | `VLLM_BASE_URL` | Local models — `qwen2.5:3b` fits 8 GB RAM |

## Key files

```
src/
  lib.rs                    Server entry point
  routes/
    chat.rs                 Chat + SSE streaming
    cases.rs                Case prep orchestration
    indian_kanoon.rs        Indian Kanoon proxy + search
    statutes.rs             Statute lookup
    documents.rs            Upload + extraction
    sync.rs                 Local folder RAG sync
  llm/
    kanoon_tool.rs          Agent tool: Indian Kanoon search
    statute_tool.rs         Agent tool: statute lookup
    aws_verification.rs     Citation verification against source docs
    builtin_tools.rs        Tool dispatch
  agents/case_prep/         7 case analysis agents + orchestrator
  pii/                      GLiNER NER + PII scrubbing

frontend/src/app/
  components/assistant/     Chat UI, message rendering, citations
  components/shared/        Sidebar, doc viewer, shared components
  account/case-search/      Indian Kanoon search page
  data/thinkingSnippets.ts  Loading messages with Indian legal humor
  hooks/                    Chat, doc fetch, SSE hooks
```

## Environment variables

See [.env.example](.env.example) for the full reference.

| Variable | Required | Default |
|---|---|---|
| `JWT_SECRET` | **yes** | — |
| `DEEPSEEK_API_KEY` | for DeepSeek | — |
| `IK_API_KEY` | for Indian Kanoon | — |
| `DATABASE_URL` | no | `sqlite://mike.db` |
| `STORAGE_PATH` | no | `./data/storage` |
| `ANTHROPIC_API_KEY` | for Claude | — |
| `GEMINI_API_KEY` | for Gemini | — |
| `VLLM_BASE_URL` | for local LLM | — |
| `MCP_SERVERS` | no | `[]` |
| `PORT` | no | `3001` |

## Documentation

- [SETUP.md](SETUP.md) — full setup guide (Codespaces + local)
- [docs/MANUAL.md](docs/MANUAL.md) — operator manual
- [docs/CACHE.md](docs/CACHE.md) — chat-attachment cache layout
- [docs/DOCX.md](docs/DOCX.md) — DOCX extraction details

## License

AGPL-3.0, inherited from [willchen96/mike](https://github.com/willchen96/mike). See [LICENSE](LICENSE).
