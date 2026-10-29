//! Shared POSIX file identities and descriptor-relative, no-follow traversal.
use crate::{EffectError, FileIdentity, FileTarget};
use pcagent_shared::Status;
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, FileType, Mode, OFlags, Stat},
};
use std::{ffi::OsStr, io, path::{Component, Path}};

const DIR: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
pub(super) fn error(detail: &str) -> EffectError {
    EffectError::new(Status::Partial, detail)
}
pub(super) fn io_status(e: impl Into<io::Error>) -> Status {
    match e.into().kind() {
        io::ErrorKind::PermissionDenied => Status::PermissionDenied,
        io::ErrorKind::NotFound => Status::Unsupported,
        _ => Status::Error,
    }
}
pub(super) fn clean(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}
pub(super) fn directory(path: &Path, create: bool) -> Result<OwnedFd, Status> {
    if !clean(path) { return Err(Status::Unsupported); }
    let mut fd = fs::open("/", DIR, Mode::empty()).map_err(io_status)?;
    for component in path.components() {
        if let Component::Normal(name) = component {
            if create {
                match fs::mkdirat(&fd, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(e) => return Err(io_status(e)),
                }
            }
            fd = fs::openat(&fd, name, DIR, Mode::empty()).map_err(io_status)?;
        }
    }
    Ok(fd)
}
pub(super) fn parent(path: &Path) -> Result<(OwnedFd, &OsStr), Status> {
    if !clean(path) { return Err(Status::Unsupported); }
    let name = path.file_name().ok_or(Status::Unsupported)?;
    Ok((directory(path.parent().ok_or(Status::Unsupported)?, false)?, name))
}
// Darwin and Linux differ in the signedness/width of stat fields.
#[allow(clippy::useless_conversion)]
pub(super) fn identity(s: &Stat) -> Result<FileIdentity, Status> {
    let kind = FileType::from_raw_mode(s.st_mode);
    let sec = u64::try_from(s.st_mtime).map_err(|_| Status::Partial)?;
    let nanos = u64::try_from(s.st_mtime_nsec).map_err(|_| Status::Partial)?;
    if nanos >= 1_000_000_000 { return Err(Status::Partial); }
    let modified = sec.checked_add(11_644_473_600)
        .and_then(|s| s.checked_mul(10_000_000))
        .and_then(|ticks| ticks.checked_add(nanos / 100)).ok_or(Status::Partial)?;
    Ok(FileIdentity {
        volume_serial: u64::try_from(s.st_dev).map_err(|_| Status::Partial)?,
        file_id: u128::from(s.st_ino),
        size_bytes: u64::try_from(s.st_size).map_err(|_| Status::Partial)?,
        modified_filetime: modified,
        is_directory: kind == FileType::Directory,
        is_reparse_point: !matches!(kind, FileType::RegularFile | FileType::Directory),
    })
}
pub(super) fn observe_file(path: &Path) -> Result<Option<FileIdentity>, Status> {
    let (dir, name) = match parent(path) {
        Ok(v) => v,
        Err(Status::Unsupported) if !path.exists() => return Ok(None),
        Err(e) => return Err(e),
    };
    match fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(s) => identity(&s).map(Some),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(e) => Err(io_status(e)),
    }
}
pub(super) fn pinned(file: &FileTarget) -> Result<(OwnedFd, OwnedFd, &OsStr), EffectError> {
    let (dir, name) = parent(&file.path)
        .map_err(|_| error("Cannot pin the file's parent without following links."))?;
    let fd = fs::openat(&dir, name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK, Mode::empty())
        .map_err(|_| error("Cannot open the approved file without following links."))?;
    let stat = fs::fstat(&fd).map_err(|_| error("Cannot read the pinned identity."))?;
    if identity(&stat).ok().as_ref() != Some(&file.identity)
        || FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile || stat.st_nlink != 1 {
        return Err(error("The approved file changed, is not regular, or has multiple hard links."));
    }
    Ok((dir, fd, name))
}
