//! Bulk reading of the `$MFT` stream: a few large cluster-aligned reads instead of one filesystem
//! control per record. The elevated worker supplies the raw volume reads; this module only maps
//! record ordinals onto the stream's extents and keeps the contract of `Source::record_at_or_below`
//! (the nearest in-use record at or below the ordinal). Records are returned in their on-disk,
//! update-sequence-protected form, so `parse` validates every one exactly as before.
use super::{collection::CollectError, collection::RecordData, *};

/// Bytes read per request. At 1 KiB per record this replaces about a thousand controls.
pub const CHUNK_BYTES: u64 = 1024 * 1024;
/// `$MFT` extents accepted from record 0; a longer list falls back to per-record controls.
pub const MAX_EXTENTS: usize = MAX_RUNS_PER_ATTRIBUTE;

/// Raw, read-only access to the approved volume. Offsets and lengths are cluster multiples.
pub trait VolumeReader {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), CollectError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub vcn: u64,
    pub lcn: u64,
    pub clusters: u64,
}

/// Extents of the `$MFT`'s own unnamed `$DATA` stream, read from FILE record 0 (protected form,
/// as on disk or as returned by `normalize_kernel_record`). A sparse, compressed, resident or
/// incomplete stream is refused: the caller then keeps the per-record controls.
pub fn mft_extents(record0: &[u8], g: Geometry) -> Result<Vec<Extent>, ParseError> {
    validate_geometry(g)?;
    let mut total_runs = 0;
    let parsed = parse(
        &Record {
            number: 0,
            bytes: record0,
        },
        g,
        &mut total_runs,
    )?
    .ok_or(ParseError::InvalidHeader)?;
    let stream = parsed
        .data
        .iter()
        .find(|fragment| fragment.identity.key.name.is_none() && fragment.identity.key.start == 0)
        .ok_or(ParseError::InvalidAttribute)?;
    let Data::Nonresident {
        end,
        sparse: false,
        compressed: false,
        runs,
        ..
    } = &stream.data
    else {
        return Err(ParseError::InvalidAttribute);
    };
    if runs.is_empty() || runs.len() > MAX_EXTENTS {
        return Err(ParseError::BoundExceeded);
    }
    let mut vcn = 0_u64;
    let mut extents = Vec::with_capacity(runs.len());
    for &(lcn, clusters) in runs {
        extents.push(Extent { vcn, lcn, clusters });
        vcn = add(vcn, clusters)?;
    }
    // Without sparse runs every VCN maps to a cluster; the first segment must cover the stream.
    if vcn != *end {
        return Err(ParseError::InvalidRun);
    }
    Ok(extents)
}

/// A read-ahead window over the `$MFT` that serves records in descending order.
pub struct ChunkedMft<R> {
    reader: R,
    record_bytes: u64,
    cluster_bytes: u64,
    extents: Vec<Extent>,
    buffer: Vec<u8>,
    first: u64,
    count: u64,
}

impl<R: VolumeReader> ChunkedMft<R> {
    /// Accepts the common layout where a cluster holds whole records and the extents cover every
    /// record slot of the snapshot; any other layout keeps the per-record controls.
    pub fn new(
        reader: R,
        g: Geometry,
        extents: Vec<Extent>,
        record_slots: u64,
    ) -> Result<Self, CollectError> {
        validate_geometry(g)?;
        let record_bytes = g.record_bytes as u64;
        if g.cluster_bytes < record_bytes
            || !g.cluster_bytes.is_multiple_of(record_bytes)
            || !CHUNK_BYTES.is_multiple_of(g.cluster_bytes)
            || extents.is_empty()
            || extents.len() > MAX_EXTENTS
        {
            return Err(CollectError::Unsupported);
        }
        let mut vcn = 0_u64;
        for extent in &extents {
            let end = extent
                .lcn
                .checked_add(extent.clusters)
                .ok_or(CollectError::InvalidSource)?;
            if extent.vcn != vcn || extent.clusters == 0 || end > g.volume_clusters {
                return Err(CollectError::InvalidSource);
            }
            vcn += extent.clusters;
        }
        let needed = record_slots
            .checked_mul(record_bytes)
            .ok_or(CollectError::InvalidSource)?;
        if vcn
            .checked_mul(g.cluster_bytes)
            .is_none_or(|bytes| bytes < needed)
        {
            return Err(CollectError::InvalidSource);
        }
        Ok(Self {
            reader,
            record_bytes,
            cluster_bytes: g.cluster_bytes,
            extents,
            buffer: Vec::new(),
            first: 0,
            count: 0,
        })
    }

