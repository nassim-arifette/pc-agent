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

When a replay reports valid calls that were not captured, build a deduplicated union and record a
**new** situation. `--extra-calls` adds these exact variants after the offline sweep and validates
the whole list before any collector starts. It conflicts with `--calls` and `--allow-cloud`.

```powershell
.\scripts\eval-capture-calls.ps1 -Results D:\pcagent-tmp\baseline-super.json,D:\pcagent-tmp\baseline-ultra.json `
  -Output D:\pcagent-tmp\extra-calls.json
& $eval capture --name slow-pc-comparison --question 'Pourquoi mon PC rame ?' `
  --allow-recording --extra-calls D:\pcagent-tmp\extra-calls.json `
  --out D:\pcagent-tmp\captures\slow-pc-comparison.json
```

Rerun every configuration against the new scenario. Later measurements must not be appended to an
older capture: the PC and timestamps have changed. No variant is approximated, and replay's exact
matching rule remains unchanged.

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
and C5 projections apply to the question and measurements. Inspect the minimized preview; local
question metadata and notes are raw trace data, not a promise that every string is shared verbatim.
Grader `--note` values never enter model messages.

## Replay and inspect what is shared

```powershell
# No collectors or provider request; credentials are not required.
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

Results also retain `scenario_origin` and `tool_registry_sha256`, the fingerprint of the replay
tool definitions. The same fields appear in a saved MCP replay trace. An origin of
`synthetic_fixture` remains synthetic even when a paid model has answered it.

| Exit | Meaning |
| --- | --- |
| 0 | Offline replay succeeded, or a model produced a verified answer with no replay misses. This is not a quality score. |
| 1 | File, scenario, configuration or transport setup error. |
| 2 | Partial/failed/unverified investigation, a replay miss, or invalid CLI syntax. Inspect stderr and any saved report. |

## Reference models through MCP

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

For a client whose visible tool output is smaller than an MCP response, use the local stdio
adapter `scripts/eval-mcp-client.py`. It forwards the same RPC requests and pages every reply into
6,000-character pieces. Nested JSON text is decoded once for display; concatenating all pages
reconstructs the complete JSON data. Read each page with `{"bridge_next":true}` before the next
RPC, and close with `{"bridge_finish":true}`. The server's trace remains unchanged. Explicit
`--allow-cloud` and `--allow-recording` are required; use `--anonymize` for an anonymous reference.
The adjacent `.client.json` reports whether all pages were served and closure completed. This is
a transport check, not evidence that an external client actually attended to every page. Exclude
any run with visible truncation or incomplete client visibility from quality comparisons.

```powershell
python scripts/eval-mcp-client.py --binary $eval --scenario D:\pcagent-tmp\captures\slow-pc.json `
  --out D:\pcagent-tmp\references\reference-tools.json --allow-cloud --allow-recording --anonymize
python scripts/test-eval-mcp-client.py
```

For configuration C, record the actual replayed tool session as a new local file. Add these
arguments to the MCP server entry:

```json
["mcp", "--scenario", "D:\\pcagent-tmp\\captures\\slow-pc.json", "--allow-cloud",
 "--allow-recording", "--out", "D:\\pcagent-tmp\\references\\claude-tools.json"]
