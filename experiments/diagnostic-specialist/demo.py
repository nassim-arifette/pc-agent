"""Run a held-out synthetic episode; execute only exact stored replay calls."""

import argparse
import copy
import json
from pathlib import Path

from contract import prompt, SYSTEM, validate_decision
from modeling import generate, load_model, load_rows


def run_episode(rows, episode_id, model, tokenizer):
    rows = sorted([r for r in rows if r["episode_id"] == episode_id], key=lambda r: r["stage"])
    if not rows:
        raise ValueError("Unknown episode")
    import jsonschema
    state = copy.deepcopy(rows[0]["state"])
    outputs = [obs for row in rows for obs in row["state"]["observations"]]
    replay = {json.dumps({"tool": o["tool"], "arguments": o["arguments"]}, sort_keys=True): o for o in outputs}
    trace = {"origin": "held-out synthetic exact-call replay, not live telemetry", "episode_id": episode_id, "turns": []}
    for _ in range(6):
        generation = generate(model, tokenizer, [{"role": "system", "content": SYSTEM}, {"role": "user", "content": prompt(state)}])
        turn = {"state": json.loads(json.dumps(state)), "generation": generation}
        trace["turns"].append(turn)
        print(json.dumps(turn, indent=2), flush=True)
        try:
            decision = json.loads(generation["text"])
            validate_decision(decision, state)
        except (ValueError, TypeError, jsonschema.ValidationError) as exc:
            trace["stop_reason"] = f"Invalid decision: {exc}"
            break
        if decision["decision"] != "collect":
            trace["stop_reason"] = decision["decision"]
            break
        key = json.dumps(decision["next_call"], sort_keys=True)
        if key not in replay:
            trace["stop_reason"] = "unsupported_replay_call; no result fabricated"
            break
        state["observations"].append(replay[key])
        state["completed_calls"].append(decision["next_call"])
    else:
        trace["stop_reason"] = "turn_budget"
    return trace


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--adapter", type=Path)
    parser.add_argument("--episode")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    if args.out.exists():
        raise FileExistsError(args.out)
    rows = load_rows(args.data)
    episode_id = args.episode or next(r["episode_id"] for r in rows if r["domain"] == "memory" and r["stage"] == 3 and r["target"]["hypotheses"] == ["memory_pressure"])
    model, tokenizer = load_model(args.adapter)
    trace = run_episode(rows, episode_id, model, tokenizer)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(trace, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
