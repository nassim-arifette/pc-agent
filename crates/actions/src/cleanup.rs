use serde::Serialize;
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

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
        Ok(cache)
    }
    pub fn open(root: impl Into<PathBuf>) -> io::Result<Self> {
        let cache = Self { root: root.into() };
        cache.verify()?;
        Ok(cache)
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
        if !fs::symlink_metadata(&marker)?.is_file() {
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
        for entry in fs::read_dir(&self.root)?.take(MAX_ENTRIES) {
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
    pub(crate) fn clean(&self) -> io::Result<CacheInventory> {
        let mut removed = CacheInventory {
            files: 0,
            bytes: Some(0),
        };
        for (path, size) in self.regular_files()? {
            fs::remove_file(path)?;
            removed.files += 1;
            removed.bytes = removed.bytes.and_then(|b| b.checked_add(size));
        }
        Ok(removed)
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