    /// Reads the window that ends with the cluster holding `ordinal`, within one extent.
    fn load(&mut self, ordinal: u64) -> Result<(), CollectError> {
        let vcn = ordinal
            .checked_mul(self.record_bytes)
            .ok_or(CollectError::InvalidSource)?
            / self.cluster_bytes;
        let extent = *self
            .extents
            .iter()
            .find(|e| (e.vcn..e.vcn + e.clusters).contains(&vcn))
            .ok_or(CollectError::InvalidSource)?;
        let window = CHUNK_BYTES / self.cluster_bytes;
        let start = extent.vcn.max((vcn + 1).saturating_sub(window));
        let clusters = vcn + 1 - start;
        let offset = (extent.lcn + (start - extent.vcn))
            .checked_mul(self.cluster_bytes)
            .ok_or(CollectError::InvalidSource)?;
        // Drop the previous window before reading: a failed read leaves nothing to reuse.
        self.count = 0;
        self.buffer
            .resize((clusters * self.cluster_bytes) as usize, 0);
        self.reader.read_at(offset, &mut self.buffer)?;
        self.first = start * self.cluster_bytes / self.record_bytes;
        self.count = clusters * self.cluster_bytes / self.record_bytes;
        Ok(())
    }

    /// The nearest in-use FILE record at or below `ordinal`, like FSCTL_GET_NTFS_FILE_RECORD.
    /// Unused, zeroed or non-FILE slots are skipped; damaged in-use records are still returned
    /// so the collector counts them as invalid.
    pub fn record_at_or_below(&mut self, ordinal: u64) -> Result<RecordData, CollectError> {
        let size = self.record_bytes as usize;
        let mut n = ordinal;
        loop {
            if !(self.first..self.first + self.count).contains(&n) {
                self.load(n)?;
            }
            let at = ((n - self.first) * self.record_bytes) as usize;
            let raw = &self.buffer[at..at + size];
            if &raw[..4] == b"FILE" && word(raw, 22).is_ok_and(|flags| flags & 1 != 0) {
                let sequence = word(raw, 16).map_err(CollectError::from)?;
                return Ok(RecordData {
                    number: n,
                    returned_sequence: (sequence != 0).then_some(sequence),
                    bytes: raw.to_vec(),
                });
            }
            // Record 0 ($MFT itself) is always in use: reaching it unused means a bad source.
            n = n.checked_sub(1).ok_or(CollectError::InvalidSource)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{geometry, nonresident, record, resident};
    use super::*;

    /// An in-memory volume: cluster-addressed bytes and a count of read requests.
    struct Image {
        bytes: Vec<u8>,
        reads: usize,
    }
    impl VolumeReader for Image {
        fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), CollectError> {
            self.reads += 1;
            let start = offset as usize;
            let slice = self
                .bytes
                .get(start..start + buffer.len())
                .ok_or(CollectError::Unavailable)?;
            buffer.copy_from_slice(slice);
            Ok(())
        }
    }

    const RECORD: usize = 1024;

    /// A volume of `clusters` 4 KiB clusters whose $MFT occupies `extents` (lcn, clusters); every
    /// slot listed in `in_use` holds a FILE record with a 10-byte resident stream.
    fn volume(
        clusters: u64,
        extents: &[(u64, u64)],
        in_use: &[u64],
    ) -> (Image, Geometry, Vec<Extent>) {
        let mut g = geometry(RECORD);
        g.volume_clusters = clusters;
        let mut bytes = vec![0_u8; (clusters * g.cluster_bytes) as usize];
        let mut vcn = 0;
        let mut map = Vec::new();
        for &(lcn, length) in extents {
            map.push(Extent {
                vcn,
                lcn,
                clusters: length,
            });
            vcn += length;
        }
        let per_cluster = g.cluster_bytes as usize / RECORD;
        for &n in in_use {
            let raw = record(RECORD, 3, 0, 1, &[resident(DATA, 10, 1)]);
            let cluster = n as usize / per_cluster;
            let extent = map
                .iter()
                .find(|e| (e.vcn..e.vcn + e.clusters).contains(&(cluster as u64)))
                .unwrap();
            let at = (extent.lcn as usize + cluster - extent.vcn as usize)
                * g.cluster_bytes as usize
                + (n as usize % per_cluster) * RECORD;
            bytes[at..at + RECORD].copy_from_slice(&raw);
        }
        (Image { bytes, reads: 0 }, g, map)
    }

    #[test]
    fn extents_come_from_record_zero_and_must_cover_the_stream() {
        let g = geometry(RECORD);
        let mut runs = vec![0x11, 4, 10, 0x11, 2, 20];
        runs.push(0);
        let zero = record(
            RECORD,
            1,
            0,
            1,
            &[nonresident(0, 6, 6 * 4096, 6 * 4096, 6 * 4096, 0, &runs)],
        );
        assert_eq!(
            mft_extents(&zero, g).unwrap(),
            [
                Extent {
                    vcn: 0,
                    lcn: 10,
                    clusters: 4
                },
                Extent {
                    vcn: 4,
                    lcn: 30,
                    clusters: 2
                }
            ]
        );
        // A resident or sparse $MFT stream is not trusted for raw reads.
        let small = record(RECORD, 1, 0, 1, &[resident(DATA, 10, 1)]);
        assert!(mft_extents(&small, g).is_err());
        let sparse = record(
            RECORD,
            1,
            0,
            1,
            &[nonresident(
                0,
                6,
                6 * 4096,
                6 * 4096,
                4 * 4096,
                0x8000,
                &[0x11, 4, 10, 0x01, 2, 0],
            )],
        );
        assert!(mft_extents(&sparse, g).is_err());
    }

    #[test]
    fn descending_reads_skip_unused_slots_across_fragmented_extents_in_few_requests() {
        // 64 slots over two fragments; slots 1 and 2 and every odd slot above 40 are unused.
        let in_use: Vec<u64> = (0..64)
            .filter(|n| !(1..=2).contains(n) && (*n <= 40 || n % 2 == 0))
            .collect();
        let (image, g, extents) = volume(200, &[(50, 10), (120, 6)], &in_use);
        let mut mft = ChunkedMft::new(image, g, extents, 64).unwrap();
        let mut seen = Vec::new();
        let mut cursor = 63;
        loop {
            let record = mft.record_at_or_below(cursor).unwrap();
            assert_eq!(record.bytes.len(), RECORD);
            assert_eq!(record.returned_sequence, Some(3));
            seen.push(record.number);
            match record.number.checked_sub(1) {
                Some(next) => cursor = next,
                None => break,
            }
        }
        let mut expected = in_use.clone();
        expected.reverse();
        assert_eq!(seen, expected);
        // One read per extent: 64 records cost two requests instead of 61 controls.
        assert_eq!(mft.reader.reads, 2);
        // Records stay in protected on-disk form and parse as before.
        let first = mft.record_at_or_below(63).unwrap();
        let mut runs = 0;
        assert!(parse(
            &Record {
                number: first.number,
                bytes: &first.bytes
            },
            g,
            &mut runs
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn layouts_and_sources_that_cannot_be_read_in_bulk_are_refused() {
        let (image, g, extents) = volume(100, &[(10, 4)], &[0]);
        // Extents must cover every slot of the snapshot.
        assert_eq!(
            ChunkedMft::new(image, g, extents.clone(), 17).err(),
            Some(CollectError::InvalidSource)
        );
        // Records larger than a cluster are left to the per-record controls.
        let (image, mut small, _) = volume(100, &[(10, 4)], &[0]);
        small.cluster_bytes = 512;
        assert_eq!(
            ChunkedMft::new(image, small, extents.clone(), 16).err(),
            Some(CollectError::Unsupported)
        );
        // Extents past the end of the volume are invalid.
        let (image, g, _) = volume(100, &[(10, 4)], &[0]);
        let outside = vec![Extent {
            vcn: 0,
            lcn: 98,
            clusters: 4,
        }];
        assert_eq!(
            ChunkedMft::new(image, g, outside, 16).err(),
            Some(CollectError::InvalidSource)
        );
        // A failed read reports the source as unavailable and keeps nothing stale.
        let (mut image, g, extents) = volume(100, &[(10, 4)], &[0, 5]);
        image.bytes.truncate(10 * 4096);
        let mut mft = ChunkedMft::new(image, g, extents, 16).unwrap();
        assert_eq!(
            mft.record_at_or_below(15).err(),
            Some(CollectError::Unavailable)
        );
        assert_eq!(mft.count, 0);
        // Reaching an unused record 0 means the source is wrong, never an empty volume.
        let (image, g, extents) = volume(100, &[(10, 4)], &[]);
        let mut empty = ChunkedMft::new(image, g, extents, 16).unwrap();
        assert_eq!(
            empty.record_at_or_below(15).err(),
            Some(CollectError::InvalidSource)
        );
    }
}
