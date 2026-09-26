//! Unix fallback used for portable tests. It measures the host file system honestly
//! (`st_blocks * 512`), but volume space and network-volume detection are not collected.
use crate::{
    index::VolumeSpace,
    scan::{system_ms, AttributeClass, FileFacts},
    StorageError,
};
use pcagent_shared::Status;
use std::{
    fs::Metadata,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VolumeId(u64);

pub(crate) fn check_root(root: &Path) -> Result<VolumeId, StorageError> {
    Ok(VolumeId(std::fs::symlink_metadata(root)?.dev()))
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

pub(crate) fn classify(meta: &Metadata) -> AttributeClass {
    let t = meta.file_type();
    if t.is_symlink() {
        AttributeClass::Reparse
    } else if t.is_dir() {
        AttributeClass::Dir
    } else if t.is_file() {
        AttributeClass::File
    } else {
        AttributeClass::Special
    }
}

pub(crate) fn dir_on_other_volume(meta: &Metadata, volume: VolumeId) -> bool {
    meta.dev() != volume.0
}

/// `meta` comes from `lstat` during enumeration; no file is opened.
pub(crate) fn file_facts(
    _path: &Path,
    meta: &Metadata,
    volume: VolumeId,
) -> std::io::Result<FileFacts> {
    Ok(FileFacts {
        logical: meta.len(),
        allocated: meta.blocks().checked_mul(512),
        key: Some((meta.dev(), u128::from(meta.ino()))),
        links: u32::try_from(meta.nlink()).ok(),
        modified_ms: meta.modified().ok().and_then(system_ms),
        sparse: false,
        compressed: false,
        other_volume: meta.dev() != volume.0,
    })
}
