"""Re-score raw paired generations and write an auditable Markdown comparison."""

import argparse
import json
from pathlib import Path

from contract import score
from evaluate import summarize
from modeling import load_rows


def rescore(path, rows):
    result = json.loads(path.read_text(encoding="utf-8"))
    for record in result["records"]:
        record["scores"] = score(record["generation"]["text"], rows[record["id"]])
    result["summary"] = summarize(result["records"])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data", type=Path, required=True)
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--adapted", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    rows = {r["id"]: r for r in load_rows(args.data)}
    base, adapted = [rescore(path, rows) for path in [args.base, args.adapted]]
    for key in ["base_model", "base_revision", "generation"]:
        if base[key] != adapted[key]:
            raise ValueError(f"Mismatched paired setting: {key}")
    before = {r["id"]: r for r in base["records"]}
    after = {r["id"]: r for r in adapted["records"]}
    if set(before) != set(after) or set(before) != set(rows):
        raise ValueError("Comparison must cover exactly the same frozen dataset")
    paired = {"both_pass": 0, "base_only": 0, "adapter_only": 0, "both_fail": 0}
    for ident in rows:
        a, b = before[ident]["scores"]["strict_pass"], after[ident]["scores"]["strict_pass"]
        key = "both_pass" if a and b else "base_only" if a else "adapter_only" if b else "both_fail"
        paired[key] += 1
    result = {"base": base["summary"], "adapted": adapted["summary"], "paired_strict": paired}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.with_suffix(".json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    lines = ["# Synthetic next-measure evaluation", "", f"Base: `{base['base_model']}` at `{base['base_revision']}`.", "",
             f"{len(rows)} held-out decisions from {adapted['summary']['episodes']} episodes; no scenario-parent overlap with training.",
             "Candidate selection uses validation only. This is synthetic contract/policy following, not real-PC diagnostic accuracy.", "",
             "| Metric | Base | Adapter |", "| --- | ---: | ---: |"]
    for metric, value in base["summary"]["rates"].items():
        lines.append(f"| {metric} | {value:.1%} | {adapted['summary']['rates'][metric]:.1%} |")
    for metric in ["episode_strict_pass_rate", "latency_seconds_median", "truncated_generations"]:
        lines.append(f"| {metric} | {base['summary'][metric]} | {adapted['summary'][metric]} |")
    lines += ["", "## Paired decisions", "", "```json", json.dumps(paired, indent=2), "```", "",
              "## Limitations", "",
              "The families and generator policy are shared across splits; numeric variants are not independent novel problems. "
              "The episode is the relevant grouping unit. No expert review or real-machine validation is claimed. "
              "Exact missing-evidence tags measure schema following, and evidence-ID validity does not prove causal support. "
              "A correct request is not equivalent to a correct final diagnosis. No comparison with the large cloud analyst is made. "
              "Warmup-excluded latency is measured on one L40S with sequential quantized inference, not an application-level benchmark.", "",
              "## Remaining adapter errors", ""]
    errors = [r for r in after.values() if not r["scores"]["strict_pass"]]
    if not errors:
        lines += ["None under this finite automatic rubric; this does not establish broader reliability.", ""]
    for record in errors[:6]:
        row = rows[record["id"]]
        failed = [k for k, v in record["scores"].items() if v is False and k != "unnecessary_repeat"]
        lines += [f"### {record['id']} ({record['domain']}, state {record['stage']})", "",
                  "Failed checks: " + ", ".join(failed), "", "Expected synthetic-policy target:", "```json",
                  json.dumps(row["target"], indent=2), "```", "Raw model output:", "```text",
                  record["generation"]["text"], "```", ""]
    lines += ["All raw generations, including base errors and regressions, are preserved in the paired JSON files.", ""]
    args.out.write_text("\n".join(lines), encoding="utf-8")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
