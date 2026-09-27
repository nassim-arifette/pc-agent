# Storage scanner (`crates/storage`, package `pcagent-storage`)

Lot C, P0: classic progressive scanner implementing spec sections 9.1, 9.3, 9.4, 20.2 and the
tools `get_storage_overview`, `start_storage_scan`, `get_largest_items` (13.3). The NTFS
MFT/USN backend (9.2, P1) is not implemented; `ScanBackend` is the seam for it.

## Design

- **Scope.** `ScanScope::authorize(path)` (or `StorageService::authorize`) is a local, user-initiated
  step that captures a non-link root object before canonicalizing, binds its filesystem identity,
  refuses a root that is itself a link/reparse point, refuses
  non-directories and, on Windows, UNC/network paths and drive types other than fixed,
  removable or RAM disk. The model only ever sees the opaque `scope_id`; no tool accepts a path.
- **Walker.** `ClassicBackend` is single-threaded, iterative depth-first (no recursion), and reads
  metadata only. Windows enumerates through directory handles (`FileIdBothDirectoryInfo`) and
  opens single child names relative to those handles (`NtCreateFile`, `OBJ_DONT_REPARSE`). Unix
  uses descriptor-relative `openat`/`statat` with no-follow flags. Root replacement and ancestor
  redirection cannot expand authorization. Concurrency is bounded by one worker thread per scan, one scan per
  scope and at most `MAX_CONCURRENT_SCANS` (2) scans per service.
