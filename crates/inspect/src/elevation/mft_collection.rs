//! Streaming accounting over a read-only source. Only finite aggregates leave this module.
//!
//! Memory bounds come from `tests::memory_*` and the tree's own measurements: an indexed file
//! costs about 75 accounted bytes (node, name, finish share) plus 16 per data run, a directory
//! about 115, so a 1.2 million record system volume needs roughly 100 MiB of the 256 MiB bound.
use super::memory::{btree_bytes, Chunked};
use super::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::mem::size_of;
use std::time::{Duration, Instant};

/// Records examined per collection (the memory bound admits about this many at ~64 B each).
pub const MAX_NATIVE_RECORDS: usize = 4_000_000;
/// Accounted metadata: index with its finish working memory, run list and pending groups.
pub const MAX_RETAINED_BYTES: usize = 256 * 1024 * 1024;
/// Decoded data runs kept to check that allocations do not overlap (16 bytes each).
pub const MAX_NATIVE_RUNS: usize = 4_000_000;
/// Part of the duration kept for joining, checking and summarizing what the walk read.
pub const MAX_FINISH_RESERVE: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectError {
    InvalidBounds,
    Unsupported,
    Denied,
    Unavailable,
    InvalidSource,
    CancelCheck,
    Parse(ParseError),
    Windows(u32),
}
impl From<ParseError> for CollectError {
    fn from(value: ParseError) -> Self {
        Self::Parse(value)
    }
}

#[derive(Clone, Copy)]
pub struct CollectionBounds {
    pub max_records: usize,
    pub max_retained_bytes: usize,
    pub max_runs: usize,
    pub duration: Duration,
}
impl CollectionBounds {
    pub fn for_budget(duration: Duration) -> Self {
        Self {
            max_records: MAX_NATIVE_RECORDS,
            max_retained_bytes: MAX_RETAINED_BYTES,
            max_runs: MAX_NATIVE_RUNS,
            duration,
        }
    }
    fn validate(self) -> Result<(), CollectError> {
        if !(1..=MAX_NATIVE_RECORDS).contains(&self.max_records)
            || !(MAX_RECORD_BYTES..=MAX_RETAINED_BYTES).contains(&self.max_retained_bytes)
            || !(1..=MAX_NATIVE_RUNS).contains(&self.max_runs)
            || self.duration.is_zero()
            || self.duration > Duration::from_secs(120)
        {
            return Err(CollectError::InvalidBounds);
        }
        Ok(())
    }
    /// The walk stops this early so the rest of the duration can finish what it read.
    fn walk_duration(self) -> Duration {
        self.duration - (self.duration / 8).min(MAX_FINISH_RESERVE)
    }
}

/// This local witness is never serialized, displayed, logged, or included in Collection.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JournalWitness {
    pub journal_id: u64,
    pub next_usn: u64,
    pub lowest_valid_usn: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VolumeSnapshot {
    pub geometry: Geometry,
    pub record_slots: u64,
    pub journal: Option<JournalWitness>,
}

/// Local, short-lived FILE record. Bytes are discarded as soon as metadata is parsed.
pub struct RecordData {
    pub number: u64,
    pub returned_sequence: Option<u16>,
    pub bytes: Vec<u8>,
}

/// FSCTL_GET_NTFS_FILE_RECORD returns the actual in-use ordinal at/below the request.
/// Implementations must not return names, paths, serials, or content through an error.
pub trait Source {
    fn snapshot(&mut self) -> Result<VolumeSnapshot, CollectError>;
    fn record_at_or_below(&mut self, ordinal: u64) -> Result<RecordData, CollectError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectionEnd {
    Enumerated,
    RecordLimit,
    MemoryLimit,
    RunLimit,
    Cancelled,
    TimedOut,
    SourceUnavailable,
    InvalidSource,
}

/// Supported-stream evidence from a live scan, never a whole-volume/reclaimable-size claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    pub summary: Summary,
    /// Local-only bounded directory/file inventory; its raw paths must not leave the host.
    pub tree: TreeSummary,
    pub end: CollectionEnd,
    pub record_slots_at_start: u64,
    pub records_examined: u64,
    /// All ordinal slots walked, including gaps identified by the downward API.
    pub record_slots_walked: u64,
    /// Remaining ordinal slots; includes unknown in-use and unused records.
    pub record_slots_not_walked: u64,
    pub invalid_records: u64,
    pub source_errors: u64,
    pub change_witness_available: bool,
    pub change_detected: bool,
    /// None of the scans is atomic, including one whose available witnesses did not change.
    pub allocation_additive: bool,
    pub retained_bytes_high_water: u64,
    pub duration_ms: u64,
}

