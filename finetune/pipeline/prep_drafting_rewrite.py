"""Build the working set for rewriting drafting instructions.

Takes the drafting pairs that fit the 4096-token training window, attaches
doc_type/court, and emits a preview (enough to capture parties, dispute, relief)
for Claude to read and turn into a specific, fact-bearing instruction.
"""
import json
from transformers import AutoTokenizer

MODEL = "unsloth/Qwen3-4B"
MAX_SEQ = 4096
BASE = r"C:\Users\User\Downloads\Legal Training Data-20260621T054625Z-3-001\Legal Training Data\Q&A Pairs"
OUT = r"C:\Users\User\mikeoss\finetune\data\drafting_work.jsonl"
DRAFT_SYS = ("You are Mike, an expert Indian legal clerk. Draft formal legal documents "
             "based only on facts provided by the user. If specific names, dates, "
             "addresses, or numbers are not provided, use placeholders.")

tok = AutoTokenizer.from_pretrained(MODEL)
rows = [json.loads(l) for l in open(BASE + "\\drafting_pairs.jsonl", encoding="utf-8")]

kept = []
for i, r in enumerate(rows):
    # mirror build_sft length filter: render with chat template, count tokens
    msgs = [{"role": "system", "content": DRAFT_SYS},
            {"role": "user", "content": r["instruction"]},
            {"role": "assistant", "content": r["output"]}]
    text = tok.apply_chat_template(msgs, tokenize=False, add_generation_prompt=False)
    n = len(tok(text, add_special_tokens=False)["input_ids"])
    if n > MAX_SEQ:
        continue
    kept.append({
        "idx": i,
        "doc_type": r.get("doc_type", "other"),
        "court": r.get("court", "Unknown"),
        "tokens": n,
        "output": r["output"],
        "preview": r["output"][:2500],
    })

with open(OUT, "w", encoding="utf-8") as f:
    for r in kept:
        f.write(json.dumps(r, ensure_ascii=False) + "\n")
print(f"wrote {len(kept)} trainable drafting rows to drafting_work.jsonl")
