# Recommended local model: Qwen 2.5 3B

## Why Qwen 2.5 3B?

Mike aur Donna targets Indian advocates on budget hardware (8 GB RAM Mac M1 or lower). The local LLM must:

1. **Run in ~2.5 GB RAM** — leaves headroom for the Tauri app, SQLite, and ONNX embeddings
2. **Follow detailed formatting instructions** — Indian court documents have strict conventions (numbered "That, " paragraphs, S/o Sh., verification clauses)
3. **Not refuse legal content** — criminal FIRs, domestic violence cases, maintenance disputes are routine legal work, not harmful content
4. **Handle Hindi-English code-switching** — Indian legal drafting mixes English legalese with Hindi terms

Qwen 2.5 3B meets all four. Llama 3.2 3B (including uncensored variants) fails on points 2 and 3 — it hallucinates structure and sometimes hits safety guardrails on routine legal topics.

## Setup

```bash
# Install Ollama (https://ollama.ai)
ollama pull qwen2.5:3b

# In MikeRust, configure via the UI:
# Impostazioni > Modelli LLM > Local model > qwen2.5:3b
# Or set in .env:
VLLM_BASE_URL=http://localhost:11434/v1
VLLM_MAIN_MODEL=qwen2.5:3b
```

## Alternatives tested

| Model | RAM | Instruction following | Legal content | Verdict |
|-------|-----|----------------------|---------------|---------|
| **Qwen 2.5 3B** | ~2.5 GB | Good | No refusals | **Recommended** |
| Llama 3.2 3B Uncensored | ~2.5 GB | Poor — loses structure | Still refuses some | Replaced |
| Llama 3.2 3B (base) | ~2.5 GB | Poor | Refuses legal topics | Not suitable |
| Phi-3.5 Mini 3.8B | ~3 GB | Good | Aggressive safety filter | Not suitable for legal |
| Gemma 2 2B | ~1.5 GB | Acceptable | No refusals | Fallback for very low RAM |

## Performance notes

On Mac M1 8 GB with Ollama:
- First token: ~1.5s
- Generation: ~15-20 tokens/sec
- Affidavit draft (~500 words): ~8-12 seconds

The backend detects 3B/2B models via the model name string and automatically:
- Uses a simplified system prompt (shorter, more direct)
- Loads only the relevant template block (not all document types)
- Skips the library inventory and Indian Kanoon results to save context window
