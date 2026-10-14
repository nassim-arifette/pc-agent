//! Read-only NTFS controls, only used inside the separately approved elevated worker.
use super::{
    mft::{
        self,
        chunked::{self, ChunkedMft, VolumeReader},
        CollectError, Collection, CollectionBounds, Geometry, JournalWitness, RecordData, Source,
        VolumeSnapshot,
    },
    protocol::DriveLetter,
};
use std::{
    ffi::c_void,
    mem::{offset_of, size_of},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, GetDriveTypeW, GetFileType, GetVolumeInformationByHandleW, ReadFile,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK, OPEN_EXISTING,
    },
    System::{
        Ioctl::{
            FSCTL_GET_NTFS_FILE_RECORD, FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_QUERY_USN_JOURNAL,
            NTFS_EXTENDED_VOLUME_DATA, NTFS_FILE_RECORD_INPUT_BUFFER,
            NTFS_FILE_RECORD_OUTPUT_BUFFER, NTFS_VOLUME_DATA_BUFFER, USN_JOURNAL_DATA_V0,
        },
        WindowsProgramming::DRIVE_FIXED,
        IO::{DeviceIoControl, OVERLAPPED},
    },
};

#[repr(C)]
#[derive(Default)]
struct VolumeData {
    basic: NTFS_VOLUME_DATA_BUFFER,
    extended: NTFS_EXTENDED_VOLUME_DATA,
}

fn last_error() -> CollectError {
    // SAFETY: GetLastError has no arguments or memory preconditions.
    let code = unsafe { GetLastError() };
    match code {
        5 | 1314 => CollectError::Denied,
        1 | 50 => CollectError::Unsupported,
        _ => CollectError::Windows(code),
    }
}

/// Raw reads on the approved volume handle, for the bulk `$MFT` reader.
struct VolumeHandle(HANDLE);
impl VolumeReader for VolumeHandle {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), CollectError> {
        let length = u32::try_from(buffer.len()).map_err(|_| CollectError::InvalidBounds)?;
        let mut overlapped = OVERLAPPED::default();
        overlapped.Anonymous.Anonymous.Offset = offset as u32;
        overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        let mut read = 0;
        // SAFETY: the handle is the live volume handle owned by NativeSource for this reader's
        // whole lifetime; buffer is a writable slice of exactly `length` bytes; the OVERLAPPED
        // only carries the offset of a synchronous read (the handle was not opened overlapped).
        // ReadFile on a volume handle reads sectors and never modifies the volume.
        let ok = unsafe {
            ReadFile(
                self.0,
                buffer.as_mut_ptr(),
                length,
                &mut read,
                &mut overlapped,
            )
        };
        if ok == 0 {
            return Err(last_error());
        }
        if read != length {
            return Err(CollectError::Unavailable);
        }
        Ok(())
    }
}

