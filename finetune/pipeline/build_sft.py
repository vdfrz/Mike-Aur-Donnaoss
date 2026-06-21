"""Step 3c: convert dataset.jsonl into Qwen3 chat-format SFT splits.

- Renders each row with the chosen model's OWN tokenizer chat template, exactly
  as Ollama will at inference (keeps Qwen3's native empty <think></think> for
  non-thinking mode -> train/inference templates match, which is what prevents
  "works in training, garbage in Ollama").
- max_seq_length = 4096; drops rows whose rendered length exceeds it so we never
  train on a truncated, never-ending target (operator decision).
- Stratified 95/5 split per sub-corpus so the val set has both kinds.
- Emits train/val + drafting-only / comprehension-only val slices (for the
  Step 5 citation gate) + a drafting-only training file (for the ablation).

Output rows carry: text (for SFTTrainer), plus system/user/output/corpus
(so the eval can re-run prompts and compare against reference answers).
"""
import json, random
from transformers import AutoTokenizer

MODEL = "unsloth/Qwen3-4B"
MAX_SEQ = 4096
SEED = 3407
DRAFT_SYS_PREFIX = "You are Mike, an expert Indian legal clerk"
BASE = r"C:\Users\User\Downloads\Legal Training Data-20260621T054625Z-3-001\Legal Training Data\Q&A Pairs"
OUT = r"C:\Users\User\mikeoss\finetune\data"

tok = AutoTokenizer.from_pretrained(MODEL)

def load(p):
    with open(p, encoding="utf-8") as f:
        return [json.loads(l) for l in f if l.strip()]

def build_user(r):
    user = r["instruction"]
    if str(r.get("input", "")).strip():
        user = user + "\n\n" + r["input"]
    return user

def render(system, user, output):
    msgs = [
        {"role": "system", "content": system},
        {"role": "user", "content": user},
        {"role": "assistant", "content": output},
    ]
    return tok.apply_chat_template(msgs, tokenize=False, add_generation_prompt=False)

def n_tokens(text):
    return len(tok(text, add_special_tokens=False)["input_ids"])

def main():
    import os
    os.makedirs(OUT, exist_ok=True)
    ds = load(BASE + "\\dataset.jsonl")

    kept, dropped = [], 0
    for r in ds:
        corpus = "drafting" if r["system"].startswith(DRAFT_SYS_PREFIX) else "comprehension"
        user = build_user(r)
        text = render(r["system"], user, r["output"])
        if n_tokens(text) > MAX_SEQ:
            dropped += 1
            continue
        kept.append({"text": text, "system": r["system"], "user": user,
                     "output": r["output"], "corpus": corpus})

    comp = [r for r in kept if r["corpus"] == "comprehension"]
    draft = [r for r in kept if r["corpus"] == "drafting"]
    print(f"kept {len(kept)} (comprehension={len(comp)} drafting={len(draft)}); "
          f"dropped {dropped} over-length")

    rng = random.Random(SEED)
    def split(rows):
        rows = rows[:]
        rng.shuffle(rows)
        n_val = max(1, round(len(rows) * 0.05))
        return rows[n_val:], rows[:n_val]

    comp_tr, comp_val = split(comp)
    draft_tr, draft_val = split(draft)

    train = comp_tr + draft_tr
    val = comp_val + draft_val
    rng.shuffle(train); rng.shuffle(val)

    def dump(name, rows):
        with open(OUT + "\\" + name, "w", encoding="utf-8") as f:
            for r in rows:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        print(f"  wrote {name}: {len(rows)} rows")

    dump("train.jsonl", train)
    dump("val.jsonl", val)
    dump("val_drafting.jsonl", draft_val)
    dump("val_comprehension.jsonl", comp_val)
    dump("dataset_drafting_only.jsonl", draft)  # ablation training set (<=MAX_SEQ)

if __name__ == "__main__":
    main()
