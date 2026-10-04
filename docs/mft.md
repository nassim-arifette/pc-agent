# Bounded NTFS stream accounting and local storage index

`pcagent_inspect::elevation::mft` contains a portable NTFS 3.0/3.1 FILE-record parser and a
streaming aggregate collector with a bounded local directory/file index. The Windows adapter in
`mft_native.rs` belongs exclusively to the
separately approved elevated worker. It opens one approved fixed local NTFS drive with
`GENERIC_READ`, uses read-only filesystem controls, and closes the handle on every exit. It does
not create a journal, lock/dismount a volume, decompress contents or change files. The ordinary
host does not open volume devices.

The output is **supported-stream evidence**, with explicit partial coverage. It is neither a
whole-volume storage total nor reclaimable space. Every native result retains
`live_volume_not_atomic`, `metadata_allocation_excluded` and `native_accuracy_unvalidated`, even
when all initial record slots were walked. Controlled-volume acceptance has not yet established
native accuracy.

## Input and bounds

The supported geometry is NTFS 3.0/3.1, power-of-two FILE-record sizes from 512 bytes to 16 KiB,
and power-of-two clusters from 512 bytes to 64 KiB. Volume size must fit a signed 64-bit byte
length. Larger/newer layouts are unsupported. Length, offset, VCN/LCN, cluster and total
arithmetic is checked before slicing or accumulation. Signed NTFS lengths and VCNs reject
negative encodings; the only highest-VCN sentinel is the empty first segment.

The offline `summarize` API accepts at most 4,096 records and 32 MiB of raw input from one
snapshot. Bounds fail that batch instead of treating truncation as an exact total. The native
streaming API examines at most 1,000,000 records, retains at most 96 MiB of conservatively
accounted metadata and permits at most 2,000,000 decoded runs. Each FILE record permits at most
128 attributes and each decoded run list at most 512 entries. Attribute-list entries, retained
extension groups and physical ranges count toward the metadata bound. A single bounded record
and temporary accounting buffers can add transient memory beyond retained metadata; run-vector
reallocation can additionally retain the previous buffer, at most 32 MiB. The reported high
water mark describes retained metadata, not peak process working set.

The worker's hard deadline and cancellation watchdog cover setup, volume open and synchronous
controls. The portable collector also checks monotonic elapsed time and cancellation before
source calls, after each record read and before joining/validating retained groups. Reaching a
record, run, memory or duration limit returns the known supported subset with an explicit reason
and unwalked slots. It never relabels that subset as complete.

The Windows adapter queries geometry with
[FSCTL_GET_NTFS_VOLUME_DATA](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_volume_data).
It follows the actual ordinal returned by
[FSCTL_GET_NTFS_FILE_RECORD](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_file_record)
and then requests the preceding ordinal. That control enumerates in-use records downward and
can skip unused slots. The output buffer follows Microsoft's documented record-size expression;
the returned record length and reference sequence are checked before parsing. Gaps are reported
as walked slots, not as additional files or as known file bytes.

