# mike-legal — offline Indian-legal model for Mike aur Donna

`mike-legal` is a LoRA fine-tune of **Qwen3-4B**, quantized to **Q4_K_M GGUF** and served
by **Ollama** under the alias `mike-legal`. It is the offline, no-internet model the app loads
through its model picker (`src/llm/local.rs` talks to Ollama's OpenAI-compatible endpoint).

- **Fine-tune target:** Indian-legal answer register for closed-book Q&A / comprehension.
- **Citations come from RAG at inference time, not the weights.** The fine-tune deliberately does
  not try to memorize statutes (a 4B model's statute recall is unreliable).
- **Base model:** `unsloth/Qwen3-4B` (hybrid, non-thinking trained — matches the app's prior `qwen3:4b`).

## Status

| Capability | Status |
|---|---|
| Legal Q&A / comprehension | ✅ Shipped. Cleaner, on-register answers; citation precision improved vs. base (see below). |
| Document drafting | ⚠️ **Not yet supported.** The drafting training data is heavily contaminated with court orders / daily-proceedings mislabeled as draftable documents, so drafting does not reliably follow instructions. Needs an upstream data rebuild (see "Known limitations"). |

### Citation regression gate (105 held-out comprehension questions)

| | Base Qwen3-4B | mike-legal (FT) |
|---|---|---|
| Citations emitted | 364 | 83 |
| Matching reference | 33 | 32 |
| Not in reference | 331 | 51 |
| **Precision** | 0.091 | **0.386** |

The fine-tune keeps the correct citations while cutting wrong ones ~6× — it does **not** worsen
hallucination. Residual statute hallucination remains (hence RAG owns citations).

## Install (your friend / any offline machine)

You need: Ollama installed, the `mike-legal-q4_K_M.gguf` file, and the `Modelfile`.

```powershell
# Put the .gguf next to the Modelfile, then:
ollama create mike-legal -f Modelfile
ollama run mike-legal "What is anticipatory bail and which provision governs it?"
```

The `Modelfile`'s `FROM` line should point at the quantized GGUF you were given. No GPU and no
internet are required — registration takes a few seconds and inference runs on CPU.

To get the GGUF: it is **not** in git (~2.5 GB). Export it from a machine that has the model with
`ollama` or copy the `mike-legal-q4_K_M.gguf` produced by the build below, and transfer it
out-of-band (Drive / USB / WeTransfer).

## Rebuild from scratch (needs an NVIDIA GPU)

The full pipeline is in `pipeline/`. On the dev box (Windows 11, RTX 5060 Ti, CUDA 12.8+ for
Blackwell `sm_120`):

```powershell
# 1. Env (Python 3.11–3.13). torch must be cu128+ for 50-series GPUs.
py -3.12 -m venv .venv ; .\.venv\Scripts\Activate.ps1
pip install "torch>=2.4,<2.11" --index-url https://download.pytorch.org/whl/cu128
pip install unsloth unsloth-zoo
python pipeline/check_gpu.py            # must print device cap (12, 0) + "cuda kernels ok"

# 2. Data prep + training
python pipeline/build_sft.py            # dataset.jsonl -> train/val splits (max_seq 4096)
python pipeline/train.py                # QLoRA, 2 epochs -> lora_adapters/

# 3. Evaluate the citation gate (base vs FT)
python pipeline/eval_gate.py gen --model base
python pipeline/eval_gate.py gen --model ft
python pipeline/eval_gate.py score

# 4. Export -> GGUF -> Ollama
python pipeline/merge_export.py         # lora_adapters -> mike-legal-merged-16bit/
git clone --depth 1 https://github.com/ggml-org/llama.cpp
pip install --no-deps ./llama.cpp/gguf-py   # do NOT install the full reqs (it pins CPU torch)
$env:PYTHONPATH="llama.cpp"
python llama.cpp/convert_hf_to_gguf.py mike-legal-merged-16bit --outfile mike-legal-f16.gguf --outtype f16
ollama create mike-legal -f Modelfile --quantize q4_K_M
```

### Pipeline scripts

| Script | Purpose |
|---|---|
| `check_gpu.py` | Verify Blackwell `sm_120` torch (the classic 50-series failure mode). |
| `token_analysis.py` | Token-length distribution → sets `max_seq_length`. |
| `build_sft.py` | Render `dataset.jsonl` with Qwen3's own chat template; stratified train/val splits; drop >4096-token rows. |
| `train.py` | Unsloth QLoRA SFT (trl 0.24 / transformers 5.x API). `--smoke` for a 3-step dry run. |
| `eval_gate.py` | Base-vs-FT citation regression + drafting/comprehension sanity generations. |
| `merge_export.py` | Merge LoRA → 16-bit safetensors for GGUF conversion. |

## Known limitations

- **Drafting data contamination.** `Q&A Pairs/drafting_pairs.jsonl` mixes genuine draftable documents
  with court orders / daily-proceedings (scraped from court daily-order portals and case PDFs) under
  wrong `doc_type` labels. Fine-tuning on these teaches the model to emit memorized court-order text
  instead of following a drafting instruction. A real drafting feature needs the drafting pairs
  regenerated from verified draftable source documents (`clean data/`) with instruction+facts → document.
- **Chat template.** Training and Ollama both use Qwen3's native template (with the empty
  `<think></think>` non-thinking block). Do not change one without the other.
