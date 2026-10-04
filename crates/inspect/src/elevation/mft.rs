//! Bounded NTFS 3.0/3.1 FILE-record accounting. Only finite aggregates can be serialized.
//! Input must contain raw, un-restored 512-byte update-sequence strides from one snapshot.
//! References and filename metadata stay local; resident contents are never decoded/returned.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RECORDS: usize = 4096;
pub const MAX_RECORD_BYTES: usize = 16 * 1024;
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ATTRIBUTES: usize = 128;
pub const MAX_RUNS_PER_ATTRIBUTE: usize = 512;
pub const MAX_TOTAL_RUNS: usize = 16_384;
pub const MAX_ATTRIBUTE_LIST_ENTRIES: usize = 16_384;
const STRIDE: usize = 512;
const DATA: u32 = 0x80;

/// Geometry must come from the same offline snapshot; this parser never discovers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub ntfs_major: u8,
    pub ntfs_minor: u8,
    pub record_bytes: usize,
    pub cluster_bytes: u64,
    pub volume_clusters: u64,
}

/// The external record number is local-only and is not inferred from a filename.
pub struct Record<'a> {
    pub number: u64,
    pub bytes: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    UnsupportedGeometry,
    BoundExceeded,
    Truncated,
    InvalidHeader,
    FixupMismatch,
    InvalidAttribute,
    InvalidRun,
    ArithmeticOverflow,
    ConflictingRecord,
    OverlappingAllocation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Limit {
    OfflineSubset,
    NativeAccuracyUnvalidated,
    MetadataAllocationExcluded,
    SystemRecordsExcluded,
    DirectoryAllocationExcluded,
    NamedDataExcluded,
    ReparseDataExcluded,
    EncryptedDataExcluded,
    UnsupportedDataFlags,
    AttributeListUnresolved,
    MissingBase,
    MissingData,
    IncompleteExtents,
    CompressionLayoutUnvalidated,
    LiveVolumeNotAtomic,
    VolumeChanged,
    ChangeWitnessUnavailable,
    RecordLimit,
    MemoryLimit,
    DurationLimit,
    Cancelled,
    SourceUnavailable,
    InvalidRecord,
    RunLimit,
}

/// Finite aggregate of supported file streams. Not a volume total or reclaim estimate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub input_records: usize,
    pub duplicate_records: usize,
    pub extension_records: usize,
    pub base_files: usize,
    pub hard_linked_files: usize,
    pub resident_files: usize,
    pub sparse_files: usize,
    pub compressed_files: usize,
    pub logical_bytes_known: u64,
    pub data_cluster_bytes_known: u64,
    pub resident_logical_bytes: u64,
    pub unknown_logical_files: usize,
    pub unknown_data_allocation_files: usize,
    pub data_streams: usize,
    pub named_streams: usize,
    pub named_logical_bytes_known: u64,
    pub unknown_logical_streams: usize,
    pub unknown_data_allocation_streams: usize,
    pub orphan_extension_groups: usize,
    pub limitations: BTreeSet<Limit>,
}

