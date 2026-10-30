"""CPU-only contract and leakage regressions; no GPU or model download."""

import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from contract import CALLS, call, prompt, score
from demo import run_episode
from modeling import load_rows
from prepare import build, episode


class ExperimentTests(unittest.TestCase):
    def test_every_target_passes_its_own_rubric(self):
        for domain in ["cpu", "memory", "disk", "normal", "insufficient"]:
            for index in range(6):
                for row in episode(domain, index, 0):
                    self.assertTrue(score(json.dumps(row["target"]), row)["strict_pass"], (domain, index, row))

    def test_closed_contract_and_fabricated_evidence(self):
        row = episode("cpu", 1, 0)[1]
        for key, value in [("next_call", {"tool": "shell", "arguments": {"command": "anything"}}),
                           ("evidence_refs", ["O999"]), ("hypotheses", ["hardware_failure"])]:
            bad = copy.deepcopy(row["target"])
            bad[key] = value
            self.assertFalse(score(json.dumps(bad), row)["contract_valid"])
        bad = {**row["target"], "extra": "not allowed"}
        scores = score(json.dumps(bad), row)
        self.assertFalse(scores["contract_valid"])
        self.assertTrue(scores["decision_correct"])
        self.assertTrue(scores["next_measure_correct"])
        self.assertFalse(scores["strict_pass"])
        self.assertFalse(score("not JSON", row)["json_valid"])

    def test_repeated_call_and_inconsistent_action_rejected(self):
        row = episode("cpu", 1, 0)[1]
        bad = {**row["target"], "next_call": call("get_system_snapshot")}
        scores = score(json.dumps(bad), row)
        self.assertFalse(scores["contract_valid"])
        self.assertTrue(scores["unnecessary_repeat"])
        bad = {**row["target"], "decision": "summarize"}
        self.assertFalse(score(json.dumps(bad), row)["contract_valid"])

    def test_held_out_parents_and_reproducible_data(self):
        with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
            manifest = build(Path(a))
            self.assertEqual(manifest, build(Path(b)))
            self.assertEqual([manifest["splits"][s]["decisions"] for s in ["train", "validation", "regression", "test"]], [420, 40, 40, 60])
            self.assertEqual(manifest["splits"]["test"]["episodes"], 15)
            rows = load_rows(Path(a) / "test.jsonl")
            for row in rows:
                user_text = row["prompt"][1]["content"]
                self.assertNotIn(row["episode_id"], user_text)
                self.assertNotIn(row["scenario_parent"], user_text)
                self.assertNotIn('"target":', user_text)
                self.assertEqual(len(row["state"]["observations"]), len(row["state"]["completed_calls"]))

    def test_saved_state_reconstructs_exact_prompt(self):
        with tempfile.TemporaryDirectory() as directory:
            manifest = build(Path(directory))
            for split in manifest["splits"]:
                path = Path(directory) / f"{split}.jsonl"
                self.assertNotIn(b"\r\n", path.read_bytes())
                for row in load_rows(path):
                    self.assertEqual(row["prompt"][1]["content"], prompt(row["state"]))
                    reversed_state = dict(reversed(list(row["state"].items())))
                    self.assertEqual(prompt(row["state"]), prompt(reversed_state))

    def test_subset_names_exist_in_rust_catalog(self):
        root = Path(__file__).resolve().parents[2]
        catalogs = [root / "crates/agent/src/catalog.rs", root / "crates/agent/src/catalog/history.rs"]
        contents = "\n".join(p.read_text(encoding="utf-8") for p in catalogs)
        for tool in {c["tool"] for c in CALLS}:
            self.assertIn(f'"{tool}"', contents)

    def test_replay_never_fabricates_unsupported_tool_results(self):
        rows = [{**r, "episode_id": "fixture", "stage": i} for i, r in enumerate(episode("cpu", 1, 0))]
        original = copy.deepcopy(rows)
        decision = {**rows[0]["target"], "next_call": call("get_processes", limit=5, sort_by="io_write")}
        with patch("demo.generate", return_value={"text": json.dumps(decision)}), patch("builtins.print"):
            trace = run_episode(rows, "fixture", None, None)
        self.assertEqual(trace["stop_reason"], "unsupported_replay_call; no result fabricated")
        self.assertEqual(len(trace["turns"]), 1)
        self.assertEqual(trace["turns"][0]["state"]["observations"], [])
        self.assertEqual(rows, original)


if __name__ == "__main__":
    unittest.main()
