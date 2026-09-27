//! Read-only native boundary. Enumeration and child opens are relative to held directory
//! handles; no path lookup can follow a replaced ancestor. File data is never opened.
use crate::{
    index::VolumeSpace,
    scan::{
        classify_attributes, AttributeClass, EntryFacts, FileFacts, ATTR_COMPRESSED,
        ATTR_SPARSE_FILE,
    },
    StorageError,
};
use pcagent_shared::Status;
#[cfg(test)]
use std::fs;
use std::{
    ffi::{OsStr, OsString},
    io,
    mem::{offset_of, size_of, zeroed},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Wdk::{
    Foundation::OBJECT_ATTRIBUTES,
    Storage::FileSystem::{
        NtCreateFile, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
        FILE_OPEN_NO_RECALL, FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    },
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, RtlNtStatusToDosError, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE,
        OBJ_DONT_REPARSE, UNICODE_STRING,
    },
    Storage::FileSystem::{
        CreateFileW, FileBasicInfo, FileIdBothDirectoryInfo, FileIdInfo, FileStandardInfo,
        GetDiskFreeSpaceExW, GetDriveTypeW, GetFileInformationByHandleEx,
        GetFinalPathNameByHandleW, GetVolumeInformationW, GetVolumePathNameW, FILE_BASIC_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO,
        FILE_ID_INFO, FILE_INFO_BY_HANDLE_CLASS, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
    },
    System::IO::IO_STATUS_BLOCK,
};