impl Collection {
    /// Validate the finite helper response again in the non-elevated host.
    pub fn validate(&self) -> Result<(), CollectError> {
        self.summary.validate()?;
        self.tree
            .validate(&self.summary, self.allocation_additive)?;
        let limits = &self.summary.limitations;
        let terminal_limit = match self.end {
            CollectionEnd::Enumerated => None,
            CollectionEnd::RecordLimit => Some(Limit::RecordLimit),
            CollectionEnd::MemoryLimit => Some(Limit::MemoryLimit),
            CollectionEnd::RunLimit => Some(Limit::RunLimit),
            CollectionEnd::Cancelled => Some(Limit::Cancelled),
            CollectionEnd::TimedOut => Some(Limit::DurationLimit),
            CollectionEnd::SourceUnavailable => Some(Limit::SourceUnavailable),
            CollectionEnd::InvalidSource => Some(Limit::InvalidRecord),
        };
        if self.records_examined != self.summary.input_records as u64
            || self.summary.duplicate_records != 0
            || self.records_examined > MAX_NATIVE_RECORDS as u64
            || self.tree.indexed_nodes > self.records_examined
            || self.tree.retained_bytes > self.retained_bytes_high_water
            || self.record_slots_at_start >= 1_u64 << 48
            || self
                .record_slots_walked
                .checked_add(self.record_slots_not_walked)
                != Some(self.record_slots_at_start)
            || self.records_examined > self.record_slots_walked
            || self.invalid_records > self.records_examined
            || self.source_errors > 2
            || self.retained_bytes_high_water > MAX_RETAINED_BYTES as u64
            || self.duration_ms > 120_000
            || !limits.contains(&Limit::LiveVolumeNotAtomic)
            || !limits.contains(&Limit::MetadataAllocationExcluded)
            || !limits.contains(&Limit::NativeAccuracyUnvalidated)
            || limits.contains(&Limit::OfflineSubset)
            || terminal_limit.is_some_and(|limit| !limits.contains(&limit))
            || (self.end == CollectionEnd::Enumerated && self.record_slots_not_walked != 0)
            || (self.change_detected && !limits.contains(&Limit::VolumeChanged))
            || (!self.change_witness_available
                && !limits.contains(&Limit::ChangeWitnessUnavailable))
            || (self.source_errors != 0 && !limits.contains(&Limit::SourceUnavailable))
            || (self.invalid_records != 0 && !limits.contains(&Limit::InvalidRecord))
            || (!self.allocation_additive
                && !limits.contains(&Limit::InvalidRecord)
                && !matches!(self.end, CollectionEnd::Cancelled | CollectionEnd::TimedOut))
            || (self.record_slots_at_start < 16
                && (self.record_slots_at_start != 0
                    || self.end != CollectionEnd::Cancelled
                    || self.records_examined != 0
                    || self.source_errors != 0
                    || self.change_witness_available
                    || self.change_detected
                    || self.retained_bytes_high_water != 0))
        {
            return Err(CollectError::InvalidSource);
        }
        Ok(())
    }

    pub(crate) fn cancelled_before_open() -> Self {
        Self {
            summary: Summary {
                limitations: [
                    Limit::Cancelled,
                    Limit::LiveVolumeNotAtomic,
                    Limit::MetadataAllocationExcluded,
                    Limit::NativeAccuracyUnvalidated,
                    Limit::ChangeWitnessUnavailable,
                ]
                .into_iter()
                .collect(),
                ..Summary::default()
            },
            tree: TreeSummary::default(),
            end: CollectionEnd::Cancelled,
            record_slots_at_start: 0,
            records_examined: 0,
            record_slots_walked: 0,
            record_slots_not_walked: 0,
            invalid_records: 0,
            source_errors: 0,
            change_witness_available: false,
            change_detected: false,
            allocation_additive: true,
            retained_bytes_high_water: 0,
            duration_ms: 0,
        }
    }
}

/// Pending group bookkeeping per record: its share of the `BTreeMap` entry and node.
const PENDING_ENTRY_BYTES: usize = 128;

/// Accounted heap of one parsed record held in `pending`: the record in its group vector
/// (capacity at most twice the length), the map entry, and everything the record owns. The
/// formula is checked against the real heap in `tests::memory_*`.
fn parsed_memory(parsed: &Parsed) -> usize {
    let mut total = 2 * size_of::<Parsed>() + PENDING_ENTRY_BYTES;
    total += tree::parsed_bytes(parsed);
    total += parsed.file_names.capacity() * size_of::<AttributeIdentity>();
    total += parsed.attribute_list.as_ref().map_or(0, |list| {
        btree_bytes(list.len(), size_of::<AttributeIdentity>())
    });
    total += btree_bytes(parsed.limits.len(), size_of::<Limit>());
    total += btree_bytes(parsed.tree_limits.len(), size_of::<TreeLimit>());
    total += parsed.data.capacity() * size_of::<StreamFragment>();
    for fragment in &parsed.data {
        if let Data::Nonresident { runs, .. } = &fragment.data {
            total += runs.capacity() * size_of::<(u64, u64)>();
        }
    }
    total
}

fn append_summary(into: &mut Summary, delta: Summary) -> Result<(), ParseError> {
    // All count fields are bounded by source records/attributes before reaching this function.
    macro_rules! counts {
        ($($field:ident),* $(,)?) => { $(into.$field = into.$field.checked_add(delta.$field)
            .ok_or(ParseError::ArithmeticOverflow)?;)* };
    }
    counts!(
        input_records,
        duplicate_records,
        extension_records,
        base_files,
        hard_linked_files,
        resident_files,
        sparse_files,
        compressed_files,
        unknown_logical_files,
        unknown_data_allocation_files,
        data_streams,
        named_streams,
        unknown_logical_streams,
        unknown_data_allocation_streams,
        orphan_extension_groups
    );
    into.logical_bytes_known = add(into.logical_bytes_known, delta.logical_bytes_known)?;
    into.data_cluster_bytes_known = add(
        into.data_cluster_bytes_known,
        delta.data_cluster_bytes_known,
    )?;
    into.resident_logical_bytes = add(into.resident_logical_bytes, delta.resident_logical_bytes)?;
    into.named_logical_bytes_known = add(
        into.named_logical_bytes_known,
        delta.named_logical_bytes_known,
    )?;
    into.limitations.extend(delta.limitations);
    Ok(())
}

/// Physical data runs (first cluster, cluster count) of every accounted group, in chunks so
/// the list never reallocates; checked for overlap once the walk ends.
type Runs = Chunked<(u64, u64), 64, 8192>;
type Head = Reverse<(u64, u64, usize, usize)>;

/// Accounted bytes of the run list, including the merge heap of the overlap check.
fn runs_bytes(runs: &Runs) -> usize {
    runs.bytes() + runs.chunk_slots() * size_of::<Head>()
}

