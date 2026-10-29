# Local Nemotron routing calibration

`pcagent-eval calibrate-nemotron` validates an existing, explicitly human-reviewed
campaign locally. It makes no provider request, starts no collector, reads no credential
and changes no routing setting. The API-key setup and external model comparisons are
outside this command.

The command writes a `pcagent_agent::routing::Calibration` only when its existing
`rules()` accepts the campaign. It also writes a separate provenance document containing
the checked runtime identity, hashes, sharing mode, request counts, measured durations
and checked costs. The Calibration and provenance schemas are version 2; the independent
human annotation manifest remains version 1.
Neither output includes questions, answers, rationale, raw measurements or file paths.
The input annotation manifest and original traces remain private local evidence.

```powershell
& $eval calibrate-nemotron --annotations D:\pcagent-tmp\nemotron\annotations.json `
  --out D:\pcagent-tmp\nemotron\calibration.json `
  --provenance-out D:\pcagent-tmp\nemotron\calibration-provenance.json
```

Both destinations must be new files outside Git; their directories must already exist.
All inputs are validated before either destination is reserved. If provenance output
fails, no calibration is written. Retain both outputs with the reviewed source artifacts.
A failure after an output write can leave that local diagnostic artifact in place.

## Required paired evidence

Each case names one frozen real-PC `Scenario` and actual Super/Ultra result JSON files
from `replay --allow-cloud --out`. Offline replay, preview, MCP-only traces and paid runs
of synthetic scenarios are rejected. A declared live origin is metadata, not hardware
attestation: this command checks the artifacts and their consistency, and cannot detect
someone relabeling fabricated measurements as live. Unit fixtures are explicitly
synthetic and never supplied as a calibration campaign.

The importer checks the scenario's parsed/compact SHA-256, real origin, capture time and
structured evidence origins. Both runs must match that scenario, replay tool registry
SHA-256, exact question, sharing mode and current harness version. The question hash is
SHA-256 of its exact UTF-8 bytes; it is not normalized. The two trace hashes bind their
exact file bytes, including whitespace, to the human review. Changing either result
requires another review and updated hashes.

New investigations record `run.runtime` before execution; new replay envelopes record
the same identity as `runtime`. Both objects are closed and must equal
`CalibrationRuntime::current()` when compiling the campaign. Their fields are
`registered_tool_registry_sha256`, `analyst_sha256` and `harness_version`.
The global registry identity covers registered tool definitions and their classes;
`tool_registry_sha256` separately identifies the frozen replay's tool subset and
captured-argument descriptions. These two hashes are not interchangeable.

The analyst identity binds the prompt, schemas, controller, verifier, context behavior
and budgets conservatively through the relevant source files, with LF normalization.
The harness identity is `pcagent-eval-replay-v2`, which must also match both existing
`harness_version` fields. Missing identities, unknown fields, differing identities and
old harnesses are rejected even when Super and Ultra agree with each other. Older runs
remain readable as recordings but are never stamped with the current identity on import;
obtain new executions and human reviews to calibrate a changed engine. The emitted
Calibration and provenance retain this checked runtime identity, and CLI/GUI routing
loaders refuse a Calibration whose identity is absent or no longer current.

Every recorded tool call is validated and replayed locally against the frozen capture.
The full recorded output must match, with no missed call. Served, unused and reused
capture counters must match the replay. Repeated recorded outputs remain repetitions,
never additional hardware measurements. Missing call timestamps/durations, mixed models,
unknown inference usage/cost or budget extensions are rejected. No collector is involved
in these checks.

Inference counts must match the per-request records. Prompt/completion/reasoning totals
use checked arithmetic. Each known cost is recomputed from measured usage and the
matching model's dated Nebius catalog price; request costs must sum to the reported total.
Traces without the newer `model_requests` fields can qualify only when they already carry
the required current execution identity and their full measured token usage and dated
aggregate price are checkable. This does not permit importing old runs without runtime
metadata or assigning them today's identity.
The cost is an estimate from measured tokens and published rates, **not an invoice**.
The duration retained in each case is `run.duration_ms`, measured during investigation,
not replay setup time or an estimate from token count.

## Explicit human annotations

The manifest schema and every review are closed. No label, critical-error flag or rationale
has a default. `source` must be `human`, `human_reviewed` must be true, and the review
timestamp must follow both runs. A nonempty rationale is required for every pair.