```

The trace is saved when stdin ends, including transport failure. It preserves the scenario and
registry fingerprints, sharing mode, actual calls and durations, replay misses, reuse count and
transport/recording completeness. Stdio still contains only JSON-RPC replies. Recording requires
its own explicit consent, never overwrites a file and is currently available in replay mode only.
Retained calls are bounded to 16 MiB; exhausting that budget refuses further tools and marks the
recording incomplete. `answer`, `usage` and `cost_usd` remain null because the server does not see
the client's response or bill. The trace contains local raw output; keep it private.

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

`manifest.json` records the executable SHA-256, checkout revision, scenario file hashes and origins,
requested models, sharing switch and repetitions. `source.patch` retains the tracked evaluation
and backend diff; `source-files.json` hashes tracked and untracked evaluation/backend sources.
`untracked-source/` also retains new source files so the patch can be reconstructed without a commit.
These files stay local too. The executable fingerprint identifies the tested binary. The checkout
record describes the files beside the script; confirm its relation to the binary with the person
who built it rather than assuming that the two match.

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

### Import references and prepare blind packets

Save each C/D/E answer separately, using [the local reference schema](../eval/reference.schema.json).
The requested reference models are **Claude Sonnet 5.5, medium**, **Codex `gpt-6.1-sol`, low**, or
**Codex `gpt-5.6-terra`, high**. Record `reasoning_effort` alongside the exact model name; the importer
and advisory-judge summary reject other model/effort combinations. No Opus reference or unsolicited
judge run is requested. A Codex replay/MCP reference uses C's same-tool condition, with its actual
model identified privately; PowerShell references remain D and screenshots E.
For C, copy the exact identity fields from its MCP trace and retain the client's answer and known
usage/bill. For D, retain the dated PowerShell commands; for E, retain the contemporaneous Task
Manager screenshot. Give D/E `evidence_basis: "capture_time"` and their actual start time. Their
offset from the scenario capture is reported, and they never enter an equivalent-replay B/C ratio.
Unknown duration, bill or tokens stay null. No reference answer is supplied in the repository.

Example C reference, with identity placeholders to replace from the actual trace:

```json
{
  "schema_version": 1,
  "configuration": "C",
  "model": "Claude Sonnet 5.5",
  "reasoning_effort": "medium",
  "scenario_sha256": "copy the 64-character scenario fingerprint",
  "tool_registry_sha256": "copy the 64-character registry fingerprint",
  "question": "copy the recorded question exactly",
  "sharing": "anonymized",
  "answer": "paste the actual final answer",
  "evidence_basis": "same_replay",
  "trace_file": "claude-tools.json",
  "duration_ms": null,
  "cost_usd": null,
  "usage": null,
  "unverified": null
}
```

Relative trace/evidence paths resolve inside the reference directory. D/E `evidence_files` use
`{"kind":"commands","path":"commands.txt"}` or
`{"kind":"screenshot","path":"task-manager.png"}`. This is a local import, not an upload.

Optionally create a ground-truth JSON array with one entry per scenario:
`{"scenario_sha256":"…","ground_truth":"locally established facts","limitations":["…"]}`.
The script never substitutes model prose or a local heuristic for established ground truth.

```powershell
.\scripts\grade.ps1 -CampaignDirectory D:\pcagent-tmp\eval-comparison `
  -ReferenceDirectory D:\pcagent-tmp\references -GroundTruthPath D:\pcagent-tmp\ground-truth.json `
  -OutputDirectory D:\pcagent-tmp\blind-packets -Seed 20261003

# After the human has filled grader/scores.csv; add separate rows for an independent LLM judge.
.\scripts\grade-summary.ps1 -PacketDirectory D:\pcagent-tmp\blind-packets `
  -Output D:\pcagent-tmp\graded-summary.json
```

Give the grader only `grader/`: shuffled answer labels, minimized recorded evidence, ground truth,
the rubric and a blank scores sheet. `private/` holds the seed, identity map, preview intermediates,
model, configuration, costs, duration and source/packet hashes. Repeating the same inputs and seed
reproduces the label order. Observed evidence follows the actual calls so its O/F aliases match
the answer; the complete available capture is included separately. Extra D/E evidence is copied
under opaque filenames. Its contents can still reveal context and must be reviewed before any
external grading. The scripts never contact a judge or send raw traces.

Enter all five 0–2 scores, a grader identifier, `human` or `llm`, an evidence-based rationale in
`notes`, and explicit `true`/`false` decisions for unsafe recommendations, unmarked invented numbers
and missing capture. Blank rows stay ungraded. Human and advisory LLM scores remain separate.
LLM rows must also identify the requested `grader_model` and `grader_reasoning_effort`; this records
an already authorized judgment and never starts one. The offline test's explicitly marked
`synthetic-test-model-c` is a fixture, not a Claude/Codex invocation or reference result.
The summary refuses changed packets and duplicate or incomplete scores. It reports missing C/D/E
references rather than creating substitutes. Equivalent B/C ratios use only complete real runs
with the same question, sharing and registry, established ground truth and no capture gap. The
submission target remains unassessed below 17 paired real scenarios; critical failures always
prevent claiming it even when the mean score ratio is high.
Every real campaign situation and prepared B/C answer must also be graded and complete; an
unanswered model run, ungraded repeat or omitted scenario prevents a submission-target claim.

## Validation log — 2026-10-02

The implementation supplies five synthetic situations, portable command tests, recorder/replay
round-trip and privacy tests, MCP protocol tests and a scripted model run through the actual C1
controller (including cost and persistent-number verification). No paid model comparison, Claude
Desktop session or human score is implied by these tests. Scored real campaign results are still pending.

## Local evidence — 2026-10-03

Existing raw files outside the repository include a live capture with 96 calls and 23 registered
tools, its complete anonymized offline preview, and an earlier live capture/replay with 57 calls
and 12 tools. The newer capture is reusable as a frozen local situation. This audit establishes
neither ground truth nor reference/human scores from those files. No raw file was added to this
repository.

