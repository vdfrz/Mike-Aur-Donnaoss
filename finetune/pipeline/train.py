"""Step 4: Unsloth QLoRA SFT of Qwen3-4B for the mike-legal drafting register.

API note: written for trl 0.24 / transformers 5.5 (SFTConfig.max_length,
dataset_text_field in SFTConfig, SFTTrainer(processing_class=...)). The handover's
example used the older trl API; this is the same recipe adapted to installed versions.

Run a cheap smoke test first:  python train.py --smoke
Full run:                      python train.py
"""
import sys, json, os
from unsloth import FastLanguageModel
from trl import SFTTrainer, SFTConfig
from datasets import load_dataset

MODEL = "unsloth/Qwen3-4B"          # Step 2 DECISION
MAX_SEQ = 4096                      # Step 3b DECISION
DATA = r"C:\Users\User\mikeoss\finetune\data"
OUTDIR = r"C:\Users\User\mikeoss\finetune\outputs"
ADAPTERS = r"C:\Users\User\mikeoss\finetune\lora_adapters"

SMOKE = "--smoke" in sys.argv
# Allow training the drafting-only ablation (Step 5 fallback) via flag.
DRAFTING_ONLY = "--drafting-only" in sys.argv

def main():
    model, tokenizer = FastLanguageModel.from_pretrained(
        model_name=MODEL,
        max_seq_length=MAX_SEQ,
        load_in_4bit=True,
        dtype=None,                 # bf16 auto on Blackwell
    )
    model = FastLanguageModel.get_peft_model(
        model,
        r=16, lora_alpha=32, lora_dropout=0.0,
        target_modules=["q_proj", "k_proj", "v_proj", "o_proj",
                        "gate_proj", "up_proj", "down_proj"],
        bias="none", use_gradient_checkpointing="unsloth",
        random_state=3407,
    )

    train_file = "dataset_drafting_only.jsonl" if DRAFTING_ONLY else "train.jsonl"
    out = ADAPTERS + ("_drafting_only" if DRAFTING_ONLY else "")
    train_ds = load_dataset("json", data_files=DATA + "\\" + train_file, split="train")
    val_ds = load_dataset("json", data_files=DATA + "\\val.jsonl", split="train")

    cfg = SFTConfig(
        per_device_train_batch_size=1,      # 4096 window on 16GB -> bs1
        gradient_accumulation_steps=8,      # effective batch 8
        num_train_epochs=2,
        learning_rate=2e-4,
        lr_scheduler_type="cosine",
        warmup_ratio=0.05, weight_decay=0.01,
        logging_steps=10, eval_strategy="steps", eval_steps=50,
        save_steps=100, output_dir=OUTDIR,
        seed=3407,
        max_length=MAX_SEQ,
        dataset_text_field="text",
        dataset_num_proc=1,                 # REQUIRED on Windows
        packing=False,
        report_to="none",
    )
    if SMOKE:
        cfg.max_steps = 3
        cfg.eval_steps = 2
        cfg.save_steps = 1000

    trainer = SFTTrainer(
        model=model,
        args=cfg,
        train_dataset=train_ds,
        eval_dataset=val_ds,
        processing_class=tokenizer,
    )
    result = trainer.train()
    print("train metrics:", result.metrics)

    if not SMOKE:
        model.save_pretrained(out)
        tokenizer.save_pretrained(out)
        os.makedirs(OUTDIR, exist_ok=True)
        with open(OUTDIR + "\\log_history.json", "w", encoding="utf-8") as f:
            json.dump(trainer.state.log_history, f, indent=2)
        print("saved adapters to", out)

if __name__ == "__main__":
    main()
