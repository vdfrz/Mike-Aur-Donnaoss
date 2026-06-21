import inspect, importlib.metadata as m
for p in ["triton", "triton_windows", "xformers"]:
    try:
        print("HAS", p, m.version(p))
    except Exception:
        print("MISSING", p)

from trl import SFTConfig, SFTTrainer
fields = set(SFTConfig.__dataclass_fields__.keys())
for k in ["max_seq_length", "max_length", "dataset_text_field", "packing",
          "eval_strategy", "evaluation_strategy", "dataset_num_proc",
          "per_device_train_batch_size", "gradient_accumulation_steps",
          "num_train_epochs", "learning_rate", "lr_scheduler_type",
          "warmup_ratio", "weight_decay", "logging_steps", "eval_steps",
          "save_steps", "output_dir", "seed", "report_to"]:
    print(f"SFTConfig.{k}:", k in fields)

sig = inspect.signature(SFTTrainer.__init__)
print("SFTTrainer params:", list(sig.parameters.keys()))
