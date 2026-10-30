"""QLoRA with explicit completion-only labels and no truncated targets."""

import argparse
import importlib.metadata
import json
from pathlib import Path
import time

from contract import BASE_MODEL, BASE_REVISION
from modeling import encode_row, load_model, load_rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--max-steps", type=int, default=-1)
    parser.add_argument("--epochs", type=float, default=1)
    args = parser.parse_args()
    if args.out.exists():
        raise FileExistsError("Use a new run directory; do not overwrite a checkpoint")
    import torch
    from datasets import Dataset
    from peft import LoraConfig, TaskType, get_peft_model, prepare_model_for_kbit_training
    from transformers import Trainer, TrainingArguments

    start = time.monotonic()
    model, tokenizer = load_model(train=True)
    model = prepare_model_for_kbit_training(model, use_gradient_checkpointing=True,
                                            gradient_checkpointing_kwargs={"use_reentrant": False})
    model = get_peft_model(model, LoraConfig(task_type=TaskType.CAUSAL_LM, r=16, lora_alpha=32,
                                            lora_dropout=.05, target_modules="all-linear", bias="none"))
    model.print_trainable_parameters()
    datasets = {}
    lengths = {}
    for split in ["train", "validation"]:
        encoded = [encode_row(tokenizer, row) for row in load_rows(args.data / f"{split}.jsonl")]
        lengths[split] = {"min": min(len(x["input_ids"]) for x in encoded), "max": max(len(x["input_ids"]) for x in encoded),
                          "completion_tokens_min": min(sum(t != -100 for t in x["labels"]) for x in encoded)}
        datasets[split] = Dataset.from_list(encoded)

    def collate(batch):
        size = max(len(x["input_ids"]) for x in batch)
        padding = {"input_ids": tokenizer.pad_token_id, "attention_mask": 0, "labels": -100}
        return {key: torch.tensor([x[key] + [pad] * (size - len(x[key])) for x in batch]) for key, pad in padding.items()}

    training_args = TrainingArguments(
        output_dir=str(args.out / "checkpoints"), per_device_train_batch_size=1,
        per_device_eval_batch_size=1, gradient_accumulation_steps=16, num_train_epochs=args.epochs,
        max_steps=args.max_steps, learning_rate=1e-4, lr_scheduler_type="cosine", warmup_ratio=.05,
        bf16=True, optim="adamw_torch", gradient_checkpointing=True,
        gradient_checkpointing_kwargs={"use_reentrant": False}, logging_steps=1,
        eval_strategy="epoch", save_strategy="no", report_to="none", seed=1729, data_seed=1729,
        remove_unused_columns=False, dataloader_num_workers=0,
        label_names=["labels"],
    )
    trainer = Trainer(model=model, args=training_args, train_dataset=datasets["train"],
                      eval_dataset=datasets["validation"], data_collator=collate)
    metrics = trainer.train().metrics
    validation = trainer.evaluate()
    adapter = args.out / "adapter"
    model.save_pretrained(adapter, safe_serialization=True)
    tokenizer.save_pretrained(adapter)
    report = {"base_model": BASE_MODEL, "base_revision": BASE_REVISION,
              "method": "SFT QLoRA, completion-only causal LM loss", "seed": 1729,
              "train_metrics": metrics, "validation_metrics": validation, "token_lengths": lengths,
              "total_seconds": time.monotonic() - start,
              "peak_cuda_gib": torch.cuda.max_memory_allocated() / 1024**3,
              "gpu": torch.cuda.get_device_name(0), "trainer_log": trainer.state.log_history,
              "packages": {p: importlib.metadata.version(p) for p in ["torch", "transformers", "peft", "trl", "bitsandbytes", "accelerate", "datasets", "safetensors"]},
              "configuration": {"rank": 16, "alpha": 32, "dropout": .05, "learning_rate": 1e-4,
                                "max_length": 2048, "microbatch": 1, "accumulation": 16, "epochs": args.epochs,
                                "max_steps": args.max_steps, "targets": "all-linear"}}
    (args.out / "training.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"saved": str(adapter), "metrics": metrics, "peak_cuda_gib": report["peak_cuda_gib"]}), flush=True)
    # Reload in a fresh process with evaluate.py; an in-memory model is not a reload proof.


if __name__ == "__main__":
    main()
