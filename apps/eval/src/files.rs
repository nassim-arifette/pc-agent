//! Bounded local inputs and new output files. Raw evaluation data must stay outside Git trees.
use crate::Result;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;

/// Check the serialized size while formatting, including indentation, before retaining more bytes.
/// A deeply nested imported payload must not allocate an unbounded pretty-printed output.
#[derive(Default)]
struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if (self.0.len() as u64).saturating_add(bytes.len() as u64) >= MAX_FILE_BYTES {
            return Err(io::Error::other("Output exceeds the 32 MiB limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&read_bytes(path)?)
        .map_err(|_| "Invalid JSON or incompatible document structure".into())
}

/// Read once so a human-approved hash and its parsed content refer to the same bounded bytes.
pub fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    read_bytes_limited(path, MAX_FILE_BYTES)
}

pub fn read_bytes_limited(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    if maximum > MAX_FILE_BYTES {
        return Err("Input limit exceeds 32 MiB".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err("Input exceeds its byte limit (at most 32 MiB)".into());
    }
    Ok(bytes)
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
        let mut bytes = BoundedBytes::default();
        serde_json::to_writer_pretty(&mut bytes, value)?;
        // Preserve a partial file on an I/O error so the failure can be diagnosed locally.
        self.written = true;
        let file = self.file.as_mut().expect("reserved output");
        file.write_all(&bytes.0)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_bound_is_checked_before_extending_the_buffer() {
        let mut bytes = BoundedBytes(vec![0; MAX_FILE_BYTES as usize - 2]);
        assert_eq!(bytes.write(b"x").unwrap(), 1);
        let retained = bytes.0.len();
        assert!(bytes.write(b"yz").is_err());
        assert_eq!(bytes.0.len(), retained);
    }
}
