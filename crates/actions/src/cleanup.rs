use crate::{CleanupTarget, ItemOutcome, Refusal};
use serde::Serialize;
use std::sync::atomic::AtomicBool;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};
#[cfg(windows)]
use {crate::SkipReason, std::sync::atomic::Ordering};

pub const SYNTHETIC_DEMO_CACHE_ID: &str = "synthetic_demo_cache";
const MARKER: &str = "PCAGENT_SYNTHETIC_DEMO_CACHE.txt";
const MARKER_TEXT: &str =
    "pcagent synthetic demo cache: generated placeholder data only, not user content.\n";
const MAX_ENTRIES: usize = 1024;
const MAX_CREATE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CacheInventory {
    pub files: u64,
    /// `None` when the sum overflows.
    pub bytes: Option<u64>,
}

/// Clearly labelled synthetic cache owned by pcagent, used for the cleanup demo adapter.
/// Only regular files directly inside the marked root are ever removed.
#[derive(Debug)]
pub struct SyntheticDemoCache {
    root: PathBuf,
}
impl SyntheticDemoCache {
    pub fn create(root: impl Into<PathBuf>, files: &[(&str, u64)]) -> io::Result<Self> {
        let root = root.into();
        let invalid = |m| io::Error::new(io::ErrorKind::InvalidInput, m);
        if files.len() >= MAX_ENTRIES {
            return Err(invalid("too many synthetic files"));
        }
        let total = files
            .iter()
            .try_fold(0_u64, |acc, (_, n)| acc.checked_add(*n));
        if total.is_none_or(|t| t > MAX_CREATE_BYTES) {
            return Err(invalid("synthetic cache too large"));
        }
        if !files.iter().all(|(name, _)| valid_name(name)) {
            return Err(invalid("invalid synthetic file name"));
        }
        fs::create_dir_all(&root)?;
        let cache = Self { root };
        if cache.verify().is_err() {
            if fs::read_dir(&cache.root)?.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "directory is not an empty or existing synthetic cache",
                ));
            }
            File::create_new(cache.root.join(MARKER))?.write_all(MARKER_TEXT.as_bytes())?;
        }
        let chunk = [0x5a_u8; 64 * 1024];
        for (name, size) in files {
            let mut f = File::create_new(cache.root.join(name))?;
            let mut left = *size;
            while left > 0 {
                let n = left.min(chunk.len() as u64) as usize;
                f.write_all(&chunk[..n])?;
                left -= n as u64;
            }
            f.sync_all()?;
        }
        Self::open(cache.root)
    }
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let cache = Self { root: root.into() };
        cache.verify()?;
        let root = fs::canonicalize(cache.root)?;
        // The action catalog uses ordinary absolute Windows paths, not device paths.
        #[cfg(windows)]
        let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"));
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn verify(&self) -> io::Result<()> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "synthetic cache marker missing");
        let meta = fs::symlink_metadata(&self.root)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(bad());
        }
        let marker = self.root.join(MARKER);
        let meta = fs::symlink_metadata(&marker)?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(bad());
        }
        let mut text = String::new();
        File::open(marker)?
            .take(MARKER_TEXT.len() as u64 + 1)
            .read_to_string(&mut text)?;
        if text == MARKER_TEXT {
            Ok(())
        } else {
            Err(bad())
        }
    }
    fn regular_files(&self) -> io::Result<Vec<(PathBuf, u64)>> {
        self.verify()?;
        let mut out = Vec::new();
        for (index, entry) in fs::read_dir(&self.root)?.enumerate() {
            if index >= MAX_ENTRIES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "synthetic cache entry limit exceeded",
                ));
            }
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_file() && !meta.file_type().is_symlink() && entry.file_name() != MARKER {
                out.push((entry.path(), meta.len()));
            }
        }
        Ok(out)
    }
    pub fn inventory(&self) -> io::Result<CacheInventory> {
        let files = self.regular_files()?;
        Ok(CacheInventory {
            files: files.len() as u64,
            bytes: files
                .iter()
                .try_fold(0_u64, |acc, (_, n)| acc.checked_add(*n)),
        })
    }
    pub(crate) fn manifest(&self) -> Result<CleanupTarget, Refusal> {
        #[cfg(windows)]
        {
            Ok(self.pin()?.target)
        }
        #[cfg(not(windows))]
        {
            Err(Refusal::Unverifiable {
                status: pcagent_shared::Status::Unsupported,
            })
        }
    }
    pub(crate) fn validate(&self, expected: &CleanupTarget) -> Result<(), Refusal> {
        let current = self.manifest()?;
        if matches_manifest(&current, expected) {
            Ok(())
        } else {
            Err(Refusal::AdapterPreconditionFailed)
        }
    }
    pub(crate) fn clean(&self, expected: &CleanupTarget, cancel: &AtomicBool) -> ItemOutcome {
        #[cfg(windows)]
        {
            let pinned = match self.pin() {
                Ok(p) if matches_manifest(&p.target, expected) => p,
                Ok(_) => {
                    return ItemOutcome::Refused {
                        reason: Refusal::AdapterPreconditionFailed,
                    }
                }
                Err(reason) => return ItemOutcome::Refused { reason },
            };
            let mut removed = CacheInventory {
                files: 0,
                bytes: Some(0),
            };
            for (file, pin) in pinned.target.manifest.iter().zip(pinned.files) {
                if cancel.load(Ordering::SeqCst) {
                    return if removed.files == 0 {
                        ItemOutcome::Skipped {
                            reason: SkipReason::Cancelled,
                        }
                    } else {
                        ItemOutcome::Partial {
                            status: pcagent_shared::Status::Partial,
                            detail:
                                "cleanup cancelled; remaining approved files were left in place"
                                    .into(),
                            files_removed: removed.files,
                            bytes_removed: removed.bytes,
                        }
                    };
                }
                if let Err(e) = crate::windows::delete_synthetic_file(pin) {
                    return ItemOutcome::Partial {
                        status: pcagent_shared::Status::Error,
                        detail: format!(
                            "cleanup stopped at {} ({:?})",
                            file.path.display(),
                            e.kind()
                        ),
                        files_removed: removed.files,
                        bytes_removed: removed.bytes,
                    };
                }
                removed.files += 1;
                removed.bytes = removed
                    .bytes
                    .and_then(|b| b.checked_add(file.identity.size_bytes));
            }
            ItemOutcome::Completed {
                detail: Some(format!("{} synthetic files removed", removed.files)),
                bytes_removed: removed.bytes,
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (expected, cancel);
            ItemOutcome::Refused {
                reason: Refusal::Unverifiable {
                    status: pcagent_shared::Status::Unsupported,
                },
            }
        }
    }
}

