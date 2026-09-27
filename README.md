# PC Agent

A read-only, CLI-first Windows investigation agent. Initial implementation based on **PC Agent specification v0.1 (2026-09-25), Lot A**. This is an early prototype, not a finished optimizer or a validated hackathon submission.

## Implemented in this first slice

| Command | Behavior |
| --- | --- |
| `pcagent snapshot` | System physical memory, system commit, one-second aggregate CPU and top readable process memory contributors. |
| `pcagent processes` | Local process names, PIDs, creation times, working set and private commit; sorted by readable working set. |
| `pcagent doctor` | Offline platform/configuration checks. `--check-cloud` explicitly checks the configured model in the Nebius catalog without sending telemetry. |
| `pcagent diagnose "question"` | Local deterministic observations by default. Does not call a model without `--allow-cloud`. |
| `pcagent diagnose "question" --preview-cloud` | Shows the initial minimized data envelope locally. Sends nothing. Later tool samples may differ. |
| `pcagent storage <folder> [--dirs] [--limit N]` | Metadata-only scan of one explicitly given folder: largest files or directories, coverage and limitations. Never reads contents or follows links. |
| `pcagent diagnose "question" --allow-cloud --verbose` | Bounded Nebius/Nemotron tool loop with local validation and traces of completed inspections. Requires an explicit model ID and API key. |

All commands support `--json`. Output marks missing measurements explicitly rather than substituting zero. Non-Windows builds are useful for portable tests but do **not** collect Linux/macOS metrics as though they were Windows measurements.

There is no file deletion, application termination, elevation, startup modification, document-content inspection, background collection or automatic cloud request. Ctrl+C terminates the process; there are no detached workers or maintenance operations to continue afterward.

## Build and run on Windows

Use an x64 Windows development environment with Rust/rustup and the MSVC C++ build tools. The repository pins Rust 1.90.0; Cargo.lock is generated and committed by the initial CI preparation job. Subsequent builds use that lockfile.

```powershell
cargo build --release --locked -p pcagent
.\target\release\pcagent.exe doctor
.\target\release\pcagent.exe snapshot
.\target\release\pcagent.exe processes --limit 10
.\target\release\pcagent.exe diagnose "Pourquoi mon PC rame ?" --offline
.\target\release\pcagent.exe snapshot --json
```

The CI also builds a `pcagent-windows-x64` executable artifact when its Windows job succeeds. It is an unsigned prototype, not an installer or a production release.

Exit codes: `0` = command completed, `2` = invalid input/configuration or command error, `3` = partial investigation or unavailable collection. A completed investigation may legitimately conclude that evidence is insufficient; success is not a claim that the machine is healthy.

## Opt-in cloud investigation

No model name is assumed to exist. Obtain an actual NVIDIA Nemotron ID from your Nebius account's `/v1/models` catalog, then set `NEBIUS_MODEL_ID` exactly. The client checks catalog membership and refuses non-NVIDIA/Nemotron IDs; it never silently switches providers or models.

```powershell
# Interactive input keeps the key out of the command history.
$secret = Read-Host "Nebius API key" -AsSecureString
$env:NEBIUS_API_KEY = [System.Net.NetworkCredential]::new("", $secret).Password
$env:NEBIUS_MODEL_ID = Read-Host "Exact NVIDIA Nemotron model ID from your account catalog"

.\target\release\pcagent.exe doctor --check-cloud
.\target\release\pcagent.exe diagnose "Pourquoi mon PC rame ?" --preview-cloud
.\target\release\pcagent.exe diagnose "Pourquoi mon PC rame ?" --allow-cloud --verbose

Remove-Item Env:NEBIUS_API_KEY
```

`--allow-cloud` authorizes this invocation to send **your question verbatim**, selected numeric metrics, their time windows, source labels, coverage, limitations, opaque process references and candidate findings to Nebius. Do not put a secret in the question. Process names, PIDs, process creation times, boot identifiers, full paths and collector session IDs are excluded from the cloud envelope. This is data minimization, not a claim of anonymity or provider non-retention.

The only endpoint is `https://api.tokenfactory.nebius.com/v1`; HTTPS is required and redirects are disabled. The key stays in the process environment and client memory. `.env` is not automatically loaded and no credential vault has been implemented yet. Never paste a real key into a repository, issue, terminal recording or chat.

## How the first agent works

The local program gathers an overview. The model can request read-only tools from the `Workstation` toolbox (`crates/agent/src/workstation.rs`): `get_system_snapshot`, `get_processes` (`limit` 1-10), `get_process_details` (opaque `entity_ref`), `get_process_tree`, `get_metric_history`, `get_hardware_inventory`, `list_storage_scopes`, `get_storage_overview`, `start_storage_scan`, `get_largest_items`, `get_cleanup_candidates` and `get_user_preferences`. Storage tools only accept opaque scope IDs for folders the user authorized locally with `diagnose --scan-root <PATH>`; each cloud payload is an explicit allowlist (no PIDs, executable names, scope roots, full paths or boot IDs). With `diagnose --allow-cloud --propose-actions`, the model also gets `prepare_action_plan`: it may propose opening an item in Explorer, opening an allowlisted Settings page (`storagesense`, `appsfeatures`, `startupapps`) or running the synthetic cleanup adapter (at most 3 plans). It only receives the plan's model view; after the report, the CLI shows each plan locally with its real target and runs it only if you type `yes` on an interactive terminal (never with `--json`). Execution and approval are never model tools. Moving files to the Recycle Bin stays a recommendation. `pcagent demo-cache --mb 64` creates the marked synthetic cache under `%LOCALAPPDATA%\\pcagent` to demonstrate the adapter. Tools are declared in a local registry (`ToolSpec`/`Toolbox`) with closed argument schemas; unknown keys, wrong types, out-of-range integers and non-opaque references are rejected before any dispatch. The controller executes real collectors, returns minimized observations and records completed tool calls.

