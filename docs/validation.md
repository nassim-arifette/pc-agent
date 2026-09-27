# Validation checklist

## Automated (see the exact CI run and revision)

- Format, compile and lint the complete workspace with the pinned Rust toolchain.
- Run portable unit tests on Linux and Windows.
- Run Windows-native memory/process smoke and session identity tests.
- Run CLI commands without any Nebius credentials and verify no implicit cloud mode.
- Exercise synthetic model replies for tool allowlisting, complete-batch validation, duplicate IDs, forged findings, loop budgets and provider errors.
- Check that cloud payloads omit process names, PIDs, creation times, boot IDs and session identifiers.
- Build the Windows executable and upload only the binary, not measurements or configuration.

The workflow validates pushes and pull requests with read-only permissions. It checks formatting and the committed lockfile, without pushing changes or replacing the revision under review. A failed job must not be reported as successful validation.

## Manual native acceptance — NOT completed by CI

On a separate Windows 11 x64 test PC, without elevation:

1. Run `doctor`, `snapshot --json` and `processes --json`. Verify source, units, timestamps, observation intervals, explicit unknowns and permission coverage. Compare semantics with Windows counters, not just Task Manager column names.
2. Inspect two samples. Confirm PID plus creation-time identity, including after terminating/restarting a disposable test process outside PC Agent. Never use the old PID to authorize any action.
3. Try an offline diagnosis without environment variables. Confirm no network request and no maintenance action.
4. Inspect `diagnose "test question" --preview-cloud`. Confirm the question disclosure and absence of local process metadata.
5. Configure an actual NVIDIA Nemotron model available in the account. Use `doctor --check-cloud`, then one small opt-in diagnosis with `--allow-cloud --verbose`.
6. Verify a real provider request, at least one model-selected tool, native collection, a returned tool result, a locally validated finding, an evidence reference and honest insufficient-evidence behavior.
7. Test bad credentials, unavailable model, disconnected network, malformed responses, cancellation and missing metrics. Do not put secrets or personal telemetry in issue comments, Actions logs or committed fixtures.

A live provider test incurs provider usage and must be explicitly authorized. No such test is run in CI. A successful catalog lookup is not a successful model tool-calling test.

## Remaining Lot A / next implementation work

- Verify Windows 11 build and architecture instead of only compilation target.
- Collect a documented boot identity before enabling persistent cross-boot history.
- Validate the live model and JSON/tool contract on the user's account.
- Add representative observation windows, missing sensor coverage and diagnostic evaluation fixtures.
- Evaluate existing process CPU, E/S and process-tree inspection on representative workloads.
- Replace the narrow finding-selection output with richer, locally validated explanation contracts.

Metadata storage scans and controlled actions are implemented with bounded scope; see [storage](storage.md), [actions](actions.md) and [review status](review-status.md). GUI, installer, persistent history, MFT and Tavily remain later work.
