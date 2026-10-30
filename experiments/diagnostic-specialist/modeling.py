"""Identical pinned base and quantization for paired comparisons."""

import json
from pathlib import Path
import random
import time

from contract import BASE_MODEL, BASE_REVISION


def load_rows(path):
    return [json.loads(line) for line in Path(path).read_text(encoding="utf-8").splitlines() if line.strip()]


def load_model(adapter=None, train=False):
    import torch
    from transformers import AutoModelForCausalLM, AutoTokenizer, BitsAndBytesConfig

    random.seed(1729)
    torch.manual_seed(1729)
    torch.cuda.manual_seed_all(1729)
    tokenizer = AutoTokenizer.from_pretrained(BASE_MODEL, revision=BASE_REVISION, trust_remote_code=False)
    tokenizer.pad_token = tokenizer.eos_token
    tokenizer.padding_side = "right"
    model = AutoModelForCausalLM.from_pretrained(
        BASE_MODEL, revision=BASE_REVISION, trust_remote_code=False, use_safetensors=True,
        quantization_config=BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4",
            bnb_4bit_use_double_quant=True, bnb_4bit_compute_dtype=torch.bfloat16),
        torch_dtype=torch.bfloat16, device_map={"": 0}, attn_implementation="sdpa",
    )
    model.config.pad_token_id = tokenizer.pad_token_id
    model.config.use_cache = not train
    if adapter:
        from peft import PeftModel
        model = PeftModel.from_pretrained(model, str(adapter), is_trainable=train)
    return model, tokenizer


def encode_row(tokenizer, row, max_length=2048):
    prefix = tokenizer.apply_chat_template(row["prompt"], tokenize=True, add_generation_prompt=True)
    full = tokenizer.apply_chat_template(row["prompt"] + row["completion"], tokenize=True, add_generation_prompt=False)
    if full[:len(prefix)] != prefix:
        raise ValueError("Chat template prefix is not stable; refusing incorrect loss masking")
    if len(full) > max_length:
        raise ValueError(f"{row['id']}: {len(full)} tokens exceeds {max_length}; no silent truncation")
    labels = [-100] * len(prefix) + full[len(prefix):]
    if not any(x != -100 for x in labels):
        raise ValueError("Empty completion")
    return {"input_ids": full, "attention_mask": [1] * len(full), "labels": labels}


def generate(model, tokenizer, messages, max_new_tokens=256):
    import torch

    model.eval()
    inputs = tokenizer.apply_chat_template(messages, tokenize=True, add_generation_prompt=True,
                                           return_tensors="pt").to(model.device)
    torch.cuda.synchronize()
    start = time.monotonic()
    with torch.inference_mode():
        output = model.generate(inputs, attention_mask=torch.ones_like(inputs), do_sample=False,
                                max_new_tokens=max_new_tokens, pad_token_id=tokenizer.pad_token_id,
                                eos_token_id=tokenizer.eos_token_id, use_cache=True)
    torch.cuda.synchronize()
    generated = output[0, inputs.shape[1]:]
    return {"text": tokenizer.decode(generated, skip_special_tokens=True).strip(),
            "seconds": time.monotonic() - start, "input_tokens": inputs.shape[1],
            "output_tokens": len(generated), "hit_token_limit": len(generated) >= max_new_tokens}
