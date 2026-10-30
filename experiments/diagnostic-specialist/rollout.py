"""Paired episode-level replay evaluation; unsupported calls never receive made-up results."""
import argparse
import json
from pathlib import Path

from contract import BASE_MODEL, BASE_REVISION, score
from demo import run_episode
from modeling import load_model, load_rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--adapter", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists():
        raise FileExistsError(args.out)
    model, tokenizer = load_model(args.adapter)
    rows = load_rows(args.data)
    by_stage = {(r["episode_id"], r["stage"]): r for r in rows}
    records = []
    for episode_id in sorted({r["episode_id"] for r in rows}):
        trace = run_episode(rows, episode_id, model, tokenizer)
        checks = []
        for index, turn in enumerate(trace["turns"]):
            row = by_stage.get((episode_id, index))
            checks.append(score(turn["generation"]["text"], row)["strict_pass"]
                          if row and row["state"] == turn["state"] else None)
        completed = bool(checks) and all(x is True for x in checks) and trace["stop_reason"] in ["ask", "summarize", "handoff"]
        records.append({"episode_id": episode_id, "domain": by_stage[(episode_id, 0)]["domain"],
                        "strict_checks": checks, "completed": completed, "trace": trace})
    result = {"base_model": BASE_MODEL, "base_revision": BASE_REVISION, "episodes": len(records),
              "completed": sum(r["completed"] for r in records), "records": records,
              "unscored_episodes": sum(None in r["strict_checks"] for r in records),
              "unsupported_call_episodes": sum(r["trace"]["stop_reason"].startswith("unsupported_replay_call") for r in records),
              "invalid_output_episodes": sum(r["trace"]["stop_reason"].startswith("Invalid decision") for r in records),
              "limitations": "Finite synthetic policy, not diagnostic accuracy. Unavailable alternate calls stop the exact replay. No user clarification is synthesized; states differing from saved steps are unscored (null) and not counted as strict successes."}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({k: result[k] for k in ["episodes", "completed"]}), flush=True)


if __name__ == "__main__":
    main()
