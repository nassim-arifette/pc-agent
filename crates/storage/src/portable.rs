//! Unix metadata fallback. Every lookup is relative to an already-open directory descriptor.
use crate::{
    index::VolumeSpace,
    scan::{AttributeClass, EntryFacts, FileFacts},
    StorageError,
};
use pcagent_shared::Status;
use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags, Stat};
use std::{
    ffi::{OsStr, OsString},
    io,
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VolumeId(u64);

const DIR_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
pub(crate) struct Directory {
    reader: Dir,
}
pub(crate) type Child = OsString;
pub(crate) fn child_name(child: &Child) -> OsString {
    child.clone()
}
impl Directory {
    pub fn estimated_bytes(&self) -> u64 {
        8192
    }
    pub fn open_root(path: &Path, expected: Option<(u64, u128)>) -> io::Result<Self> {
        let mut fd = fs::open("/", DIR_FLAGS, Mode::empty())?;
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => fd = fs::openat(&fd, name, DIR_FLAGS, Mode::empty())?,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "noncanonical scope",
                    ))
                }
            }
        }
        let result = Self {
            reader: Dir::new(fd)?,
        };
        if expected.is_some_and(|key| result.identity().ok() != Some(key)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "authorized root was replaced",
            ));
        }
        Ok(result)
    }
    pub fn identity(&self) -> io::Result<(u64, u128)> {
        let stat = self.reader.stat()?;
        Ok((stat.st_dev, u128::from(stat.st_ino)))
    }
    pub fn next(&mut self) -> Option<io::Result<OsString>> {
        loop {
            let entry = match self.reader.next()? {
                Ok(e) => e,
                Err(e) => return Some(Err(e.into())),
            };
            let bytes = entry.file_name().to_bytes();
            if bytes != b"." && bytes != b".." {
                return Some(Ok(OsStr::from_bytes(bytes).into()));
            }
        }
    }
    pub fn child(&self, name: &OsStr, volume: VolumeId) -> io::Result<Self> {
        let fd = fs::openat(self.reader.fd()?, name, DIR_FLAGS, Mode::empty())?;
        let result = Self {
            reader: Dir::new(fd)?,
        };
        if result.identity()?.0 != volume.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "different volume",
            ));
        }
        Ok(result)
    }
    pub fn inspect(&self, name: &Child, volume: VolumeId) -> io::Result<EntryFacts> {
        let stat = fs::statat(self.reader.fd()?, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let class = match FileType::from_raw_mode(stat.st_mode) {
            FileType::Directory => AttributeClass::Dir,
            FileType::RegularFile => AttributeClass::File,
            FileType::Symlink => AttributeClass::Reparse,
            _ => AttributeClass::Special,
        };
        let logical = u64::try_from(stat.st_size).unwrap_or(0);
        let modified_ms = modified_ms(&stat);
        let facts = (class == AttributeClass::File).then(|| FileFacts {
            logical,
            allocated: u64::try_from(stat.st_blocks)
                .ok()
                .and_then(|n| n.checked_mul(512)),
            key: Some((stat.st_dev, u128::from(stat.st_ino))),
            links: u32::try_from(stat.st_nlink).ok(),
            modified_ms,
            sparse: false,
            compressed: false,
            other_volume: stat.st_dev != volume.0,
        });
        Ok(EntryFacts {
            class,
            logical,
            modified_ms,
            facts,
            other_volume: stat.st_dev != volume.0,
        })
    }
    pub fn open_content(
        &mut self,
        name: &OsStr,
        check: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<ContentFile> {
        check()?;
        // NONBLOCK prevents a replaced regular file from blocking on a FIFO before fstat.
        let fd = fs::openat(
            self.reader.fd()?,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )?;
        Ok(ContentFile(fd.into()))
    }
}

pub(crate) struct ContentFile(std::fs::File);
impl ContentFile {
    pub fn facts(&self, volume: VolumeId) -> io::Result<FileFacts> {
        let stat = fs::fstat(&self.0)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        Ok(FileFacts {
            logical: u64::try_from(stat.st_size).map_err(|_| io::ErrorKind::InvalidData)?,
            allocated: None,
            key: Some((stat.st_dev, u128::from(stat.st_ino))),
            links: u32::try_from(stat.st_nlink).ok(),
            modified_ms: modified_ms(&stat),
            sparse: false,
            compressed: false,
            other_volume: stat.st_dev != volume.0,
        })
    }
}
impl io::Read for ContentFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        io::Read::read(&mut self.0, buffer)
    }
}
pub(crate) fn authorize_root(root: &Path) -> Result<(PathBuf, Directory), StorageError> {
    authorize_after_capture(root, || {})
}
fn authorize_after_capture(
    root: &Path,
    after_capture: impl FnOnce(),
) -> Result<(PathBuf, Directory), StorageError> {
    // Capture the object before canonicalizing. Keep the descriptor alive until the canonical
    // path has been reopened without following links and matched to that same object.
    let fd = fs::open(root, DIR_FLAGS, Mode::empty()).map_err(|e| {
        if std::fs::symlink_metadata(root).is_ok_and(|m| m.file_type().is_symlink()) {
            StorageError::ReparseRoot
        } else if io::Error::from(e).kind() == io::ErrorKind::NotADirectory {
            StorageError::NotADirectory
        } else {
            StorageError::from(io::Error::from(e))
        }
    })?;
    let stat = fs::fstat(&fd).map_err(io::Error::from)?;
    let identity = (stat.st_dev, u128::from(stat.st_ino));
    after_capture();
    let canonical = std::fs::canonicalize(root)?;
    let pinned = Directory::open_root(&canonical, Some(identity))?;
    Ok((canonical, pinned))
}
#[test]
fn replacement_during_authorization_cannot_grant_the_new_target() {
    let base = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-authorize-test"));
    let root = base.join("root");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let result = authorize_after_capture(&root, || {
        std::fs::rename(&root, base.join("saved")).unwrap();
        std::os::unix::fs::symlink(&outside, &root).unwrap();
    });
    assert!(
        result.is_err(),
        "canonicalization must not grant a replacement symlink target"
    );
    std::fs::remove_file(&root).unwrap();
    std::fs::remove_dir_all(base).unwrap();
}
// rustix stat fields have different signedness across Unix targets; keep checked conversions.
#[allow(clippy::useless_conversion)]
fn modified_ms(stat: &Stat) -> Option<u64> {
    u64::try_from(stat.st_mtime)
        .ok()?
        .checked_mul(1000)?
        .checked_add(u64::try_from(stat.st_mtime_nsec).ok()? / 1_000_000)
}
pub(crate) fn check_root(root: &Path) -> Result<VolumeId, StorageError> {
    Ok(VolumeId(Directory::open_root(root, None)?.identity()?.0))
}
pub(crate) fn volume_space(_root: &Path) -> VolumeSpace {
    VolumeSpace {
        total_bytes: None,
        free_bytes: None,
        occupied_bytes: None,
        filesystem: None,
        source: "platform".into(),
        status: Status::Unsupported,
        limitations: vec![
            "Volume space is only collected on Windows (GetDiskFreeSpaceExW).".into(),
        ],
    }
}
pub(crate) fn profile_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}
