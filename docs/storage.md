# Storage scanner (`crates/storage`, package `pcagent-storage`)

Lot C, P0: classic progressive scanner implementing spec sections 9.1, 9.3, 9.4, 20.2 and the
tools `get_storage_overview`, `start_storage_scan`, `get_largest_items` (13.3). The NTFS
MFT/USN backend (9.2, P1) is not implemented; `ScanBackend` is the seam for it.

## Design

### Hierarchy and scan-time facts

The index stores first-child/next-sibling links with parents before children. Links are rebuilt
at finalization; hard-link aliases never contribute twice to sizes or facts. The desktop's
`scan_children(scope_id, item_ref?, kind, offset)` reads direct children in groups of 50, using
the same allocation/logical-size ordering as the global ranking. Omitting the parent means the
scan root; the root itself is not selectable. Browsing children publishes no selection source.
The response includes the parent, its partial flag and the remainder of that page. The UI
recalculates **Other** over all pages it has displayed: parent size minus displayed children,
clamped to zero. This is neither free volume space nor a selectable item.

`scan_item_facts(scope_id, item_ref)` resolves only a scan-issued item reference, never a path
from JavaScript. It walks the in-memory descendants and caches completed facts per scan/item:
largest file, counts and bytes unmodified for 182 days, undated files and closed extension-only
file-type totals. Unknown allocation stays null. Starting another facts request cancels the
previous walk before queuing work on the blocking pool. Cancellation and cache publication
share a completion lock; cancelled walks never enter the cache.

The command issues an exact C3 selection source containing the one item's sibling rank,
`parent_ref`, `parent_alias`, `rank_scope: "siblings_of_parent"`, `item_facts` and
`volume_total_bytes`. Facts are frozen at scan completion, not current readings; C1 recognizes
`*_count` and flags current-tense historical claims. C5 sends file types only as nameless totals
and always aliases the largest file, even with file-name consent. Scan IDs, real parent paths
and the local system-folder hint are excluded.

The desktop offers map/tree/list navigation, a local facts inspector and session-only recent
summaries. Reopening an older scan starts a new scan; only the latest full index is retained.
The fixed-volume mount point is read locally for the C7 `requestWatch(rule)` proposal.
That hook only opens a prefilled form, never saves a watch. Removable/unsupported or unknown
volumes cannot propose a watch; creation revalidates the volume in the Watches backend.

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

Views carry `path` (full, local only), `redacted_path` (relative local names), `item_ref`
(`<scan_id>:<n>`, strictly resolved for one scan), `alias` (`file-N` or `dir-N`), `alias_path`
(`<scope>/dir-N/file-M`) and `parent_ref`. Aliases are distinct within a scan; references identify
them across scopes/scans. Before transmission, use **both** `without_paths()` and `redact_names()`:
the latter replaces relative names with the alias path. `StorageContext::project_page` adds only
trusted local classification/context and explicitly authorized basenames. Local report objects
carry real paths separately and must never be serialized as cloud tool outputs.

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
compare_content(&self, scope_id, &ContentOptions) -> Result<ContentComparison, _> // explicit local consent only
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

## Context, names and content comparisons

`Policy::item_context` returns a role, containing-folder role, recognized personal category,
application hint and conditional adapter recovery information. All explanatory strings come from
local rules. Application identification is a registered path-pattern match, not an installed-app
or ownership check. Backup and Recycle Bin availability remain unknown. This is descriptive
context, not an action authorization; existing action revalidation still applies. Propagation
of hidden/system attributes to policy remains open (review S03).

`--share-file-names` is a separate opt-in requiring `--scan-root` and `--preview-cloud` or
`--allow-cloud`. A finite allowlist contains the largest 20 files and 20 folders per scope and
their ancestors, at most 200 basenames total and 512 UTF-8 bytes per name. The root name and
absolute paths are excluded. Preview prints that list and representative tool projections
without network access. The cloud run prints its actual list to stderr before the first request;
the flag grants permission for that run. A later invocation performs a fresh scan/list, so it
can differ from the dry run. Later model-requested scans use aliases only. Names are untrusted
data, JSON-escaped in previews and sanitized in text. The question remains verbatim input.

`--compare-content` independently authorizes **local reads**. It works in `storage` and
`diagnose`, including offline. Same-size candidates among the largest 256 indexed files are
hashed in full with SHA-256, at most 256 MiB/15 seconds by default. Hashes never leave the
comparator. Reports retain match-group references and coverage; item views show the count and
up to three other copies, resolved to real paths locally. No folder equality, cross-scope
comparison, similarity matching, removal, malware detection or corruption validation is included.

Content opens use pinned no-follow ancestor handles/descriptors and validate the root identity,
file identity, length, modification time, volume and single-link status before/after reading.
Windows also checks enumeration attributes before opening (including recall-on-open), uses
no-recall/no-reparse flags and excludes concurrent write/delete access for the read handle.
Unix uses `openat(NOFOLLOW|NONBLOCK)` and regular-file checks. Unavailable, replaced, linked,
redirected or changing files do not produce matches. Cancellation/deadline checks happen during
directory lookup and between read blocks. Single OS calls remain non-interruptible; Unix checks
are best effort rather than an atomic filesystem snapshot. Files can change after observation.

`diagnose --scan-root` prepares metadata scans locally before the model loop: at most 100,000
entries/64 MiB estimated index/15 seconds per root, sharing a 30-second preparation deadline.
Optional content comparisons share that deadline and 256 MiB total; file selection is per scope.
The model can query existing scans immediately. Local preparation is retained separately from
model citations; preview uses the same `project_page` function as the tools. Incomplete scans,
budget/access exclusions, and unchecked files remain visible. A completed comparison does not
mean every file was read, nor that a file without a detected match is unique.

Wanted limitations: metadata-only default; explicit independent permissions for content reads,
names and cloud; no generic deletion; local reference resolution. Covered limitations: inferred
ownership/roles, unverified recovery, bounded and non-atomic comparison, no guaranteed reclaim.
