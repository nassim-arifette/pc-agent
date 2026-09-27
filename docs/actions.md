# `pcagent-actions` (Lot D)

Controlled local actions: spec 11.1–11.3, 15.2, 17.1 (`ActionPlan`, `ActionResult`), 17.4, tests of 20.3.

## Flow

```
model ──ActionIntent (opaque refs only)──▶ Broker::prepare_plan ──▶ ActionPlan (awaiting_approval)
                                              │ TargetResolver / Guard / ProtectionCheck
trusted local UI ──approve_locally(&mut plan, &session, ttl)──▶ ApprovalToken (single use)
trusted local UI ──Broker::execute(plan, token, session, cancel, before_ref)──▶ ActionResult
```

`execute` is never a model tool. The model sees only `ActionPlan::model_view()` (`PlanModelView`): refs,
evidence refs, estimate, recovery, preconditions, state and expiry. Resolved paths, PIDs, identities
and the fingerprint stay local. `ActionIntent::from_model_json` accepts a bounded strict JSON object
(`deny_unknown_fields`); refs must match `[A-Za-z0-9._:-]{1,128}`, so paths, shell syntax or free
text such as "fais tout" are rejected and can never produce approval.

## Wiring (`pcagent-agent`, CLI)

`Workstation::enable_action_proposals(ActionDesk)` adds the `prepare_action_plan` tool
(`crates/agent/src/proposals.rs`); the CLI enables it with `diagnose --allow-cloud --propose-actions`.

- Allowed kinds: `open_in_explorer` (storage `scope_id` + `item_ref`, identity observed by
  `NativeGuard`), `open_settings_page` (allowlisted page) and `run_cleanup_adapter` (a directory the
  policy classifies as `pcagent.synthetic_demo_cache` with a verified marker, reopened with
  `SyntheticDemoCache::open`; refused when `permissions.reversible_only`). `recycle_selected_files`
  and `request_normal_close` cannot be proposed. Recycle is executable in the actions crate but
  is not a model-proposable kind; wiring it is left to the coordinator.
- Targets are resolved locally into a `FixedTargets` resolver and checked by `PolicyProtection`
  (baseline + policy protected paths + user exclusions). The model gets `PlanModelView` only; at most
  `MAX_PENDING_PLANS` (3) per investigation.
- After the report, the CLI prints each plan with its local target, recovery and preconditions and
  asks for a typed `yes` on an interactive terminal (stdin and stderr). Only then does it call
  `approve_locally` and `Broker::execute`. With `--json` or without a terminal nothing runs
  (`not_reviewed`). Tokens never leave the CLI process.
- Verification depends on the action. Normal-close verification uses `VERIFY_SAMPLES` (3)
  system samples on each side, separated by 2 s; incomplete, duplicate or overlapping measurement
  windows cannot establish an improvement. Cleanup reports actual deleted bytes and volume free
  space before/after. Opening Explorer or Settings makes no performance-improvement claim.
- Proposals require prior usable observations and retain their local `observation_id` references.
  These references identify the investigation context, not a proof that the action solves a cause.
- JSON output contains one report with pending manifests and decisions. It never grants approval.
- `pcagent demo-cache --mb N` (1–256) creates the marked synthetic cache under
  `%LOCALAPPDATA%\pcagent\synthetic-demo-cache`.

## Manifest and state machine

- Synthetic cleanup binds the root identity, marker identity and every file's path, volume/file
  identity, size and modification time into the approval fingerprint. Added/replaced files,
  replacement roots and enumeration overflow require a new plan. More than 1024 directory entries
  is refused rather than truncated. This is a metadata manifest, not a hash of file contents.
- On Windows, the adapter pins the directories, marker and complete approved file set before
  the first effect, compares the complete manifest again, then removes each file through its
  verified handle with `SetFileInformationByHandle(FileDispositionInfo)`. Redirection of a path
  cannot change the object deleted. No arbitrary-path deletion fallback exists. Other platforms
  may inventory/create demo data but cannot execute this adapter.
- `permissions.reversible_only` defaults to `true`. Synthetic deletion needs an explicit `false`
  preference as well as local approval; regenerable data is not considered restorable data.
