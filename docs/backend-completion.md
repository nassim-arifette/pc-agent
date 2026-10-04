# Backend completion, 4 October 2026

This lot extends the backend already merged in #33. The separate C3 contract changes are in
#34 and #35. It includes Rust collectors, the analyst controller, CLI/evaluator and the desktop
commands; it does not change frontend files, API-key handling or external model comparisons.

## Delivered behavior

- Immutable selection of a resource, process, event, recording or authorized scan item. Explain
  exposes only that evidence; Investigate retains the normal read-only toolbox. Exact C5 previews
  bind the question, conversation, sharing, model policy and source to an expiring single-use ticket.
  Changing policy, removing context, clearing history or replacing a scan invalidates the relevant
  tickets. Unknown measurements remain unknown and captured measurements retain their dates.
- Local model routing: configured model, explicit Super, explicit Ultra, or calibrated automatic
  routing. An optional single escalation shares the existing tool/request/time and repair budgets,
  retains observations/citations, honors cancellation and accounts for each model's usage/cost.
  Failed or unpriced requests make aggregate cost unknown. The configured default is preserved.
- A local Nemotron calibration compiler: original real run artifacts, human annotations, trace
  and registry hashes, costs and failures are checked before producing bounded calibration and
  provenance files. Original runtime identities are required by the compiler and CLI/GUI loaders.
  Old traces are readable for evaluation but cannot be relabeled to enable automatic routing.
- Bounded native MFT metadata inventory, including a local top-directory/file tree and explicit
  unattributed bytes, hard links, stream limits, change witness and allocation uncertainty.
  C4 uses opaque references; filenames and paths in the helper tree remain local.
- Closed helper inspections for VSS shadow storage, disk reliability counters, protected process
  image identities and historical Windows E3 energy estimates. Energy is historical attribution
  of accepted provider rows, with civil-time and coverage limits; it is not physical per-app power
  or exhaustive attribution. The battery health/recording work remains documented in [battery.md](battery.md).
- Fixed 30/60-second ETW DTOs, schema parsing and cleanup supervision are implemented. Native
  activation stays `unsupported` before UAC/session/ACL effects until hard-termination cleanup is
  proven. [etw.md](etw.md) records this remaining requirement.
- Private helper cleanup does not report completion while an export is locked. Prepared-job
  cancellation, approval errors and normal GUI shutdown keep admission closed until actual
  cleanup. CLI Ctrl+C is relayed to the helper and keeps the host alive during cleanup.

Previously approved helper results are available to the analyst through the closed read-only
`get_approved_elevated_inspection` tool. This tool cannot launch UAC or grant approval. Its bounded
cache retains the original measurement date and expires after five minutes.

## Frontend connections remaining

Use the existing conversation events with `selection_sources`, `selection_preview` and
`selection_send`. Display the exact preview and pass its matching ticket; see [selection.md](selection.md).
Expose the routing commands and additive model-change/usage metadata. Keep automatic availability
and rejected-calibration limitations visible.

The existing elevation prepare/run/cancel commands accept the additional fixed tasks. Display
their C4 coverage and the local MFT tree without putting its paths into a model message. Tree
directories followed by files correspond in order to C4 `ntfs_item_category` entities; a fixture
checks this association. The completed tree is returned locally, not persisted by the analyst cache.
ETW cannot be presented as available while the native cleanup gate is closed.

The battery view can combine health and recorded evolution with separately approved historical
E3 app estimates. Neither CPU activity nor a missing energy sample proves physical battery usage.
The startup, TEMP/shader and Settings actions delivered in #33 retain their local review flow.

## Acceptance limits

Synthetic fixtures validate controller behavior, bounds, parsing, privacy, cancellation and
cleanup ownership. They are not model-quality or hardware acceptance results. No new paid
campaign, human quality labels, actual E3 export, raw-volume UAC scan, protected-process probe,
ETW session, driver installation or modification of user startup/cache files is claimed here.

Remaining acceptance work is a human-reviewed real Nemotron campaign, hybrid continuation quality,
native UAC/helper checks on controlled targets and a second PC, active LHM/HWiNFO providers,
battery discharge/runtime behavior, E3 provider acceptance and the ETW hard-exit cleanup proof.
External research remains conditional on a separately adopted C6; #31 is not adopted by this lot.

## Validation

Local validation on 4–5 October 2026 uses Rust 1.90.0, locked dependencies and one Cargo operation
at a time. Windows builds and temporary files are on D:; WSL uses a D: build/cache directory and
Linux temporary files for filesystem fixtures.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed on Windows and Linux. |
| `cargo clippy --workspace --all-targets --locked -j 1 -- -D warnings` | Passed on Windows and Linux. |
| `cargo test --workspace --locked -j 1`, Windows | 671 passed, 0 failed, 1 ignored. |
| `cargo test --workspace --locked -j 1`, Linux/WSL | 605 passed, 0 failed, 1 ignored. |

The ignored test requires an explicitly authorized live cloud request. These counts include
command, controller, synthetic parser, native-file lifecycle and documentation suites; they do
not imply the outstanding hardware or quality acceptance listed above.

The required Windows CI job also builds the CLI, desktop app, evaluator and isolated helper with:

```powershell
cargo build --release --locked -j 1 -p pcagent -p pcagent-gui -p pcagent-eval -p pcagent-elevated
```

Read-only native smoke checks use `inspect get_power_and_thermal --anonymize`, the missing approved
helper-result path and fixed 30/60-second `elevated-inspect performance-trace` requests. The latter
must return `unsupported`, `cleanup_unvalidated` and `trace_session_started: false` before UAC.
They do not substitute for the native acceptance listed above.
