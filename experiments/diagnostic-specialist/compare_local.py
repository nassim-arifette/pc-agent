"""Paired local comparison on frozen synthetic prompts; no Nebius account required."""

import argparse
import json
from pathlib import Path
from urllib.request import Request, urlopen

from contract import score
from local import HOST, PORT
from modeling import load_rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True, help="Frozen test.jsonl from the release")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--limit", type=int, default=4, help="CPU default: four decisions; 0 runs all")
    args = parser.parse_args()
    rows = load_rows(args.data)
    if args.limit < 0:
        parser.error("--limit must be nonnegative")
    if args.limit:
        rows = rows[:args.limit]
    results = []
    for row in rows:
        pair = {"id": row["id"], "data_origin": "frozen synthetic test; not a live PC"}
        for variant in ("base", "adapter"):
            request = Request(f"http://{HOST}:{PORT}/decide",
                              data=json.dumps({"state": row["state"], "variant": variant}).encode(),
                              headers={"Content-Type": "application/json"})
            with urlopen(request, timeout=600) as response:
                result = json.load(response)
            result["scores"] = score(result["raw"], row)
            pair[variant] = result
            print(f"{row['id']} {variant}: strict={result['scores']['strict_pass']}, {result['seconds']:.2f}s", flush=True)
        results.append(pair)
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