fn account(
    key: Key,
    group: Vec<Parsed>,
    summary: &mut Summary,
    runs: &mut Runs,
    g: Geometry,
    tree: &mut tree::TreeBuilder,
    available: usize,
) -> Result<(), ParseError> {
    // Failed group parsing cannot leave half-added bytes/counts in a usable native summary.
    let mut delta = Summary::default();
    let mut ranges = Vec::new();
    let metadata = tree::Metadata::from_group(key, &group);
    account_group(key, group, &mut delta, &mut ranges, g)?;
    let mut merged = summary.clone();
    append_summary(&mut merged, delta.clone())?;
    // Capacity was reserved by `reserve_runs`; a failed push means the allocator failed.
    if !ranges.into_iter().all(|range| runs.push(range)) {
        return Err(ParseError::BoundExceeded);
    }
    tree.record(metadata, &delta, available);
    *summary = merged;
    Ok(())
}

fn physical_runs(parsed: &Parsed) -> usize {
    parsed
        .data
        .iter()
        .map(|fragment| match &fragment.data {
            Data::Resident(_) => 0,
            Data::Nonresident { runs, .. } => runs.len(),
        })
        .sum()
}

/// Admits `additional` runs if the run bound and the memory bound (with `other_bytes`) allow.
fn reserve_runs(
    runs: &Runs,
    additional: usize,
    other_bytes: usize,
    bounds: CollectionBounds,
    high_water: &mut u64,
) -> bool {
    let Some(needed) = runs.len().checked_add(additional) else {
        return false;
    };
    let memory = other_bytes
        .saturating_add(runs_bytes(runs))
        .saturating_add(runs.growth(additional));
    if needed > bounds.max_runs || memory > bounds.max_retained_bytes {
        return false;
    }
    *high_water = (*high_water).max(memory as u64);
    true
}

/// Sorts each chunk in place, then merges them in start order: any run starting before the
/// previous one ends is an overlap. Returns the allocated bytes.
fn allocation_of(runs: &mut Runs, cluster_bytes: u64) -> Result<u64, ParseError> {
    let chunks = runs.chunks_mut();
    for chunk in chunks.iter_mut() {
        chunk.sort_unstable();
    }
    let mut heads: BinaryHeap<Head> = BinaryHeap::with_capacity(chunks.len());
    for (k, chunk) in chunks.iter().enumerate() {
        if let Some(&(start, length)) = chunk.first() {
            heads.push(Reverse((start, length, k, 0)));
        }
    }
    let (mut previous_end, mut count) = (0, 0);
    while let Some(Reverse((start, length, k, at))) = heads.pop() {
        if start < previous_end {
            return Err(ParseError::OverlappingAllocation);
        }
        previous_end = add(start, length)?;
        count = add(count, length)?;
        if let Some(&(start, length)) = chunks[k].get(at + 1) {
            heads.push(Reverse((start, length, k, at + 1)));
        }
    }
    mul(count, cluster_bytes)
}

/// A pending group can be joined once its base and every record its resident attribute list
/// names have been read. Groups without such a list wait for the end of the walk.
fn complete(key: Key, group: &[Parsed]) -> bool {
    let Some(base) = group
        .iter()
        .find(|record| record.is_base && record.key == key)
    else {
        return false;
    };
    match &base.attribute_list {
        Some(list) if !list.is_empty() && !base.unresolved_list => list
            .iter()
            .filter(|identity| identity.record != key)
            .all(|identity| group.iter().any(|record| record.key == identity.record)),
        _ => false,
    }
}

/// Joins and accounts one group, then releases its pending bytes. False: the memory bound.
#[allow(clippy::too_many_arguments)]
fn flush(
    key: Key,
    group: Vec<Parsed>,
    out: &mut Collection,
    runs: &mut Runs,
    tree: &mut tree::TreeBuilder,
    pending_bytes: &mut usize,
    g: Geometry,
    bounds: CollectionBounds,
) -> bool {
    let count: usize = group.iter().map(physical_runs).sum();
    let group_memory: usize = group.iter().map(parsed_memory).sum();
    if !reserve_runs(
        runs,
        count,
        pending_bytes.saturating_add(tree.retained_bytes()),
        bounds,
        &mut out.retained_bytes_high_water,
    ) {
        return false;
    }
    let available = bounds.max_retained_bytes.saturating_sub(
        pending_bytes
            .saturating_add(tree.retained_bytes())
            .saturating_add(runs_bytes(runs))
            .saturating_add(runs.growth(count)),
    );
    if account(key, group, &mut out.summary, runs, g, tree, available).is_err() {
        out.invalid_records += 1;
        out.summary.limitations.insert(Limit::InvalidRecord);
    }
    *pending_bytes = pending_bytes.saturating_sub(group_memory);
    out.retained_bytes_high_water = out.retained_bytes_high_water.max(
        pending_bytes
            .saturating_add(tree.retained_bytes())
            .saturating_add(runs_bytes(runs)) as u64,
    );
    true
}

fn stop(
    cancelled: &mut dyn FnMut() -> Result<bool, CollectError>,
    started: Instant,
    bound: Duration,
) -> Result<Option<CollectionEnd>, CollectError> {
    if cancelled()? {
        return Ok(Some(CollectionEnd::Cancelled));
    }
    if started.elapsed() >= bound {
        return Ok(Some(CollectionEnd::TimedOut));
    }
    Ok(None)
}

