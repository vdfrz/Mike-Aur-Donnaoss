"""Step 5 GATE: citation regression + sanity inference + drafting quality.

Run in three passes so base and FT never share VRAM:
    python eval_gate.py gen --model base
    python eval_gate.py gen --model ft         # uses lora_adapters
    python eval_gate.py gen --model ft_drafting # uses lora_adapters_drafting_only (ablation)
    python eval_gate.py score                   # compares whatever gens exist

`gen` writes outputs/gen_<model>.json holding, per val_comprehension row, the
generated answer; plus a handful of drafting + comprehension sanity samples.
`score` extracts statute/section citations from each answer, compares the set to
the reference answer's set, and reports correct / hallucinated counts per model.

Citation extraction is a heuristic (regex over common Indian-legal forms); treat
the numbers as a directional signal, not ground truth.
"""
import sys, json, os, re, argparse

DATA = r"C:\Users\User\mikeoss\finetune\data"
OUTDIR = r"C:\Users\User\mikeoss\finetune\outputs"
BASE_MODEL = "unsloth/Qwen3-4B"
ADAPTERS = {
    "ft": r"C:\Users\User\mikeoss\finetune\lora_adapters",
    "ft_drafting": r"C:\Users\User\mikeoss\finetune\lora_adapters_drafting_only",
}
MAX_SEQ = 4096

# ---- citation extraction (heuristic) ----
ACT_ACRONYMS = ["IPC", "CrPC", "CPC", "BNS", "BNSS", "BSA", "NI Act", "GST",
                "CGST", "SGST", "IGST", "RTI", "HMA", "CrPC", "NDPS", "POCSO",
                "IBC", "SARFAESI", "MV Act", "PMLA"]
SECRE = re.compile(
    r"(?:section|sec\.?|s\.|u/s|under section|article|art\.?|order|rule|clause)\s*"
    r"([0-9]+\s*[A-Z]{0,3})", re.IGNORECASE)
ACT_YEAR_RE = re.compile(r"([A-Z][A-Za-z&'(),. ]{4,60}?Act),?\s*(\d{4})")

def extract_citations(text):
    cites = set()
    for m in SECRE.finditer(text):
        num = re.sub(r"\s+", "", m.group(1)).upper()
        cites.add("S" + num)
    for m in ACT_YEAR_RE.finditer(text):
        name = re.sub(r"\s+", " ", m.group(1)).strip().lower()
        cites.add(f"{name} {m.group(2)}")
    low = text.lower()
    for ac in ACT_ACRONYMS:
        if re.search(r"\b" + re.escape(ac.lower()) + r"\b", low):
            cites.add(ac.lower())
    return cites

def strip_think(text):
    return re.sub(r"<think>.*?</think>", "", text, flags=re.DOTALL).strip()

# ---- generation ----
def run_gen(which):
    from unsloth import FastLanguageModel
    import torch
    name = ADAPTERS[which] if which in ADAPTERS else BASE_MODEL
    model, tok = FastLanguageModel.from_pretrained(
        model_name=name, max_seq_length=MAX_SEQ, load_in_4bit=True, dtype=None)
    FastLanguageModel.for_inference(model)

    def gen(system, user, max_new):
        msgs = [{"role": "system", "content": system},
                {"role": "user", "content": user}]
        ids = tok.apply_chat_template(msgs, add_generation_prompt=True,
                                      return_tensors="pt").to("cuda")
        with torch.no_grad():
            out = model.generate(input_ids=ids, max_new_tokens=max_new,
                                 do_sample=False, temperature=None, top_p=None,
                                 top_k=None, pad_token_id=tok.eos_token_id)
        return strip_think(tok.decode(out[0][ids.shape[1]:], skip_special_tokens=True))

    comp = [json.loads(l) for l in open(DATA + "\\val_comprehension.jsonl", encoding="utf-8")]
    draft = [json.loads(l) for l in open(DATA + "\\val_drafting.jsonl", encoding="utf-8")]

    result = {"comprehension": [], "sanity_comprehension": [], "sanity_drafting": []}
    for r in comp:
        result["comprehension"].append({
            "user": r["user"], "reference": r["output"],
            "answer": gen(r["system"], r["user"], 320)})
    for r in comp[:5]:
        result["sanity_comprehension"].append({
            "user": r["user"], "reference": r["output"][:400],
            "answer": gen(r["system"], r["user"], 320)})
    for r in draft[:5]:
        result["sanity_drafting"].append({
            "user": r["user"], "reference": r["output"][:400],
            "answer": gen(r["system"], r["user"], 900)})

    os.makedirs(OUTDIR, exist_ok=True)
    with open(OUTDIR + f"\\gen_{which}.json", "w", encoding="utf-8") as f:
        json.dump(result, f, ensure_ascii=False, indent=2)
    print(f"wrote gen_{which}.json ({len(result['comprehension'])} comprehension rows)")

# ---- scoring ----
def run_score():
    models = [m for m in ["base", "ft", "ft_drafting"]
              if os.path.exists(OUTDIR + f"\\gen_{m}.json")]
    print("scoring models:", models)
    for m in models:
        data = json.load(open(OUTDIR + f"\\gen_{m}.json", encoding="utf-8"))
        tot_model_cites = correct = hallucinated = rows_with_ref_cites = 0
        for row in data["comprehension"]:
            ref = extract_citations(row["reference"])
            ans = extract_citations(row["answer"])
            if ref:
                rows_with_ref_cites += 1
            tot_model_cites += len(ans)
            correct += len(ans & ref)
            hallucinated += len(ans - ref)
        acc = correct / tot_model_cites if tot_model_cites else 0.0
        print(f"\n=== {m} ===")
        print(f"  comprehension rows: {len(data['comprehension'])} "
              f"(with citations in reference: {rows_with_ref_cites})")
        print(f"  citations emitted: {tot_model_cites} | "
              f"matching reference: {correct} | not in reference: {hallucinated}")
        print(f"  citation precision (correct/emitted): {acc:.3f}")

    print("\n--- GATE: ship full FT only if its precision is NOT worse than base ---")
    print("--- (and eyeball sanity_drafting for register quality) ---")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mode", choices=["gen", "score"])
    ap.add_argument("--model", choices=["base", "ft", "ft_drafting"])
    a = ap.parse_args()
    if a.mode == "gen":
        run_gen(a.model)
    else:
        run_score()

if __name__ == "__main__":
    main()