Offline input must carry on-disk update-sequence protection. Every protected 512-byte tail is
checked before its saved word is restored in an owned bounded copy. The kernel-only normalizer
also accepts a uniformly restored transfer whose tails all equal their saved words; mixed or
torn transfers fail closed. Offline callers cannot use that normalization to admit arbitrary
restored input. See
[FILE_RECORD_SEGMENT_HEADER](https://learn.microsoft.com/en-us/windows/win32/devnotes/file-record-segment-header),
[MULTI_SECTOR_HEADER](https://learn.microsoft.com/en-us/windows/win32/devnotes/multi-sector-header)
and [NTFS3 v6.18 format structures](https://github.com/torvalds/linux/blob/v6.18/fs/ntfs3/ntfs.h).

## Accounting scope

Supported unnamed and named `$DATA` streams of ordinary in-use base files contribute once.
Names are compared using local SHA-256 keys without decoding or returning their text. Named
logical bytes and stream counts remain distinguishable from the overall supported-stream
amounts. Multiple filename attributes and a hard-link count do not multiply file sizes.

The first 16 system records and directory data/index allocation are excluded explicitly.
Reparse data, encrypted data and unsupported flags/units remain unknown. Resident logical bytes
are separate from nonresident data clusters: resident contents occupy shared MFT space, so zero
separate data clusters does not mean zero disk footprint. MFT, directory, index and security
overhead is not attributed. Missing data is unknown, never a zero-length file. Supported empty
resident/nonresident streams remain distinguishable from missing streams.

Mapping pairs are checked against their declared VCN extent. Physical clusters and sparse holes
are distinguished; signed relative LCN deltas are checked against the volume bounds. Sparse
holes retain the previous physical LCN and may describe a virtual span larger than the volume.
Logical size is not inferred from physical allocation. For a complete supported extent chain,
mapped clusters must agree with the first segment's allocation field, or the extended physical
allocation field for sparse/compressed streams. Compressed streams with the supported unit are
counted without decompression; their compression-unit layout remains explicitly unvalidated.
See [ATTRIBUTE_RECORD_HEADER](https://learn.microsoft.com/en-us/windows/win32/devnotes/attribute-record-header),
the [NTFS3 v6.18 run decoder](https://github.com/torvalds/linux/blob/v6.18/fs/ntfs3/run.c) and
[record validator](https://github.com/torvalds/linux/blob/v6.18/fs/ntfs3/record.c).
The Rust implementation is independently written; no driver implementation is incorporated.

## Membership, consistency and coverage

Offline repeated identical records are counted once using a local byte fingerprint. Conflicting
bytes under one number fail the batch. Native requests strictly descend, so repeated/forward
ordinals are invalid source behavior. Extensions join only the matching base record and sequence.
Missing/stale bases remain `missing_base` coverage and never add a second logical file.

Resident `$ATTRIBUTE_LIST` entries validate supported DATA membership using attribute type,
name key, lowest VCN, record sequence and instance. Multiple resident FILE_NAME instances are
allowed for hard links/DOS names. The list's non-DATA attributes are not used to claim metadata
allocation. Nonresident lists are not read through arbitrary data offsets and remain
`attribute_list_unresolved`. Missing/list-mismatched extents can retain a directly observed base
stream logical size while leaving its full allocation unknown. Overlapping VCN extents or
multiple first segments fail that group.

Overlapping physical allocations within a group fail the group. The collector also validates
physical ranges across accounted groups before declaring `allocation_additive`. If that flag is
false, the host must withhold the aggregate `data_cluster_bytes_known` quantity from measured
cloud evidence. Local diagnostic aggregates remain available with the invalid-record or
interruption limitation. These amounts cannot authorize an action.

The adapter compares start/end geometry, MFT valid-data length and available USN journal
witnesses. It serializes only witness availability and change detection; journal identities and
USNs stay local. A changed volume is explicit. Unchanged available witnesses do not make a
live scan atomic. Missing/failed final queries remain partial change coverage.

`Collection`, `Summary`, `CollectionEnd` and `Limit` form a closed finite helper response.
`Collection::validate()` checks count/slot relationships, bounds, byte-subset relationships and
mandatory limitations again in the host. Record references, raw bytes, serials, resident contents
and local journal witnesses are not serialized or logged. The helper's private local response
also includes the bounded metadata paths described below; their Debug output is redacted and
they must never be copied directly into model evidence. The helper's outer measurement window
covers the full job; `duration_ms` covers the portable collector itself.

## Local directory/file inventory

`mft_tree.rs` reads resident FILE_NAME metadata only: filename, namespace and parent record
reference/sequence. It ignores the FILE_NAME copies of logical/allocated sizes and timestamps;
all byte amounts still come from validated DATA streams. Invalid UTF-16, path separators,
control characters, missing names and unsupported filename forms leave explicit limitations
without discarding otherwise valid DATA aggregates. The field layout and namespaces were
checked against the original
[NTFS3 format structures](https://github.com/torvalds/linux/blob/v6.18/fs/ntfs3/ntfs.h).

The index permits at most 250,000 nodes and 32 MiB of conservatively accounted metadata within
the collector's 96 MiB bound. Smaller caller bounds also reduce index capacity. Half is reserved
for directory metadata so a full file index does not prevent collecting parent directories later
in the descending walk. A separate 32-byte reservation preserves the root record/sequence even
if the directory index fills before record 5 arrives; this anchor is excluded from `indexed_nodes`.
Directory rollup entries and path fingerprints are
included in the conservative reservation; filename/data parsing can add bounded transient
memory. Index saturation drops additional index entries while ordinary DATA accounting
continues. Known bytes whose complete metadata path cannot be resolved stay **unattributed**.

Each file retains at most 16 observed name links. A DOS alias is ignored when a Win32 or combined
Win32/DOS name for the same parent is available. Attribution chooses one resolvable observed
name deterministically by parent record/sequence, namespace preference, name and attribute
instance. It does not promise every hard-link path is present or a globally lexicographic path.
The file contributes once to the chosen path and each ancestor; an alternate hard link does not
inflate totals. Missing/stale parents, cycles, unsafe parent metadata, excessive depth and paths
longer than 512 UTF-8 bytes stay explicit. Resident attribute lists validate extension filename
membership before those names are admitted; unresolved lists or truncated aliases remain
partial. A live rename/replacement that reuses an observed path records a conflict and leaves
later conflicting file bytes unattributed.

`Collection.tree` returns at most 16 directory subtrees and 16 files, ranked by known allocated
DATA bytes and then known logical bytes. The root is omitted from the subtree list. These are
the largest entries of the **indexed, attributable subset**, not a claim of the largest entries
on the whole volume. Parent and child subtree amounts overlap and must not be summed. Top-N
truncation and index/collection gaps remain distinguishable.

Attributed and unattributed file counts and known byte amounts sum exactly to the aggregate
supported evidence. Per-item unknown counts remain explicit; a zero known subtotal does not
mean a file with unknown streams occupies zero bytes. If `allocation_additive` is false, all
tree allocation amounts become null rather than a false total or zero. `TreeSummary::validate`
checks those relationships, local path forms, item counts and a 48 KiB serialized limit, leaving
room inside the helper's 64 KiB whole-response bound.

Paths are volume-relative metadata for local exploration. They are neither authorization to
open/delete a file nor a model-ready payload. The host must assign opaque references and apply
C5 finite filename consent before projecting any selected item; raw profile paths never become
cloud evidence. No file contents are read by this index.

## Validation and remaining acceptance

Colocated synthetic fixtures exercise resident/nonresident/empty data, named streams, sparse
and compressed allocation, negative LCN deltas, holes, hard links, sequence/instance/VCN list
membership, extension gaps, duplicate/conflicting input, torn/restored transfer forms, invalid
attributes/run widths, overlapping allocation, changing witnesses, descending enumeration,
count/run/memory/time/cancellation bounds and closed-response invariants. A generated source
also streams 250,000 ordinary resident records without retaining their contents. These fixtures
never open a device, enumerate or modify the user's files. Workspace validation records belong
in [validation.md](validation.md); synthetic tests alone do not prove native-volume accuracy.

Before removing `native_accuracy_unvalidated`, use a dedicated disposable NTFS lab volume with
known resident/nonresident, named, sparse, compressed, hard-linked and extended-attribute cases.
Compare logical sizes and separately allocated DATA clusters to independent per-file metadata;
exclude volume overhead and shared hard-link multiplication in the expected result. Include a
controlled changing-volume scan, denied/non-NTFS targets, local cancellation and forced budget
limits. Preserve a traceable acceptance artifact and retain partial coverage for limitations
that still apply. Do not use the development system volume as the controlled fixture.

Tree fixtures additionally cover multiple hard links and DOS names, parent sequence replacement,
missing/cyclic parents, invalid filename metadata, overlapping parent/child rollups, bounded top-N
output, reserved parent capacity after index saturation, sparse/compressed amounts, unresolved
attribute lists, long/truncated/conflicting paths and closed local DTO invariants. A full
persistent treemap/file browser is not supplied by this bounded top-N response; it would require
a separately versioned local index with pagination and its own lifetime/revocation policy.
