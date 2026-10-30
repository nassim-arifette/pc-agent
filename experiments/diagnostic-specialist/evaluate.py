"""Frozen held-out decisions, exact paired settings, raw generations retained."""

import argparse
import json
from pathlib import Path
import statistics

from contract import BASE_MODEL, BASE_REVISION, score
from modeling import generate, load_model, load_rows


def summarize(records):
    metrics = [k for k, v in records[0]["scores"].items() if isinstance(v, bool)]
    episodes = sorted({r["episode_id"] for r in records})
    result = {"decisions": len(records), "episodes": len(episodes),
              "rates": {k: sum(r["scores"][k] for r in records) / len(records) for k in metrics},
              "latency_seconds_median": statistics.median(r["generation"]["seconds"] for r in records),
              "truncated_generations": sum(r["generation"]["hit_token_limit"] for r in records),
              "episode_strict_pass_rate": sum(all(r["scores"]["strict_pass"] for r in records if r["episode_id"] == ep) for ep in episodes) / len(episodes),
              "by_domain": {}}
    for domain in sorted({r["domain"] for r in records}):
        subset = [r for r in records if r["domain"] == domain]
        result["by_domain"][domain] = {"decisions": len(subset), "strict_pass": sum(r["scores"]["strict_pass"] for r in subset)}
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--adapter", type=Path)
    parser.add_argument("--limit", type=int)
    args = parser.parse_args()
    if args.out.exists():
        raise FileExistsError(args.out)
    rows = load_rows(args.data)
    if args.limit:
        rows = rows[:args.limit]
    model, tokenizer = load_model(args.adapter)
    # Same untimed warmup for both variants; never use a test example as a warmup.
    generate(model, tokenizer, [{"role": "system", "content": "detailed thinking off"}, {"role": "user", "content": "Reply OK."}], 8)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    records = []
    for row in rows:
        generation = generate(model, tokenizer, row["prompt"])
        record = {"id": row["id"], "episode_id": row["episode_id"], "domain": row["domain"],
                  "stage": row["stage"], "generation": generation, "scores": score(generation["text"], row)}
        records.append(record)
        print(json.dumps({"id": row["id"], "scores": record["scores"], "seconds": generation["seconds"]}), flush=True)
    result = {"base_model": BASE_MODEL, "base_revision": BASE_REVISION, "adapter": args.adapter.name if args.adapter else None,
              "generation": {"do_sample": False, "max_new_tokens": 256, "quantization": "NF4 double, BF16 compute", "seed": 1729},
              "summary": summarize(records), "records": records,
              "limitations": ["Automatic synthetic-policy rubric, not human diagnostic accuracy.",
                              "Exact missing-evidence tags test contract following; synonymous free text is rejected.",
                              "Domain families are shared; scenario parents and episodes are disjoint.",
                              "No comparison with the large Token Factory analyst; no real-PC generalization claim."]}
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result["summary"], indent=2), flush=True)


if __name__ == "__main__":
    main()
