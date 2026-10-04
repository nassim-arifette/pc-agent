# Validation checklist

## Automated (see the exact CI run and revision)

- Format, compile and lint the complete workspace with the pinned Rust toolchain.
- Run portable unit tests on Linux and Windows.
- Run Windows-native memory/process smoke and session identity tests.
- Run CLI commands without any Nebius credentials and verify no implicit cloud mode.
- Exercise synthetic model replies for tool allowlisting, complete-batch validation, duplicate IDs, forged findings/observation citations, loop budgets and provider errors. Cover extension consent/refusal/caps, preserved conversation and duplicate protection, hardware/storage/history citations, and independent execution/coverage/support states.
- Check that cloud payloads omit process names, PIDs, creation times, boot IDs and session identifiers.
- Check distinct file/folder aliases and local path restoration; default storage projections must
  omit file/directory names. Opt-in names must belong to the previewed finite allowlist; rescans
  cannot expand it. Exercise CLI previews without credentials and compare default/opt-in modes.
- Check local content matches, equal-size different content, byte/file/time/cancellation limits,
  replaced files, hard links, symlink refusal, and Windows cancellation during directory lookup.
  Compare-content must remain separate from metadata scans, cloud permission and name permission.
- Build the Windows executable and upload only the binary, not measurements or configuration.

The workflow validates pushes and pull requests with read-only permissions. It checks formatting and the committed lockfile, without pushing changes or replacing the revision under review. A failed job must not be reported as successful validation.

GitHub's [hosted Windows runner](https://docs.github.com/en/actions/reference/runners/github-hosted-runners#administrative-privileges)
is elevated with UAC disabled. The CI-only
`scripts/ci-windows-tests.ps1` compiles every workspace test executable, verifies that the real
CLI refuses the elevated runner, then runs all test executables under an ephemeral local account
in the Users group. Cargo doctests are checked separately. The account is removed even when a test
fails; its workspace and temporary permissions are confined to the disposable CI VM. Local
unelevated development retains the normal `cargo test --workspace --locked` command. No product
guard or test is skipped to accommodate CI.

## Manual native acceptance — NOT completed by CI

On a separate Windows 11 x64 test PC, without elevation:

1. Run `doctor`, `snapshot --json` and `processes --json`. Verify source, units, timestamps, observation intervals, explicit unknowns and permission coverage. Compare semantics with Windows counters, not just Task Manager column names.
2. Inspect two samples. Confirm PID plus creation-time identity, including after terminating/restarting a disposable test process outside PC Agent. Never use the old PID to authorize any action.
3. Try an offline diagnosis without environment variables. Confirm no network request and no maintenance action.
4. Inspect `diagnose "test question" --preview-cloud`. Confirm the question disclosure and absence of local process metadata.
5. Configure an actual NVIDIA Nemotron model available in the account. Use `doctor --check-cloud`, then one small opt-in diagnosis with `--allow-cloud --verbose`.
6. Verify a real provider request, at least one model-selected tool, native collection, a returned tool result, a locally validated finding, an evidence reference and honest insufficient-evidence behavior.
   The report shows tokens per request and an estimated cost with the catalog price and its read time; `scripts/compare-models.ps1` repeats typical questions per model and writes a CSV ([results of 2026-09-27](archive/model-comparison.md)).
7. Test bad credentials, unavailable model, disconnected network, malformed responses, a reply cut off at the output limit (`output_limit`, billed tokens still counted), a request timeout (`timeout`), cancellation and missing metrics. Do not put secrets or personal telemetry in issue comments, Actions logs or committed fixtures.
8. On a PC with an NVIDIA GPU, compare `snapshot` NVIDIA values (temperature, memory, clock, slowdown reasons) with `nvidia-smi --query-gpu=temperature.gpu,memory.used,memory.total,clocks.sm,clocks_event_reasons.active --format=csv`, idle and under load. `memory.used` there excludes the driver reserve (shown as Reserved by `nvidia-smi -q -d MEMORY`), while pcagent reports total minus free, reserve included: 84 MiB against 0 MiB on an idle MX350.

A live provider test incurs provider usage and must be explicitly authorized. No such test is run in CI. A successful catalog lookup is not a successful model tool-calling test.

