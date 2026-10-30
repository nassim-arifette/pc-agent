"""Audit completion-only masking and token budgets without loading model weights."""
import argparse
import json
from pathlib import Path

from contract import BASE_MODEL, BASE_REVISION
from modeling import encode_row, load_rows


def main():
    from transformers import AutoTokenizer

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    tokenizer = AutoTokenizer.from_pretrained(BASE_MODEL, revision=BASE_REVISION, trust_remote_code=False)
    report = {}
    for path in sorted(args.data.glob("*.jsonl")):
        encoded = [encode_row(tokenizer, row) for row in load_rows(path)]
        report[path.stem] = {"count": len(encoded), "max_tokens": max(len(r["input_ids"]) for r in encoded),
                             "min_response_tokens": min(sum(x != -100 for x in r["labels"]) for r in encoded)}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
