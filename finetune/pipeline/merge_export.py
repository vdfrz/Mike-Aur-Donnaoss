"""Step 6a: merge the LoRA adapter into the base weights as 16-bit safetensors.

Loads base (4bit) + trained adapter, dequantizes, merges, writes a standard HF
16-bit checkpoint that convert_hf_to_gguf.py can turn into a GGUF.
"""
from unsloth import FastLanguageModel

ADAPTERS = r"C:\Users\User\mikeoss\finetune\lora_adapters"
OUT = r"C:\Users\User\mikeoss\finetune\mike-legal-merged-16bit"
MAX_SEQ = 4096

model, tok = FastLanguageModel.from_pretrained(
    model_name=ADAPTERS, max_seq_length=MAX_SEQ, load_in_4bit=True, dtype=None)
model.save_pretrained_merged(OUT, tok, save_method="merged_16bit")
print("merged 16-bit checkpoint written to", OUT)
