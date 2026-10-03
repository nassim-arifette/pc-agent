# Evaluation harness

`pcagent-eval` captures a situation once and replays its tool outputs for repeated investigations.
It implements the tooling for work package 7; a passing offline test is not a quality comparison
against Claude or ChatGPT. The current agent uses contract C1 (configuration B).

## Build and local data

On the development PC, put both compilation and temporary files on D:. No new external Rust
dependency is needed; the evaluator uses the workspace's pinned versions.

```powershell
$env:CARGO_TARGET_DIR = 'D:\pcagent-target'
$env:TEMP = 'D:\pcagent-tmp'
$env:TMP = 'D:\pcagent-tmp'
cargo build --release --locked -p pcagent-eval
$eval = 'D:\pcagent-target\release\pcagent-eval.exe'
New-Item -ItemType Directory -Force D:\pcagent-tmp\captures | Out-Null
```

Raw captures and result files must stay outside Git repositories. The program rejects destinations
inside one, including a worktree, and never overwrites an existing file. Parent directories must
exist. Failed setup removes only its own empty output reservation. Files are limited to 32 MiB;
a scenario contains at most 4,096 recorded calls. Unix output permissions are `0600`; Windows
outputs inherit their destination directory's permissions. Keep that directory private.

`--anonymize` changes what the model sees. It does **not** anonymize the local trace: snapshots,
process metadata, the question, notes and the investigation report remain local raw data.
There is no automatic publishing or training export. Check C5 and manually review an anonymized
copy before considering version control. Only synthetic examples are supplied in this repository.

## Capture a real situation

An offline capture calls the implemented read-only tools, without credentials or a provider:

```powershell
& $eval capture --name slow-pc --question 'Pourquoi mon PC rame ?' `
  --allow-recording --out D:\pcagent-tmp\captures\slow-pc.json
```

The sweep records a fresh system snapshot; process rankings (default and each explicit sort with
limit 10); details for those process references; process tree; process activity grouped by
application (default and `window_seconds: 3, limit: 10`); startup programs; services; recent
events (default and `days: 7, limit: 50`); system state; power and thermal; installed apps;
hardware inventory; cleanup opportunities; each allowlisted `query_system` query; preferences;
and every persisted history metric and the anomaly events over the last hour and the last day.
`record_activity` is not swept: a recording takes 30 to 120 seconds on purpose.

History tools read PC Agent's local database (`pcagent.sqlite` beside its settings) unless
`--history` names another file; the sampler keeps recording into it during the capture. A new or
empty history yields partial results with explicit gaps, never invented samples. Each output keeps
its own measurement time; the sweep is a sequence, not one simultaneous snapshot. Live MCP reads
the same default history.

Add `--scope 'D:\some-folder'` for explicitly authorized metadata scans. The sweep starts scans,
waits for their bounded workers, and captures overviews plus the largest items and cleanup
candidates (files and directories, limit 20). No scope means no folder scan. Partial, denied or
unsupported readings remain visible. No contents are read and no cleanup runs.

Use `--calls calls.json` instead of the sweep to record a chosen sequence. The whole list is
validated before any measurement:

```json
[
  {"tool": "get_system_snapshot", "arguments": {}},
  {"tool": "get_processes", "arguments": {"limit": 5, "sort_by": "cpu"}}
]
```

For a trace of a real model investigation, add `--allow-cloud`. This records the initial snapshot,
every actual tool call and its duration, the checked answer, unverified passages, errors,
per-request tokens, catalog-priced cost and total investigation time:

```powershell
& $eval capture --name slow-pc-super --question 'Pourquoi mon PC rame ?' `
  --allow-recording --allow-cloud --model nvidia/nemotron-3-super-120b-a12b `
  --out D:\pcagent-tmp\captures\slow-pc-super.json
```

Credentials use the existing environment / `%APPDATA%\pcagent\.env` loader. Keys never appear in
arguments or trace serialization. `--model` overrides only the model ID; the NVIDIA/Nemotron guard
stays active. No budget extension is accepted automatically. A failed or unverified investigation
still saves its trace, then exits with code 2. Before a live run, the same PC Agent sharing contract
applies: the question is currently verbatim (free-text C5 filtering remains work package 3), so keep
personal information and secrets out of it. Grader `--note` values never enter model messages.

## Replay and inspect what is shared

```powershell
# No collectors, provider or credentials; inspect the recorded sequence.
& $eval replay D:\pcagent-tmp\captures\slow-pc.json

# Model-facing initial envelope and every recorded tool output, with names removed.
& $eval replay D:\pcagent-tmp\captures\slow-pc.json --preview-cloud --anonymize

# New model investigation, using only the frozen recordings.
& $eval replay D:\pcagent-tmp\captures\slow-pc.json --allow-cloud `
  --model nvidia/Nemotron-3-Ultra-550b-a55b --out D:\pcagent-tmp\ultra-result.json
```

`--question` can override the captured question. Replay otherwise inherits the captured sharing
mode; `--anonymize` can reduce it. `--calls` supplies an offline probe list with the same format as
capture. Without `--out`, replay writes one JSON document to stdout. Provider progress goes to
stderr. Preview displays minimized payloads, not the raw local trace; the later ordering, citations
and context depend on the model's actual calls. It is not a preview of all future HTTP requests.

Replay matches the tool name and **exact validated argument object**. JSON key order is irrelevant;
omitted defaults differ from explicit defaults. Each repeated matching call consumes the next
recorded output; after exhaustion, the last one repeats and `reused_calls` increases. It does not
collect, sleep to simulate the original duration, advance timestamps or substitute nearby calls.
Original evidence, findings, process references and storage scope/item references are preserved.
The agent still assigns conversation-local `O1`/`F1` aliases as usual. Replayed local storage paths
are intentionally absent, and execution/action-proposal tools are not offered.