fn matches_manifest(a: &CleanupTarget, b: &CleanupTarget) -> bool {
    a.adapter_id == b.adapter_id
        && a.root == b.root
        && crate::same_object(&a.root_identity, &b.root_identity)
        && a.marker == b.marker
        && a.manifest == b.manifest
        && a.files == b.files
        && a.bytes == b.bytes
}

#[cfg(windows)]
struct PinnedCache {
    target: CleanupTarget,
    _directories: Vec<crate::windows::OwnedHandle>,
    _marker: crate::windows::OwnedHandle,
    files: Vec<crate::windows::OwnedHandle>,
}

#[cfg(windows)]
impl SyntheticDemoCache {
    fn pin(&self) -> Result<PinnedCache, Refusal> {
        use crate::{windows, FileTarget};
        let (_directories, root_identity) = windows::pin_directories(&self.root)?;
        let observe = |path: PathBuf| -> Result<FileTarget, Refusal> {
            let identity = windows::observe_file(&path)
                .map_err(|status| Refusal::Unverifiable { status })?
                .ok_or(Refusal::TargetMissing)?;
            Ok(FileTarget { path, identity })
        };
        let marker = observe(self.root.join(MARKER))?;
        let _marker = windows::pin_verified(&marker)?;
        let mut manifest = self
            .regular_files()
            .map_err(|_| Refusal::AdapterPreconditionFailed)?
            .into_iter()
            .map(|(path, _)| observe(path))
            .collect::<Result<Vec<_>, _>>()?;
        manifest.sort_by(|a, b| a.path.cmp(&b.path));
        let mut identities = std::collections::HashSet::new();
        let mut pins = Vec::with_capacity(manifest.len());
        for file in &manifest {
            if file.path.parent() != Some(self.root.as_path())
                || !identities.insert((file.identity.volume_serial, file.identity.file_id))
            {
                return Err(Refusal::AdapterPreconditionFailed);
            }
            pins.push(windows::pin_verified(file)?);
        }
        let files = manifest.len() as u64;
        let bytes = manifest
            .iter()
            .try_fold(0_u64, |n, f| n.checked_add(f.identity.size_bytes));
        Ok(PinnedCache {
            target: CleanupTarget {
                adapter_id: SYNTHETIC_DEMO_CACHE_ID.into(),
                root: self.root.clone(),
                root_identity,
                marker,
                manifest,
                files,
                bytes,
            },
            _directories,
            _marker,
            files: pins,
        })
    }
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != MARKER
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