impl Summary {
    pub(crate) fn validate(&self) -> Result<(), ParseError> {
        let unique = self
            .input_records
            .checked_sub(self.duplicate_records)
            .ok_or(ParseError::InvalidHeader)?;
        let counted = self
            .base_files
            .checked_add(self.extension_records)
            .ok_or(ParseError::ArithmeticOverflow)?;
        let max_streams = unique
            .checked_mul(MAX_ATTRIBUTE_LIST_ENTRIES)
            .ok_or(ParseError::ArithmeticOverflow)?;
        let max_unknown_streams = self
            .data_streams
            .checked_add(self.base_files)
            .ok_or(ParseError::ArithmeticOverflow)?;
        if self.input_records > collection::MAX_NATIVE_RECORDS
            || counted > unique
            || self.hard_linked_files > self.base_files
            || self.resident_files > self.base_files
            || self.sparse_files > self.base_files
            || self.compressed_files > self.base_files
            || self.unknown_logical_files > self.base_files
            || self.unknown_data_allocation_files > self.base_files
            || self.named_streams > self.data_streams
            || self.data_streams > max_streams
            || (self.base_files == 0
                && (self.data_streams != 0
                    || self.logical_bytes_known != 0
                    || self.data_cluster_bytes_known != 0))
            || (self.data_streams == 0
                && (self.logical_bytes_known != 0 || self.data_cluster_bytes_known != 0))
            || (self.resident_files == 0 && self.resident_logical_bytes != 0)
            || (self.named_streams == 0 && self.named_logical_bytes_known != 0)
            || self.unknown_logical_streams > max_unknown_streams
            || self.unknown_data_allocation_streams > max_unknown_streams
            || self.unknown_logical_files > self.unknown_logical_streams
            || self.unknown_data_allocation_files > self.unknown_data_allocation_streams
            || (self.unknown_logical_files == 0 && self.unknown_logical_streams != 0)
            || (self.unknown_data_allocation_files == 0
                && self.unknown_data_allocation_streams != 0)
            || self.orphan_extension_groups > self.extension_records
            || self.resident_logical_bytes > self.logical_bytes_known
            || self.named_logical_bytes_known > self.logical_bytes_known
        {
            return Err(ParseError::InvalidAttribute);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key(u64, u16);

/// Attribute names are compared locally without decoding or retaining their text.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AttributeKey {
    kind: u32,
    name: Option<[u8; 32]>,
    start: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AttributeIdentity {
    key: AttributeKey,
    record: Key,
    instance: u16,
}

struct StreamFragment {
    identity: AttributeIdentity,
    data: Data,
}

#[derive(PartialEq, Eq)]
enum Data {
    Resident(u64),
    Nonresident {
        start: u64,
        end: u64,
        logical: Option<u64>,
        virtual_bytes: Option<u64>,
        physical_bytes: Option<u64>,
        sparse: bool,
        compressed: bool,
        runs: Vec<(u64, u64)>,
    },
}

struct Parsed {
    key: Key,
    base: Key,
    is_base: bool,
    directory: bool,
    links: u16,
    data: Vec<StreamFragment>,
    attributes: BTreeSet<AttributeIdentity>,
    attribute_list: Option<BTreeSet<AttributeIdentity>>,
    unresolved_list: bool,
    excluded: bool,
    limits: BTreeSet<Limit>,
    names: Vec<tree::NameLink>,
    tree_limits: BTreeSet<tree::TreeLimit>,
}

fn bytes(body: &[u8], at: usize, count: usize) -> Result<&[u8], ParseError> {
    let end = at
        .checked_add(count)
        .ok_or(ParseError::ArithmeticOverflow)?;
    body.get(at..end).ok_or(ParseError::Truncated)
}
fn word(body: &[u8], at: usize) -> Result<u16, ParseError> {
    Ok(u16::from_le_bytes(bytes(body, at, 2)?.try_into().unwrap()))
}
fn dword(body: &[u8], at: usize) -> Result<u32, ParseError> {
    Ok(u32::from_le_bytes(bytes(body, at, 4)?.try_into().unwrap()))
}
fn qword(body: &[u8], at: usize) -> Result<u64, ParseError> {
    Ok(u64::from_le_bytes(bytes(body, at, 8)?.try_into().unwrap()))
}
fn nonnegative_qword(body: &[u8], at: usize) -> Result<u64, ParseError> {
    let value = qword(body, at)?;
    if value > i64::MAX as u64 {
        return Err(ParseError::InvalidAttribute);
    }
    Ok(value)
}
fn add(left: u64, right: u64) -> Result<u64, ParseError> {
    left.checked_add(right)
        .ok_or(ParseError::ArithmeticOverflow)
}
fn mul(left: u64, right: u64) -> Result<u64, ParseError> {
    left.checked_mul(right)
        .ok_or(ParseError::ArithmeticOverflow)
}
fn unsigned(raw: &[u8]) -> u64 {
    let mut packed = [0; 8];
    packed[..raw.len()].copy_from_slice(raw);
    u64::from_le_bytes(packed)
}
fn signed(raw: &[u8]) -> i64 {
    let mut packed = [if raw[raw.len() - 1] & 0x80 == 0 {
        0
    } else {
        255
    }; 8];
    packed[..raw.len()].copy_from_slice(raw);
    i64::from_le_bytes(packed)
}

fn validate_geometry(g: Geometry) -> Result<(), ParseError> {
    if g.ntfs_major != 3
        || g.ntfs_minor > 1
        || !(STRIDE..=MAX_RECORD_BYTES).contains(&g.record_bytes)
        || !g.record_bytes.is_power_of_two()
        || !(512..=65_536).contains(&g.cluster_bytes)
        || !g.cluster_bytes.is_power_of_two()
        || g.volume_clusters == 0
        || g.volume_clusters > i64::MAX as u64
        || g.volume_clusters
            .checked_mul(g.cluster_bytes)
            .is_none_or(|bytes| bytes > i64::MAX as u64)
    {
        return Err(ParseError::UnsupportedGeometry);
    }
    Ok(())
}

fn restore(raw: &[u8], g: Geometry) -> Result<Vec<u8>, ParseError> {
    if raw.len() != g.record_bytes {
        return Err(ParseError::Truncated);
    }
    if bytes(raw, 0, 4)? != b"FILE" {
        return Err(ParseError::InvalidHeader);
    }
    let offset = usize::from(word(raw, 4)?);
    let count = usize::from(word(raw, 6)?);
    // The on-disk count includes the update-sequence word itself; stride is always 512.
    if count != raw.len() / STRIDE + 1
        || offset < 42
        || offset % 2 != 0
        || offset
            .checked_add(count * 2)
            .is_none_or(|end| end > STRIDE - 2)
    {
        return Err(ParseError::InvalidHeader);
    }
    let sequence = word(raw, offset)?;
    if sequence == 0 || sequence == u16::MAX {
        return Err(ParseError::InvalidHeader);
    }
    let mut restored = raw.to_vec();
    for index in 1..count {
        let tail = index * STRIDE - 2;
        if word(raw, tail)? != sequence {
            return Err(ParseError::FixupMismatch);
        }
        restored[tail..tail + 2].copy_from_slice(bytes(raw, offset + index * 2, 2)?);
    }
    Ok(restored)
}

/// Normalize only a kernel-returned FILE record, whose transfer fixups may already be applied.
/// Offline callers must continue using `summarize` with protected on-disk records.
#[cfg(any(windows, test))]
pub(crate) fn normalize_kernel_record(raw: &[u8], g: Geometry) -> Result<Vec<u8>, ParseError> {
    validate_geometry(g)?;
    if raw.len() != g.record_bytes || bytes(raw, 0, 4)? != b"FILE" {
        return Err(ParseError::InvalidHeader);
    }
    let offset = usize::from(word(raw, 4)?);
    let count = usize::from(word(raw, 6)?);
    if count != raw.len() / STRIDE + 1
        || offset < 42
        || offset % 2 != 0
        || offset
            .checked_add(count * 2)
            .is_none_or(|end| end > STRIDE - 2)
    {
        return Err(ParseError::InvalidHeader);
    }
    let sequence = word(raw, offset)?;
    if sequence == 0 || sequence == u16::MAX {
        return Err(ParseError::InvalidHeader);
    }
    let protected = (1..count).all(|i| word(raw, i * STRIDE - 2) == Ok(sequence));
    if protected {
        restore(raw, g)?;
        return Ok(raw.to_vec());
    }
    // Every tail must agree with its own saved word: mixed/torn transfers fail closed.
    if !(1..count).all(|i| word(raw, i * STRIDE - 2) == word(raw, offset + i * 2)) {
        return Err(ParseError::FixupMismatch);
    }
    let mut protected = raw.to_vec();
    for i in 1..count {
        protected[i * STRIDE - 2..i * STRIDE].copy_from_slice(&sequence.to_le_bytes());
    }
    Ok(protected)
}

fn attribute_name(
    attr: &[u8],
    at: usize,
    characters: usize,
) -> Result<Option<[u8; 32]>, ParseError> {
    if characters == 0 {
        return Ok(None);
    }
    Ok(Some(
        Sha256::digest(bytes(attr, at, characters * 2)?).into(),
    ))
}

fn attribute_list(raw: &[u8]) -> Result<BTreeSet<AttributeIdentity>, ParseError> {
    let mut entries = BTreeSet::new();
    let mut data_keys = BTreeSet::new();
    let mut at = 0_usize;
    while at < raw.len() {
        if entries.len() >= MAX_ATTRIBUTE_LIST_ENTRIES {
            return Err(ParseError::BoundExceeded);
        }
        let length = usize::from(word(raw, at + 4)?);
        if length < 26 || length % 8 != 0 {
            return Err(ParseError::InvalidAttribute);
        }
        let entry = bytes(raw, at, length)?;
        let name_at = usize::from(entry[7]);
        let name_len = usize::from(entry[6]);
        if name_len != 0 && (name_at < 26 || name_at % 2 != 0) {
            return Err(ParseError::InvalidAttribute);
        }
        let reference = qword(entry, 16)?;
        let key = AttributeKey {
            kind: dword(entry, 0)?,
            name: attribute_name(entry, name_at, name_len)?,
            start: nonnegative_qword(entry, 8)?,
        };
        let identity = AttributeIdentity {
            key,
            record: Key(reference & ((1_u64 << 48) - 1), (reference >> 48) as u16),
            instance: word(entry, 24)?,
        };
        // Multiple resident FILE_NAME attributes have the same type/name/VCN but distinct
        // instances (hard links/DOS names). Only DATA streams require a unique name/VCN key.
        if identity.record.1 == 0
            || (key.kind == DATA && !data_keys.insert(key))
            || !entries.insert(identity)
        {
            return Err(ParseError::InvalidAttribute);
        }
        at = at
            .checked_add(length)
            .ok_or(ParseError::ArithmeticOverflow)?;
    }
    Ok(entries)
}

fn runs(
    raw: &[u8],
    start: u64,
    end: u64,
    sparse_allowed: bool,
    g: Geometry,
    total_runs: &mut usize,
) -> Result<Vec<(u64, u64)>, ParseError> {
    let mut at = 0;
    let mut vcn = start;
    let mut previous_lcn = 0_i64;
    let mut physical = Vec::new();
    for run_index in 0..=MAX_RUNS_PER_ATTRIBUTE {
        let header = *raw.get(at).ok_or(ParseError::Truncated)?;
        at += 1;
        if header == 0 {
            if vcn != end {
                return Err(ParseError::InvalidRun);
            }
            return Ok(physical);
        }
        if run_index == MAX_RUNS_PER_ATTRIBUTE || *total_runs >= MAX_TOTAL_RUNS {
            return Err(ParseError::BoundExceeded);
        }
        *total_runs += 1;
        let width = usize::from(header & 15);
        let delta_width = usize::from(header >> 4);
        if !(1..=8).contains(&width) || delta_width > 8 {
            return Err(ParseError::InvalidRun);
        }
        let length = unsigned(bytes(raw, at, width)?);
        at += width;
        if length == 0 {
            return Err(ParseError::InvalidRun);
        }
        vcn = add(vcn, length)?;
        if vcn > end {
            return Err(ParseError::InvalidRun);
        }
        if delta_width == 0 {
            if !sparse_allowed {
                return Err(ParseError::InvalidRun);
            }
        } else {
            let delta = signed(bytes(raw, at, delta_width)?);
            at += delta_width;
            previous_lcn = previous_lcn
                .checked_add(delta)
                .ok_or(ParseError::ArithmeticOverflow)?;
            let lcn = u64::try_from(previous_lcn).map_err(|_| ParseError::InvalidRun)?;
            // Cluster zero/zero deltas are conservatively excluded from user data.
            if delta == 0 || lcn == 0 || add(lcn, length)? > g.volume_clusters {
                return Err(ParseError::InvalidRun);
            }
            physical.push((lcn, length));
        }
    }
    Err(ParseError::BoundExceeded)
}

fn parse(
    record: &Record<'_>,
    g: Geometry,
    total_runs: &mut usize,
) -> Result<Option<Parsed>, ParseError> {
    if record.number >= 1_u64 << 48 {
        return Err(ParseError::InvalidHeader);
    }
    let restored = restore(record.bytes, g)?;
    let body = restored.as_slice();
    let flags = word(body, 22)?;
    let sequence = word(body, 16)?;
    let used = usize::try_from(dword(body, 24)?).map_err(|_| ParseError::BoundExceeded)?;
    let available = usize::try_from(dword(body, 28)?).map_err(|_| ParseError::BoundExceeded)?;
    let mut at = usize::from(word(body, 20)?);
    let usa_end = usize::from(word(body, 4)?) + usize::from(word(body, 6)?) * 2;
    if available != g.record_bytes
        || used > available
        || used % 8 != 0
        || at < usa_end.max(48)
        || at % 8 != 0
        || at.checked_add(4).is_none_or(|end| end > used)
        || flags & !3 != 0
    {
        return Err(ParseError::InvalidHeader);
    }
    if flags & 1 == 0 {
        return Ok(None);
    }
    if sequence == 0 {
        return Err(ParseError::InvalidHeader);
    }
    let base = qword(body, 32)?;
    let key = Key(record.number, sequence);
    let mut out = Parsed {
        key,
        base: if base == 0 {
            key
        } else {
            Key(base & ((1_u64 << 48) - 1), (base >> 48) as u16)
        },
        is_base: base == 0,
        directory: flags & 2 != 0,
        links: word(body, 18)?,
        data: Vec::new(),
        attributes: BTreeSet::new(),
        attribute_list: None,
        unresolved_list: false,
        excluded: false,
        limits: BTreeSet::new(),
        names: Vec::new(),
        tree_limits: BTreeSet::new(),
    };
    let mut ids = BTreeSet::new();
    for _ in 0..=MAX_ATTRIBUTES {
        let kind = dword(bytes(body, 0, used)?, at)?;
        if kind == u32::MAX {
            return Ok(Some(out));
        }
        if ids.len() >= MAX_ATTRIBUTES {
            return Err(ParseError::BoundExceeded);
        }
        let length =
            usize::try_from(dword(body, at + 4)?).map_err(|_| ParseError::BoundExceeded)?;
        if length < 24 || length % 8 != 0 {
            return Err(ParseError::InvalidAttribute);
        }
        let attr = bytes(bytes(body, 0, used)?, at, length)?;
        let form = attr[8];
        let header_bytes = match form {
            0 => 24,
            1 => 64,
            _ => return Err(ParseError::InvalidAttribute),
        };
        if attr.len() < header_bytes || !ids.insert(word(attr, 14)?) {
            return Err(ParseError::InvalidAttribute);
        }
        let named = attr[9] != 0;
        let mut name_end = header_bytes;
        if named {
            let name_at = usize::from(word(attr, 10)?);
            let name_len = usize::from(attr[9]) * 2;
            if name_at < header_bytes || name_at % 2 != 0 {
                return Err(ParseError::InvalidAttribute);
            }
            bytes(attr, name_at, name_len)?;
            name_end = name_at + name_len;
        }
        let content_at = usize::from(word(attr, if form == 0 { 20 } else { 32 })?);
        if content_at < name_end {
            return Err(ParseError::InvalidAttribute);
        }
        if form == 0 {
            bytes(attr, content_at, dword(attr, 16)? as usize)?;
        } else {
            bytes(attr, content_at, 1)?;
        }
        let identity = AttributeIdentity {
            key: AttributeKey {
                kind,
                name: attribute_name(attr, usize::from(word(attr, 10)?), usize::from(attr[9]))?,
                start: if form == 0 {
                    0
                } else {
                    nonnegative_qword(attr, 16)?
                },
            },
            record: key,
            instance: word(attr, 14)?,
        };
        out.attributes.insert(identity);
        if kind == 0x30 {
            if form != 0 || named || word(attr, 12)? != 0 {
                out.tree_limits.insert(tree::TreeLimit::InvalidFileName);
            } else {
                match tree::parse_name(
                    bytes(attr, content_at, dword(attr, 16)? as usize)?,
                    identity,
                ) {
                    Ok(link) => {
                        if !tree::retain_link(&mut out.names, link) {
                            out.tree_limits.insert(tree::TreeLimit::NameLinksTruncated);
                        }
                    }
                    Err(limit) => {
                        out.tree_limits.insert(limit);
                    }
                }
            }
        }
        if kind == 0x20 {
            if out.attribute_list.is_some() || out.unresolved_list || named {
                return Err(ParseError::InvalidAttribute);
            }
            if form == 0 {
                let list = attribute_list(bytes(attr, content_at, dword(attr, 16)? as usize)?)?;
                // Empty lists do not establish extension membership.
                out.unresolved_list = list.is_empty();
                out.attribute_list = Some(list);
            } else {
                // No raw data reads: nonresident attribute-list metadata remains unavailable.
                out.unresolved_list = true;
            }
        }
        if kind == 0xC0 {
            out.excluded = true;
            out.limits.insert(Limit::ReparseDataExcluded);
        }
        if kind == DATA {
            if out
                .data
                .iter()
                .any(|fragment| fragment.identity.key == identity.key)
            {
                return Err(ParseError::InvalidAttribute);
            }
            let attr_flags = word(attr, 12)?;
            if attr_flags & 0x4000 != 0 {
                out.excluded = true;
                out.limits.insert(Limit::EncryptedDataExcluded);
            } else if attr_flags & !0x8001 != 0 || (form == 0 && attr_flags != 0) {
                out.excluded = true;
                out.limits.insert(Limit::UnsupportedDataFlags);
            } else if form == 0 {
                out.data.push(StreamFragment {
                    identity,
                    data: Data::Resident(u64::from(dword(attr, 16)?)),
                });
            } else {
                let start = nonnegative_qword(attr, 16)?;
                let highest = qword(attr, 24)?;
                let end = if start == 0 && highest == u64::MAX {
                    0
                } else {
                    add(nonnegative_qword(attr, 24)?, 1)?
                };
                if end < start {
                    return Err(ParseError::InvalidAttribute);
                }
                let sparse = attr_flags & 0x8000 != 0;
                let compressed = attr_flags & 1 != 0;
                let unit = word(attr, 34)?;
                if (!compressed && unit != 0) || (compressed && unit != 4) {
                    out.excluded = true;
                    out.limits.insert(Limit::UnsupportedDataFlags);
                } else {
                    let extended = start == 0 && (sparse || compressed);
                    if extended && content_at < 72 {
                        return Err(ParseError::InvalidAttribute);
                    }
                    let logical = (start == 0)
                        .then(|| nonnegative_qword(attr, 48))
                        .transpose()?;
                    let virtual_bytes = (start == 0)
                        .then(|| nonnegative_qword(attr, 40))
                        .transpose()?;
                    let physical_bytes = (start == 0)
                        .then(|| nonnegative_qword(attr, if extended { 64 } else { 40 }))
                        .transpose()?;
                    if let (Some(logical), Some(virtual_bytes), Some(physical_bytes)) =
                        (logical, virtual_bytes, physical_bytes)
                    {
                        if nonnegative_qword(attr, 56)? > logical
                            || logical > virtual_bytes
                            || physical_bytes > virtual_bytes
                            || virtual_bytes % g.cluster_bytes != 0
                            || physical_bytes % g.cluster_bytes != 0
                            || physical_bytes > mul(g.volume_clusters, g.cluster_bytes)?
                        {
                            return Err(ParseError::InvalidAttribute);
                        }
                    }
                    let physical = runs(
                        &attr[content_at..],
                        start,
                        end,
                        sparse || compressed,
                        g,
                        total_runs,
                    )?;
                    out.data.push(StreamFragment {
                        identity,
                        data: Data::Nonresident {
                            start,
                            end,
                            logical,
                            virtual_bytes,
                            physical_bytes,
                            sparse,
                            compressed,
                            runs: physical,
                        },
                    });
                    if compressed {
                        out.limits.insert(Limit::CompressionLayoutUnvalidated);
                    }
                }
            }
        }
        at = at
            .checked_add(length)
            .ok_or(ParseError::ArithmeticOverflow)?;
    }
    Err(ParseError::BoundExceeded)
}

fn allocation(ranges: &mut [(u64, u64)], cluster_bytes: u64) -> Result<u64, ParseError> {
    ranges.sort_unstable();
    let mut previous_end = 0;
    let mut count = 0;
    for &(start, length) in ranges.iter() {
        if start < previous_end {
            return Err(ParseError::OverlappingAllocation);
        }
        previous_end = add(start, length)?;
        count = add(count, length)?;
    }
    mul(count, cluster_bytes)
}

/// Parse only supplied raw records. No file/volume/network operations or model projection.
/// Malformed input fails the whole batch; incomplete supported evidence stays explicitly partial.
pub fn summarize(records: &[Record<'_>], g: Geometry) -> Result<Summary, ParseError> {
    validate_geometry(g)?;
    if records.len() > MAX_RECORDS {
        return Err(ParseError::BoundExceeded);
    }
    let input_bytes = records.iter().try_fold(0_usize, |total, record| {
        total
            .checked_add(record.bytes.len())
            .ok_or(ParseError::BoundExceeded)
    })?;
    if input_bytes > MAX_INPUT_BYTES {
        return Err(ParseError::BoundExceeded);
    }
    let mut summary = Summary {
        input_records: records.len(),
        ..Summary::default()
    };
    summary.limitations.extend([
        Limit::OfflineSubset,
        Limit::MetadataAllocationExcluded,
        Limit::NativeAccuracyUnvalidated,
    ]);
    let mut seen = BTreeMap::<u64, [u8; 32]>::new();
    let mut groups = BTreeMap::<Key, Vec<Parsed>>::new();
    let mut total_runs = 0;
    for record in records {
        let digest: [u8; 32] = Sha256::digest(record.bytes).into();
        if let Some(previous) = seen.insert(record.number, digest) {
            if previous != digest {
                return Err(ParseError::ConflictingRecord);
            }
            summary.duplicate_records += 1;
            continue;
        }
        let Some(parsed) = parse(record, g, &mut total_runs)? else {
            continue;
        };
        if record.number < 16 {
            summary.limitations.insert(Limit::SystemRecordsExcluded);
            continue;
        }
        summary.limitations.extend(&parsed.limits);
        if !parsed.is_base {
            summary.extension_records += 1;
        }
        groups.entry(parsed.base).or_default().push(parsed);
    }
    let mut all_physical = Vec::new();
    for (key, group) in groups {
        account_group(key, group, &mut summary, &mut all_physical, g)?;
    }
    // Overlapping runs across files/streams never establish additive allocation.
    allocation(&mut all_physical, g.cluster_bytes)?;
    Ok(summary)
}

fn account_group(
    key: Key,
    mut group: Vec<Parsed>,
    summary: &mut Summary,
    all_physical: &mut Vec<(u64, u64)>,
    g: Geometry,
) -> Result<(), ParseError> {
    let Some(base_index) = group
        .iter()
        .position(|record| record.is_base && record.key == key)
    else {
        summary.limitations.insert(Limit::MissingBase);
        summary.orphan_extension_groups += 1;
        return Ok(());
    };
    let base = group.remove(base_index);
    if base.directory {
        summary
            .limitations
            .insert(Limit::DirectoryAllocationExcluded);
        return Ok(());
    }
    summary.base_files += 1;
    summary.hard_linked_files += usize::from(base.links > 1);
    if base.excluded || group.iter().any(|r| r.excluded || r.directory) {
        summary.unknown_logical_files += 1;
        summary.unknown_data_allocation_files += 1;
        summary.unknown_logical_streams += 1;
        summary.unknown_data_allocation_streams += 1;
        return Ok(());
    }
    let unresolved_list = base.unresolved_list
        || group
            .iter()
            .any(|r| r.unresolved_list || r.attribute_list.is_some())
        || (!group.is_empty() && base.attribute_list.is_none());
    let expected = base.attribute_list;
    let mut streams = BTreeMap::<Option<[u8; 32]>, Vec<StreamFragment>>::new();
    for fragment in base
        .data
        .into_iter()
        .chain(group.into_iter().flat_map(|r| r.data))
    {
        streams
            .entry(fragment.identity.key.name)
            .or_default()
            .push(fragment);
    }
    if let Some(expected) = &expected {
        for identity in expected.iter().filter(|identity| identity.key.kind == DATA) {
            streams.entry(identity.key.name).or_default();
        }
    }
    if streams.is_empty() {
        summary.limitations.insert(Limit::MissingData);
        summary.unknown_logical_files += 1;
        summary.unknown_data_allocation_files += 1;
        summary.unknown_logical_streams += 1;
        summary.unknown_data_allocation_streams += 1;
        return Ok(());
    }
    let mut unknown_logical = false;
    let mut unknown_allocation = false;
    let mut resident_file = false;
    let mut sparse_file = false;
    let mut compressed_file = false;
    for (name, mut fragments) in streams {
        summary.data_streams += 1;
        summary.named_streams += usize::from(name.is_some());
        let membership = !unresolved_list
            && expected.as_ref().is_none_or(|expected| {
                let listed: BTreeSet<_> = expected
                    .iter()
                    .filter(|identity| identity.key.kind == DATA && identity.key.name == name)
                    .copied()
                    .collect();
                let observed: BTreeSet<_> =
                    fragments.iter().map(|fragment| fragment.identity).collect();
                !listed.is_empty() && listed == observed
            });
        if !membership {
            summary.limitations.insert(Limit::AttributeListUnresolved);
        }
        let resident = fragments
            .iter()
            .filter(|fragment| matches!(fragment.data, Data::Resident(_)))
            .count();
        if resident != 0 {
            if resident != 1 || fragments.len() != 1 {
                return Err(ParseError::InvalidAttribute);
            }
            let Data::Resident(logical) = fragments[0].data else {
                unreachable!()
            };
            if fragments[0].identity.record != key && !membership {
                summary.unknown_logical_streams += 1;
                summary.unknown_data_allocation_streams += 1;
                unknown_logical = true;
                unknown_allocation = true;
                continue;
            }
            resident_file = true;
            summary.resident_logical_bytes = add(summary.resident_logical_bytes, logical)?;
            summary.logical_bytes_known = add(summary.logical_bytes_known, logical)?;
            if name.is_some() {
                summary.named_logical_bytes_known =
                    add(summary.named_logical_bytes_known, logical)?;
            }
            if !membership {
                summary.unknown_data_allocation_streams += 1;
                unknown_allocation = true;
            }
            continue;
        }
        if fragments
            .iter()
            .filter(|fragment| matches!(fragment.data, Data::Nonresident { start: 0, .. }))
            .count()
            > 1
        {
            return Err(ParseError::InvalidAttribute);
        }
        fragments.sort_by_key(|fragment| fragment.identity.key.start);
        let Some(first) = fragments.first() else {
            summary.limitations.insert(Limit::MissingData);
            summary.unknown_logical_streams += 1;
            summary.unknown_data_allocation_streams += 1;
            unknown_logical = true;
            unknown_allocation = true;
            continue;
        };
        let Data::Nonresident {
            logical: Some(logical),
            virtual_bytes: Some(virtual_bytes),
            physical_bytes: Some(declared_physical),
            sparse,
            compressed,
            ..
        } = &first.data
        else {
            summary.limitations.insert(Limit::IncompleteExtents);
            summary.unknown_logical_streams += 1;
            summary.unknown_data_allocation_streams += 1;
            unknown_logical = true;
            unknown_allocation = true;
            continue;
        };
        let (logical, virtual_bytes, declared_physical, sparse, compressed) = (
            *logical,
            *virtual_bytes,
            *declared_physical,
            *sparse,
            *compressed,
        );
        if first.identity.record != key && !membership {
            summary.unknown_logical_streams += 1;
            summary.unknown_data_allocation_streams += 1;
            unknown_logical = true;
            unknown_allocation = true;
            continue;
        }
        summary.logical_bytes_known = add(summary.logical_bytes_known, logical)?;
        if name.is_some() {
            summary.named_logical_bytes_known = add(summary.named_logical_bytes_known, logical)?;
        }
        sparse_file |= sparse;
        compressed_file |= compressed;
        let mut end_vcn = 0;
        let mut coherent = membership;
        let mut physical = Vec::new();
        for fragment in fragments {
            let Data::Nonresident {
                start,
                end,
                sparse: fragment_sparse,
                compressed: fragment_compressed,
                runs,
                ..
            } = fragment.data
            else {
                unreachable!()
            };
            if start < end_vcn {
                return Err(ParseError::InvalidRun);
            }
            coherent &=
                start == end_vcn && sparse == fragment_sparse && compressed == fragment_compressed;
            end_vcn = end;
            physical.extend(runs);
        }
        let known = allocation(&mut physical, g.cluster_bytes)?;
        if !coherent || mul(end_vcn, g.cluster_bytes)? != virtual_bytes {
            summary.limitations.insert(Limit::IncompleteExtents);
            summary.unknown_data_allocation_streams += 1;
            unknown_allocation = true;
            continue;
        }
        if known != declared_physical {
            return Err(ParseError::InvalidAttribute);
        }
        summary.data_cluster_bytes_known = add(summary.data_cluster_bytes_known, known)?;
        all_physical.extend(physical);
    }
    summary.resident_files += usize::from(resident_file);
    summary.sparse_files += usize::from(sparse_file);
    summary.compressed_files += usize::from(compressed_file);
    summary.unknown_logical_files += usize::from(unknown_logical);
    summary.unknown_data_allocation_files += usize::from(unknown_allocation);
    Ok(())
}

#[path = "mft_tree.rs"]
mod tree;
pub use tree::{
    TreeItem, TreeLimit, TreeSummary, MAX_TREE_ITEMS, MAX_TREE_PATH_BYTES, MAX_TREE_RESPONSE_BYTES,
};

#[path = "mft_collection.rs"]
mod collection;
pub use collection::{
    collect, CollectError, Collection, CollectionBounds, CollectionEnd, JournalWitness, RecordData,
    Source, VolumeSnapshot,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn put16(raw: &mut [u8], at: usize, value: u16) {
        raw[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn put32(raw: &mut [u8], at: usize, value: u32) {
        raw[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn put64(raw: &mut [u8], at: usize, value: u64) {
        raw[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn align(value: usize) -> usize {
        (value + 7) & !7
    }
    pub(super) fn geometry(record_bytes: usize) -> Geometry {
        Geometry {
            ntfs_major: 3,
            ntfs_minor: 1,
            record_bytes,
            cluster_bytes: 4096,
            volume_clusters: 1000,
        }
    }
    pub(super) fn resident(kind: u32, size: usize, id: u16) -> Vec<u8> {
        let mut attr = vec![0; align(24 + size)];
        let length = attr.len() as u32;
        put32(&mut attr, 0, kind);
        put32(&mut attr, 4, length);
        put16(&mut attr, 14, id);
        put32(&mut attr, 16, size as u32);
        put16(&mut attr, 20, 24);
        attr
    }
    pub(super) fn nonresident(
        start: u64,
        span: u64,
        logical: u64,
        virtual_bytes: u64,
        physical: u64,
        flags: u16,
        runs: &[u8],
    ) -> Vec<u8> {
        let header = if start == 0 && flags & 0x8001 != 0 {
            72
        } else {
            64
        };
        let mut attr = vec![0; align(header + runs.len())];
        let length = attr.len() as u32;
        put32(&mut attr, 0, DATA);
        put32(&mut attr, 4, length);
        attr[8] = 1;
        put16(&mut attr, 12, flags);
        put64(&mut attr, 16, start);
        put64(
            &mut attr,
            24,
            if span == 0 {
                u64::MAX
            } else {
                start + span - 1
            },
        );
        put16(&mut attr, 32, header as u16);
        if flags & 1 != 0 {
            put16(&mut attr, 34, 4);
        }
        put64(&mut attr, 40, virtual_bytes);
        put64(&mut attr, 48, logical);
        put64(&mut attr, 56, logical);
        if header == 72 {
            put64(&mut attr, 64, physical);
        }
        attr[header..header + runs.len()].copy_from_slice(runs);
        attr
    }
    fn protect(raw: &mut [u8]) {
        let offset = usize::from(word(raw, 4).unwrap());
        let count = raw.len() / STRIDE + 1;
        put16(raw, offset, 0x1234);
        for index in 1..count {
            let tail = index * STRIDE - 2;
            let saved = word(raw, tail).unwrap();
            put16(raw, offset + index * 2, saved);
            put16(raw, tail, 0x1234);
        }
    }
    pub(super) fn record(
        size: usize,
        sequence: u16,
        base: u64,
        links: u16,
        attrs: &[Vec<u8>],
    ) -> Vec<u8> {
        let mut raw = vec![0; size];
        raw[..4].copy_from_slice(b"FILE");
        put16(&mut raw, 4, 48);
        put16(&mut raw, 6, (size / STRIDE + 1) as u16);
        put16(&mut raw, 16, sequence);
        put16(&mut raw, 18, links);
        let first = align(48 + (size / STRIDE + 1) * 2);
        put16(&mut raw, 20, first as u16);
        put16(&mut raw, 22, 1);
        put32(&mut raw, 28, size as u32);
        put64(&mut raw, 32, base);
        let mut at = first;
        for attr in attrs {
            raw[at..at + attr.len()].copy_from_slice(attr);
            at += attr.len();
        }
        put32(&mut raw, at, u32::MAX);
        put32(&mut raw, 24, align(at + 4) as u32);
        protect(&mut raw);
        raw
    }
    fn one(raw: &[u8]) -> Result<Summary, ParseError> {
        summarize(
            &[Record {
                number: 42,
                bytes: raw,
            }],
            geometry(raw.len()),
        )
    }

    #[test]
    fn empty_default_stream_and_unsupported_flags_preserve_unknowns() {
        let empty = record(1024, 1, 0, 1, &[nonresident(0, 0, 0, 0, 0, 0, &[0])]);
        let summary = one(&empty).unwrap();
        assert_eq!(summary.base_files, 1);
        assert_eq!(summary.logical_bytes_known, 0);
        assert_eq!(summary.data_cluster_bytes_known, 0);
        assert_eq!(summary.unknown_data_allocation_files, 0);
        let encrypted = record(
            1024,
            1,
            0,
            1,
            &[nonresident(0, 1, 1, 4096, 4096, 0x4000, &[0x11, 1, 10, 0])],
        );
        let summary = one(&encrypted).unwrap();
        assert!(summary.limitations.contains(&Limit::EncryptedDataExcluded));
        assert_eq!(summary.unknown_logical_files, 1);
        let mut unsupported =
            nonresident(0, 16, 60000, 65536, 8192, 1, &[0x11, 2, 10, 0x01, 14, 0]);
        put16(&mut unsupported, 34, 3);
        let summary = one(&record(1024, 1, 0, 1, &[unsupported])).unwrap();
        assert!(summary.limitations.contains(&Limit::UnsupportedDataFlags));
        assert_eq!(summary.unknown_data_allocation_files, 1);
        let mut invalid = resident(DATA, 8, 0);
        put16(&mut invalid, 20, 10);
        assert_eq!(
            one(&record(1024, 1, 0, 1, &[invalid])),
            Err(ParseError::InvalidAttribute)
        );
    }

    #[test]
    fn resident_is_logical_data_without_separate_cluster_or_metadata_allocation() {
        let raw = record(1024, 1, 0, 1, &[resident(DATA, 11, 0)]);
        let summary = one(&raw).unwrap();
        assert_eq!(summary.logical_bytes_known, 11);
        assert_eq!(summary.resident_logical_bytes, 11);
        assert_eq!(summary.data_cluster_bytes_known, 0);
        assert_eq!(summary.resident_files, 1);
        assert!(summary
            .limitations
            .contains(&Limit::MetadataAllocationExcluded));
        assert!(summary
            .limitations
            .contains(&Limit::NativeAccuracyUnvalidated));
    }

    #[test]
    fn ordinary_sparse_and_compressed_allocation_are_distinct() {
        let ordinary = record(
            1024,
            1,
            0,
            1,
            &[nonresident(0, 2, 5000, 8192, 8192, 0, &[0x11, 2, 10, 0])],
        );
        assert_eq!(one(&ordinary).unwrap().data_cluster_bytes_known, 8192);
        let sparse = record(
            1024,
            1,
            0,
            1,
            &[nonresident(
                0,
                3,
                10000,
                12288,
                4096,
                0x8000,
                &[0x11, 1, 10, 0x01, 2, 0],
            )],
        );
        let summary = one(&sparse).unwrap();
        assert_eq!(summary.logical_bytes_known, 10000);
        assert_eq!(summary.data_cluster_bytes_known, 4096);
        assert_eq!(summary.sparse_files, 1);
        let compressed = record(
            1024,
            1,
            0,
            1,
            &[nonresident(
                0,
                16,
                60000,
                65536,
                8192,
                1,
                &[0x11, 2, 10, 0x01, 14, 0],
            )],
        );
        let summary = one(&compressed).unwrap();
        assert_eq!(summary.data_cluster_bytes_known, 8192);
        assert_eq!(summary.compressed_files, 1);
        assert!(summary
            .limitations
            .contains(&Limit::CompressionLayoutUnvalidated));
    }

    #[test]
    fn sparse_virtual_span_can_exceed_the_volume_with_bounded_physical_runs() {
        let span = 17 * 4096;
        let raw = record(
            1024,
            1,
            0,
            1,
            &[nonresident(
                0,
                17,
                span - 1,
                span,
                4096,
                0x8000,
                &[0x11, 1, 10, 0x01, 16, 0],
            )],
        );
        let mut small_volume = geometry(1024);
        small_volume.volume_clusters = 16;
        let summary = summarize(
            &[Record {
                number: 42,
                bytes: &raw,
            }],
            small_volume,
        )
        .unwrap();
        assert_eq!(summary.logical_bytes_known, span - 1);
        assert_eq!(summary.data_cluster_bytes_known, 4096);
        assert_eq!(summary.unknown_data_allocation_files, 0);
        assert_eq!(summary.sparse_files, 1);
    }

    #[test]
    fn a_zero_length_head_cannot_hide_a_conflicting_zero_vcn_extension() {
        let head = record(1024, 3, 0, 1, &[nonresident(0, 0, 0, 0, 0, 0, &[0])]);
        let extension = record(
            1024,
            8,
            42 | (3_u64 << 48),
            0,
            &[nonresident(0, 1, 1, 4096, 4096, 0, &[0x11, 1, 10, 0])],
        );
        assert_eq!(
            summarize(
                &[
                    Record {
                        number: 42,
                        bytes: &head,
                    },
                    Record {
                        number: 43,
                        bytes: &extension,
                    },
                ],
                geometry(1024),
            ),
            Err(ParseError::InvalidAttribute)
        );
    }

    #[test]
    fn signed_negative_sizes_and_vcns_are_not_unsigned_allocation_evidence() {
        let invalid_large_sparse = record(
            1024,
            1,
            0,
            1,
            &[nonresident(
                0,
                1_u64 << 51,
                1_u64 << 63,
                1_u64 << 63,
                4096,
                0x8000,
                &[
                    0x11, 1, 10, 0x07, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x07, 0,
                ],
            )],
        );
        assert_eq!(
            one(&invalid_large_sparse),
            Err(ParseError::InvalidAttribute)
        );
        for field in [16, 24, 40, 48, 56, 64] {
            let mut invalid = nonresident(0, 1, 1, 4096, 4096, 0x8000, &[0x11, 1, 10, 0]);
            put64(&mut invalid, field, 1_u64 << 63);
            assert_eq!(
                one(&record(1024, 1, 0, 1, &[invalid])),
                Err(ParseError::InvalidAttribute),
                "negative signed field at {field}"
            );
        }
    }

    #[test]
    fn negative_relative_lcn_is_signed_and_sparse_does_not_reset_it() {
        let raw = record(
            1024,
            1,
            0,
            1,
            &[nonresident(
                0,
                4,
                16000,
                16384,
                12288,
                0x8000,
                &[0x11, 1, 20, 0x01, 1, 0x11, 2, 0xf6, 0],
            )],
        );
        assert_eq!(one(&raw).unwrap().data_cluster_bytes_known, 12288);
    }

    #[test]
    fn hard_links_and_repeated_records_do_not_multiply_file_sizes() {
        let raw = record(
            1024,
            3,
            0,
            2,
            &[
                resident(0x30, 70, 1),
                resident(0x30, 70, 2),
                resident(DATA, 41, 3),
            ],
        );
        let summary = summarize(
            &[
                Record {
                    number: 42,
                    bytes: &raw,
                },
                Record {
                    number: 42,
                    bytes: &raw,
                },
            ],
            geometry(1024),
        )
        .unwrap();
        assert_eq!(summary.base_files, 1);
        assert_eq!(summary.hard_linked_files, 1);
        assert_eq!(summary.duplicate_records, 1);
        assert_eq!(summary.logical_bytes_known, 41);
        let debug = format!("{summary:?}");
        assert!(!debug.contains("1234"));
        assert!(!debug.contains("FILE"));
    }

    #[test]
    fn extension_fragments_share_one_base_size_and_validate_sequence() {
        let head = record(
            1024,
            3,
            0,
            2,
            &[
                resident(0x20, 0, 1),
                nonresident(0, 1, 7000, 8192, 8192, 0, &[0x11, 1, 10, 0]),
            ],
        );
        let extension = record(
            1024,
            8,
            42 | (3_u64 << 48),
            0,
            &[nonresident(1, 1, 0, 0, 0, 0, &[0x11, 1, 20, 0])],
        );
        let records = [
            Record {
                number: 43,
                bytes: &extension,
            },
            Record {
                number: 42,
                bytes: &head,
            },
            Record {
                number: 43,
                bytes: &extension,
            },
        ];
        let summary = summarize(&records, geometry(1024)).unwrap();
        assert_eq!(summary.base_files, 1);
        assert_eq!(summary.extension_records, 1);
        assert_eq!(summary.logical_bytes_known, 7000);
        assert_eq!(summary.data_cluster_bytes_known, 0);
        assert!(summary
            .limitations
            .contains(&Limit::AttributeListUnresolved));
        let stale = record(
            1024,
            8,
            42 | (4_u64 << 48),
            0,
            &[nonresident(1, 1, 0, 0, 0, 0, &[0x11, 1, 20, 0])],
        );
        let summary = summarize(
            &[
                Record {
                    number: 42,
                    bytes: &head,
                },
                Record {
                    number: 43,
                    bytes: &stale,
                },
            ],
            geometry(1024),
        )
        .unwrap();
        assert!(summary.limitations.contains(&Limit::MissingBase));
        assert_eq!(summary.unknown_data_allocation_files, 1);
        assert_eq!(summary.data_cluster_bytes_known, 0);
    }

    #[test]
    fn incomplete_extents_preserve_declared_logical_size_but_not_full_allocation() {
        let head = record(
            1024,
            3,
            0,
            1,
            &[nonresident(0, 1, 7000, 8192, 8192, 0, &[0x11, 1, 10, 0])],
        );
        let summary = one(&head).unwrap();
        assert_eq!(summary.logical_bytes_known, 7000);
        assert_eq!(summary.data_cluster_bytes_known, 0);
        assert_eq!(summary.unknown_data_allocation_files, 1);
        assert!(summary.limitations.contains(&Limit::IncompleteExtents));
    }

    #[test]
    fn torn_truncated_and_already_restored_records_are_rejected() {
        let raw = record(1024, 1, 0, 1, &[resident(DATA, 600, 0)]);
        assert_eq!(one(&raw).unwrap().logical_bytes_known, 600);
        let mut torn = raw.clone();
        torn[510] ^= 1;
        assert_eq!(one(&torn), Err(ParseError::FixupMismatch));
        let restored = restore(&raw, geometry(1024)).unwrap();
        assert_eq!(one(&restored), Err(ParseError::FixupMismatch));
        assert_eq!(
            summarize(
                &[Record {
                    number: 42,
                    bytes: &raw[..900]
                }],
                geometry(1024)
            ),
            Err(ParseError::Truncated)
        );
        let mut bad = raw.clone();
        put16(&mut bad, 6, 65535);
        assert_eq!(one(&bad), Err(ParseError::InvalidHeader));
    }

    #[test]
    fn malformed_attribute_lengths_and_run_widths_fail_closed() {
        let mut attr = resident(DATA, 10, 0);
        put32(&mut attr, 4, 0);
        assert_eq!(
            one(&record(1024, 1, 0, 1, &[attr])),
            Err(ParseError::InvalidAttribute)
        );
        for run in [
            vec![0x19, 1, 2, 0],
            vec![0x11, 0, 10, 0],
            vec![0x11, 1, 0xff, 0],
            vec![0x01, 1, 0],
        ] {
            let raw = record(1024, 1, 0, 1, &[nonresident(0, 1, 1, 4096, 4096, 0, &run)]);
            assert_eq!(one(&raw), Err(ParseError::InvalidRun));
        }
        let mut count = 0;
        assert_eq!(
            runs(&[0x11, 1], 0, 1, false, geometry(1024), &mut count),
            Err(ParseError::Truncated)
        );
        assert_eq!(
            runs(&[0x11, 1, 10], 0, 1, false, geometry(1024), &mut count),
            Err(ParseError::Truncated)
        );
    }

    #[test]
    fn overlapping_or_conflicting_snapshots_never_return_additive_totals() {
        let attr = nonresident(0, 1, 1, 4096, 4096, 0, &[0x11, 1, 10, 0]);
        let raw = record(1024, 1, 0, 1, &[attr]);
        assert_eq!(
            summarize(
                &[
                    Record {
                        number: 42,
                        bytes: &raw
                    },
                    Record {
                        number: 43,
                        bytes: &raw
                    }
                ],
                geometry(1024)
            ),
            Err(ParseError::OverlappingAllocation)
        );
        let changed = record(1024, 2, 0, 1, &[resident(DATA, 1, 0)]);
        assert_eq!(
            summarize(
                &[
                    Record {
                        number: 42,
                        bytes: &raw
                    },
                    Record {
                        number: 42,
                        bytes: &changed
                    }
                ],
                geometry(1024)
            ),
            Err(ParseError::ConflictingRecord)
        );
        let bad_size = record(
            1024,
            1,
            0,
            1,
            &[nonresident(0, 1, 1, 4096, 0, 0x8000, &[0x11, 1, 10, 0])],
        );
        assert_eq!(one(&bad_size), Err(ParseError::InvalidAttribute));
    }

    #[test]
    fn bounds_and_geometry_do_not_produce_partial_success_as_exact() {
        let raw = record(1024, 1, 0, 1, &[resident(DATA, 1, 0)]);
        let records: Vec<_> = (0..=MAX_RECORDS)
            .map(|n| Record {
                number: n as u64,
                bytes: raw.as_slice(),
            })
            .collect();
        assert_eq!(
            summarize(&records, geometry(1024)),
            Err(ParseError::BoundExceeded)
        );
        let attrs: Vec<_> = (0..=MAX_ATTRIBUTES)
            .map(|n| resident(0x10, 0, n as u16))
            .collect();
        assert_eq!(
            one(&record(4096, 1, 0, 1, &attrs)),
            Err(ParseError::BoundExceeded)
        );
        let mut stream = Vec::new();
        for _ in 0..=MAX_RUNS_PER_ATTRIBUTE {
            stream.extend([0x11, 1, 1]);
        }
        stream.push(0);
        let mut count = 0;
        assert_eq!(
            runs(&stream, 0, 513, false, geometry(1024), &mut count),
            Err(ParseError::BoundExceeded)
        );
        let mut count = MAX_TOTAL_RUNS;
        assert_eq!(
            runs(&[0x11, 1, 10, 0], 0, 1, false, geometry(1024), &mut count),
            Err(ParseError::BoundExceeded)
        );
        let mut g = geometry(1024);
        g.ntfs_major = 4;
        assert_eq!(summarize(&[], g), Err(ParseError::UnsupportedGeometry));
        assert_eq!(add(u64::MAX, 1), Err(ParseError::ArithmeticOverflow));
        assert_eq!(mul(u64::MAX, 2), Err(ParseError::ArithmeticOverflow));
    }

    #[test]
    fn unsupported_semantics_and_nonuser_records_are_explicit() {
        let raw = record(
            1024,
            1,
            0,
            1,
            &[resident(0xC0, 8, 1), resident(DATA, 10, 2)],
        );
        let summary = one(&raw).unwrap();
        assert_eq!(summary.logical_bytes_known, 0);
        assert_eq!(summary.unknown_logical_files, 1);
        assert!(summary.limitations.contains(&Limit::ReparseDataExcluded));
        let mut named = resident(DATA, 8, 0);
        named[9] = 1;
        put16(&mut named, 10, 24);
        put16(&mut named, 20, 26);
        put32(&mut named, 16, 6);
        let summary = one(&record(1024, 1, 0, 1, &[named])).unwrap();
        assert_eq!(summary.named_streams, 1);
        assert_eq!(summary.named_logical_bytes_known, 6);
        assert_eq!(summary.logical_bytes_known, 6);
        assert_eq!(summary.unknown_logical_files, 0);
        let plain = record(1024, 1, 0, 1, &[resident(DATA, 10, 0)]);
        let summary = summarize(
            &[Record {
                number: 5,
                bytes: &plain,
            }],
            geometry(1024),
        )
        .unwrap();
        assert_eq!(summary.base_files, 0);
        assert!(summary.limitations.contains(&Limit::SystemRecordsExcluded));
        let mut directory = plain;
        put16(&mut directory, 22, 3);
        let summary = one(&directory).unwrap();
        assert_eq!(summary.base_files, 0);
        assert!(summary
            .limitations
            .contains(&Limit::DirectoryAllocationExcluded));
    }

    fn named_resident(name: &[u16], size: usize, id: u16) -> Vec<u8> {
        let content = 24 + name.len() * 2;
        let mut attr = vec![0; align(content + size)];
        let length = attr.len() as u32;
        put32(&mut attr, 0, DATA);
        put32(&mut attr, 4, length);
        attr[9] = name.len() as u8;
        put16(&mut attr, 10, 24);
        put16(&mut attr, 14, id);
        put32(&mut attr, 16, size as u32);
        put16(&mut attr, 20, content as u16);
        for (i, character) in name.iter().enumerate() {
            put16(&mut attr, 24 + i * 2, *character);
        }
        attr
    }

    pub(super) fn listed_data(entries: &[(u64, u16, u64, u16)]) -> Vec<u8> {
        let mut attr = resident(0x20, entries.len() * 32, 9);
        for (i, &(number, sequence, start, instance)) in entries.iter().enumerate() {
            let at = 24 + i * 32;
            put32(&mut attr, at, DATA);
            put16(&mut attr, at + 4, 32);
            put64(&mut attr, at + 8, start);
            put64(&mut attr, at + 16, number | (u64::from(sequence) << 48));
            put16(&mut attr, at + 24, instance);
        }
        attr
    }

    #[test]
    fn named_streams_share_file_identity_without_leaking_names_or_resident_contents() {
        let name: Vec<_> = "private-stream-name".encode_utf16().collect();
        let mut named = named_resident(&name, 9, 2);
        let at = usize::from(word(&named, 20).unwrap());
        named[at..at + 9].copy_from_slice(b"secret123");
        let raw = record(1024, 3, 0, 2, &[resident(DATA, 11, 1), named]);
        let summary = one(&raw).unwrap();
        assert_eq!(summary.base_files, 1);
        assert_eq!(summary.hard_linked_files, 1);
        assert_eq!(summary.data_streams, 2);
        assert_eq!(summary.named_streams, 1);
        assert_eq!(summary.logical_bytes_known, 20);
        assert_eq!(summary.named_logical_bytes_known, 9);
        assert_eq!(summary.resident_files, 1);
        let output = format!("{summary:?}");
        assert!(!output.contains("private-stream-name"));
        assert!(!output.contains("secret123"));
        let duplicate = record(
            1024,
            3,
            0,
            1,
            &[named_resident(&name, 9, 1), named_resident(&name, 9, 2)],
        );
        assert_eq!(one(&duplicate), Err(ParseError::InvalidAttribute));
    }

    #[test]
    fn attribute_list_membership_checks_sequence_instance_and_lowest_vcn() {
        let list = listed_data(&[(42, 3, 0, 0), (43, 8, 1, 0)]);
        let head = record(
            1024,
            3,
            0,
            1,
            &[
                list.clone(),
                nonresident(0, 1, 7000, 8192, 8192, 0, &[0x11, 1, 10, 0]),
            ],
        );
        let extension = record(
            1024,
            8,
            42 | (3_u64 << 48),
            0,
            &[nonresident(1, 1, 0, 0, 0, 0, &[0x11, 1, 20, 0])],
        );
        let supplied = [
            Record {
                number: 42,
                bytes: &head,
            },
            Record {
                number: 43,
                bytes: &extension,
            },
        ];
        let summary = summarize(&supplied, geometry(1024)).unwrap();
        assert_eq!(summary.logical_bytes_known, 7000);
        assert_eq!(summary.data_cluster_bytes_known, 8192);
        assert!(!summary
            .limitations
            .contains(&Limit::AttributeListUnresolved));
        for entries in [
            vec![(42, 3, 0, 0), (43, 9, 1, 0)],
            vec![(42, 3, 0, 0), (43, 8, 1, 7)],
            vec![(42, 3, 0, 0), (43, 8, 2, 0)],
            vec![(42, 3, 0, 0), (43, 8, 1, 0), (44, 1, 2, 0)],
        ] {
            let inconsistent = record(
                1024,
                3,
                0,
                1,
                &[
                    listed_data(&entries),
                    nonresident(0, 1, 7000, 8192, 8192, 0, &[0x11, 1, 10, 0]),
                ],
            );
            let summary = summarize(
                &[
                    Record {
                        number: 42,
                        bytes: &inconsistent,
                    },
                    Record {
                        number: 43,
                        bytes: &extension,
                    },
                ],
                geometry(1024),
            )
            .unwrap();
            assert_eq!(summary.logical_bytes_known, 7000);
            assert_eq!(summary.data_cluster_bytes_known, 0);
            assert!(summary
                .limitations
                .contains(&Limit::AttributeListUnresolved));
            assert_eq!(summary.unknown_data_allocation_streams, 1);
        }
        let duplicate = listed_data(&[(42, 3, 0, 0), (43, 8, 0, 0)]);
        assert_eq!(
            one(&record(1024, 3, 0, 1, &[duplicate])),
            Err(ParseError::InvalidAttribute)
        );
    }

    #[test]
    fn attribute_lists_allow_distinct_resident_filename_instances_for_hard_links() {
        let mut list = listed_data(&[(42, 3, 0, 0), (42, 3, 0, 1), (42, 3, 0, 2)]);
        put32(&mut list, 24 + 32, 0x30);
        put32(&mut list, 24 + 64, 0x30);
        let raw = record(
            1024,
            3,
            0,
            2,
            &[
                list,
                resident(DATA, 11, 0),
                resident(0x30, 8, 1),
                resident(0x30, 8, 2),
            ],
        );
        let summary = one(&raw).unwrap();
        assert_eq!(summary.base_files, 1);
        assert_eq!(summary.hard_linked_files, 1);
        assert_eq!(summary.data_streams, 1);
        assert_eq!(summary.logical_bytes_known, 11);
        assert!(!summary
            .limitations
            .contains(&Limit::AttributeListUnresolved));
        summary.validate().unwrap();
    }

    #[test]
    fn named_nonresident_streams_contribute_distinct_supported_clusters() {
        let name: Vec<_> = "private-alternate-stream".encode_utf16().collect();
        let base = nonresident(0, 1, 200, 4096, 4096, 0, &[0x11, 1, 20, 0]);
        let content = align(64 + name.len() * 2);
        let mut named = vec![0; align(content + 4)];
        named[..64].copy_from_slice(&base[..64]);
        let length = named.len() as u32;
        put32(&mut named, 4, length);
        named[9] = name.len() as u8;
        put16(&mut named, 10, 64);
        put16(&mut named, 14, 2);
        put16(&mut named, 32, content as u16);
        for (i, character) in name.iter().enumerate() {
            put16(&mut named, 64 + i * 2, *character);
        }
        named[content..content + 4].copy_from_slice(&[0x11, 1, 20, 0]);
        let raw = record(
            1024,
            3,
            0,
            2,
            &[
                nonresident(0, 1, 100, 4096, 4096, 0, &[0x11, 1, 10, 0]),
                named,
            ],
        );
        let summary = one(&raw).unwrap();
        assert_eq!(summary.base_files, 1);
        assert_eq!(summary.data_streams, 2);
        assert_eq!(summary.named_streams, 1);
        assert_eq!(summary.logical_bytes_known, 300);
        assert_eq!(summary.named_logical_bytes_known, 200);
        assert_eq!(summary.data_cluster_bytes_known, 8192);
        let output = serde_json::to_string(&summary).unwrap();
        assert!(!output.contains("private-alternate-stream"));
        assert!(!output.contains("attribute_list"));
        summary.validate().unwrap();
    }

    #[test]
    fn kernel_normalization_accepts_uniform_forms_and_rejects_mixed_or_torn_tails() {
        let raw = record(1024, 1, 0, 1, &[resident(DATA, 600, 0)]);
        assert_eq!(normalize_kernel_record(&raw, geometry(1024)).unwrap(), raw);
        let restored = restore(&raw, geometry(1024)).unwrap();
        let normalized = normalize_kernel_record(&restored, geometry(1024)).unwrap();
        assert_eq!(one(&normalized).unwrap().logical_bytes_known, 600);
        assert_eq!(one(&restored), Err(ParseError::FixupMismatch));
        let mut mixed = restored;
        mixed[510..512].copy_from_slice(&raw[510..512]);
        assert_eq!(
            normalize_kernel_record(&mixed, geometry(1024)),
            Err(ParseError::FixupMismatch)
        );
        let mut torn = raw;
        torn[510] ^= 1;
        assert_eq!(
            normalize_kernel_record(&torn, geometry(1024)),
            Err(ParseError::FixupMismatch)
        );
    }
}