Unrecorded calls return `unsupported`, `inspection_complete: false`, and the available argument
objects for that tool. Results include `replay_misses`, `served_captures`, `unused_captures`,
`reused_calls` and `replay_complete`. A model may recover by asking for a recorded variant, but the
miss stays in the run. Add missing variants in a **new capture**, then rerun all configurations
against that same scenario. Never compare partial runs as if they saw equivalent evidence.

The result's `scenario_sha256` hashes the parsed scenario serialized as compact JSON, including
its defaults and metadata; whitespace in the source file does not change it. `harness_version`
is the package version. Also retain the Git revision and any uncommitted patch with each campaign:
the package version alone does not identify the source build.

| Exit | Meaning |
| --- | --- |
| 0 | Offline replay succeeded, or a model produced a verified answer with no replay misses. This is not a quality score. |
| 1 | File, scenario, configuration or transport setup error. |
| 2 | Partial/failed/unverified investigation, a replay miss, or invalid CLI syntax. Inspect stderr and any saved report. |

## Claude through MCP

The server uses newline-delimited JSON-RPC over stdio, following the MCP
[transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
[initialization](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) and
[tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) specifications.
It negotiates `2025-11-25`, `2025-06-18`, `2025-03-26` or `2024-11-05`, supports `ping`, `tools/list`
and `tools/call`, and handles `notifications/initialized`. It advertises no resources, prompts or
HTTP transport. Invalid requests return JSON-RPC errors; invalid tool arguments and missing
captures produce tool errors without dispatching a collector. Frames are limited to 64 KiB and
a session to 4,096 dispatched calls. Notifications never run tools.

Example server entry for a desktop MCP client (adapt the absolute scenario path):

```json
{
  "mcpServers": {
    "pcagent-eval": {
      "command": "D:\\pcagent-target\\release\\pcagent-eval.exe",
      "args": ["mcp", "--scenario", "D:\\pcagent-tmp\\captures\\slow-pc.json", "--allow-cloud"]
    }
  }
}
```

`--allow-cloud` explicitly permits exposing minimized outputs to the client and its model. The
server itself makes no provider requests. The extra `get_initial_context` tool supplies the same
initial observation and question as PC Agent. Ask Claude to call it first. Subsequent tools use
the same closed schemas, minimized outputs, finding aliases, citations and program-name projection
as the agent. Local notes, raw snapshots, PIDs, boot IDs and local paths are not exposed.

For a live toolbox, replace `--scenario ...` with `--live`, optionally adding `--scope` folders and
`--anonymize`. Live capture and live MCP require Windows; replay and its tests run on Linux too.
MCP stdout contains only protocol messages. Ending stdin exits and drops any background scan jobs.
An MCP client's final answer and token bill are **not** captured or verified by this tool server;
save them separately for configuration C. There is no `submit_answer` tool on MCP.

## Campaigns and grading

```powershell
# Smoke campaign with the supplied synthetic examples, no API calls.
.\scripts\evaluate.ps1 -OutputDirectory D:\pcagent-tmp\eval-offline

# Each scenario/model pair gets its own fresh replay cursor and JSON result.
.\scripts\evaluate.ps1 -Scenarios D:\pcagent-tmp\captures `
  -OutputDirectory D:\pcagent-tmp\eval-comparison -AllowCloud -Repeat 2 `
  -Models nvidia/nemotron-3-super-120b-a12b,nvidia/Nemotron-3-Ultra-550b-a55b
```

The script requires a new output directory and writes individual JSON results, stderr logs,
`summary.csv` (coverage, token counts, cost, duration, verification issues), and an empty
`scores.csv`. Missing usage/cost stays unknown, never an assumed zero. Synthetic runs exercise
the harness; they cannot establish diagnostic quality on real PCs.

For the weekly comparison, hold the scenario hash, question, sharing mode and tool registry
constant. Archive configuration A's historical build/results separately: the current harness
does not resurrect the old identifier-only controller. B uses the current C1 controller; C uses
Claude with the replay MCP server; D uses Claude Code/PowerShell during the captured situation;
E uses a contemporaneous Task Manager screenshot. D and E cannot reproduce an old PC state;
mark collection-time differences. Keep D's commands as evidence of tools we still need.

Before human grading, randomly label answers and keep the configuration/model mapping separately.
Give the grader the question, measured evidence, known limitations and locally established ground
truth; hide model, price and duration until scores are frozen. Use the same packet and rubric for
the independent LLM judge, with explicit cloud consent and C5 review of what is sent. The harness
does not automatically send raw traces to a judge. Compare disagreements with the human score;
do not silently replace it. The rubric is in [eval/rubric.md](../eval/rubric.md).

Report the five 0–2 scores, total out of 10, model bill, wall time, verification issues, capture
misses and reuse count. Separate capture gaps from model errors. The submission target remains
B >= 80% of C, with no unsafe recommendation and no unmarked invented number. Neither that ratio
nor a successful synthetic replay can be claimed without a real scored campaign.

## Validation log — 2026-10-02

The implementation supplies five synthetic situations, portable command tests, recorder/replay
round-trip and privacy tests, MCP protocol tests and a scripted model run through the actual C1
controller (including cost and persistent-number verification). No paid model comparison, Claude
Desktop session or human score is implied by these tests. Real campaign results are still pending.
