use crate::{ErrorCode, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(any(windows, test))]
use std::io::Read;
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const MODEL_BYTES: u64 = 147_951_465;
pub const MODEL_SHA256: &str = "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe";
pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin";

#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub url: &'static str,
    pub installed: bool,
    pub downloading: bool,
    pub downloaded_bytes: u64,
    pub error: Option<crate::VoiceError>,
}
impl ModelInfo {
    pub fn new(root: &Path) -> Self {
        Self {
            name: "Whisper base multilingual",
            bytes: MODEL_BYTES,
            sha256: MODEL_SHA256,
            url: MODEL_URL,
            installed: path(root).is_file(),
            downloading: false,
            downloaded_bytes: 0,
            error: None,
        }
    }
}
pub fn path(root: &Path) -> PathBuf {
    root.join("ggml-base.bin")
}

#[cfg(windows)]
pub fn verify(path: &Path, cancelled: &AtomicBool) -> Result<()> {
    let file = File::open(path).map_err(|_| ErrorCode::ModelMissing)?;
    if file
        .metadata()
        .map_err(|_| ErrorCode::StorageUnavailable)?
        .len()
        != MODEL_BYTES
    {
        return Err(ErrorCode::ModelInvalid.into());
    }
    check_digest(file, MODEL_BYTES, MODEL_SHA256, cancelled, |_| {})
}

#[cfg(any(windows, test))]
fn check_digest(
    mut reader: impl Read,
    expected_bytes: u64,
    expected_sha256: &str,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let mut digest = Sha256::new();
    let mut count = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(ErrorCode::Cancelled.into());
        }
        let read = reader
            .read(&mut buffer)
            .map_err(|_| ErrorCode::DownloadFailed)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > expected_bytes {
            return Err(ErrorCode::ModelInvalid.into());
        }
        digest.update(&buffer[..read]);
        progress(count);
    }
    if count != expected_bytes || format!("{:x}", digest.finalize()) != expected_sha256 {
        return Err(ErrorCode::ModelInvalid.into());
    }
    Ok(())
}

struct PartialFile(PathBuf);
impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn download(root: &Path, cancelled: &AtomicBool, progress: impl FnMut(u64)) -> Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ErrorCode::DownloadFailed)?
        .block_on(download_async(root, cancelled, progress))
}

async fn cancellable<T>(
    future: impl std::future::Future<Output = std::result::Result<T, reqwest::Error>>,
    cancelled: &AtomicBool,
) -> Result<T> {
    tokio::pin!(future);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(ErrorCode::Cancelled.into());
        }
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(50), future.as_mut()).await {
            return result.map_err(|_| ErrorCode::DownloadFailed.into());
        }
    }
}

async fn download_async(
    root: &Path,
    cancelled: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    fs::create_dir_all(root).map_err(|_| ErrorCode::StorageUnavailable)?;
    let partial = PartialFile(root.join("ggml-base.bin.partial"));
    let mut output = File::create(&partial.0).map_err(|_| ErrorCode::StorageUnavailable)?;
    // Only this fixed model request can use the network. No caller data or credentials.
    let client = reqwest::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| ErrorCode::DownloadFailed)?;
    let mut response = cancellable(client.get(MODEL_URL).send(), cancelled)
        .await?
        .error_for_status()
        .map_err(|_| ErrorCode::DownloadFailed)?;
    if response.content_length().is_some_and(|n| n != MODEL_BYTES) {
        return Err(ErrorCode::ModelInvalid.into());
    }
    let mut digest = Sha256::new();
    let mut count = 0;
    while let Some(chunk) = cancellable(response.chunk(), cancelled).await? {
        count += chunk.len() as u64;
        if count > MODEL_BYTES {
            return Err(ErrorCode::ModelInvalid.into());
        }
        output
            .write_all(&chunk)
            .map_err(|_| ErrorCode::StorageUnavailable)?;
        digest.update(&chunk);
        progress(count);
    }
    if count != MODEL_BYTES || format!("{:x}", digest.finalize()) != MODEL_SHA256 {
        return Err(ErrorCode::ModelInvalid.into());
    }
    output
        .sync_all()
        .map_err(|_| ErrorCode::StorageUnavailable)?;
    drop(output);
    if cancelled.load(Ordering::Acquire) {
        return Err(ErrorCode::Cancelled.into());
    }
    fs::rename(&partial.0, path(root)).map_err(|_| ErrorCode::StorageUnavailable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_interrupts_a_stalled_network_future() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            worker_cancel.store(true, Ordering::Release);
        });
        let started = std::time::Instant::now();
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(cancellable(
                std::future::pending::<std::result::Result<(), reqwest::Error>>(),
                &cancel,
            ));
        worker.join().unwrap();
        assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn verifies_exact_bytes_digest_and_cancellation() {
        let bytes = b"synthetic model";
        let sha = format!("{:x}", Sha256::digest(bytes));
        let cancel = AtomicBool::new(false);
        assert!(check_digest(&bytes[..], bytes.len() as u64, &sha, &cancel, |_| {}).is_ok());
        assert_eq!(
            check_digest(&bytes[..], bytes.len() as u64 + 1, &sha, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::ModelInvalid
        );
        assert_eq!(
            check_digest(&bytes[..], 1, &sha, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::ModelInvalid
        );
        assert_eq!(
            check_digest(&bytes[..], bytes.len() as u64, "bad", &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::ModelInvalid
        );
        cancel.store(true, Ordering::Release);
        assert_eq!(
            check_digest(&bytes[..], bytes.len() as u64, &sha, &cancel, |_| {})
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
    }
}
