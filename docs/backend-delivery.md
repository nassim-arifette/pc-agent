# Backend delivery — 2026-10-04

This delivery was developed and validated in the isolated `wp13-backend-integration` worktree,
based on main `60b09b8`. It preserves existing application contracts and changes no frontend files.
Repository integration requires successful Windows and Linux CI. The separate plan/C6 proposal
in PR #31 is excluded; the preserved local plan draft is not adopted by this backend delivery.

## Delivered backend

| Workstream | Delivered implementation | Details |
| --- | --- | --- |
| 7 — Evaluation | Reused capture/replay/MCP harness, reference import, frozen reproducible blind packets and retained failure accounting. A complete independent Codex Sol 6.1 low v4 reference shares the actual Ultra v2 capture, registry and binary. | [Evaluation](eval.md) |
| 4, lot 3 — Local search keys | RAM/SMBIOS inventory, structured event details, safeguard indicators, installed drivers and explicitly requested Windows Update offers, with bounded collection and unknown-value/privacy handling. | [System inspection](system-inspection.md) |
| 13 — Context | Readable measurements, references for exactly repeated results and older-context metadata compaction, preserving original verifier evidence, citations and freshness. The offline capture benchmark measures bytes only. | [Tool context](tool-context.md) |
| 10 — Models | NVIDIA account catalogue, dated prices and capabilities, nullable limits, model configuration and CLI/Rust GUI catalogue access. The default remains unchanged pending evaluation. | [Model catalogue](model-catalog.md) |
| 8 — Elevated inspection | Separate closed helper without network or LLM, secure local file protocol, approval binding, cache metadata inventory, cancellation and persistent supervision through pending consent/unconfirmed exit. | [Elevation](elevation.md) |
| 8 — MFT preparation | Portable bounded offline FILE-record parser and synthetic allocation/deduplication fixtures; no device access or model tool. Production MFT remains unsupported. | [MFT preparation](mft.md) |
| 11 — Actions | Current-user startup disable and journal-bound restoration reachable through CLI and Rust GUI preparation; finite TEMP/shader recycle manifests, fixed Settings pages, local approval, revalidation and durable journals. | [Actions](actions.md) |
| 9 — Sensors | Passive bounded readers for compatible already-running LibreHardwareMonitor WMI and HWiNFO shared-memory publishers, with explicit age/absence and opaque projections. | [Sensors](sensors.md) |
| 4/5/11 — Battery | Separate per-battery capacities, capacity-loss estimates, state/rates/runtime, persisted recording evolution and gaps, dated CPU/GPU/I/O recording evidence, Rust retrieval command and locally approved Windows usage-page action. | [Battery](battery.md) |

## Frontend handoff

Existing frontend commands/events remain available. These additions require frontend wiring;
none grants permission to execute an action merely because a model proposed it.

- Catalogue: call `cloud_models`, display unavailable metadata clearly, and save the selected model
  through the existing settings path. Credentials remain in Rust; see [model-catalog.md](model-catalog.md).
- Elevation: display the exact task/budget from `elevation_prepare`; call `elevation_run` only after
  local approval. Retain/recover the request ID with `elevation_status`, render running/cancelling
  state and use `elevation_cancel`. A bounded operation error can precede actual cleanup; wait
  until `elevation_status` returns `null` before offering another preparation; see
  [elevation.md](elevation.md#gui-and-cli-bindings).
- Startup recovery: `startup_restore_prepare({sourcePlanId, itemIndex})` returns a local
  `{turn_id, plan}` for review through existing `action_plans`, `action_run` and decline commands.
  This `turn_id` identifies an action group, not a conversation turn. Preparation does not execute
  restoration; see [actions.md](actions.md#current-user-startup-approval-and-restoration).
- Battery: show the health/state fields in `get_power_and_thermal`; retrieve
  `battery_recording({recordingId})` during/after a recording and align its timestamps/gaps with
  existing resource evidence. The fixed `batterysaver-usagedetails` Settings action still requires
  local approval. Application activity is correlation, not measured per-application energy; see
  [battery.md](battery.md#recording-and-attribution-limits).
- Other local-inspection additions use the existing observation/citation path. Present partial,
  denied and unsupported coverage without converting unknown values to zero; see
  [system-inspection.md](system-inspection.md) and [backend-phase1.md](backend-phase1.md).

## Limits and conditional work

Native MFT collection is not implemented. Offline parsing does not resolve full attribute-list
membership or prove compressed-layout/native allocation accuracy; metadata and additional-stream
coverage are incomplete. Dedicated-volume acceptance is required before usable MFT totals. Classic
cache enumeration does not satisfy this requirement. Restore-point sizes remain unsupported and
Delivery Optimization coverage is partial.

Real UAC acceptance and cross-machine helper installation remain unverified. Sensor absence only
validates absence handling; actual supplier readings need an existing compatible publisher. An
absent/unreadable battery cannot validate real capacity/rate/runtime readings. Native startup and
some effect acceptance remain limited to controlled fixtures; no user startup/cache mutation is
required to demonstrate this delivery.

The real quality campaign remains ungraded by humans; the attempted advisory review is also
ungraded because complete evidence did not reach its context. No ground truth, quality ratio or
submission-target result is established. The Super timeout belongs to the frozen historical
budget, not current-harness performance. Context-byte reductions establish neither token/cost
savings nor equal diagnostic quality.

WP14 automatic Super→Ultra escalation remains conditional on evaluated triggers and economics.
WP15 remains conditional on adopted C6 and evaluated evidence of missing recent information;
there is no Tavily implementation or consent shortcut in this delivery. See [the plan](plan.md).

## Validation status

Observed on the final backend code, including the scoped Linux-only lint annotation:

| Check | Status |
| --- | --- |
| Rustfmt | PASS |
| Windows workspace Clippy with warnings denied | PASS after Linux-only annotation |
| Python paged MCP client tests | PASS — three tests |
| Windows workspace tests | PASS after Linux-only annotation — 559 passed, one live credential/network test ignored |
| Windows release builds, including new binaries | PASS — CLI, GUI, evaluator and elevated helper |
| Linux Rustfmt and workspace Clippy with warnings denied | PASS |
| Linux workspace tests | PASS — 510 passed, one live credential/network test ignored |
| Linux release/stub validation | PASS — all four binaries; CLI, GUI and helper expose unsupported Windows coverage; offline projection identities and byte metrics match Windows |
| Offline evaluator grading/import/reproduction checks | PASS — synthetic harness validation, not model quality |
| Same-capture offline context benchmark | PASS — 96 calls; about 24% fewer context bytes |
| Fresh authorized non-admin Windows lot-3, catalogue and battery probes | PASS — explicit partial coverage; see [battery.md](battery.md#native-validation--2026-10-04) |

The local check record and its limits are detailed in [validation.md](validation.md).
Source/binary hash manifests and private captures remain on D:. Compilation/tests alone do not
establish diagnostic quality or native acceptance.