- `ActionPlan` fields follow 17.4. The fingerprint is SHA-256 (`sha2 =0.10.9`, published
  2025-04-30, `default-features = false`) over a serde_json serialization of a fixed-order struct
  containing every field except `state` and `fingerprint`. Every check recomputes it; a mismatch
  moves the plan to `invalidated`.
- States: `draft → validated → awaiting_approval → approved → executing → verifying →
  completed | partial | failed | cancelled`, plus `expired` / `invalidated`; any other transition is
  `ActionError::IllegalTransition`. `exclude_targets` recomputes the fingerprint and returns the plan
  to `awaiting_approval`, voiding outstanding tokens.
- Plan TTL defaults to 15 min (max 1 h); approval TTL is capped at 10 min and at the plan expiry.
- Recycle on the same volume estimates `Some(0)` immediate reclaim.

## Approval

`LocalSession` (trusted UI process) issues `ApprovalToken`s holding a UUID v4 secret (OS CSPRNG via
`getrandom`), bound to session id, plan id, fingerprint and expiry. The token is not `Clone`,
`Serialize` or `Deserialize`; `Debug` omits the secret. Redeeming removes it first, so a double
click or any rejected use burns it.

## Revalidation before every item

- File: `X:\...` path only, `ProtectionCheck` (policy crate later; `BaselineProtection` is a
  fail-closed minimum), not a reparse point, current `FileIdentity` (volume serial, file id, size,
  mtime, directory flag) equal to the planned one.
- Process: PID + creation time + boot id; a missing boot id never matches.
- Any mismatch refuses the item (`Refusal::*`); a new plan is required. Refusals are reported, never
  escalated.

## Catalog (`Effects`)

Effects take a `Permit` only the broker can build. `Capability` gates `prepare_plan` and `execute`.

| Action | Windows backend | Executable |
|---|---|---|
| `open_in_explorer` | `%WINDIR%\explorer.exe /select,"<path>"` via `Command::raw_arg`, no shell; paths with `" < > | ? * /`, controls or extra `:` refused | yes |
| `open_settings_page` | fixed allowlist `storagesense`, `appsfeatures`, `startupapps` → `ms-settings:` via explorer.exe | yes |
| `request_normal_close` | process handle held (`QUERY_LIMITED_INFORMATION | SYNCHRONIZE`), identity rechecked, `WM_CLOSE` to visible unowned top-level windows, bounded wait; never `TerminateProcess` | yes |
| `recycle_selected_files` | exclusive pin + `IFileOperation::DeleteItem` (`FOFX_RECYCLEONDELETE | FOF_ALLOWUNDO | FOFX_EARLYFAILURE`, no UI) after `SHQueryRecycleBinW`, fail-closed post-check (below); no permanent-delete fallback | yes |
| `run_cleanup_adapter` | `SyntheticDemoCache`: exact approved manifest, verified handles, only regular files directly inside the marked root | Windows, configured cache and explicit permission |

### Recycle binding (spec 11.3)

`windows::recycle_file` runs on its own COM STA thread (`IFileOperation` requires STA):

1. Refuse `BaselineProtection` paths (the broker has already applied `PolicyProtection`).
2. Pin: `CreateFileW(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE, FILE_SHARE_READ,
   FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)`. Without `FILE_SHARE_DELETE` /
   `FILE_SHARE_WRITE` no other open can rename, delete, replace or write the file while the pin is
   held; if someone else already has such access the open fails and the item is refused `in_use`.
3. On the pinned handle: `GetFileInformationByHandle` identity (volume serial, file id, size,
   mtime) must equal the plan's, no reparse point, final path (`GetFinalPathNameByHandleW`) equal
   to the planned path, not a directory, exactly one hard link (`multiple_links` otherwise, since
   recycling one name would not remove the data), NTFS (`GetVolumeInformationByHandleW`).
4. A share-all attribute handle is opened while pinned, so it is bound to the same object and
   follows it through the move into the recycle bin.