## Next work

The work order and its acceptance criteria are in [the October plan](plan.md) (in French). This
checklist is updated as each work package changes the product. Metadata storage scans and controlled
actions are implemented with bounded scope; see [storage](storage.md) and [actions](actions.md).

## Local backend integration — 2026-10-04

The isolated `wp13-backend-integration` worktree is based on main `60b09b8`. The existing
phase-1 frontend from PR #30 is preserved; no frontend file was authored by this backend delivery.
These local checks preceded publication. Repository integration requires a separate successful CI
run. [backend-delivery.md](backend-delivery.md) records the delivered modules, frontend command
bindings and incomplete acceptance; the concurrent plan/C6 proposal remains separate.

Rust checks used the pinned 1.90.0 toolchain, the committed lockfile, `-j 1` and one Cargo
operation at a time. Targets and Windows temporary artifacts stayed on D:. Test/dev debug info
was disabled and codegen units set to one to fit the development PC's memory. WSL tests used a
Linux filesystem for fixtures; bulky targets stayed on D:. The Windows release directory is
`D:\pcagent-target\backend-final-20261004`, separate from the already-running older GUI executable.
No existing target or running GUI was removed.

| Check | Observed result |
| --- | --- |
| Windows and Linux `cargo fmt --all -- --check` | PASS |
| Windows and Linux workspace/all-targets Clippy, `--locked -j 1 -- -D warnings` | PASS |
| Windows workspace tests | 559 passed, zero failed, one ignored |
| Linux workspace tests | 510 passed, zero failed, one ignored |
| Windows release: CLI, GUI, evaluator, elevated helper | PASS |
| Linux release: CLI, GUI stub, evaluator, elevated stub | PASS |
| Linux release stubs | Read-only Windows inspections return `unsupported` (exit 3), GUI exits 2 with its Windows-only message, helper rejects native execution as `Unsupported` (exit 1) |
| Python paged MCP client | Three tests passed |
| Offline evaluation/import/blind-grading harness | PASS; synthetic harness fixtures do not score real model quality |
| Same unchanged live-origin capture, offline context benchmark | 96 calls, one exact repeat; 1,136,940 full / 865,970 compact projection bytes; 64,695,008 full / 49,309,431 compact cumulative context bytes |
| Linux/Windows offline projection | Same scenario/registry hashes and byte metrics |

The ignored test requires live credentials/network and was not enabled. Linux Clippy first
identified fields used only by native Windows worker execution. A scoped non-Windows dead-code
annotation preserves portable argument parsing and the honest unsupported stub; Linux checks and
Windows Clippy/tests subsequently passed. This is a local validation record, not a published CI run.

Authorized Windows inspections of the final release ran after compilation stopped, without admin,
and stored anonymized outputs in a separate private D: directory. Power/thermal, inventory, events,
system state and drivers all returned explicit partial coverage with 47, 44, 116, 43 and 325
observations respectively. There were 64
installed-driver entries; no online Windows Update search was performed. The account catalogue
GET succeeded with four NVIDIA models and sent no PC observations. Battery capacities/state and
the 29-point, 30-second persisted recording and the earlier build-time probe are detailed in
[battery.md](battery.md#native-validation--2026-10-04).
No user startup entry or cache was mutated, Settings page opened, driver installed or real UAC
request launched. Fixtures exercise action approval/revalidation, journal-bound startup reversal,
bounded cleanup, helper supervision, battery discontinuities and fifteen offline MFT parser cases.

Raw captures, recording database, compiler logs, grading packets and source/binary hash manifests
remain outside Git under `D:\pcagent-tmp\backend-20261003`. Native sensor-supplier readings,
recorded discharge/runtime and per-process GPU acceptance remain unverified on this PC. Native MFT
collection is not implemented; dedicated-volume accuracy, real UAC/install acceptance and some
native action effects remain outstanding. Real answers have no human quality scores, and the
advisory judge remains ungraded. Byte reductions establish neither token/cost savings nor preserved
diagnostic quality. Default-model changes, automatic escalation and external research remain
conditional on the plan's missing evaluation/contract evidence.
