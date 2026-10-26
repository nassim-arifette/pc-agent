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

### Local check record (4 October 2026)

This record moved here from validation.md on 26 October 2026; it describes the code of
4 October, not the current one.

The isolated `wp13-backend-integration` worktree was based on main `60b09b8`. The existing
phase-1 frontend from PR #30 was preserved; no frontend file was authored by this backend
delivery. These local checks preceded publication; repository integration required a separate
successful CI run.

Rust checks used the pinned 1.90.0 toolchain, the committed lockfile, `-j 1` and one Cargo
operation at a time. Targets and Windows temporary artifacts stayed on D:. Test/dev debug info
was disabled and codegen units set to one to fit the development PC's memory. WSL tests used a
Linux filesystem for fixtures; bulky targets stayed on D:. The Windows release directory was
`D:\pcagent-target\backend-final-20261004`, separate from the already-running older GUI
executable. No existing target or running GUI was removed.

On Linux, the release stubs behaved as intended: read-only Windows inspections returned
`unsupported` (exit 3), the GUI exited with code 2 and its Windows-only message, and the helper
rejected native execution as `Unsupported` (exit 1). Linux and Windows offline projections gave
the same scenario and registry hashes and byte metrics.

The ignored test requires live credentials/network and was not enabled. Linux Clippy first
identified fields used only by native Windows worker execution. A scoped non-Windows dead-code
annotation preserves portable argument parsing and the honest unsupported stub; Linux checks and
Windows Clippy/tests subsequently passed. This is a local validation record, not a published CI
run.

Authorized Windows inspections of the final release ran after compilation stopped, without
admin, and stored anonymized outputs in a separate private D: directory. Power/thermal,
inventory, events, system state and drivers all returned explicit partial coverage with 47, 44,
116, 43 and 325 observations respectively. There were 64 installed-driver entries; no online
Windows Update search was performed. The account catalogue GET succeeded with four NVIDIA models
and sent no PC observations. Battery capacities/state and the 29-point, 30-second persisted
recording and the earlier build-time probe are detailed in
[battery.md](battery.md#native-validation--2026-10-04). No user startup entry or cache was
mutated, Settings page opened, driver installed or real UAC request launched. Fixtures exercise
action approval/revalidation, journal-bound startup reversal, bounded cleanup, helper
supervision, battery discontinuities and fifteen offline MFT parser cases.

Raw captures, recording database, compiler logs, grading packets and source/binary hash
manifests remain outside Git under `D:\pcagent-tmp\backend-20261003`. At that date, native
sensor-supplier readings, recorded discharge/runtime and per-process GPU acceptance remained
unverified on this PC; native MFT collection was not implemented; dedicated-volume accuracy, real
UAC/install acceptance and some native action effects remained outstanding. Real answers had no
human quality scores, and the advisory judge remained ungraded. Byte reductions establish neither
token/cost savings nor preserved diagnostic quality. Compilation/tests alone do not establish
diagnostic quality or native acceptance.