/// Reads the root record (5) once more after a walk that stopped before it, only so complete
/// paths can be anchored. It is neither counted as examined nor accounted as file data.
fn read_root(source: &mut impl Source, g: Geometry, tree: &mut tree::TreeBuilder) {
    let Ok(record) = source.record_at_or_below(5) else {
        return;
    };
    if record.number != 5
        || record.bytes.len() != g.record_bytes
        || record
            .returned_sequence
            .is_some_and(|sequence| word(&record.bytes, 16) != Ok(sequence))
    {
        return;
    }
    let raw = Record {
        number: 5,
        bytes: &record.bytes,
    };
    if let Ok(Some(parsed)) = parse(&raw, g, &mut 0) {
        if parsed.directory && parsed.is_base {
            let metadata = tree::Metadata::from_group(parsed.key, std::slice::from_ref(&parsed));
            tree.record(metadata, &Summary::default(), 0);
        }
    }
}

/// Stream every available record within count/memory/time limits, keeping only metadata.
/// A caller must be the approved helper; this function never starts or requests elevation.
pub fn collect(
    source: &mut impl Source,
    bounds: CollectionBounds,
    cancelled: &mut dyn FnMut() -> Result<bool, CollectError>,
) -> Result<Collection, CollectError> {
    bounds.validate()?;
    let started = Instant::now();
    let walk = bounds.walk_duration();
    // Do not open/query a source after cancellation has already been requested.
    if cancelled()? {
        return Ok(Collection::cancelled_before_open());
    }
    let initial = source.snapshot()?;
    validate_geometry(initial.geometry)?;
    if !(16..1_u64 << 48).contains(&initial.record_slots) {
        return Err(CollectError::InvalidSource);
    }
    if initial
        .record_slots
        .checked_mul(initial.geometry.record_bytes as u64)
        .is_none_or(|bytes| {
            bytes > initial.geometry.volume_clusters * initial.geometry.cluster_bytes
        })
    {
        return Err(CollectError::InvalidSource);
    }
    let g = initial.geometry;
    let mut out = Collection {
        summary: Summary::default(),
        tree: TreeSummary::default(),
        end: CollectionEnd::Enumerated,
        record_slots_at_start: initial.record_slots,
        records_examined: 0,
        record_slots_walked: 0,
        record_slots_not_walked: initial.record_slots,
        invalid_records: 0,
        source_errors: 0,
        change_witness_available: false,
        change_detected: false,
        allocation_additive: true,
        retained_bytes_high_water: 0,
        duration_ms: 0,
    };
    out.summary.limitations.extend([
        Limit::LiveVolumeNotAtomic,
        Limit::MetadataAllocationExcluded,
        Limit::NativeAccuracyUnvalidated,
    ]);
    let mut cursor = initial.record_slots - 1;
    let mut runs = Runs::with_bound(
        bounds
            .max_runs
            .saturating_mul(size_of::<(u64, u64)>())
            .min(bounds.max_retained_bytes),
    );
    let mut pending = BTreeMap::<Key, Vec<Parsed>>::new();
    let mut pending_bytes = 0_usize;
    let mut total_runs = 0_usize;
    let mut tree = tree::TreeBuilder::new(bounds.max_retained_bytes);
    out.retained_bytes_high_water = (tree.retained_bytes() + runs_bytes(&runs)) as u64;
    loop {
        if let Some(end) = stop(cancelled, started, walk)? {
            out.end = end;
            break;
        }
        if out.records_examined >= bounds.max_records as u64 {
            out.end = CollectionEnd::RecordLimit;
            break;
        }
        let record = match source.record_at_or_below(cursor) {
            Ok(record) => record,
            Err(_) => {
                out.source_errors += 1;
                out.end = CollectionEnd::SourceUnavailable;
                break;
            }
        };
        if record.number > cursor || record.bytes.len() != g.record_bytes {
            out.end = CollectionEnd::InvalidSource;
            out.source_errors += 1;
            break;
        }
        out.records_examined += 1;
        out.summary.input_records += 1;
        out.record_slots_walked = initial.record_slots - record.number;
        out.record_slots_not_walked = record.number;
        let next = record.number.checked_sub(1);
        if let Some(end) = stop(cancelled, started, walk)? {
            out.end = end;
            break;
        }
        let mut record_runs = 0;
        let parsed = if record
            .returned_sequence
            .is_some_and(|sequence| word(&record.bytes, 16) != Ok(sequence))
        {
            Err(ParseError::ConflictingRecord)
        } else {
            parse(
                &Record {
                    number: record.number,
                    bytes: &record.bytes,
                },
                g,
                &mut record_runs,
            )
        };
        // `record` and its resident bytes are not retained by Parsed.
        drop(record.bytes);
        total_runs = total_runs.saturating_add(record_runs);
        if total_runs > bounds.max_runs {
            out.end = CollectionEnd::RunLimit;
            break;
        }
        match parsed {
            Ok(Some(parsed)) if record.number < 16 => {
                if parsed.directory {
                    let retained = pending_bytes
                        .saturating_add(parsed_memory(&parsed))
                        .saturating_add(tree.retained_bytes())
                        .saturating_add(runs_bytes(&runs));
                    let available = bounds.max_retained_bytes.saturating_sub(retained);
                    tree.record(
                        tree::Metadata::from_group(parsed.key, std::slice::from_ref(&parsed)),
                        &Summary::default(),
                        available,
                    );
                    out.retained_bytes_high_water = out
                        .retained_bytes_high_water
                        .max((pending_bytes + tree.retained_bytes() + runs_bytes(&runs)) as u64);
                }
                drop(parsed);
                out.summary.limitations.insert(Limit::SystemRecordsExcluded);
            }
            Ok(Some(parsed)) => {
                out.summary.limitations.extend(&parsed.limits);
                out.summary.extension_records += usize::from(!parsed.is_base);
                let retain = parsed_memory(&parsed);
                let memory = pending_bytes
                    .saturating_add(retain)
                    .saturating_add(tree.retained_bytes())
                    .saturating_add(runs_bytes(&runs));
                if memory > bounds.max_retained_bytes {
                    out.end = CollectionEnd::MemoryLimit;
                    break;
                }
                out.retained_bytes_high_water = out.retained_bytes_high_water.max(memory as u64);
                let key = parsed.base;
                pending_bytes = pending_bytes.saturating_add(retain);
                let group = if parsed.is_base
                    && parsed.attribute_list.is_none()
                    && !parsed.unresolved_list
                    && !pending.contains_key(&key)
                {
                    Some(vec![parsed])
                } else {
                    let group = pending.entry(key).or_insert_with(|| Vec::with_capacity(1));
                    group.push(parsed);
                    // A complete group is joined now instead of waiting for the walk's end.
                    if complete(key, group) {
                        pending.remove(&key)
                    } else {
                        None
                    }
                };
                if let Some(group) = group {
                    if !flush(
                        key,
                        group,
                        &mut out,
                        &mut runs,
                        &mut tree,
                        &mut pending_bytes,
                        g,
                        bounds,
                    ) {
                        out.end = CollectionEnd::MemoryLimit;
                        break;
                    }
                }
            }
            Ok(None) | Err(_) => {
                // The native API promises an in-use record; deleted/invalid results are gaps.
                out.invalid_records += 1;
                out.summary.limitations.insert(Limit::InvalidRecord);
            }
        }
        let Some(next) = next else { break };
        cursor = next;
    }
    // The rest of the duration joins, checks and summarizes what the walk read.
    let finish = bounds.duration;
    for (key, group) in std::mem::take(&mut pending) {
        if let Some(end) = stop(cancelled, started, finish)? {
            out.end = end;
            // No partially joined chain is claimed once cancellation/deadline wins.
            out.summary
                .limitations
                .insert(Limit::AttributeListUnresolved);
            break;
        }
        if !flush(
            key,
            group,
            &mut out,
            &mut runs,
            &mut tree,
            &mut pending_bytes,
            g,
            bounds,
        ) {
            out.end = CollectionEnd::MemoryLimit;
            out.summary
                .limitations
                .insert(Limit::AttributeListUnresolved);
            break;
        }
    }
    if let Some(end) = stop(cancelled, started, finish)? {
        out.end = end;
        out.allocation_additive = false;
    } else if allocation_of(&mut runs, g.cluster_bytes).is_err() {
        out.allocation_additive = false;
        out.summary.limitations.insert(Limit::InvalidRecord);
    }
    drop(runs);
    if !tree.has_root()
        && matches!(
            out.end,
            CollectionEnd::RecordLimit
                | CollectionEnd::MemoryLimit
                | CollectionEnd::RunLimit
                | CollectionEnd::TimedOut
        )
        && stop(cancelled, started, finish)?.is_none()
    {
        read_root(source, g, &mut tree);
    }
    if let Some(end) = stop(cancelled, started, finish)? {
        out.end = end;
    }
    let incomplete =
        out.end != CollectionEnd::Enumerated || out.invalid_records != 0 || out.source_errors != 0;
    let (inventory, ending) = tree.finish(
        &out.summary,
        out.allocation_additive,
        incomplete,
        &mut || stop(cancelled, started, finish),
    )?;
    out.tree = inventory;
    if let Some(end) = ending {
        out.end = end;
    }
    if !matches!(out.end, CollectionEnd::Cancelled | CollectionEnd::TimedOut) {
        match source.snapshot() {
            Ok(final_snapshot) => {
                out.change_witness_available =
                    initial.journal.is_some() && final_snapshot.journal.is_some();
                out.change_detected = initial.geometry != final_snapshot.geometry
                    || initial.record_slots != final_snapshot.record_slots
                    || (out.change_witness_available && initial.journal != final_snapshot.journal);
                if out.change_detected {
                    out.summary.limitations.insert(Limit::VolumeChanged);
                    out.tree.limitations.insert(TreeLimit::CollectionIncomplete);
                }
            }
            Err(_) => {
                out.source_errors += 1;
                out.summary.limitations.insert(Limit::SourceUnavailable);
            }
        }
    }
    if !out.change_witness_available {
        out.tree.limitations.insert(TreeLimit::CollectionIncomplete);
        out.summary
            .limitations
            .insert(Limit::ChangeWitnessUnavailable);
    }
    if out.source_errors != 0 {
        out.summary.limitations.insert(Limit::SourceUnavailable);
    }
    let terminal_limit = match out.end {
        CollectionEnd::Enumerated => None,
        CollectionEnd::RecordLimit => Some(Limit::RecordLimit),
        CollectionEnd::RunLimit => Some(Limit::RunLimit),
        CollectionEnd::MemoryLimit => Some(Limit::MemoryLimit),
        CollectionEnd::Cancelled => Some(Limit::Cancelled),
        CollectionEnd::TimedOut => Some(Limit::DurationLimit),
        CollectionEnd::SourceUnavailable => Some(Limit::SourceUnavailable),
        CollectionEnd::InvalidSource => Some(Limit::InvalidRecord),
    };
    if let Some(limit) = terminal_limit {
        out.summary.limitations.insert(limit);
    }
    out.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{geometry, listed_data, nonresident, record, resident};
    use super::*;
    use crate::test_alloc::measure;

    struct Fixtures {
        records: BTreeMap<u64, Vec<u8>>,
        snapshot: VolumeSnapshot,
        requests: Vec<u64>,
        snapshots: usize,
        final_journal: Option<JournalWitness>,
        repeated_ordinal: bool,
        wrong_sequence: bool,
        fail_final_snapshot: bool,
        delay: Duration,
    }
    impl Fixtures {
        fn new(records: impl IntoIterator<Item = (u64, Vec<u8>)>) -> Self {
            let journal = Some(JournalWitness {
                journal_id: 1234567,
                next_usn: 4000,
                lowest_valid_usn: 0,
            });
            Self {
                records: records.into_iter().collect(),
                snapshot: VolumeSnapshot {
                    geometry: geometry(1024),
                    record_slots: 64,
                    journal,
                },
                requests: Vec::new(),
                snapshots: 0,
                final_journal: journal,
                repeated_ordinal: false,
                wrong_sequence: false,
                fail_final_snapshot: false,
                delay: Duration::ZERO,
            }
        }
    }
    impl Source for Fixtures {
        fn snapshot(&mut self) -> Result<VolumeSnapshot, CollectError> {
            self.snapshots += 1;
            if self.snapshots > 1 && self.fail_final_snapshot {
                return Err(CollectError::Unavailable);
            }
            let mut snapshot = self.snapshot;
            if self.snapshots > 1 {
                snapshot.journal = self.final_journal;
            }
            Ok(snapshot)
        }
        fn record_at_or_below(&mut self, ordinal: u64) -> Result<RecordData, CollectError> {
            self.requests.push(ordinal);
            std::thread::sleep(self.delay);
            let (number, bytes) = self
                .records
                .range(..=ordinal)
                .next_back()
                .ok_or(CollectError::Unavailable)?;
            Ok(RecordData {
                number: if self.repeated_ordinal {
                    self.snapshot.record_slots - 1
                } else {
                    *number
                },
                returned_sequence: self.wrong_sequence.then_some(9),
                bytes: bytes.clone(),
            })
        }
    }
    fn bounds() -> CollectionBounds {
        CollectionBounds::for_budget(Duration::from_secs(30))
    }
    fn ordinary(lcn: u8) -> Vec<u8> {
        record(
            1024,
            1,
            0,
            2,
            &[nonresident(0, 1, 100, 4096, 4096, 0, &[0x11, 1, lcn, 0])],
        )
    }
    fn system_record() -> (u64, Vec<u8>) {
        (0, record(1024, 1, 0, 1, &[resident(DATA, 0, 0)]))
    }

    #[test]
    fn actual_downward_ordinals_walk_gaps_and_never_duplicate_hard_link_sizes() {
        let mut source = Fixtures::new([(50, ordinary(10)), (42, ordinary(20)), system_record()]);
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert_eq!(source.requests, [63, 49, 41]);
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.records_examined, 3);
        assert_eq!(collection.record_slots_walked, 64);
        assert_eq!(collection.record_slots_not_walked, 0);
        assert_eq!(collection.summary.base_files, 2);
        assert_eq!(collection.summary.hard_linked_files, 2);
        assert_eq!(collection.summary.logical_bytes_known, 200);
        assert_eq!(collection.summary.data_cluster_bytes_known, 8192);
        assert!(collection.allocation_additive);
        assert!(collection.change_witness_available);
        assert!(!collection.change_detected);
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::LiveVolumeNotAtomic));
        let output = format!("{collection:?}");
        assert!(!output.contains("1234567"));
        collection.validate().unwrap();
    }

    #[test]
    fn count_run_and_source_bounds_preserve_known_subsets_and_unknown_slots() {
        let mut source = Fixtures::new([(50, ordinary(10)), (42, ordinary(20)), system_record()]);
        let mut bound = bounds();
        bound.max_records = 1;
        let collection = collect(&mut source, bound, &mut || Ok(false)).unwrap();
        // After an early end the root record (5) is read once more, only to anchor paths.
        assert_eq!(source.requests, [63, 5]);
        assert_eq!(collection.end, CollectionEnd::RecordLimit);
        assert_eq!(collection.record_slots_walked, 14);
        assert_eq!(collection.record_slots_not_walked, 50);
        assert_eq!(collection.summary.logical_bytes_known, 100);
        bound.max_records = 10;
        bound.max_runs = 1;
        source.requests.clear();
        let collection = collect(&mut source, bound, &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::RunLimit);
        assert_eq!(collection.records_examined, 2);
        assert_eq!(collection.summary.data_cluster_bytes_known, 4096);
        assert!(collection.summary.limitations.contains(&Limit::RunLimit));
        collection.validate().unwrap();
        source.repeated_ordinal = true;
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::InvalidSource);
        assert_eq!(collection.summary.base_files, 1);
        assert_eq!(collection.source_errors, 1);
        collection.validate().unwrap();
    }

    #[test]
    fn sequence_mismatch_and_corrupt_records_do_not_abort_other_supported_files() {
        let mut torn = ordinary(10);
        torn[510] ^= 1;
        let mut source = Fixtures::new([(50, torn), (42, ordinary(20)), system_record()]);
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.invalid_records, 1);
        assert_eq!(collection.summary.logical_bytes_known, 100);
        assert_eq!(collection.summary.data_cluster_bytes_known, 4096);
        source.wrong_sequence = true;
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert_eq!(collection.invalid_records, 3);
        assert_eq!(collection.summary.base_files, 0);
        collection.validate().unwrap();
    }

    #[test]
    fn changed_or_missing_journal_witnesses_are_never_atomic_snapshot_proof() {
        let mut source = Fixtures::new([(42, ordinary(10)), system_record()]);
        source.final_journal = Some(JournalWitness {
            next_usn: 4001,
            ..source.snapshot.journal.unwrap()
        });
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert!(collection.change_detected);
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::VolumeChanged));
        source.snapshots = 0;
        source.final_journal = None;
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert!(!collection.change_witness_available);
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::ChangeWitnessUnavailable));
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::LiveVolumeNotAtomic));
        collection.validate().unwrap();
    }

    #[test]
    fn pending_extensions_join_only_the_listed_record_sequence_and_instance() {
        let head = record(
            1024,
            3,
            0,
            2,
            &[
                listed_data(&[(42, 3, 0, 0), (50, 8, 1, 0)]),
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
        let mut source = Fixtures::new([(42, head), (50, extension), system_record()]);
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert_eq!(collection.summary.base_files, 1);
        assert_eq!(collection.summary.extension_records, 1);
        assert_eq!(collection.summary.data_cluster_bytes_known, 8192);
        assert_eq!(collection.summary.unknown_data_allocation_streams, 0);
        assert!(!collection
            .summary
            .limitations
            .contains(&Limit::AttributeListUnresolved));
        collection.validate().unwrap();
    }

    #[test]
    fn overlapping_live_allocations_hide_the_additive_allocation_total() {
        let mut source = Fixtures::new([(50, ordinary(10)), (42, ordinary(10)), system_record()]);
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert!(!collection.allocation_additive);
        assert_eq!(collection.summary.logical_bytes_known, 200);
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::InvalidRecord));
        collection.validate().unwrap();
    }

    #[test]
    fn cancellation_precedes_source_calls_and_deadline_precedes_post_read_accounting() {
        let mut source = Fixtures::new([(42, ordinary(10)), system_record()]);
        let collection = collect(&mut source, bounds(), &mut || Ok(true)).unwrap();
        assert_eq!(collection.end, CollectionEnd::Cancelled);
        collection.validate().unwrap();
        assert_eq!(source.snapshots, 0);
        assert!(source.requests.is_empty());
        let mut checks = 0;
        let collection = collect(&mut source, bounds(), &mut || {
            checks += 1;
            Ok(checks >= 3)
        })
        .unwrap();
        assert_eq!(collection.end, CollectionEnd::Cancelled);
        assert_eq!(collection.summary.base_files, 0);
        collection.validate().unwrap();
        assert_eq!(source.requests, [63]);
        source.requests.clear();
        source.delay = Duration::from_millis(3);
        let mut bound = bounds();
        bound.duration = Duration::from_millis(1);
        let collection = collect(&mut source, bound, &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::TimedOut);
        assert_eq!(collection.summary.base_files, 0);
        collection.validate().unwrap();
        bound.duration = Duration::ZERO;
        assert_eq!(
            collect(&mut source, bound, &mut || Ok(false)),
            Err(CollectError::InvalidBounds)
        );
    }

    #[test]
    fn resident_attribute_lists_are_counted_in_the_retained_memory_limit() {
        let entries: Vec<_> = (0..150).map(|start| (42, 3, start, 0)).collect();
        let head = record(
            16384,
            3,
            0,
            1,
            &[listed_data(&entries), resident(DATA, 1, 0)],
        );
        let mut source = Fixtures::new([(42, head)]);
        source.snapshot.geometry = geometry(16384);
        let mut bound = bounds();
        bound.max_retained_bytes = MAX_RECORD_BYTES;
        let collection = collect(&mut source, bound, &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::MemoryLimit);
        assert!(collection.summary.limitations.contains(&Limit::MemoryLimit));
        assert!(collection.retained_bytes_high_water <= bound.max_retained_bytes as u64);
        assert_eq!(collection.summary.logical_bytes_known, 0);
        collection.validate().unwrap();
    }

    #[test]
    fn a_failed_final_query_retains_explicitly_partial_change_coverage() {
        let mut source = Fixtures::new([(42, ordinary(10)), system_record()]);
        source.fail_final_snapshot = true;
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.source_errors, 1);
        assert!(!collection.change_witness_available);
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::SourceUnavailable));
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::ChangeWitnessUnavailable));
        assert_eq!(collection.summary.logical_bytes_known, 100);
        collection.validate().unwrap();
    }

    #[test]
    fn aggregate_protocol_is_closed_and_rejects_impossible_accounting() {
        let mut source = Fixtures::new([(42, ordinary(10)), system_record()]);
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        let value = serde_json::to_value(&collection).unwrap();
        let parsed: Collection = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed, collection);
        parsed.validate().unwrap();
        for field in ["path", "journal_id", "content", "serial"] {
            let mut changed = value.clone();
            changed[field] = serde_json::json!("private");
            assert!(serde_json::from_value::<Collection>(changed).is_err());
        }
        let mut changed = value;
        changed["summary"]["record_reference"] = serde_json::json!(42);
        assert!(serde_json::from_value::<Collection>(changed).is_err());
        for corrupt in 0..9 {
            let mut changed = collection.clone();
            match corrupt {
                0 => changed.record_slots_not_walked += 1,
                1 => changed.records_examined += 1,
                2 => changed.summary.base_files = 10,
                3 => changed.summary.named_streams = 10,
                4 => changed.summary.resident_logical_bytes = 1000,
                5 => {
                    changed
                        .summary
                        .limitations
                        .remove(&Limit::LiveVolumeNotAtomic);
                }
                6 => changed.retained_bytes_high_water = MAX_RETAINED_BYTES as u64 + 1,
                7 => changed.invalid_records = changed.records_examined + 1,
                _ => changed.change_detected = true,
            }
            assert!(changed.validate().is_err());
        }
        let mut before_open = Collection::cancelled_before_open();
        before_open.summary.logical_bytes_known = 100;
        assert!(before_open.validate().is_err());
    }

    struct Generated {
        raw: Vec<u8>,
        records: u64,
        calls: u64,
    }
    impl Source for Generated {
        fn snapshot(&mut self) -> Result<VolumeSnapshot, CollectError> {
            let mut geometry = geometry(1024);
            geometry.volume_clusters = 1_000_000;
            Ok(VolumeSnapshot {
                geometry,
                record_slots: self.records,
                journal: None,
            })
        }
        fn record_at_or_below(&mut self, ordinal: u64) -> Result<RecordData, CollectError> {
            self.calls += 1;
            Ok(RecordData {
                number: ordinal,
                returned_sequence: Some(1),
                bytes: self.raw.clone(),
            })
        }
    }

    #[test]
    fn native_streaming_exceeds_offline_batch_bounds_without_retaining_resident_contents() {
        let mut source = Generated {
            raw: record(1024, 1, 0, 1, &[resident(DATA, 7, 0)]),
            records: 250_016,
            calls: 0,
        };
        let collection = collect(
            &mut source,
            CollectionBounds::for_budget(Duration::from_secs(120)),
            &mut || Ok(false),
        )
        .unwrap();
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.records_examined, 250_016);
        assert_eq!(collection.summary.base_files, 250_000);
        assert_eq!(collection.summary.logical_bytes_known, 1_750_000);
        assert_eq!(collection.summary.resident_files, 250_000);
        assert_eq!(collection.summary.data_cluster_bytes_known, 0);
        // Only the fixed chunk tables of the index and run list: nothing is kept per record.
        assert!(collection.retained_bytes_high_water < 128 * 1024);
        collection.validate().unwrap();
    }

    /// A file whose second data extent lives in an extension record one ordinal above it.
    fn listed_pair(base: u64, lcn: u64) -> [(u64, Vec<u8>); 2] {
        let run = |lcn: u64| [0x31, 1, lcn as u8, (lcn >> 8) as u8, (lcn >> 16) as u8, 0];
        let head = record(
            1024,
            3,
            0,
            2,
            &[
                listed_data(&[(base, 3, 0, 0), (base + 1, 8, 1, 0)]),
                nonresident(0, 1, 7000, 8192, 8192, 0, &run(lcn)),
            ],
        );
        let extension = record(
            1024,
            8,
            base | (3_u64 << 48),
            0,
            &[nonresident(1, 1, 0, 0, 0, 0, &run(lcn + 2))],
        );
        [(base, head), (base + 1, extension)]
    }

    #[test]
    fn complete_pending_groups_are_joined_during_the_walk() {
        let mut records = vec![system_record()];
        for k in 0..300 {
            records.extend(listed_pair(1000 + 2 * k, 100 + 4 * k));
        }
        let mut source = Fixtures::new(records);
        source.snapshot.record_slots = 2000;
        source.snapshot.geometry.volume_clusters = 1 << 20;
        source.final_journal = source.snapshot.journal;
        let collection = collect(&mut source, bounds(), &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.summary.base_files, 300);
        assert_eq!(collection.summary.extension_records, 300);
        assert_eq!(collection.summary.data_cluster_bytes_known, 300 * 8192);
        assert!(!collection
            .summary
            .limitations
            .contains(&Limit::AttributeListUnresolved));
        // Holding all 300 groups until the end would need about 1 MiB; joining each group
        // once its extension and base are read keeps one at a time.
        let fixed = (tree::TreeBuilder::new(MAX_RETAINED_BYTES).retained_bytes()
            + runs_bytes(&Runs::with_bound(MAX_NATIVE_RUNS * 16))) as u64;
        assert!(collection.retained_bytes_high_water < fixed + 64 * 1024);
        collection.validate().unwrap();
    }

    #[test]
    fn memory_accounting_covers_pending_records() {
        let entries: Vec<_> = (0..150).map(|start| (42, 3, start, 0)).collect();
        let cases = [
            (1024, record(1024, 1, 0, 1, &[resident(DATA, 7, 0)])),
            (1024, ordinary(10)),
            (1024, listed_pair(42, 10)[0].1.clone()),
            (1024, listed_pair(42, 10)[1].1.clone()),
            (
                16384,
                record(
                    16384,
                    3,
                    0,
                    1,
                    &[listed_data(&entries), resident(DATA, 1, 0)],
                ),
            ),
        ];
        for (size, bytes) in cases {
            let (parsed, usage) = measure(|| {
                parse(
                    &Record {
                        number: 42,
                        bytes: &bytes,
                    },
                    geometry(size),
                    &mut 0,
                )
                .unwrap()
                .unwrap()
            });
            // The record itself sits in its group vector; the rest is its own heap.
            let heap = parsed_memory(&parsed) - 2 * size_of::<Parsed>() - PENDING_ENTRY_BYTES;
            assert!(
                usage.retained <= heap,
                "real {} accounted {heap}",
                usage.retained
            );
            eprintln!(
                "pending record of {size} bytes: real heap {} + struct {}, accounted {}",
                usage.retained,
                size_of::<Parsed>(),
                parsed_memory(&parsed)
            );
        }
    }

    #[test]
    fn chunked_overlap_check_matches_a_sorted_scan() {
        let mut runs = Runs::with_bound(1 << 20);
        for i in (0..20_000_u64).rev() {
            assert!(runs.push((10 + 3 * i, 2)));
        }
        assert_eq!(allocation_of(&mut runs, 4096), Ok(20_000 * 2 * 4096));
        assert!(runs.push((11, 1)));
        assert_eq!(
            allocation_of(&mut runs, 4096),
            Err(ParseError::OverlappingAllocation)
        );
    }

    #[test]
    fn limits_are_consistent_with_the_measured_costs() {
        // Each examined record yields at most one node; runs and the index share the bound.
        assert!(tree::MAX_TREE_RETAINED_BYTES + MAX_NATIVE_RUNS * 16 <= MAX_RETAINED_BYTES);
        assert!(tree::MAX_TREE_NODES <= MAX_NATIVE_RECORDS);
        // The reserve leaves the walk most of the duration and the finish at least 1/8.
        let bounds = CollectionBounds::for_budget(Duration::from_secs(120));
        assert_eq!(bounds.walk_duration(), Duration::from_secs(112));
        let short = CollectionBounds::for_budget(Duration::from_secs(8));
        assert_eq!(short.walk_duration(), Duration::from_secs(7));
    }
}
