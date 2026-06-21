"""Verify the Qwen3-4B chat template does NOT inject <think> blocks into the
assistant target when rendering a full (system,user,assistant) conversation for
training. Tries default and enable_thinking=False; reports which is clean."""
from transformers import AutoTokenizer
MODEL = "unsloth/Qwen3-4B"
tok = AutoTokenizer.from_pretrained(MODEL)

msgs = [
    {"role": "system", "content": "You are Mike, an expert Indian legal clerk."},
    {"role": "user", "content": "Draft a legal notice."},
    {"role": "assistant", "content": "NOTICE\n\nDear Sir,\nThis is a notice."},
]

def render(**kw):
    return tok.apply_chat_template(msgs, tokenize=False, add_generation_prompt=False, **kw)

for label, kw in [("default", {}), ("enable_thinking=False", {"enable_thinking": False})]:
    try:
        txt = render(**kw)
    except Exception as e:
        print(f"--- {label}: ERROR {e}")
        continue
    print(f"--- {label} ---")
    print(repr(txt))
    print("contains <think>:", "<think>" in txt)
    print()
