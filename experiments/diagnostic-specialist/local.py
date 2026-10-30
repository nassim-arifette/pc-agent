"""Loopback-only inference with the published adapter; no tool execution or cloud analyst."""

import argparse
from contextlib import nullcontext
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import threading
import time

from contract import BASE_MODEL, BASE_REVISION, SYSTEM, prompt, validate_decision

HOST = "127.0.0.1"
PORT = 8737
MAX_BODY = 65536
MAX_INPUT_TOKENS = 1792
MAX_OUTPUT_TOKENS = 256
ADAPTER_SHA256 = "b85a03348ebeb27a327bcf53c5b3da3bde931a34337650d0eccb9d5887dbd459"
CONFIG_SHA256 = "d5baa035eef489ac452dad3c982942869f1802a32769d57428d183a9c0d27e59"


def check_state(state):
    if not isinstance(state, dict) or set(state) != {
        "symptom", "symptom_active_now", "observations", "completed_calls", "data_origin"
    }:
        raise ValueError("Invalid state")
    if not isinstance(state["symptom"], str) or not 1 <= len(state["symptom"]) <= 4096:
        raise ValueError("Invalid symptom")
    if state["symptom_active_now"] is not None and type(state["symptom_active_now"]) is not bool:
        raise ValueError("Unknown symptom timing must be null")
    if not isinstance(state["data_origin"], str) or len(state["data_origin"]) > 256:
        raise ValueError("Invalid origin")
    if not isinstance(state["observations"], list) or not isinstance(state["completed_calls"], list):
        raise ValueError("Invalid observations")
    if len(state["observations"]) > 16 or len(state["completed_calls"]) > 16:
        raise ValueError("State exceeds the specialist scope")
    ids = []
    for observation in state["observations"]:
        if not isinstance(observation, dict) or not isinstance(observation.get("id"), str):
            raise ValueError("Observation has no citation")
        ids.append(observation["id"])
    if len(set(ids)) != len(ids):
        raise ValueError("Duplicate citation")


class Runtime:
    def __init__(self, adapter, device="auto", nf4=False, offline=False):
        import torch
        from peft import PeftModel
        from transformers import AutoModelForCausalLM, AutoTokenizer, BitsAndBytesConfig

        adapter = Path(adapter).resolve(strict=True)
        for name, digest in [("adapter_config.json", CONFIG_SHA256),
                             ("adapter_model.safetensors", ADAPTER_SHA256)]:
            if hashlib.sha256((adapter / name).read_bytes()).hexdigest() != digest:
                raise ValueError("Adapter differs from the verified v0.1.0 release")
        config = json.loads((adapter / "adapter_config.json").read_text(encoding="utf-8"))
        if config.get("base_model_name_or_path") != BASE_MODEL or config.get("peft_type") != "LORA":
            raise ValueError("Adapter is not for the pinned Nano 4B model")
        if not (adapter / "adapter_model.safetensors").is_file():
            raise ValueError("A safetensors adapter is required")
        device = "cuda" if device == "auto" and torch.cuda.is_available() else "cpu" if device == "auto" else device
        if device == "cuda" and not torch.cuda.is_available():
            raise ValueError("CUDA is unavailable; use --device cpu")
        if nf4 and device != "cuda":
            raise ValueError("NF4 requires CUDA; CPU uses float32")
        self.tokenizer = AutoTokenizer.from_pretrained(
            BASE_MODEL, revision=BASE_REVISION, trust_remote_code=False, local_files_only=offline)
        self.tokenizer.pad_token = self.tokenizer.eos_token
        dtype = torch.bfloat16 if device == "cuda" and torch.cuda.is_bf16_supported() else torch.float16 if device == "cuda" else torch.float32
        options = {"torch_dtype": dtype, "device_map": {"": device}}
        if nf4:
            options["quantization_config"] = BitsAndBytesConfig(
                load_in_4bit=True, bnb_4bit_quant_type="nf4", bnb_4bit_use_double_quant=True,
                bnb_4bit_compute_dtype=dtype)
        base = AutoModelForCausalLM.from_pretrained(
            BASE_MODEL, revision=BASE_REVISION, trust_remote_code=False, use_safetensors=True,
            local_files_only=offline, attn_implementation="sdpa", **options)
        self.model = PeftModel.from_pretrained(base, str(adapter), is_trainable=False,
                                             use_safetensors=True, local_files_only=True,
                                             device_map={"": device})
        self.model.eval()
        self.identity = {
            "protocol": 1, "model": BASE_MODEL, "revision": BASE_REVISION,
            "adapter_sha256": hashlib.sha256((adapter / "adapter_model.safetensors").read_bytes()).hexdigest(),
            "device": device, "precision": "nf4" if nf4 else str(dtype),
            "synthetic_training_only": True,
        }

    def decide(self, state, variant="adapter"):
        import torch

        check_state(state)
        if variant not in ("adapter", "base"):
            raise ValueError("Unknown variant")
        messages = [{"role": "system", "content": SYSTEM}, {"role": "user", "content": prompt(state)}]
        inputs = self.tokenizer.apply_chat_template(messages, tokenize=True, add_generation_prompt=True,
                                                    return_tensors="pt").to(self.model.device)
        if inputs.shape[1] > MAX_INPUT_TOKENS:
            raise ValueError("Context exceeds training budget; no silent truncation")
        if self.model.device.type == "cuda":
            torch.cuda.synchronize()
        start = time.monotonic()
        with torch.inference_mode(), self.model.disable_adapter() if variant == "base" else nullcontext():
            output = self.model.generate(
                inputs, attention_mask=torch.ones_like(inputs), do_sample=False,
                max_new_tokens=MAX_OUTPUT_TOKENS, pad_token_id=self.tokenizer.pad_token_id,
                eos_token_id=self.tokenizer.eos_token_id, use_cache=True)
        if self.model.device.type == "cuda":
            torch.cuda.synchronize()
        generated = output[0, inputs.shape[1]:]
        raw = self.tokenizer.decode(generated, skip_special_tokens=True).strip()
        hit_limit = len(generated) >= MAX_OUTPUT_TOKENS
        decision = None
        try:
            candidate = json.loads(raw)
            validate_decision(candidate, state)
            if not hit_limit:
                decision = candidate
        except (ValueError, TypeError, __import__("jsonschema").ValidationError):
            pass
        return {"identity": self.identity, "variant": variant, "decision": decision,
                "raw": raw, "input_tokens": inputs.shape[1], "output_tokens": len(generated),
                "seconds": time.monotonic() - start, "hit_token_limit": hit_limit}


class LocalServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, runtime, port=PORT):
        self.runtime = runtime
        self.inference_lock = threading.Lock()
        super().__init__((HOST, port), Handler)


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        # Never log questions, observations or model output.
        pass

    def allowed(self):
        return (self.headers.get("Host") in (f"{HOST}:{self.server.server_port}",)
                and self.headers.get("Origin") is None
                and self.headers.get("Sec-Fetch-Site") in (None, "none"))

    def respond(self, code, body):
        data = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        try:
            self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        if not self.allowed():
            return self.respond(403, {"error": "loopback_clients_only"})
        if self.path != "/health":
            return self.respond(404, {"error": "not_found"})
        return self.respond(200, self.server.runtime.identity)

    def do_POST(self):
        self.connection.settimeout(5)
        if not self.allowed():
            return self.respond(403, {"error": "loopback_clients_only"})
        if self.path != "/decide":
            return self.respond(404, {"error": "not_found"})
        if self.headers.get_content_type() != "application/json" or self.headers.get("Transfer-Encoding"):
            return self.respond(415, {"error": "json_required"})
        try:
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= MAX_BODY:
                return self.respond(413, {"error": "body_limit"})
            request = json.loads(self.rfile.read(size))
            if not isinstance(request, dict) or set(request) - {"state", "variant"}:
                raise ValueError("Invalid request")
            check_state(request["state"])
            variant = request.get("variant", "adapter")
            if variant not in ("adapter", "base"):
                raise ValueError("Invalid variant")
        except (ValueError, KeyError, TypeError, TimeoutError):
            return self.respond(400, {"error": "invalid_state"})
        if not self.server.inference_lock.acquire(blocking=False):
            return self.respond(409, {"error": "inference_busy"})
        try:
            result = self.server.runtime.decide(request["state"], variant)
            return self.respond(200, result)
        except ValueError:
            return self.respond(422, {"error": "unsupported_context"})
        except Exception:
            return self.respond(500, {"error": "inference_failed"})
        finally:
            self.server.inference_lock.release()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--adapter", required=True, type=Path)
    parser.add_argument("--device", choices=("auto", "cpu", "cuda"), default="auto")
    parser.add_argument("--nf4", action="store_true", help="CUDA only; install bitsandbytes==0.45.5 separately")
    parser.add_argument("--offline", action="store_true", help="Require an already cached pinned base model")
    args = parser.parse_args()
    runtime = Runtime(args.adapter, args.device, args.nf4, args.offline)
    with LocalServer(runtime) as server:
        print(f"Specialist ready at http://{HOST}:{PORT}; {runtime.identity['device']}; no cloud analyst", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
