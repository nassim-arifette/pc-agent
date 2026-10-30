"""Generate transparent synthetic episodes; never ingest personal captures."""

import argparse
import copy
import hashlib
from itertools import combinations
import json
from pathlib import Path
import random

from contract import PREMATURE, SYSTEM, call, prompt, validate_decision

DOMAINS = ["cpu", "memory", "disk", "normal", "insufficient"]
CONTEXTS = ["editing a document", "browsing several tabs", "opening a spreadsheet", "using a drawing application",
            "joining a video call", "viewing a presentation", "loading a CAD project", "scrolling a photo library",
            "switching between windows", "running a local development workspace", "opening a PDF", "playing a video",
            "using a music application", "opening a database viewer", "loading a map", "using a translation editor",
            "opening a design board", "using a note-taking application", "opening a terminal", "using a 3D viewer",
            "reading a large report", "opening a slide deck", "using a planning application", "viewing image thumbnails",
            "switching virtual desktops", "opening an archive viewer", "using a code editor", "viewing a timeline",
            "using a browser dashboard", "working with a diagram",
            "exporting a chart", "previewing a web page", "organizing a task board",
            "reviewing an audio project", "displaying a simulation", "annotating a drawing",
            "comparing document revisions", "arranging a workspace", "navigating a reference manual",
            "reviewing a project schedule", "presenting a dashboard", "editing a table",
            "viewing a training lesson", "checking a monitoring panel", "reordering a playlist"]


