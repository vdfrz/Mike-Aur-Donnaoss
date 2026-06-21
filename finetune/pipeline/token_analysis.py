"""Step 3b: real token-length analysis with the Qwen3-4B tokenizer.

Splits dataset.jsonl into comprehension vs drafting by their (distinct) system
prompt, renders each row with the model's chat template exactly as build_sft.py
will, and reports the token distribution per sub-corpus. This is what sets
max_seq_length and reveals how many drafting docs would be truncated.
"""
import json, statistics, sys
from transformers import AutoTokenizer

MODEL = "unsloth/Qwen3-4B"
BASE = r"C:\Users\User\Downloads\Legal Training Data-20260621T054625Z-3-001\Legal Training Data\Q&A Pairs"

DRAFT_SYS_PREFIX = "You are Mike, an expert Indian legal clerk"

def load(p):
    with open(p, encoding="utf-8") as f:
        return [json.loads(l) for l in f if l.strip()]

def to_messages(r):
    user = r["instruction"]
    if str(r.get("input", "")).strip():
        user = user + "\n\n" + r["input"]
    return [
        {"role": "system", "content": r["system"]},
        {"role": "user", "content": user},
        {"role": "assistant", "content": r["output"]},
    ]

def pct(a, p):
    a = sorted(a)
    return a[min(len(a) - 1, int(p * len(a)))]

def main():
    tok = AutoTokenizer.from_pretrained(MODEL)
    ds = load(BASE + "\\dataset.jsonl")
    comp, draft = [], []
    for r in ds:
        (draft if r["system"].startswith(DRAFT_SYS_PREFIX) else comp).append(r)
    print(f"comprehension={len(comp)} drafting={len(draft)}")

    for name, sub in [("COMPREHENSION", comp), ("DRAFTING", draft), ("ALL", ds)]:
        lens = []
        for r in sub:
            text = tok.apply_chat_template(to_messages(r), tokenize=False,
                                           add_generation_prompt=False)
            lens.append(len(tok(text, add_special_tokens=False)["input_ids"]))
        print(f"\n=== {name} ({len(sub)} rows) TOKENS ===")
        print(f"  p50={pct(lens,.5)} p90={pct(lens,.9)} p95={pct(lens,.95)} "
              f"p99={pct(lens,.99)} max={max(lens)} mean={int(statistics.mean(lens))}")
        for thr in [2048, 4096, 8192, 16384]:
            n = sum(1 for x in lens if x > thr)
            print(f"  rows > {thr} tokens: {n}/{len(sub)} ({100*n/len(sub):.1f}%)")

if __name__ == "__main__":
    main()
