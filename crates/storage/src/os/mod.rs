//! Scope authorization and native directory operations; Unix shares descriptor-relative reads.
#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(not(windows))]
mod unix;
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux as selected;
#[cfg(target_os = "macos")]
pub(crate) use macos as selected;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) use unix as selected;
#[cfg(windows)]
pub(crate) use windows as selected;

use crate::{StorageError, VolumeSpace};
use std::path::{Path, PathBuf};

pub(crate) trait Platform {
    type Directory;
    type VolumeId;
    fn authorize_root(root: &Path) -> Result<(PathBuf, Self::Directory), StorageError>;
    fn check_root(root: &Path) -> Result<Self::VolumeId, StorageError>;
    fn profile_dir() -> Option<PathBuf>;
    fn volume_space(root: &Path) -> VolumeSpace;
}

pub(crate) struct Native;
impl Platform for Native {
    type Directory = selected::Directory;
    type VolumeId = selected::VolumeId;
    fn authorize_root(root: &Path) -> Result<(PathBuf, Self::Directory), StorageError> {
        selected::authorize_root(root)
    }
    fn check_root(root: &Path) -> Result<Self::VolumeId, StorageError> {
        selected::check_root(root)
    }
    fn profile_dir() -> Option<PathBuf> {
        selected::profile_dir()
    }
    fn volume_space(root: &Path) -> VolumeSpace {
        selected::volume_space(root)
    }
}