def compact(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def digest(value):
    return hashlib.sha256(compact(value).encode()).hexdigest()


def observation(ident, tool_call, payload):
    return {"id": ident, **tool_call, "output": payload}


def target(decision, hypotheses, refs, missing, next_call):
    return {"decision": decision, "hypotheses": hypotheses, "evidence_refs": refs,
            "missing": missing, "next_call": next_call, "premature_conclusions": PREMATURE}


def episode(domain, parent_index, variant):
    rng = random.Random(1729 + parent_index * 1009 + variant * 7919)
    active = domain != "normal" or parent_index % 2 == 0
    state = {"symptom": f"My PC feels slow while {CONTEXTS[parent_index]}; this has lasted about {rng.randint(2, 45)} minutes.",
             "symptom_active_now": active, "observations": [], "completed_calls": [],
             "data_origin": "synthetic controlled episode; not a real PC measurement"}
    rows = []

    def add(value, alternatives=None):
        validate_decision(value, state)
        rows.append({"state": copy.deepcopy(state), "target": value,
                     "rubric": {"decisions": [value["decision"]], "hypotheses": value["hypotheses"],
                                "next_calls": alternatives or [value["next_call"]], "required_missing": value["missing"]}})

    add(target("collect", ["unknown"], [], ["current_measurements"], call("get_system_snapshot")))
    total = rng.choice([8, 16, 32, 64]) * 1024**3
    values = {"physical_total_bytes": total, "physical_available_bytes": int(total * rng.uniform(.35, .7)),
              "cpu_busy_basis_points": rng.randint(400, 2100), "cpu_busiest_core_basis_points": rng.randint(2500, 5500),
              "disk_read_latency_microseconds": rng.randint(200, 1800), "disk_write_latency_microseconds": rng.randint(200, 1800),
              "hard_fault_pages_per_second_x100": rng.randint(0, 80)}
    if domain == "cpu":
        values["cpu_busy_basis_points"] = rng.randint(8500, 9900)
        values["cpu_busiest_core_basis_points"] = max(values["cpu_busy_basis_points"], rng.randint(9700, 10000))
    elif domain == "memory":
        values["physical_available_bytes"] = int(total * rng.uniform(.015, .06))
    elif domain == "disk":
        values["disk_read_latency_microseconds"] = rng.randint(30000, 85000)
    elif domain == "insufficient":
        values = {k: None for k in values}
    units = {k: ("bytes" if k.endswith("bytes") else "microseconds" if "latency" in k else
                 "pages_per_second_x100" if "fault" in k else "basis_points") for k in values}
    snap = {"schema_version": 1, "origin": "synthetic_fixture", "status": "partial" if domain == "insufficient" else "ok",
            "measured_at_unix_ms": 1793318400000 + parent_index * 100000 + variant * 1000,
            "window_ms": 1000, "measurements": {"values": {"system": values}, "units": units},
            "limitations": ["Collectors unavailable; values are unknown"] if domain == "insufficient" else ["One-second snapshot; not proof of persistence or cause"]}
    state["observations"].append(observation("O1", call("get_system_snapshot"), {"cloud": snap}))
    state["completed_calls"].append(call("get_system_snapshot"))
    hypothesis = {"cpu": "cpu_contention", "memory": "memory_pressure", "disk": "disk_contention",
                  "normal": "none_observed", "insufficient": "unknown"}[domain]
    metric = {"cpu": "cpu_busy_basis_points", "memory": "physical_available_bytes", "disk": "disk_read_latency_microseconds",
              "normal": "cpu_busy_basis_points", "insufficient": "cpu_busy_basis_points"}[domain]
    history_call = call("get_metric_history", metric=metric, window_seconds=60)
    recording = call("record_activity", duration_seconds=30)
    if domain == "insufficient":
        add(target("collect", [hypothesis], ["O1"], ["sensor_coverage"], recording))
        follow_call = recording
    else:
        add(target("collect", [hypothesis], ["O1"], ["persistence"], history_call), [history_call, recording])
        follow_call = history_call
    transient = domain in ["cpu", "memory", "disk"] and parent_index % 6 in [0, 3, 4]
    if domain == "insufficient":
        history = {"status": "partial", "coverage_fraction": 0, "points": [], "limitations": ["Collector still unavailable after retry"]}
    else:
        v = values[metric]
        if transient:
            v = int(total * .45) if domain == "memory" else 1100 if domain == "disk" else 1400
        history = {"status": "ok", "metric": metric, "unit": units[metric], "window_seconds": 60,
                   "coverage_fraction": 1.0, "overlaps_reported_symptom": active,
                   "points": [{"offset_seconds": i * 10, "value": max(0, int(v * rng.uniform(.94, 1.04)))} for i in range(6)],
                   "limitations": ["Synthetic replay of recorded samples; correlation is not causation"]}
    state["observations"].append(observation("O2", follow_call, history))
    state["completed_calls"].append(follow_call)
    if domain == "insufficient":
        add(target("handoff", ["unknown"], ["O1", "O2"], ["sensor_coverage"], None))
        # A user clarification changes timing, not the missing sensor evidence.
        state["symptom_active_now"] = True
        state["symptom"] += " The issue is still occurring, but no measurements are available."
        add(target("handoff", ["unknown"], ["O1", "O2"], ["sensor_coverage"], None))
        return rows
    if transient or domain == "normal":
        state["symptom_active_now"] = False if transient else active
        add(target("ask" if not state["symptom_active_now"] else "summarize", ["none_observed"], ["O1", "O2"],
                   ["symptom_timing"] if not state["symptom_active_now"] else [], None))
        state["symptom_active_now"] = True
        state["symptom"] += " The same symptom is present throughout this new recording."
        normal_recording = {"status": "ok", "duration_seconds": 30, "coverage_fraction": 1,
                            "overlaps_reported_symptom": True, "cpu_busy_basis_points_range": [800, 1800],
                            "physical_available_fraction_range": [.4, .5], "disk_read_latency_microseconds_range": [500, 1700],
                            "limitations": ["No measured CPU, memory or disk contention in this window; other causes are untested"]}
        state["observations"].append(observation("O3", recording, normal_recording))
        state["completed_calls"].append(recording)
        add(target("summarize", ["none_observed"], ["O2", "O3"], [], None))
        return rows
    if domain == "memory":
        next_call = recording
        missing = ["correlated_paging_and_io"]
        payload = {"status": "ok", "duration_seconds": 30, "coverage_fraction": 1, "overlaps_reported_symptom": True,
                   "physical_available_fraction_range": [.02, .05], "hard_fault_pages_per_second_x100_range": [9000, 17000],
                   "disk_read_latency_microseconds_range": [35000, 65000],
                   "limitations": ["Coincident pressure and paging observed; not proof of a memory leak or hardware fault"]}
        alternatives = [recording, call("get_metric_history", metric="hard_fault_pages_per_second_x100", window_seconds=60)]
    else:
        next_call = call("get_processes", limit=5, sort_by="cpu" if domain == "cpu" else "io_read")
        missing = ["process_contributors"]
        payload = {"status": "ok", "window_ms": 1000, "processes": [
            {"entity_ref": "proc-00000001", "cpu_basis_points": rng.randint(7000, 8500) if domain == "cpu" else 500,
             "io_read_bytes_per_second": rng.randint(50000000, 150000000) if domain == "disk" else 1000}],
            "coverage": "complete_for_enumerated_processes", "limitations": ["A contributor is not a proven root cause"]}
        alternatives = [next_call] if domain == "cpu" else [next_call, call("get_processes", limit=5, sort_by="io_write")]
    add(target("collect", [hypothesis], ["O1", "O2"], missing, next_call), alternatives)
    state["observations"].append(observation("O3", next_call, payload))
    state["completed_calls"].append(next_call)
    add(target("summarize", [hypothesis], ["O1", "O2", "O3"], [], None))
    return rows


def build(out):
    out.mkdir(parents=True, exist_ok=True)
    manifest = {"format_version": 2, "seed": 1729, "origin": "synthetic_only", "human_reviewed_examples": 0,
                "serialization": "canonical sorted-key JSON for stored and runtime prompts",
                "test_policy": "Fresh parents 30-44 reserved after the v1 serialization bug; previously inspected parents are regression-only, never final test or training.",
                "label_method": "Explicit, auditable measurement-policy contracts; no teacher-LLM causal labels.",
                "limitations": ["Programmatic contract checks are not independent expert review.",
                                "Held-out scenario parents, not held-out diagnostic domains or real machines.",
                                "No real PC captures, personal traces or future measurements are in training inputs."], "splits": {}}
    all_rows = {}
    for split, indices, variants in [("train", range(3), 7), ("validation", range(3, 4), 2),
                                     ("regression", range(4, 6), 1), ("test", range(3), 1)]:
        rows = []
        for domain_index, domain in enumerate(DOMAINS):
            for index in indices:
                parent = 30 + domain_index * 3 + index if split == "test" else domain_index * 6 + index
                parent_id = "scenario-" + digest([domain, parent])[:12]
                for variant in range(variants):
                    episode_id = "episode-" + digest([parent_id, variant])[:12]
                    for stage, row in enumerate(episode(domain, parent, variant)):
                        row.update({"id": f"{episode_id}-s{stage}", "episode_id": episode_id, "scenario_parent": parent_id,
                                    "domain": domain, "stage": stage, "origin": "synthetic_fixture"})
                        row["prompt"] = [{"role": "system", "content": SYSTEM}, {"role": "user", "content": prompt(row["state"])}]
                        row["completion"] = [{"role": "assistant", "content": compact(row["target"])}]
                        rows.append(row)
        random.Random(1730).shuffle(rows)
        path = out / f"{split}.jsonl"
        path.write_text("".join(compact(r) + "\n" for r in rows), encoding="utf-8", newline="\n")
        manifest["splits"][split] = {"decisions": len(rows), "episodes": len({r["episode_id"] for r in rows}),
                                     "scenario_parents": sorted({r["scenario_parent"] for r in rows}),
                                     "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        all_rows[split] = rows
    for a, b in combinations(all_rows, 2):
        for field in ["episode_id", "scenario_parent"]:
            assert not {r[field] for r in all_rows[a]} & {r[field] for r in all_rows[b]}, field
        assert not {digest(r["prompt"]) for r in all_rows[a]} & {digest(r["prompt"]) for r in all_rows[b]}, "duplicate prompt across splits"
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")
    return manifest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(build(args.out), indent=2))