- **Never followed / never opened.** Symlinks, junctions, mount points and all other reparse points
  (files and directories) are counted in `reparse_skipped` and not entered. Entries with
  `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, `FILE_ATTRIBUTE_RECALL_ON_OPEN` or `FILE_ATTRIBUTE_OFFLINE`
  in the directory-enumeration attributes (`FileIdBothDirectoryInfo`, before a child is opened) are
  counted as `placeholders` and never opened; their reported size goes to
  `placeholder_logical_bytes`, not to local allocation. On unix, directories on another device
  are skipped (`other_volume_skipped`); on Windows, mount points are reparse points and are skipped,
  and files whose volume serial differs from the root are skipped too.
- **Accounting (9.3).** Per file: logical size, allocation on the volume (Windows:
  `GetFileInformationByHandleEx(FileStandardInfo).AllocationSize` on a handle opened with
  `FILE_READ_ATTRIBUTES` and native no-reparse/no-recall options, no data
  access; unix: `st_blocks * 512`), `estimated_reclaim_bytes` always `None`. Hard links are
  deduplicated by `(volume serial, 128-bit file id)` (Windows `FileIdInfo`) or `(dev, ino)` when
  link count > 1: the first path seen is attributed, later ones are `hard_link_aliases` and
  excluded from totals and rankings. Sparse/compressed files keep both sizes. Directory aggregates
  are computed once bottom-up so each file contributes once to each ancestor. Unknown allocation
  is counted in `allocation_unknown_files` and never added as zero-byte truth.
- **Dates (9.4).** Only `modified_unix_ms`; `last_used` is always `"unknown"`.
- **Robustness.** Unicode names are kept as `OsString`; on Windows the canonical root is `\\?\`
  verbatim so native calls work past `MAX_PATH`, and the prefix is stripped only for display.
  Denied directories are counted and listed (bounded by `max_denied_listed`, default 64). Files
  that vanish or whose modification time is after scan start are counted and make the scan
  `partial`. Limits: `max_entries`, `max_duration`, `index_budget_bytes` (default 1 GiB, spec 19.1;
  an estimate including active directory buffers and the index) each stop with `truncated` and
  `partial`. Entries are streamed, with cancellation/deadline checks and a progress callback
  before each metadata access; a large directory is never collected into an unbounded child list.
  Dropping a `ScanJob` cancels and joins it; the CLI cancels all jobs before joining them.
  These are cooperative bounds: a blocking native call, final sorting and allocator overhead mean
  the duration is not a hard deadline and the estimate is not a process RSS limit.
- **Index.** In memory (P0; SQLite later). Entries store name + parent index, not full paths.
  Rankings are sorted once at the end: allocation when known, else logical; ties by logical.
- **Status.** `ok`, `partial`, `cancelled`, `permission_denied` (root), `error` (root) via
  `pcagent_shared::Status`. Overview is `partial` without a completed scan or when volume space
  is unavailable.

## Privacy

Views carry `path` (full, local only), `redacted_path` (`<scope>/rel/path`, with any component
equal to the profile directory name replaced by `<user>`) and `item_ref` (`<scan_id>:<n>`, valid for
one scan, validated strictly by `StorageIndex::resolve`). Call `without_paths()` on
`LargestItemsPage` / `StorageOverview` before anything leaves the machine. File names remain
untrusted text and may still be personal; `redacted_path` is minimization, not anonymity.

## Public API (for integration)

```rust
StorageService::new() / with_backend(Arc<dyn ScanBackend>)
authorize(&self, &Path) -> Result<ScopeInfo, StorageError>          // local UI/CLI only
revoke(&self, &str) -> bool;  scopes(&self) -> Vec<ScopeInfo>
start_storage_scan(self: &Arc<Self>, scope_id, ScanOptions) -> Result<ScanJob, StorageError>
scan_blocking(&self, scope_id, &ScanOptions, &mut dyn FnMut(&ScanProgress)) -> Result<ScanSummary, _>
cancel(&self, scope_id) -> Result<bool, _>;  index(&self, scope_id) -> Result<Arc<StorageIndex>, _>
get_storage_overview(&self, scope_id) -> Result<StorageOverview, _>
get_largest_items(&self, scope_id, ItemKind, offset, limit /* 1..=100 */) -> Result<LargestItemsPage, _>
ScanJob::{cancel, progress, is_finished, wait}
StorageIndex::{totals, item, resolve, local_path, deduplicated_totals, largest_items, summary}
dedup_parent_child(&[PathBuf]) -> Vec<PathBuf>;  redact_path(root, path, profile) -> String
LargestItemsArgs / ScopeArgs: strict (`deny_unknown_fields`) tool argument types
TOOL_OVERVIEW, TOOL_START_SCAN, TOOL_LARGEST_ITEMS, MAX_PAGE
```

## Validation status

Linux (local and CI `ubuntu-latest`): all portable tests, including unix symlinks, `chmod 000`
denial (self-skips when running as root), hard links, sparse `st_blocks` allocation, Unicode,
cancellation, limits, files changed/removed mid-scan, pagination, parent/child dedup, redaction.

Native Windows tests run locally and in CI: junctions, root/ancestor substitution, attribute-only
reparse mutation during an active scan, long paths > 300 chars, allocation/volume space,
attribute constants and ordinary nested trees. Linux tests also cover root replacement during
authorization and descriptor-relative traversal. Test helpers do not open visible terminals.

Not validated with live fixtures: OneDrive/Cloud Files placeholders (enumerated placeholder
attributes prevent child opens, tested through an injected open callback), compressed/sparse NTFS files, ReFS file IDs,
denied system folders, removable/RAM-disk and mapped network drive refusal, very large volumes
(duration and the index-size estimate vs real memory), and `GetDiskFreeSpaceExW` coherence with
attributed totals.

Not implemented: MFT/USN backend, persistent (SQLite) index, incremental tracking, reclaim
estimation, VHD/VHDX rules, and treating fully-hydrated cloud files (which are reparse points)
as attributed; they currently count as `reparse_skipped`.

## Cloud-bound names

Agent tools apply `redact_names` on top of `without_paths`: only placeholders (`<scope>`, `<user>`) and generic directory names from `GENERIC_NAMES` (e.g. `node_modules`, `target`, `.cache`) stay visible. Every other component becomes `*`, keeping a short extension on the last one (`<scope>/*/*.pdf`).