`CalibrationCase` currently uses human quality labels **0 through 4**. The manifest must
explicitly state `quality_scale: "routing_human_0_to_4"` and supply both labels. The
compiler does not convert the evaluation rubric's five 0–2 scores or its total out of 10,
infer a label from confidence/verifier flags, generate a score or invoke a judge. A human
campaign reviewer must define and use a consistent interpretation of this separate scale.
Those labels are retained exactly.

The trigger must appear in the primary trace: low confidence (with insufficient-evidence
precedence matching the controller), the insufficient-evidence category, or exhausted
verification repairs with flagged passages. `primary_request_limit` is a human-reviewed
candidate threshold based on the actual primary request count; it is not a recorded
escalation. Cases and traces must be independent and unique. The current routing contract
requires at least three live captures and two human-rated recoveries for a supported
trigger, no quality regression, no critical Ultra error and an explicit extra-cost cap.
Campaigns that support no rule produce no Calibration.

Example manifest shape; all placeholders must be replaced with reviewed real artifacts,
dates, hashes and human decisions. One pair is shown for schema illustration; it cannot
satisfy the three-capture minimum.

```json
{
  "schema_version": 1,
  "super_model_id": "exact NVIDIA Nemotron Super model ID from the runs",
  "ultra_model_id": "exact NVIDIA Nemotron Ultra model ID from the runs",
  "reviewed_at_unix_ms": 0,
  "maximum_extra_cost_usd": "reviewer's explicit decimal cap",
  "pairs": [
    {
      "scenario": "capture.json",
      "primary_result": "super-result.json",
      "ultra_result": "ultra-result.json",
      "scenario_sha256": "64 hexadecimal characters from the checked scenario",
      "tool_registry_sha256": "64 hexadecimal characters from both results",
      "question_sha256": "SHA-256 of the exact question UTF-8 bytes",
      "sharing": "anonymized",
      "primary_trace_sha256": "SHA-256 of the exact Super result file bytes",
      "ultra_trace_sha256": "SHA-256 of the exact Ultra result file bytes",
      "review": {
        "source": "human",
        "quality_scale": "routing_human_0_to_4",
        "human_reviewed": false,
        "trigger": "low_confidence",
        "primary_quality": 0,
        "ultra_quality": 0,
        "primary_critical_error": false,
        "ultra_critical_error": false,
        "rationale": "replace with the human review of this exact pair"
      }
    }
  ]
}
```

Relative input paths resolve beside the manifest. Its maximum size is 256 KiB; scenario
and trace files retain the evaluator's 32 MiB limit. The serialized Calibration, including
indentation and its final newline, must fit the routing loader's 64 KiB limit. The
provenance `calibration_sha256` hashes the parsed Calibration serialized as compact JSON;
its trace and annotation hashes identify exact source file bytes.

## Limits of the routing evidence

The paired artifacts measure **independent** single-model investigations of the same
frozen evidence. They do not measure Ultra continuing from Super's messages, remaining
budget and provisional answer. The compiler retains that evidence basis in provenance;
an accepted campaign does not establish continuation quality, the hackathon's B/C target
or a general model ranking. There is no default model choice or automatic activation.

`Calibration` carries the shared current runtime identity. Its individual cases lack
replay-registry/question/sharing fields and Ultra request count. The compiler checks
those inputs before constructing each case; the separate provenance retains them and
is bound to the Calibration hash. Keep that provenance as part of the complete campaign
record.

## Measuring automatic continuation after review

Once a real paired campaign passes human review and compiles, run the automatic policy
against the same frozen captures. This measures the continuation that independent
Super/Ultra pairs cannot establish:

```powershell
& $eval replay D:\pcagent-tmp\nemotron\capture.json --allow-cloud --anonymize `
  --calibration D:\pcagent-tmp\nemotron\calibration.json `
  --super-model nvidia/nemotron-3-super-120b-a12b `
  --ultra-model nvidia/Nemotron-3-Ultra-550b-a55b `
  --out D:\pcagent-tmp\nemotron\automatic-result.json
```

The bounded calibration file is validated before credentials are read or an output file
is reserved. Missing human review, stale runtime identities and unsupported thresholds
remain errors. `--model` cannot be combined with automatic replay. No default is changed.
`run.model` names the initial Super model; the report's routing state, model requests,
handover reason and separate model costs describe any continuation. The original replay
capture is unchanged; actual calls, misses and reuse counters retain their normal meaning.
