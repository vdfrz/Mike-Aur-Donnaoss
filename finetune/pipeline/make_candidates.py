"""Drop order-sheet/proceedings rows; keep genuine draftable docs (+ambiguous) as
candidates for instruction rewriting."""
import json, re
WORK = r"C:\Users\User\mikeoss\finetune\data\drafting_work.jsonl"
OUT = r"C:\Users\User\mikeoss\finetune\data\drafting_candidates.jsonl"
rows = [json.loads(l) for l in open(WORK, encoding="utf-8")]

def is_ordersheet(o):
    head = o[:600]; sig = 0
    if re.search(r"\bPresent\s*:", head): sig += 1
    if re.search(r"Ld\.?\s*(Counsel|MM|ASJ|Presiding)", head, re.I): sig += 1
    if re.search(r"Put up (on|for)|next date of hearing|NDOH|for further proceedings", o, re.I): sig += 1
    if re.search(r"Digitally signed|Ld\. MM|DISTRICT,", o): sig += 1
    return sig >= 2

kept = [r for r in rows if not is_ordersheet(r["output"])]
with open(OUT, "w", encoding="utf-8") as f:
    for r in kept:
        f.write(json.dumps(r, ensure_ascii=False) + "\n")
print(f"candidates: {len(kept)} (dropped {len(rows)-len(kept)} order-sheets)")
