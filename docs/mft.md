# Offline NTFS record accounting preparation

`pcagent_inspect::elevation::mft` is a portable parser of caller-supplied raw NTFS 3.0/3.1 FILE
record bytes. It does not open a file, volume or device, enumerate a machine, elevate, modify
data, decompress contents or register an LLM tool. Production `mft_inventory` remains
`unsupported / mft_accuracy_unvalidated`; this module does not change that gate or its protocol.
No volume accuracy or reclaim estimate is claimed.

## Input and bounds

The caller supplies an offline batch, each record's local number, and geometry from that same
snapshot. Record numbers and base references are used only inside the parser to join fragments.
Raw names, labels, serials, attribute names and resident contents are never decoded or returned.
The input and result have no serde implementations or cloud projection. The aggregate summary
contains counts, byte amounts and finite limitations only.

The supported geometry is NTFS 3.0/3.1, power-of-two FILE record sizes from 512 bytes to 16 KiB,
and power-of-two clusters from 512 bytes to 64 KiB. Volume size must fit a signed 64-bit byte
length. Larger/newer layouts are unsupported rather than approximated. A batch is bounded to
4,096 records and 32 MiB of raw input; each record permits at most 128 attributes, each decoded
run list at most 512 entries, and the batch at most 16,384 decoded runs. Length, offset, VCN/LCN,
cluster and total arithmetic is checked before slicing or accumulation. Signed NTFS lengths and
VCNs reject negative encodings; the sole highest-VCN sentinel is the empty first segment. Bounds fail the batch
instead of reporting a truncated batch as an exact total.

Input must still carry its on-disk update-sequence protection. All protected 512-byte tails are
checked before the saved words are restored in an owned bounded copy. Already restored records
are not the supported input form. Microsoft documents the FILE header and the 512-byte
update-sequence stride for NTFS 3.0/3.1; the count includes the sequence word and one saved word
per stride. The parser uses the on-disk word-count layout, also described in the original NTFS3
record structures. See [FILE_RECORD_SEGMENT_HEADER](https://learn.microsoft.com/en-us/windows/win32/devnotes/file-record-segment-header),
[MULTI_SECTOR_HEADER](https://learn.microsoft.com/en-us/windows/win32/devnotes/multi-sector-header)
and [NTFS3 v6.18 format structures](https://github.com/torvalds/linux/blob/v6.18/fs/ntfs3/ntfs.h).

## Accounting scope

Only the unnamed `$DATA` stream of ordinary in-use base files contributes. The first 16 system
records and directory data allocation are excluded explicitly. Named streams, reparse data and
encrypted data have explicit exclusions; unknown compression flags/units are unsupported.
Consequently these figures never describe total file storage, a whole-volume allocation or
reclaimable space. Every result retains `OfflineSubset`, `MetadataAllocationExcluded` and
`NativeAccuracyUnvalidated` limitations, even for a fully parsed fixture.

Resident logical bytes are separate from nonresident cluster bytes. A resident stream needs no
separate data clusters, but its contents occupy shared MFT record space; zero separate clusters
does not mean zero disk footprint. MFT/directory/index/security overhead is not attributed.
Missing data is counted as unknown, not a zero-length file. Zero-length resident and nonresident
streams are supported separately from missing streams.

Nonresident mapping pairs are checked against their declared VCN extent. Physical cluster runs
and sparse holes are distinguished; signed relative LCN deltas are checked against the supplied
volume bounds. Sparse holes retain the previous physical LCN; their virtual span may exceed the
volume's physical size. Logical size is not inferred from physical allocation. For a complete
supplied extent chain, actual mapped clusters must agree
with the first segment's allocation field (or extended total-allocation field for sparse and
compressed streams). Compressed streams with the supported unit encoding are counted by mapped
clusters, without decompressing data; compression-unit layout remains explicitly unvalidated.
The format fields are documented in [ATTRIBUTE_RECORD_HEADER](https://learn.microsoft.com/en-us/windows/win32/devnotes/attribute-record-header).
Signed LCN deltas, sparse run encoding and extended allocation checks were cross-checked against
the original [NTFS3 v6.18 run decoder](https://github.com/torvalds/linux/blob/v6.18/fs/ntfs3/run.c)
and [record validator](https://github.com/torvalds/linux/blob/v6.18/fs/ntfs3/record.c).
The Rust implementation is independently written; no driver implementation is incorporated.

## Deduplication and incomplete evidence

A repeated identical local record number is counted once, using a local SHA-256 byte fingerprint.
Conflicting bytes under that number fail the batch: this parser does not combine successive
snapshots. Multiple filename attributes and a hard-link count do not multiply file sizes.
Extensions join only a base record with the matching sequence reference. Missing/stale bases
remain explicit `MissingBase` coverage. Extension fragments contribute no second logical file.

Extent gaps retain a known first-segment logical size but leave full data allocation unknown;
overlapping VCN extents and multiple first segments (including empty ones) fail. Overlapping
physical allocations within one supported file or
between distinct fully accounted files fail instead of yielding an additive total. Orphaned
extension runs are not added. `$ATTRIBUTE_LIST` contents and membership are not resolved, so an
attribute list or any joined extension chain retains `AttributeListUnresolved` even when supplied
unnamed extents cover the declared allocation. This prevents treating a base reference as proof
of a complete filesystem inventory. Microsoft describes extension records and attribute-list
membership in [Master File Table](https://learn.microsoft.com/en-us/windows/win32/devnotes/master-file-table).

## Validation and next acceptance step

The colocated fifteen synthetic tests cover resident/nonresident/empty data, sparse/compressed
allocation, negative LCN deltas, holes, repeated records, hard links, matched/stale extension
references, missing extents, torn fixups, restored/truncated inputs, negative size/VCN fields, invalid attributes/run widths,
overlapping allocations, changed records, geometry, bounds and unsupported semantics. These
fixtures are byte arrays; they never inspect or mutate the user's filesystem or read a device.
All fifteen fixtures passed in the local Windows and Linux workspace test runs on 4 October.
They validate the stated offline subset and rejection rules; they do not establish native-volume
accuracy. Full validation and remaining acceptance limits are recorded in [validation.md](validation.md).

Before a usable native MFT task, a collector still needs controlled-volume acceptance, a safe
volume/read lifetime, `$ATTRIBUTE_LIST` and additional-stream handling, consistent-snapshot and
coverage rules, and comparisons against known resident, compressed, sparse and hard-linked
allocations. Collection belongs only in the separately approved helper. The current parser is
local offline preparation and must not be wired into host collection or cloud payloads as a
shortcut around that acceptance gate.