5. `SHCreateItemFromParsingName(path)` + `IFileOperation::DeleteItem` + `PerformOperations`, with
   an `IFileOperationProgressSink`. The pin is closed in `StartOperations` (see below).
   `PreDeleteItem` checks that it is the only item, the `TSF_DELETE_RECYCLE_IF_POSSIBLE` flag is
   set, the item path is the planned path, the tracking handle still sees the same object with one
   link and the path still names it (not through a reparse point); otherwise it aborts. Any
   rename/move/copy/new-item callback aborts.
6. Post-check (`native::verify_recycled`, pure and unit-tested): operation succeeded and was not
   aborted; `PostDeleteItem` reported a recycle bin item (otherwise it may have been deleted
   permanently); that item is under `<same drive>:\$Recycle.Bin\<SID>\` and has the same volume,
   file id, size and mtime; the tracking handle's final path is that item; the original path no
   longer names the object. Any mismatch is an error, never `done`.

**Remaining limitation.** The shell takes items only as paths/PIDLs and opens the file itself
with `DELETE`; there is no documented handle-based recycle API. It does so twice: in a pre-flight
step after `StartOperations` and before `PreDeleteItem` (observed on Windows 11 with and without
`FOFX_EARLYFAILURE`, `FOF_WANTNUKEWARNING` and `FOFX_RECYCLEONDELETE`, and on the `windows-latest`
runner; with the pin still held it fails with
`COPYENGINE_E_SHARING_VIOLATION_DEST`, `0x80270028`), and for the move itself. The pin is therefore
released in `StartOperations`, the last callback before that pre-flight open. From then until the
move, a local process with write access to the parent directory could swap the file. A swap before
`PreDeleteItem` makes it abort; one after it cannot be prevented through the shell, is detected by
step 6 and reported as an error (the substituted file is then in the recycle bin, i.e. restorable,
not deleted), and the verified file stays at its original path or wherever it was moved. Content
written to the file after the pin is released changes its size or mtime and also fails step 6.

Non-Windows builds report `Status::Unsupported` for every native effect and observation.

## Journal and recovery

`Journal::open(dir)` writes `<plan_id>.jsonl`, one fsync'd line per `started` / `finished` record
with fingerprint, item index, target ref, pre-state and outcome. No token or secret is recorded.
If a line cannot be written, remaining items are skipped (`journal_unavailable`). Even failure of
the final `finished` write prevents an overall `completed` result; `journal_status` records the error
without erasing effects already observed. A partially completed cleanup reports its removed file
and byte counts. A failure before any removal is an overall failure, not a partial success.

The CLI calls `recover(&journal)` when action proposals are enabled. Unfinished `started` records
become `needs_inspection`; subsequent runs continue reporting them. Nothing is replayed. Reads are
bounded (16 MiB per journal, 64 KiB per line, 4096 journals).

During approved execution, a scoped Windows console handler converts Ctrl+C/Ctrl+Break into the
broker's cancellation flag. Cleanup checks it between files, subsequent plans remain unexecuted,
and verification can stop early. A native operation already in progress is not forcibly interrupted.

## Safety boundaries

- Crate root: `forbid(unsafe_code)` off Windows, `deny(unsafe_code)` on Windows with the only
  `allow` on `mod windows`; every block has a `// SAFETY:` comment and handles are RAII.
- No elevation, `runas`, token privileges, `cmd`/PowerShell, `ShellExecute` verbs, `TerminateProcess`
  or path-based `DeleteFileW/A` (asserted by a source test). Handle-based permanent deletion belongs
  only to the explicitly approved synthetic adapter; recycle never falls back to it.
- `FileIdentity::is_reparse_point` is also set when the final path differs from the requested
  path, so resolvers must supply normalized long paths.

## Validation boundaries

Windows regression tests run locally as well as in CI, using disposable fixtures: identity,
synthetic-manifest changes, partial deletion, journal failure and recovery. Recycle has tests for
the pin (rename/delete/write by
others refused while pinned), refusal of swapped, hard-linked, in-use, missing and protected targets,
and an end-to-end recycle of a temp file (skipped if the runner has no recycle bin). Not exercised
natively as a full acceptance scenario: Explorer and Settings launching, `WM_CLOSE` behaviour, Ctrl+C during a real action, and
behaviour on non-NTFS, network, or redirected-folder volumes.
