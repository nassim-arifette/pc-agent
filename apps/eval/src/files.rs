//! Bounded local inputs and new output files. Raw evaluation data must stay outside Git trees.
use crate::Result;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("Input exceeds the 32 MiB limit".into());
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| "Invalid JSON or incompatible document structure".into())
}

/// Reserve the destination before collection or paid inference. Never truncate an existing file.
/// On failure, remove only the empty reservation created by this object.
pub struct OutputFile {
    file: Option<File>,
    path: PathBuf,
    written: bool,
}
impl OutputFile {
    pub fn reserve(path: &Path) -> Result<Self> {
        let name = path.file_name().ok_or("Output must name a file")?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = parent.canonicalize()?;
        if parent.ancestors().any(|p| p.join(".git").exists()) {
            return Err("Raw captures and results must be written outside a Git repository".into());
        }
        let path = parent.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path)?;
        Ok(Self {
            file: Some(file),
            path,
            written: false,
        })
    }
    pub fn save(mut self, value: &impl Serialize) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value)?;
        if bytes.len() as u64 >= MAX_FILE_BYTES {
            return Err("Output exceeds the 32 MiB limit".into());
        }
        // Preserve a partial file on an I/O error so the failure can be diagnosed locally.
        self.written = true;
        let file = self.file.as_mut().expect("reserved output");
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        Ok(())
    }
}
impl Drop for OutputFile {
    fn drop(&mut self) {
        if !self.written {
            self.file.take();
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
