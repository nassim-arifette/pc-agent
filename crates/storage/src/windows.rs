//! The only unsafe boundary: read-only metadata queries. Handles are opened with
//! `FILE_READ_ATTRIBUTES` only, as the reparse point itself, and closed by RAII.
use crate::{
    index::VolumeSpace,
    scan::{classify_attributes, AttributeClass, FileFacts, ATTR_COMPRESSED, ATTR_SPARSE_FILE},
    StorageError,
};
use pcagent_shared::Status;
use std::{
    fs::Metadata,
    io,
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FileBasicInfo, FileIdInfo, FileStandardInfo, GetDiskFreeSpaceExW,
        GetDriveTypeW, GetFileInformationByHandleEx, GetVolumeInformationW, GetVolumePathNameW,
        FILE_BASIC_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_INFO_BY_HANDLE_CLASS, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
    },
};

// GetDriveTypeW results (WinBase.h); kept local to avoid another windows-sys feature.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_RAMDISK: u32 = 6;
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VolumeId(u64);

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateFileW and has exactly one owning wrapper.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn open_attributes(path: &Path) -> io::Result<OwnedHandle> {
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated and outlives the call. Only FILE_READ_ATTRIBUTES is
    // requested (no data access) and FILE_FLAG_OPEN_REPARSE_POINT opens links as themselves.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(handle))
}

/// # Safety
/// `T` must be the documented output structure for `class`.
unsafe fn query<T>(handle: &OwnedHandle, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
    // SAFETY: all callers pass plain-data structures for which all-zero bytes are valid.
    let mut value: T = unsafe { zeroed() };
    // SAFETY: the buffer is writable for size_of::<T>() bytes and the handle is open.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            class,
            (&mut value as *mut T).cast(),
            size_of::<T>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn id_info(handle: &OwnedHandle) -> io::Result<FILE_ID_INFO> {
    // SAFETY: FileIdInfo returns FILE_ID_INFO.
    unsafe { query(handle, FileIdInfo) }
}

fn volume_path(root: &Path) -> io::Result<Vec<u16>> {
    let name = wide(root);
    let mut buffer = vec![0_u16; 32768];
    // SAFETY: `name` is NUL-terminated; the output buffer length is passed in characters.
    if unsafe { GetVolumePathNameW(name.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    buffer.truncate(len);
    buffer.push(0);
    Ok(buffer)
}

pub(crate) fn check_root(root: &Path) -> Result<VolumeId, StorageError> {
    let text = root.as_os_str().to_string_lossy();
    if text.starts_with(r"\\?\UNC\") || (text.starts_with(r"\\") && !text.starts_with(r"\\?\")) {
        return Err(StorageError::UnsupportedVolume("network path"));
    }
    let volume = volume_path(root)?;
    // SAFETY: `volume` is a NUL-terminated volume root returned by GetVolumePathNameW.
    match unsafe { GetDriveTypeW(volume.as_ptr()) } {
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK => {}
        DRIVE_REMOTE => return Err(StorageError::UnsupportedVolume("network drive")),
        _ => {
            return Err(StorageError::UnsupportedVolume(
                "unknown or unmounted drive type",
            ))
        }
    }
    let info = id_info(&open_attributes(root)?)?;
    Ok(VolumeId(info.VolumeSerialNumber))
}

pub(crate) fn volume_space(root: &Path) -> VolumeSpace {
    const SOURCE: &str = "Win32.GetDiskFreeSpaceExW";
    let name = wide(root);
    let (mut caller_free, mut total, mut free) = (0_u64, 0_u64, 0_u64);
    // SAFETY: `name` is NUL-terminated and all three outputs point to valid u64 values.
    let ok = unsafe { GetDiskFreeSpaceExW(name.as_ptr(), &mut caller_free, &mut total, &mut free) };
    let filesystem = volume_path(root).ok().and_then(|volume| {
        let mut fs_name = [0_u16; 64];
        // SAFETY: `volume` is NUL-terminated; unused outputs are null; the file-system name
        // buffer length is passed in characters.
        let ok = unsafe {
            GetVolumeInformationW(
                volume.as_ptr(),
                null_mut(),
                0,
                null_mut(),
                null_mut(),
                null_mut(),
                fs_name.as_mut_ptr(),
                fs_name.len() as u32,
            )
        };
        let len = fs_name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(fs_name.len());
        (ok != 0).then(|| String::from_utf16_lossy(&fs_name[..len]))
    });
    if ok == 0 {
        return VolumeSpace {
            total_bytes: None,
            free_bytes: None,
            occupied_bytes: None,
            filesystem,
            source: SOURCE.into(),
            status: if io::Error::last_os_error().kind() == io::ErrorKind::PermissionDenied {
                Status::PermissionDenied
            } else {
                Status::Error
            },
            limitations: vec!["Volume space query failed.".into()],
        };
    }
    let occupied = total.checked_sub(free);
    VolumeSpace {
        total_bytes: Some(total),
        free_bytes: Some(free),
        occupied_bytes: occupied,
        filesystem,
        source: SOURCE.into(),
        status: if occupied.is_some() { Status::Ok } else { Status::Error },
        limitations: vec![
            "Free space is volume-wide; per-user quotas may allow less.".into(),
            "Occupied space includes file-system metadata, system files and areas outside the scope.".into(),
        ],
    }
}

pub(crate) fn profile_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Uses attributes captured by directory enumeration (FindFirstFileExW); nothing is opened.
pub(crate) fn classify(meta: &Metadata) -> AttributeClass {
    classify_attributes(meta.file_attributes())
}

/// Mount points are reparse points and are never entered, so a directory cannot change volume.
pub(crate) fn dir_on_other_volume(_meta: &Metadata, _volume: VolumeId) -> bool {
    false
}

fn filetime_ms(t: i64) -> Option<u64> {
    u64::try_from(t)
        .ok()?
        .checked_sub(FILETIME_UNIX_EPOCH)
        .map(|v| v / 10_000)
}

/// Only called for non-placeholder, non-reparse entries.
pub(crate) fn file_facts(path: &Path, meta: &Metadata, volume: VolumeId) -> io::Result<FileFacts> {
    let handle = open_attributes(path)?;
    let id = id_info(&handle)?;
    // SAFETY: FileStandardInfo returns FILE_STANDARD_INFO.
    let standard: FILE_STANDARD_INFO = unsafe { query(&handle, FileStandardInfo)? };
    // SAFETY: FileBasicInfo returns FILE_BASIC_INFO.
    let basic: FILE_BASIC_INFO = unsafe { query(&handle, FileBasicInfo)? };
    Ok(FileFacts {
        logical: u64::try_from(standard.EndOfFile).unwrap_or(meta.len()),
        allocated: u64::try_from(standard.AllocationSize).ok(),
        key: Some((
            id.VolumeSerialNumber,
            u128::from_le_bytes(id.FileId.Identifier),
        )),
        links: Some(standard.NumberOfLinks),
        modified_ms: filetime_ms(basic.LastWriteTime),
        sparse: basic.FileAttributes & ATTR_SPARSE_FILE != 0,
        compressed: basic.FileAttributes & ATTR_COMPRESSED != 0,
        other_volume: id.VolumeSerialNumber != volume.0,
    })
}
