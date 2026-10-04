//! Offline NTFS 3.0/3.1 FILE-record accounting, never a volume collector or cloud DTO.
//! Input must contain raw, un-restored 512-byte update-sequence strides from one snapshot.
//! References are used only locally; names and resident contents are never decoded/returned.
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_RECORDS: usize = 4096;
pub const MAX_RECORD_BYTES: usize = 16 * 1024;
pub const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ATTRIBUTES: usize = 128;
pub const MAX_RUNS_PER_ATTRIBUTE: usize = 512;
pub const MAX_TOTAL_RUNS: usize = 16_384;
const STRIDE: usize = 512;
const DATA: u32 = 0x80;

/// Geometry must come from the same offline snapshot; this parser never discovers it.
#[derive(Clone, Copy)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
}

/// Aggregate, non-serialized preparatory result. Not a volume total or reclaim estimate.
#[derive(Debug, Default, PartialEq, Eq)]
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
    pub limitations: BTreeSet<Limit>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key(u64, u16);

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
    data: Option<Data>,
    excluded: bool,
    limits: BTreeSet<Limit>,
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
        data: None,
        excluded: false,
        limits: BTreeSet::new(),
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
        if kind == 0x20 {
            out.limits.insert(Limit::AttributeListUnresolved);
        }
        if kind == 0xC0 {
            out.excluded = true;
            out.limits.insert(Limit::ReparseDataExcluded);
        }
        if kind == DATA && named {
            out.limits.insert(Limit::NamedDataExcluded);
        }
        if kind == DATA && !named {
            if out.data.is_some() {
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
                out.data = Some(Data::Resident(u64::from(dword(attr, 16)?)));
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
                    out.data = Some(Data::Nonresident {
                        start,
                        end,
                        logical,
                        virtual_bytes,
                        physical_bytes,
                        sparse,
                        compressed,
                        runs: physical,
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
    for (key, mut group) in groups {
        let Some(base_index) = group
            .iter()
            .position(|record| record.is_base && record.key == key)
        else {
            summary.limitations.insert(Limit::MissingBase);
            continue;
        };
        let base = group.remove(base_index);
        if !group.is_empty() {
            // Base references alone do not validate the on-disk ATTRIBUTE_LIST membership.
            summary.limitations.insert(Limit::AttributeListUnresolved);
        }
        if base.directory {
            summary
                .limitations
                .insert(Limit::DirectoryAllocationExcluded);
            continue;
        }
        summary.base_files += 1;
        summary.hard_linked_files += usize::from(base.links > 1);
        if base.excluded || group.iter().any(|r| r.excluded || r.directory) {
            summary.unknown_logical_files += 1;
            summary.unknown_data_allocation_files += 1;
            continue;
        }
        let mut data: Vec<Data> = base
            .data
            .into_iter()
            .chain(group.into_iter().filter_map(|r| r.data))
            .collect();
        if data.len() == 1 && matches!(data[0], Data::Resident(_)) {
            let Data::Resident(logical) = data[0] else {
                unreachable!()
            };
            summary.resident_files += 1;
            summary.resident_logical_bytes = add(summary.resident_logical_bytes, logical)?;
            summary.logical_bytes_known = add(summary.logical_bytes_known, logical)?;
            // No separate data clusters; shared MFT record allocation remains excluded.
            continue;
        }
        if data.is_empty() || data.iter().any(|d| matches!(d, Data::Resident(_))) {
            summary.limitations.insert(Limit::MissingData);
            summary.unknown_logical_files += 1;
            summary.unknown_data_allocation_files += 1;
            continue;
        }
        // Even an empty first segment cannot share the stream header with another head.
        if data
            .iter()
            .filter(|d| matches!(d, Data::Nonresident { start: 0, .. }))
            .count()
            > 1
        {
            return Err(ParseError::InvalidAttribute);
        }
        data.sort_by_key(|d| match d {
            Data::Nonresident { start, .. } => *start,
            _ => 0,
        });
        let Data::Nonresident {
            logical: Some(logical),
            virtual_bytes: Some(virtual_bytes),
            physical_bytes: Some(declared_physical),
            sparse,
            compressed,
            ..
        } = &data[0]
        else {
            summary.limitations.insert(Limit::IncompleteExtents);
            summary.unknown_logical_files += 1;
            summary.unknown_data_allocation_files += 1;
            continue;
        };
        let (logical, virtual_bytes, declared_physical, sparse, compressed) = (
            *logical,
            *virtual_bytes,
            *declared_physical,
            *sparse,
            *compressed,
        );
        summary.logical_bytes_known = add(summary.logical_bytes_known, logical)?;
        summary.sparse_files += usize::from(sparse);
        summary.compressed_files += usize::from(compressed);
        let mut end_vcn = 0;
        let mut coherent = true;
        let mut physical = Vec::new();
        for fragment in data {
            let Data::Nonresident {
                start,
                end,
                sparse: fragment_sparse,
                compressed: fragment_compressed,
                runs,
                ..
            } = fragment
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
            summary.unknown_data_allocation_files += 1;
            continue;
        }
        if known != declared_physical {
            return Err(ParseError::InvalidAttribute);
        }
        summary.data_cluster_bytes_known = add(summary.data_cluster_bytes_known, known)?;
        all_physical.extend(physical);
    }
    // Overlapping data runs across distinct files cannot establish additive allocation.
    allocation(&mut all_physical, g.cluster_bytes)?;
    Ok(summary)
}

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
    fn geometry(record_bytes: usize) -> Geometry {
        Geometry {
            ntfs_major: 3,
            ntfs_minor: 1,
            record_bytes,
            cluster_bytes: 4096,
            volume_clusters: 1000,
        }
    }
    fn resident(kind: u32, size: usize, id: u16) -> Vec<u8> {
        let mut attr = vec![0; align(24 + size)];
        let length = attr.len() as u32;
        put32(&mut attr, 0, kind);
        put32(&mut attr, 4, length);
        put16(&mut attr, 14, id);
        put32(&mut attr, 16, size as u32);
        put16(&mut attr, 20, 24);
        attr
    }
    fn nonresident(
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
    fn record(size: usize, sequence: u16, base: u64, links: u16, attrs: &[Vec<u8>]) -> Vec<u8> {
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
        assert_eq!(summary.data_cluster_bytes_known, 8192);
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
        assert!(summary.limitations.contains(&Limit::NamedDataExcluded));
        assert_eq!(summary.unknown_logical_files, 1);
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
}