// GetDriveTypeW results (WinBase.h); kept local to avoid another windows-sys feature.
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const DRIVE_REMOTE: u32 = 4;
const DRIVE_RAMDISK: u32 = 6;
const FILETIME_UNIX_EPOCH: u64 = 116_444_736_000_000_000;
const SYNCHRONIZE: u32 = 0x0010_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VolumeId(u64);

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: CreateFileW/NtCreateFile returned this handle; there is exactly one owner.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn open_attributes(path: &Path) -> io::Result<OwnedHandle> {
    open_attributes_sharing(path, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
}
fn open_attributes_sharing(path: &Path, share: u32) -> io::Result<OwnedHandle> {
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated and outlives the call. Only FILE_READ_ATTRIBUTES is
    // requested (no data access) and FILE_FLAG_OPEN_REPARSE_POINT opens links as themselves.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES,
            share,
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

/// The buffer is bounded per active DFS frame. Ancestor pins prevent renames; handle-relative
/// opens also resist attribute-only reparse changes (which share flags do not prevent).
pub(crate) struct Directory {
    handle: OwnedHandle,
    ancestors: Vec<OwnedHandle>,
    buffer: Box<[u64; 1024]>,
    offset: Option<usize>,
    exhausted: bool,
}
pub(crate) struct Child {
    name: OsString,
    attributes: u32,
    logical: u64,
    modified_ms: Option<u64>,
}
pub(crate) fn child_name(child: &Child) -> OsString {
    child.name.clone()
}
impl Directory {
    pub fn estimated_bytes(&self) -> u64 {
        8192 + self.ancestors.len() as u64 * 32
    }
    pub fn open_root(path: &Path, expected: Option<(u64, u128)>) -> io::Result<Self> {
        let paths: Vec<_> = path.ancestors().collect();
        let mut parts = paths.iter().rev();
        let anchor = parts.next().ok_or_else(|| invalid("empty scope"))?;
        let mut handle = open_directory(anchor, paths.len() == 1)?;
        let mut ancestors = Vec::new();
        for path in parts {
            let child = open_relative(
                &handle,
                path.file_name()
                    .ok_or_else(|| invalid("noncanonical scope"))?,
                true,
                *path == paths[0],
            )?;
            ensure_directory(&child)?;
            ancestors.push(handle);
            handle = child;
        }
        let mut result = Self::from_handle(handle);
        result.ancestors = ancestors;
        if expected.is_some_and(|key| result.identity().ok() != Some(key)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "authorized root was replaced",
            ));
        }
        Ok(result)
    }
    fn from_handle(handle: OwnedHandle) -> Self {
        Self {
            handle,
            ancestors: Vec::new(),
            buffer: Box::new([0; 1024]),
            offset: None,
            exhausted: false,
        }
    }
    pub fn identity(&self) -> io::Result<(u64, u128)> {
        let id = id_info(&self.handle)?;
        Ok((
            id.VolumeSerialNumber,
            u128::from_le_bytes(id.FileId.Identifier),
        ))
    }
    pub fn next(&mut self) -> Option<io::Result<Child>> {
        loop {
            if self.exhausted {
                return None;
            }
            let offset = match self.offset.take() {
                Some(offset) => offset,
                None => {
                    self.buffer.fill(0);
                    // SAFETY: the buffer is 8-byte aligned and writable for its full size.
                    let ok = unsafe {
                        GetFileInformationByHandleEx(
                            self.handle.0,
                            FileIdBothDirectoryInfo,
                            self.buffer.as_mut_ptr().cast(),
                            8192,
                        )
                    };
                    if ok == 0 {
                        self.exhausted = true;
                        let error = io::Error::last_os_error();
                        return if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                            None
                        } else {
                            Some(Err(error))
                        };
                    }
                    0
                }
            };
            match self.decode(offset) {
                Ok(child) if child.name == "." || child.name == ".." => continue,
                result => {
                    if result.is_err() {
                        self.exhausted = true;
                    }
                    return Some(result);
                }
            }
        }
    }
    fn decode(&mut self, offset: usize) -> io::Result<Child> {
        if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > 8192 {
            return Err(invalid("directory record"));
        }
        // SAFETY: the entire fixed header lies inside the initialized buffer. Read unaligned
        // to avoid relying on a driver-provided NextEntryOffset for Rust reference alignment.
        let raw = unsafe {
            self.buffer
                .as_ptr()
                .cast::<u8>()
                .add(offset)
                .cast::<FILE_ID_BOTH_DIR_INFO>()
                .read_unaligned()
        };
        let name_start = offset + offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
        let name_end = name_start + raw.FileNameLength as usize;
        let next = raw.NextEntryOffset as usize;
        if !raw.FileNameLength.is_multiple_of(2)
            || name_end > 8192
            || (next != 0
                && (next < size_of::<FILE_ID_BOTH_DIR_INFO>()
                    || offset + next > 8192
                    || name_end > offset + next))
        {
            return Err(invalid("directory name bounds"));
        }
        // SAFETY: UTF-16 elements start at an even offset and all bytes lie within the buffer.
        if !name_start.is_multiple_of(2) {
            return Err(invalid("directory name alignment"));
        }
        let name = unsafe {
            std::slice::from_raw_parts(
                self.buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(name_start)
                    .cast::<u16>(),
                raw.FileNameLength as usize / 2,
            )
        };
        self.offset = (next != 0).then_some(offset + next);
        Ok(Child {
            name: OsString::from_wide(name),
            attributes: raw.FileAttributes,
            logical: u64::try_from(raw.EndOfFile).unwrap_or(0),
            modified_ms: filetime_ms(raw.LastWriteTime),
        })
    }
    pub fn child(&self, name: &OsStr, volume: VolumeId) -> io::Result<Self> {
        let handle = open_relative(&self.handle, name, true, true)?;
        ensure_directory(&handle)?;
        if id_info(&handle)?.VolumeSerialNumber != volume.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "different volume",
            ));
        }
        Ok(Self::from_handle(handle))
    }
    pub fn inspect(&self, child: &Child, volume: VolumeId) -> io::Result<EntryFacts> {
        // RECALL_ON_OPEN is an enumeration-only attribute. Classify before any child opens.
        inspect_enumerated(child.attributes, child.logical, child.modified_ms, || {
            let handle = open_relative(&self.handle, &child.name, false, false)?;
            file_facts(&handle, volume)
        })
    }
}
fn inspect_enumerated(
    attributes: u32,
    logical: u64,
    modified_ms: Option<u64>,
    query: impl FnOnce() -> io::Result<FileFacts>,
) -> io::Result<EntryFacts> {
    let class = classify_attributes(attributes);
    let facts = if class == AttributeClass::File {
        Some(query()?)
    } else {
        None
    };
    Ok(EntryFacts {
        class,
        logical,
        modified_ms,
        facts,
        other_volume: false,
    })
}