Real Super and Ultra attempts against the same 96-call scenario reported missing `get_processes`
argument variants. Their exact union contains five variants: limits 5 and 10 with omitted sort,
limit 5 with `private_commit`, and limit 2 with `cpu` or `working_set`. These attempts identify a
capture gap and cannot establish relative quality or a model default.

An independent real Codex `gpt-6.1-sol`, low reference then used the same capture through MCP
and returned a final answer. Its `record_activity` call with `{}` had no exact captured match,
so the trace is incomplete and excluded from scores, B/C ratios and model-default decisions.
The earlier MCP `tools/list` response also omitted the replay's captured-argument descriptions.
Its tokens and bill remain unknown; no human score or established ground truth was supplied.

MCP `tools/list` now obtains descriptions and closed argument schemas from the replay toolbox's
definitions. A subprocess regression checks that captured-argument descriptions reach the
client. Calls still require exact matching, including omitted versus explicit defaults;
missing combinations remain unsupported. The complete independent Sol v4 reference and its
paired blind packets are now available locally (see the final status below). References D/E,
human grading and an established ground truth remain pending.

The offline grading-script test passed with the centrally built evaluator: provenance,
actual stdio MCP tool evidence, blind answer identities, missing references, null costs,
blank human scores and synthetic/critical-failure exclusions were checked. Its two fake
answers are explicitly harness fixtures, not model or reference results. A fresh anonymized
offline replay of the existing live scenario served all 96 recorded calls with zero misses
and `replay_complete = true`; no collector or provider was contacted. Reports remain on D:
outside Git. This validates replay and grading mechanics, not diagnostic quality.

## Final local evidence status — 2026-10-04

An independent Codex `gpt-6.1-sol`, reasoning effort `low`, produced the valid configuration C
reference v4 through replay MCP. It used the same frozen real-origin, anonymized 96-call capture,
question, tool registry and evaluator v2 binary as the actual Ultra v2 run. The MCP trace has
zero replay misses; the client sidecar records all 27 pages served, zero pending pages, an explicit
finish and child exit 0. These checks establish complete replay and transport delivery; they do
not establish diagnostic quality or prove attention to every delivered page. The first, second
and third Sol attempts remain retained as incomplete or truncated attempts and are excluded from
quality comparisons. The valid v4 reference supersedes them.

The local pairing is identified by scenario SHA-256
`fd88d433400c30f9bae713d5f1c89741f8d3b44c834afe2da577c2c5bed55240`, registry SHA-256
`5c861d0357dd93d3bb4e6b840bbdd3a5edf26cc570b97ce976bbd5fbf1c53038` and frozen binary SHA-256
`9e46881148f909b8ef146651a4490cd22a456dde6af61d31d3da5c2e9db624b1`. The frozen binary's build
revision was not recorded and remains unknown; the current checkout is not asserted to match it.
The evidence bundle under `D:\pcagent-tmp\backend-20261003\grading-sol-v4-20261004-a0e5c36e`
retains copied inputs, the exact reference and model results, assembly provenance, frozen grading
scripts and a local reconstruction command in its README. `delivery-audit-final.json` and
`repro/validated-reproduction-final.json` confirm identical reconstructed reference and answer
packet hashes, without new provider calls, collection or compilation. The original
`reference-sol61-low-v4-mcp.json.client.json` retains the client delivery check. Raw traces and
packets remain outside Git.

Ultra v2 completed on this same capture with a final answer and seven verifier issues. Those
issues include actual conversion faults and conservative flags; their count is not a fidelity
score, and each passage still needs review against its own cited observation. Super v2 produced
no final answer after the frozen build's old 90-second request timeout. Its failed result is
retained in campaign accounting rather than omitted. That historical timeout is not a measurement
of the current harness: the current source includes already merged PR #32's 600-second turn and
300-second request budgets. This pairing does not measure those newer budgets.

The two prepared blind packets contain the actual Sol v4 and Ultra v2 answers, their actual tool
requests, answer-scoped observed evidence and the available capture. Human score rows remain
blank. The separately attempted advisory blind LLM review also remains ungraded because complete
relevant evidence did not reach the judge context; null scores and failure flags record an
incomplete review, not absent original measurements or a passed safety/fidelity check. No local
ground truth or confirmed cause has been established. Sol usage, bill and duration remain unknown.
This single situation supplies no quality score or B/C ratio, no submission-target result, no
model-default or escalation decision, and no evidence for starting Tavily. References D/E and
independent human assessment remain outstanding.
