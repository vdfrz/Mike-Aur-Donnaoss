"""Dump a batch of candidate drafting rows to a UTF-8 text file for reading."""
import json, sys
WORK = r"C:\Users\User\mikeoss\finetune\data\drafting_candidates.jsonl"
OUTV = r"C:\Users\User\mikeoss\finetune\data\batch_view.txt"
start, end = int(sys.argv[1]), int(sys.argv[2])
rows = [json.loads(l) for l in open(WORK, encoding="utf-8")]
with open(OUTV, "w", encoding="utf-8") as f:
    for r in rows[start:end]:
        f.write(f"\n===== idx={r['idx']} | doc_type={r['doc_type']} | court={r['court']} | tokens={r['tokens']} =====\n")
        f.write(r["preview"][:1500].replace("\r", " ") + "\n")
    f.write(f"\n[shown {start}..{min(end,len(rows))} of {len(rows)}]\n")
print("wrote", OUTV)