#[test]
fn enumerated_placeholders_and_reparse_entries_never_open_children() {
    for attributes in [
        crate::ATTR_RECALL_ON_OPEN,
        crate::ATTR_RECALL_ON_DATA_ACCESS,
        crate::ATTR_OFFLINE,
        crate::ATTR_REPARSE_POINT,
    ] {
        let observed = inspect_enumerated(attributes, 42, None, || {
            panic!("must classify enumeration attributes before any child handle opens")
        })
        .unwrap();
        assert!(matches!(
            observed.class,
            AttributeClass::Placeholder | AttributeClass::Reparse
        ));
        assert!(observed.facts.is_none());
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn ensure_directory(pin: &OwnedHandle) -> io::Result<()> {
    // SAFETY: FileBasicInfo returns FILE_BASIC_INFO.
    let basic: FILE_BASIC_INFO = unsafe { query(pin, FileBasicInfo)? };
    if classify_attributes(basic.FileAttributes) != AttributeClass::Dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directory is redirected or unavailable",
        ));
    }
    Ok(())
}
fn open_directory(path: &Path, enumerate: bool) -> io::Result<OwnedHandle> {
    let name = wide(path);
    // SAFETY: name is NUL terminated. Only metadata and (for the scan root) directory-list
    // access is requested, never file-data access. This is the volume anchor, not a child path.
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES | if enumerate { FILE_LIST_DIRECTORY } else { 0 },
            FILE_SHARE_READ,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let handle = OwnedHandle(raw);
    ensure_directory(&handle)?;
    Ok(handle)
}
fn open_relative(
    parent: &OwnedHandle,
    name: &OsStr,
    directory: bool,
    enumerate: bool,
) -> io::Result<OwnedHandle> {
    let mut name: Vec<u16> = name.encode_wide().collect();
    if name.is_empty()
        || name.iter().any(|c| matches!(*c, 0 | 47 | 58 | 92))
        || name == [46]
        || name == [46, 46]
    {
        return Err(invalid("child must be a single relative name"));
    }
    let bytes = u16::try_from(name.len() * 2).map_err(|_| invalid("child name length"))?;
    let string = UNICODE_STRING {
        Length: bytes,
        MaximumLength: bytes,
        Buffer: name.as_mut_ptr(),
    };
    let attrs = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.0,
        ObjectName: &string,
        Attributes: OBJ_DONT_REPARSE,
        SecurityDescriptor: null(),
        SecurityQualityOfService: null(),
    };
    let mut raw = null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: all input buffers outlive this synchronous call. RootDirectory is held open;
    // FILE_OPEN never creates, no data/write access is requested, and reparsing is disabled.
    let result = unsafe {
        NtCreateFile(
            &mut raw,
            FILE_READ_ATTRIBUTES | SYNCHRONIZE | if enumerate { FILE_LIST_DIRECTORY } else { 0 },
            &attrs,
            &mut status,
            null(),
            0,
            if directory {
                FILE_SHARE_READ
            } else {
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
            },
            FILE_OPEN,
            FILE_OPEN_REPARSE_POINT
                | FILE_OPEN_FOR_BACKUP_INTENT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | if directory {
                    FILE_DIRECTORY_FILE
                } else {
                    FILE_OPEN_NO_RECALL
                },
            null(),
            0,
        )
    };
    if result < 0 {
        // SAFETY: this routine only translates a returned NTSTATUS into a Win32 error code.
        return Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ));
    }
    Ok(OwnedHandle(raw))
}
pub(crate) fn authorize_root(root: &Path) -> Result<(PathBuf, Directory), StorageError> {
    authorize_after_capture(root, || {})
}
fn authorize_after_capture(
    root: &Path,
    after_capture: impl FnOnce(),
) -> Result<(PathBuf, Directory), StorageError> {
    if root.as_os_str().encode_wide().any(|c| c == 0) {
        return Err(StorageError::Io("invalid root name".into()));
    }
    let captured = open_attributes_sharing(root, FILE_SHARE_READ)?;
    // SAFETY: FileBasicInfo returns FILE_BASIC_INFO for this still-open object.
    let basic: FILE_BASIC_INFO = unsafe { query(&captured, FileBasicInfo)? };
    match classify_attributes(basic.FileAttributes) {
        AttributeClass::Reparse | AttributeClass::Placeholder => {
            return Err(StorageError::ReparseRoot)
        }
        AttributeClass::Dir => {}
        _ => return Err(StorageError::NotADirectory),
    }
    let id = id_info(&captured)?;
    let identity = (
        id.VolumeSerialNumber,
        u128::from_le_bytes(id.FileId.Identifier),
    );
    after_capture();
    let mut path = vec![0_u16; 32768];
    // SAFETY: output capacity is in UTF-16 characters; the handle identifies the captured root.
    let len =
        unsafe { GetFinalPathNameByHandleW(captured.0, path.as_mut_ptr(), path.len() as u32, 0) }
            as usize;
    if len == 0 || len >= path.len() {
        return Err(StorageError::Io("canonical root unavailable".into()));
    }
    let root = PathBuf::from(OsString::from_wide(&path[..len]));
    let pinned = Directory::open_root(&root, Some(identity))?;
    Ok((root, pinned))
}