struct NativeSource {
    volume: OwnedHandle,
    geometry: Option<Geometry>,
    /// Bulk reader over the `$MFT` extents; `None` keeps one control per record.
    chunked: Option<ChunkedMft<VolumeHandle>>,
    /// Set once a bulk setup or read failed, so it is not retried during this collection.
    chunked_failed: bool,
}
impl NativeSource {
    fn open(letter: DriveLetter) -> Result<Self, CollectError> {
        // Only one approved drive letter can form the device path; no caller-supplied path.
        let root: Vec<u16> = format!("{}:\\", letter.letter())
            .encode_utf16()
            .chain([0])
            .collect();
        // SAFETY: root is a live null-terminated UTF-16 drive-root string.
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
            return Err(CollectError::Unsupported);
        }
        let path: Vec<u16> = format!("\\\\.\\{}:", letter.letter())
            .encode_utf16()
            .chain([0])
            .collect();
        // SAFETY: path is a fixed, validated, null-terminated volume path; other pointers are null.
        // GENERIC_READ and OPEN_EXISTING never create/write, lock, dismount, or change a volume.
        let raw = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(last_error());
        }
        // SAFETY: CreateFileW returned a unique valid owned handle; it is closed on all exits.
        let volume = unsafe { OwnedHandle::from_raw_handle(raw) };
        // SAFETY: the handle is live and GetFileType does not modify it.
        if unsafe { GetFileType(volume.as_raw_handle()) } != FILE_TYPE_DISK {
            return Err(CollectError::Unsupported);
        }
        let mut filesystem = [0_u16; 16];
        // SAFETY: the writable filesystem buffer is correctly sized; unwanted serial/name outputs
        // are null. The live pinned volume handle prevents switching the target between controls.
        if unsafe {
            GetVolumeInformationByHandleW(
                volume.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                filesystem.as_mut_ptr(),
                filesystem.len() as u32,
            )
        } == 0
        {
            return Err(last_error());
        }
        if filesystem[..5]
            != [
                u16::from(b'N'),
                u16::from(b'T'),
                u16::from(b'F'),
                u16::from(b'S'),
                0,
            ]
        {
            return Err(CollectError::Unsupported);
        }
        Ok(Self {
            volume,
            geometry: None,
            chunked: None,
            chunked_failed: false,
        })
    }

    /// Sets up bulk reading from record 0's `$DATA` extents; any doubt keeps the controls.
    fn start_chunked(&mut self, g: Geometry, record_slots: u64) {
        if self.chunked.is_some() || self.chunked_failed {
            return;
        }
        let ready = self
            .fsctl_record(0)
            .ok()
            .filter(|zero| zero.number == 0)
            .and_then(|zero| chunked::mft_extents(&zero.bytes, g).ok())
            .and_then(|extents| {
                ChunkedMft::new(VolumeHandle(self.handle()), g, extents, record_slots).ok()
            });
        self.chunked_failed = ready.is_none();
        self.chunked = ready;
    }

    fn handle(&self) -> HANDLE {
        self.volume.as_raw_handle()
    }

    fn journal(&self) -> Option<JournalWitness> {
        let mut journal = USN_JOURNAL_DATA_V0::default();
        let mut returned = 0;
        // SAFETY: journal and returned are valid writable buffers of the exact declared sizes.
        // FSCTL_QUERY_USN_JOURNAL only queries an existing journal; none is created/read/changed.
        let ok = unsafe {
            DeviceIoControl(
                self.handle(),
                FSCTL_QUERY_USN_JOURNAL,
                null(),
                0,
                (&mut journal as *mut USN_JOURNAL_DATA_V0).cast::<c_void>(),
                size_of::<USN_JOURNAL_DATA_V0>() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0
            || returned as usize != size_of::<USN_JOURNAL_DATA_V0>()
            || journal.NextUsn < 0
            || journal.LowestValidUsn < 0
        {
            return None;
        }
        Some(JournalWitness {
            journal_id: journal.UsnJournalID,
            next_usn: journal.NextUsn as u64,
            lowest_valid_usn: journal.LowestValidUsn as u64,
        })
    }
}

impl Source for NativeSource {
    fn snapshot(&mut self) -> Result<VolumeSnapshot, CollectError> {
        let mut data = VolumeData::default();
        let mut returned = 0;
        // SAFETY: the repr(C) structures are laid out consecutively in an aligned owned buffer.
        // All pointers/lengths stay valid during this synchronous read-only filesystem control.
        if unsafe {
            DeviceIoControl(
                self.handle(),
                FSCTL_GET_NTFS_VOLUME_DATA,
                null(),
                0,
                (&mut data as *mut VolumeData).cast::<c_void>(),
                size_of::<VolumeData>() as u32,
                &mut returned,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        if (returned as usize) < size_of::<NTFS_VOLUME_DATA_BUFFER>() + 8
            || returned as usize > size_of::<VolumeData>()
            || data.extended.ByteCount < 8
            || data.basic.MftValidDataLength <= 0
            || data.basic.TotalClusters <= 0
            || data.basic.BytesPerSector < 512
            || !data.basic.BytesPerSector.is_power_of_two()
        {
            return Err(CollectError::InvalidSource);
        }
        let geometry = Geometry {
            ntfs_major: u8::try_from(data.extended.MajorVersion)
                .map_err(|_| CollectError::Unsupported)?,
            ntfs_minor: u8::try_from(data.extended.MinorVersion)
                .map_err(|_| CollectError::Unsupported)?,
            record_bytes: data.basic.BytesPerFileRecordSegment as usize,
            cluster_bytes: u64::from(data.basic.BytesPerCluster),
            volume_clusters: data.basic.TotalClusters as u64,
        };
        if geometry.record_bytes == 0
            || !(data.basic.MftValidDataLength as u64).is_multiple_of(geometry.record_bytes as u64)
            || data.basic.BytesPerCluster % data.basic.BytesPerSector != 0
        {
            return Err(CollectError::InvalidSource);
        }
        self.geometry.get_or_insert(geometry);
        let record_slots = data.basic.MftValidDataLength as u64 / geometry.record_bytes as u64;
        self.start_chunked(geometry, record_slots);
        Ok(VolumeSnapshot {
            geometry,
            record_slots,
            journal: self.journal(),
        })
    }

    fn record_at_or_below(&mut self, ordinal: u64) -> Result<RecordData, CollectError> {
        if let Some(chunked) = self.chunked.as_mut() {
            match chunked.record_at_or_below(ordinal) {
                Ok(record) => return Ok(record),
                // A failed bulk read falls back to the per-record control for the rest.
                Err(_) => {
                    self.chunked = None;
                    self.chunked_failed = true;
                }
            }
        }
        self.fsctl_record(ordinal)
    }
}

impl NativeSource {
    /// One FSCTL_GET_NTFS_FILE_RECORD: the nearest in-use record at or below `ordinal`.
    fn fsctl_record(&mut self, ordinal: u64) -> Result<RecordData, CollectError> {
        let g = self.geometry.ok_or(CollectError::InvalidSource)?;
        if ordinal >= 1_u64 << 48 || g.record_bytes > mft::MAX_RECORD_BYTES {
            return Err(CollectError::InvalidSource);
        }
        let input = NTFS_FILE_RECORD_INPUT_BUFFER {
            FileReferenceNumber: ordinal as i64,
        };
        let mut output =
            vec![0_u8; size_of::<NTFS_FILE_RECORD_OUTPUT_BUFFER>() + g.record_bytes - 1];
        let mut returned = 0;
        // SAFETY: input and bounded output are live buffers with the exact advertised sizes;
        // no OVERLAPPED is supplied. This filesystem control only returns record metadata.
        if unsafe {
            DeviceIoControl(
                self.handle(),
                FSCTL_GET_NTFS_FILE_RECORD,
                (&input as *const NTFS_FILE_RECORD_INPUT_BUFFER).cast::<c_void>(),
                size_of::<NTFS_FILE_RECORD_INPUT_BUFFER>() as u32,
                output.as_mut_ptr().cast::<c_void>(),
                output.len() as u32,
                &mut returned,
                null_mut(),
            )
        } == 0
        {
            return Err(last_error());
        }
        let header = offset_of!(NTFS_FILE_RECORD_OUTPUT_BUFFER, FileRecordBuffer);
        if (returned as usize) < header || returned as usize > output.len() {
            return Err(CollectError::InvalidSource);
        }
        let reference = u64::from_le_bytes(output[..8].try_into().unwrap());
        let length = u32::from_le_bytes(output[8..12].try_into().unwrap()) as usize;
        if length != g.record_bytes || header + length > returned as usize {
            return Err(CollectError::InvalidSource);
        }
        let raw = &output[header..header + length];
        // The portable normalizer validates either all protected or all restored USA tails.
        // On failure retain the bounded bytes solely to let the scanner count this invalid record.
        let bytes = mft::normalize_kernel_record(raw, g).unwrap_or_else(|_| raw.to_vec());
        let sequence = (reference >> 48) as u16;
        Ok(RecordData {
            number: reference & ((1_u64 << 48) - 1),
            returned_sequence: (sequence != 0).then_some(sequence),
            bytes,
        })
    }
}

pub(crate) fn collect(
    volume: DriveLetter,
    budget: Duration,
    cancelled: &mut dyn FnMut() -> Result<bool, CollectError>,
) -> Result<Collection, CollectError> {
    // Defense in depth: this adapter never opens a volume from an ordinary host process.
    if !super::native::is_elevated().map_err(|_| CollectError::Denied)? {
        return Err(CollectError::Denied);
    }
    let bounds = CollectionBounds::for_budget(budget);
    if budget.is_zero() || budget > Duration::from_secs(120) {
        return Err(CollectError::InvalidBounds);
    }
    if cancelled()? {
        return Ok(Collection::cancelled_before_open());
    }
    // Account for the open/query time in the worker's outer hard deadline as well as this budget.
    let mut source = NativeSource::open(volume)?;
    // The cancel check reads the job's marker file. The collector asks before and after every
    // record, which on a million-record volume would add millions of filesystem calls; the
    // helper's watchdog already polls the marker every 50 ms, so ask at most that often.
    // Once cancellation is seen it stays seen.
    let mut checked: Option<Instant> = None;
    let mut seen = false;
    let mut throttled = || {
        if !seen && checked.is_none_or(|at| at.elapsed() >= CANCEL_CHECK_INTERVAL) {
            checked = Some(Instant::now());
            seen = cancelled()?;
        }
        Ok(seen)
    };
    mft::collect(&mut source, bounds, &mut throttled)
}

/// Longest wait between two reads of the cancel marker during a collection.
const CANCEL_CHECK_INTERVAL: Duration = Duration::from_millis(50);
