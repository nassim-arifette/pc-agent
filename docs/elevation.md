# On-demand elevated inspection

`pcagent-elevated` is a separate, read-only helper for one locally approved task. The GUI,
CLI and model stay unelevated. The helper has no model client, HTTP client, command interpreter
or arbitrary query entry point. Its executable forbids unsafe code; the Windows calls live in
the native `pcagent-inspect::elevation` module.

## Tasks and coverage

| Closed task | Current behavior |
| --- | --- |
| `system_caches` | Bounded metadata inventory of `Windows.old` on the Windows volume, `SoftwareDistribution\Download`, and the documented default Delivery Optimization cache directory |
| `mft_inventory`, with an uppercase drive letter | `unsupported`, with `mft_accuracy_unvalidated`; no volume handle is opened and no size is returned |

The Windows directory is obtained from `GetWindowsDirectoryW`, never a caller-supplied path or
environment variable. Cache enumeration reuses the storage crate's handle-based `ClassicBackend`.
Each category has a limit of 100,000 entries and an approximate index budget of 32 MiB. All
categories share the approved time budget, between 1 ms and 120 s. An incomplete scan retains its
coverage and observed lower bounds; it does not report a complete directory total.

The Delivery Optimization collector covers only the
[documented default directory](https://learn.microsoft.com/en-us/windows/deployment/do/delivery-optimization-workflow).
The cache can be relocated by the
[DOModifyCacheDrive policy](https://learn.microsoft.com/en-us/windows/client-management/mdm/policy-csp-deliveryoptimization#domodifycachedrive).
Consequently that category is always partial. A missing default directory does not mean the
cache is empty. Relocated caches are not searched by guessing paths.

Restore-point sizes remain `unsupported`, with unknown bytes. Summing `System Volume Information`
would incorrectly attribute unrelated data to restore points, so that directory is not enumerated.
Disk health, protected-process paths and ETW are not implemented by this helper.

For each cache the response separates logical bytes from the sum of known file allocations and
reports denied directories/files, skipped reparse points and cloud placeholders, other volumes,
special files, vanished/changed files, hard-link aliases, allocation unknowns, errors and truncation.
Hard links are attributed once within a category; categories must not be summed as a distinct
volume total. Directory metadata allocation is excluded. The result is a changing metadata
snapshot, not a point-in-time filesystem image.

**Observed allocation is not reclaimable space.** There is no cleanup recommendation, deletion
action or reclaim estimate in this protocol. Exact MFT allocation and attribution of compressed,
sparse and hard-linked files require dedicated test volumes before the MFT task can become usable.
Reusing the classic scanner does not satisfy that MFT acceptance criterion.

## Local approval API

```rust
use pcagent_inspect::elevation::{ElevatedTask, PreparedElevation, to_inspection};
use pcagent_storage::CancelToken;
use std::time::Duration;

let mut job = PreparedElevation::prepare(ElevatedTask::SystemCaches, Duration::from_secs(60))?;
// The local UI displays job.request().task() and job.request().budget().
// This call belongs to the explicit local approval gesture:
let permit = job.approve_locally()?;
let cancel = CancelToken::default();
let response = job.run_locally(permit, &cancel)?;
let measurements = to_inspection(&response);
```

Preparation creates a private request and does not display UAC. `ElevationApproval` has no
public constructor, `Clone` implementation or serde representation. It is bound to one prepared
request and consumed with the job. A second approval cannot be issued. Expired requests must be
prepared again. The caller retains a clone of `CancelToken` to cancel a running approval.

The application must store this local object behind its own opaque proposal reference and call
approval only from its UI/CLI validation path. A model proposal is not a permit. Calls which can
block belong on the application's blocking worker, not its UI thread.

`completion()` returns a read-only local capability, without approval or serialization authority.
`run_locally_bounded()` returns the operation result separately from that completion: cancellation,
timeout or an exit-query error can return while Windows consent or confirmed process exit is still
pending. Its caller must keep the host alive until `ElevationCompletion::on_complete` runs or a
blocking `wait` succeeds. Completion is published after the owned helper has exited, its handles
are released and finite private-file cleanup has run. The synchronous `run_locally()` waits for
that completion before returning, so the CLI cannot abandon an outstanding launch on exit.

`to_inspection` produces C4 observations and coverage with their source, measurement window and
status. It excludes job IDs, nonce, request digest, user SID and job paths. Raw request/response
JSON is local protocol data and must not be sent to the model. Errors carry only a code or
`ErrorKind`; their display never includes an argument, path or identity.

## GUI and CLI bindings

The Rust shell registers these local commands. Tauri input argument names use camelCase; the
returned review fields retain their Rust snake_case names.

| Command | Arguments | Result |
| --- | --- | --- |
| `elevation_status` | none | Current review/progress, or `null` when the slot is free |
| `elevation_prepare` | `task: {kind: "system_caches"}`, `budgetMs: 1..120000` | `{request_id, task, budget_ms, state: "awaiting_local_approval"}` |
| `elevation_run` | `requestId` from that review | An `Inspection` with C4 observations and coverage, after local approval and Windows consent |
| `elevation_cancel` | `requestId` | Declines a prepared task, or signals cancellation of the matching running task |

`elevation_status` recovers the same request ID, task and budget after a webview reload, with state
`awaiting_local_approval`, `running` or `cancelling`. Reading this status creates no new job or
permit. A recovering frontend must still obtain the local approval gesture for a prepared task;
running/cancelling requests cannot be approved again.

The frontend must display the returned task and budget before offering the local approval
gesture. Only that gesture calls `elevation_run`. Retrying a consumed request or supplying a
different ID cannot execute a task. Declining before approval creates no helper process.
Cancelling a running task keeps the slot occupied until native supervision and cleanup finish.
An `elevation_run` error is not proof that the slot is free; wait for `elevation_status` to return
`null` before preparing another task.

The backend allows one prepared or running job per app state. A `Run` releases its matching
slot through a weak reference only after its native completion capability reports cleanup,
including when the operation fails, unwinds or its command response is abandoned. A bounded
operation error leaves the slot `cancelling` while consent or exit confirmation remains pending.
A stale completion cannot clear another job. Destroying the slot signals its running cancellation
token. Dropping a browser-side
promise alone does not stop its already running helper; explicitly call `elevation_cancel` when
the user stops or leaves that progress flow.

Normal window exit and the tray's Quit command explicitly close admission, cancel the current
task and wait on a blocking worker for supervision and approved-resource cleanup. The condition
variable releases the slot mutex while waiting. The app calls Tauri's process exit only after
the slot is idle; a five-second wait timeout keeps the host alive and permits another Quit
attempt. Prepared requests are dropped without approval. Each native termination attempt waits
on the owned helper handle for at most two seconds; timeout or query failure is an operation
error. A background supervisor retains that handle and pinned job, retries termination and waits
for observed exit before publishing completion. `WaitForSingleObject` can confirm exit even when
an exit-code query fails. Persistent failure to confirm exit keeps the window open and the slot
occupied; elapsed time and the helper watchdog never substitute for that confirmation.

These Rust commands are present; no administrator-analysis button or frontend progress flow
was added in this backend lot. A future frontend retains its review ID or recovers it with
`elevation_status`. Native preparation creates small private files; preparation and execution
run on Tauri's blocking worker.
The GUI and CLI call `ensure_unelevated_host` before starting normal work and reject an
administrative host session.

The CLI command `pcagent elevated-inspect system-caches --timeout-seconds 60` displays a
review without invoking UAC. Adding `--approve-local` authorizes the closed task and then opens
Windows consent. `pcagent elevated-inspect mft --volume C` reports unsupported without launching
the helper. The CLI exposes no arbitrary worker arguments or output path. Its collection budget
is bounded; its final cleanup wait can extend beyond that budget while Windows consent remains
unresolved or helper exit cannot be confirmed.

## Restricted file protocol

Protocol version 1 accepts only the two tasks above. Unknown fields are rejected at every JSON
level, including the nested measurement window and coverage. A request is at most 8 KiB; a response
is at most 64 KiB. Reads are bounded before JSON parsing. Versions, enum values, budget, UUID-shaped
references, request age, task, nonce, measurement window and response structure are checked.

The worker CLI contains exactly these six arguments:

```text
--job-dir <private-directory> --nonce <nonce-reference> --request-digest <sha256-hex>
```

It accepts no task on the command line, output filename, shell command, scan path or raw device
name. The job directory must have the generated `pcagent-elevjob-<uuid>` leaf name and live on
a fixed local drive. UNC/device paths, traversal, alternate streams, reserved device names,
quotes, control characters and reparse points in the directory chain are refused.

The parent creates the directory with a protected DACL granting access only to its current user,
Local System and Builtin Administrators. Every protocol file has the same explicit protected
DACL. Native checks validate the owner, allowed ACEs and file type. Ancestors and the immutable
request remain pinned without delete sharing; file reads also deny writes. Hard-linked protocol
files are refused. The only filenames are `request.json`, `response.json`, `running` and `cancel`.
All writes use `CREATE_NEW`, so neither a response nor another file can be overwritten.

The CLI SHA-256 digest binds the exact request bytes prepared for the approved task. The response
must match the request's version, task, job ID and random nonce. Creating the `running` marker
claims a job once; replay is refused. The request expires 120 s after preparation. `Debug`
representations redact its nonce and job ID.

Current native validation requires the same account before and after elevation. Supplying
credentials for another administrator fails the owner check; that workflow remains unsupported.
No job-directory or ACL weakening is used as a fallback.

## Launch, cancellation and lifetime

Only the fixed sibling `pcagent-elevated.exe` next to the current application is resolved. Its
ancestors and regular executable are pinned against replacement while launching.
`ShellExecuteExW` uses the fixed `runas` verb and `SEE_MASK_NOCLOSEPROCESS`; `SEE_MASK_NOASYNC`
is used because the launch thread has no message loop. These flags follow the
[ShellExecute documentation](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfoa).
The working directory comes from `GetSystemDirectoryW`; no shell or PATH lookup is used.

The parent supervisor includes time waiting for launch in the task budget and remains cancellable
while Windows displays consent. Windows owns the UAC dialog; cancellation cannot dismiss it.
The supervisor owns the entire launch, so the pinned request stays alive until that call resolves.
If consent arrives after cancellation or timeout, it terminates only its own returned helper
handle and retains it until exit is observed. The cancel marker and request expiry also prevent
a late task from collecting data. A pending consent dialog keeps GUI shutdown pending; the user
must resolve the Windows dialog. UAC refusal maps to `UacDeclined` rather than a generic failure.

An independent helper watchdog covers setup and blocked I/O with a 120 s process deadline.
Another checks the approved collection budget and cancel marker. The parent terminates only the
process handle returned for this job, including cancellation/timeout races. There is no arbitrary
PID termination API. Malformed/truncated responses, a missing response, failed exit codes and
unmatched nonce/digest are rejected without producing a measurement.

After completion the parent removes only the four fixed job files and its empty directory;
cleanup is never recursive. Pending consent retains its private job directory rather than
publishing cleanup. A forced host termination, crash or failed file removal can leave a private
directory. No broad temporary-directory cleanup runs automatically.

On non-Windows platforms preparing or running elevation returns `Unsupported`; protocol parsing,
C4 projection and controlled scanner fixtures remain testable.

## Integration and validation

The integrating branch adds `pub mod elevation;` to `crates/inspect/src/lib.rs`, adds the existing
workspace dependencies `pcagent-storage` and `sha2` to `crates/inspect/Cargo.toml`, enables
`Win32_Security_Authorization` for its `windows-sys` dependency, and registers `apps/elevated`
as a workspace member. No new external dependency version is required. Ship/build the helper
alongside the CLI and GUI; absence of the sibling helper is a launch failure.

The fixture suite covers hostile arguments and task/response fields, oversized/truncated JSON,
unsupported versions, nonce/digest mismatch, budget and expiry checks, local approval binding,
owned-process cancellation/timeout, helper exit codes and simulated UAC refusal. Windows tests
create private fixture jobs to exercise ACL checks, an immutable request, replay and truncated
output without calling `runas`. Cache fixtures check read-only enumeration and hard-link
deduplication. They never inspect or change the real Windows caches.

The GUI slot tests use private fixture-task adapters without native preparation, files or UAC.
They cover matching/stale request IDs, approval consumption and replay, prepared decline, running
cancellation before execution, abandoned/error completion, host destruction, failed approval and
status recovery without issuing another approval. Shutdown fixtures also cover closed admission,
timeout/retry, unlocked status reads, bounded result errors with pending cleanup, second-admission
refusal and cleanup before idle notification. Portable supervisor fixtures keep a simulated
launcher blocked across cancellation/timeout, deliver a late helper or consent refusal, and inject
termination and exit-query failures before a confirmed exit. They verify that the process and
job resources remain owned until that confirmation. Completion callbacks are checked for single
delivery outside the completion lock. Native exit-wait tests use scripted polling without
launching a process.

Rustfmt 1.90 and whitespace checks were run in the isolated worktree. Cargo validation is
centralized in the integration branch. Real UAC acceptance, cross-machine installation and MFT
test-volume accuracy have not been exercised by this lot; no runtime acceptance is claimed.