#[test]
fn attribute_only_junction_change_cannot_redirect_an_active_scan() {
    use crate::{CancelToken, ClassicBackend, ScanBackend, ScanOptions, ScanScope};
    let base = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-reparse-test"));
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(root.join("same.bin"), [0; 10]).unwrap();
    fs::write(outside.join("same.bin"), [0; 1000]).unwrap();
    let scope = ScanScope::authorize(&root).unwrap();
    let mut attempted = false;
    let mut redirected = false;
    let index = ClassicBackend.scan(
        &scope,
        &ScanOptions::default(),
        &CancelToken::default(),
        &mut |_| {
            if std::mem::replace(&mut attempted, true) {
                return;
            }
            fs::remove_file(root.join("same.bin")).unwrap();
            redirected = set_test_junction(&root, &outside).is_ok();
        },
    );
    // Remove the junction itself before recursively cleaning the disposable fixture.
    if redirected {
        fs::remove_dir(&root).unwrap();
    }
    fs::remove_dir_all(&base).unwrap();
    assert!(attempted);
    assert_eq!(
        index.totals().files,
        0,
        "must not read same.bin through the changed root"
    );
}

#[test]
fn replacement_during_authorization_cannot_grant_the_new_target() {
    let base = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-authorize-test"));
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let mut redirected = false;
    let result = authorize_after_capture(&root, || {
        redirected = set_test_junction(&root, &outside).is_ok();
    });
    assert!(
        redirected,
        "fixture must exercise the attribute-only mutation"
    );
    assert!(
        result.is_err(),
        "authorization must stay bound to the captured non-link object"
    );
    fs::remove_dir(&root).unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[cfg(test)]
fn set_test_junction(root: &Path, outside: &Path) -> io::Result<()> {
    use windows_sys::Win32::{
        Storage::FileSystem::FILE_WRITE_ATTRIBUTES, System::IO::DeviceIoControl,
    };
    let name = wide(root);
    // SAFETY: opens only a disposable test directory; the path is NUL terminated.
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_WRITE_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let handle = OwnedHandle(raw);
    let target: Vec<u16> = OsString::from(format!(r"\??\{}", outside.display()))
        .encode_wide()
        .collect();
    let bytes = (target.len() * 2) as u16;
    let mut buffer = Vec::new();
    buffer.extend_from_slice(&0xa0000003_u32.to_le_bytes()); // IO_REPARSE_TAG_MOUNT_POINT
    for value in [bytes + 12, 0, 0, bytes, bytes + 2, 0] {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    for value in target.into_iter().chain([0, 0]) {
        buffer.extend_from_slice(&value.to_le_bytes());
    }
    let mut returned = 0;
    // SAFETY: the mount-point buffer targets another disposable test directory and outlives the call.
    let ok = unsafe {
        DeviceIoControl(
            handle.0,
            0x900a4,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
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

fn filetime_ms(t: i64) -> Option<u64> {
    u64::try_from(t)
        .ok()?
        .checked_sub(FILETIME_UNIX_EPOCH)
        .map(|v| v / 10_000)
}

/// Only called for non-placeholder, non-reparse entries.
fn file_facts(handle: &OwnedHandle, volume: VolumeId) -> io::Result<FileFacts> {
    let id = id_info(handle)?;
    // SAFETY: FileStandardInfo returns FILE_STANDARD_INFO.
    let standard: FILE_STANDARD_INFO = unsafe { query(handle, FileStandardInfo)? };
    // SAFETY: FileBasicInfo returns FILE_BASIC_INFO.
    let basic: FILE_BASIC_INFO = unsafe { query(handle, FileBasicInfo)? };
    if classify_attributes(basic.FileAttributes) != AttributeClass::File {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file changed type during scan",
        ));
    }
    Ok(FileFacts {
        logical: u64::try_from(standard.EndOfFile).map_err(|_| invalid("negative file size"))?,
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