The final model response is deliberately restricted to `{"finding_ids":["primary ID", up to two alternative IDs]}`, all distinct existing candidates. The controller resolves those IDs to locally computed facts and evidence references. It rejects fabricated IDs and extra free-form claims. This first version **selects a diagnostic focus rather than generating unrestricted medical-style or causal explanations of a PC**; general natural-language synthesis with stronger claim validation remains future work.

Each finding follows the spec 8.3 structure: support level (`supported`/`plausible`/`insufficient`, never `supported` from one sample), locally generated facts citing evidence IDs, an optional hypothesis, counter-evidence, missing data, next step and limitations. "No contention observed" is a valid conclusion when all core metrics are measured and moderate. `pcagent_diagnostics::compare` implements the spec 18 before/after verdict (`improved`, `no_net_change`, `regressed`, `not_comparable`, `needs_more_measurement`) and only reports a change when the full before and after ranges do not overlap.

Initial local observation thresholds are below 10% physical memory available, at least 90% of the current commit limit, and at least 90% aggregate CPU in the sample. These are prototype triggers for further investigation, not universal diagnostic rules or validated anomaly thresholds. System commit is not physical RAM occupancy, working sets may share pages, and a short sample cannot establish a memory leak or justify a hardware purchase.

Until at least one tool has run, requests set `tool_choice: "required"`: the initial overview already carries candidate findings, and without it the model tends to select one immediately. A reply that still concludes before any tool call is rejected. The loop is bounded to six inference turns, twelve tool calls and a 90-second investigation budget checked between operations. HTTP calls have a maximum 30-second budget including initial catalog validation, and response bodies are capped at 256 KiB. A synchronous native collection may overrun a deadline before control returns. Errors, malformed replies or exhausted budgets produce an explicitly partial report with the observations retained, not a fabricated success.

## Workspace

```text
apps/cli/             argument parsing and rendering
crates/shared/        versioned observations, units, statuses and process identities
crates/telemetry/     read-only Windows adapters; unsupported results elsewhere
crates/diagnostics/   conservative deterministic candidate findings
crates/agent/         minimized cloud envelope, tool registry, bounded orchestration, Nebius transport
crates/storage/       bounded metadata-only storage scanner (docs/storage.md)
crates/policy/        protected paths, preferences, cleanup classification and plans (docs/policy.md)
crates/actions/       manifests, local single-use approval, restricted action catalog (docs/actions.md)
```

The CLI has no cleanup or diagnostic business rules. The crates are intended to be reusable by a future Tauri interface. Unsafe code is isolated to the native Windows adapter; all other crates forbid it.

## Validation

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The GitHub workflow formats Rust and commits the initial dependency lockfile in a preparation job, then tests that exact revision on Windows and Linux. The build/test jobs have read-only repository permissions, use no provider credentials and do not upload live telemetry. This initial source-normalization write can be removed once the bootstrap is complete.

The tests cover missing/stale data, large integers, CPU counter regressions, commit-versus-RAM semantics, identity stability, cloud redaction, unknown tool rejection, duplicate call IDs, fabricated findings, resource budgets, provider failures and offline CLI behavior. Scripted provider replies are synthetic test fixtures, not evidence of a live Nemotron integration.

**Not yet validated:** real Nebius/Nemotron tool calling and latency, a representative investigation on a separate Windows 11 PC, diagnostic quality, hardware advice and performance/resource budgets. A hosted Windows CI smoke test is not a substitute for those validations. Check the Actions results for the actual status of compilation/tests; this README does not imply that a pending run has passed.

## Known gaps versus the specification

Action plans are proposed by the model and approved in the CLI, but the before/after re-measurement of an executed action is not yet automatic, and Recycle Bin moves are not executable. Disk latency/idle/queue, hard-fault page reads, per-core CPU and GPU engine utilization come from `_Total`/aggregated PDH counters only (no per-device attribution, no GPU memory, no per-process GPU/disk attribution). No persistent history, NTFS MFT backend, Tauri UI, installer or Tavily integration is implemented yet. `doctor` does not yet validate the Windows build number or storage/database capabilities. Process identities use session-local aliases plus PID and creation time; a true boot identifier is explicitly absent and cross-boot correlation is disabled. Large (>64 logical processor) systems report aggregate CPU as unsupported rather than misreporting one processor group as the whole machine.

JSON byte counts remain exact 64-bit integers in Rust. Future JavaScript/IPC consumers must preserve them as BigInt or a lossless representation; a JavaScript Number is not safe for every value.

See [architecture and safety](docs/architecture.md) and [validation checklist](docs/validation.md).

## License

No open-source license has been selected yet. Keep this repository private until license and submission requirements have been reviewed.
