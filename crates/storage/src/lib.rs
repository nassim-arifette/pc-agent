//! Read-only, scope-bound storage inventory (spec 9.1, 9.3, 9.4). File contents are never read.
//! Paths stay local: agent-facing views should use `item_ref`/`redacted_path` via `without_paths`.
#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]
mod index;
#[cfg(not(windows))]
mod portable;
mod scan;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(not(windows))]
use portable as platform;
#[cfg(windows)]
use windows as platform;

pub use index::{
    dedup_parent_child, redact_names, redact_path, ItemKind, ItemView, LargestItemsPage, LastUse,
    StorageIndex, StorageOverview, Totals, VolumeSpace, GENERIC_NAMES,
};
/// Live free space of the volume that contains `root` (Win32 `GetDiskFreeSpaceExW`).
pub fn volume_space(root: &Path) -> VolumeSpace {
    platform::volume_space(root)
}
pub use scan::{
    classify_attributes, AttributeClass, ClassicBackend, ATTR_COMPRESSED, ATTR_DIRECTORY,
    ATTR_OFFLINE, ATTR_RECALL_ON_DATA_ACCESS, ATTR_RECALL_ON_OPEN, ATTR_REPARSE_POINT,
    ATTR_SPARSE_FILE,
};

use pcagent_shared::{new_id, Status};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread::JoinHandle,
    time::Duration,
};
use thiserror::Error;

pub const TOOL_OVERVIEW: &str = "get_storage_overview";
pub const TOOL_START_SCAN: &str = "start_storage_scan";
pub const TOOL_LARGEST_ITEMS: &str = "get_largest_items";
pub const MAX_PAGE: usize = 100;
/// Spec 19.1 design target for the file index budget.
pub const DEFAULT_INDEX_BUDGET_BYTES: u64 = 1 << 30;
pub const MAX_CONCURRENT_SCANS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum StorageError {
    #[error("Unknown or unauthorized scope ID")]
    UnknownScope,
    #[error("Scope root is not an accessible directory")]
    NotADirectory,
    #[error("Scope root is a link or reparse point; authorize its target explicitly")]
    ReparseRoot,
    #[error("Volume is not supported for scanning: {0}")]
    UnsupportedVolume(&'static str),
    #[error("Scope root cannot be accessed: {0}")]
    Io(String),
    #[error("A scan is already running for this scope")]
    ScanInProgress,
    #[error("Too many concurrent scans")]
    Busy,
    #[error("No completed scan for this scope")]
    NotScanned,
    #[error("Page limit must be between 1 and 100")]
    InvalidPage,
    #[error("Unknown item reference")]
    UnknownItem,
}
impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(format!("{:?}", e.kind()))
    }
}

/// An explicitly authorized root. Only local UI/CLI code may create one; the model only sees `id`.
#[derive(Debug, Clone)]
pub struct ScanScope {
    id: String,
    root: PathBuf,
    volume: platform::VolumeId,
    identity: (u64, u128),
    profile: Option<PathBuf>,
}
impl ScanScope {
    pub fn authorize(root: &Path) -> Result<Self, StorageError> {
        let (root, pinned) = platform::authorize_root(root)?;
        let identity = pinned.identity()?;
        let volume = platform::check_root(&root)?;
        Ok(Self {
            id: new_id("scope"),
            root,
            volume,
            identity,
            profile: platform::profile_dir(),
        })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    /// Canonical root (`\\?\` verbatim form on Windows).
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn info(&self) -> ScopeInfo {
        ScopeInfo {
            scope_id: self.id.clone(),
            root: index::display(&self.root),
            redacted_root: index::redact_root(&self.root, self.profile.as_deref()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeInfo {
    pub scope_id: String,
    /// Local display only.
    pub root: String,
    pub redacted_root: String,
}

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    pub max_entries: usize,
    pub max_duration: Duration,
    /// Approximate in-memory index budget; exceeding it stops the scan cleanly as `partial`.
    pub index_budget_bytes: u64,
    pub max_denied_listed: usize,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            max_entries: 20_000_000,
            max_duration: Duration::from_secs(30 * 60),
            index_budget_bytes: DEFAULT_INDEX_BUDGET_BYTES,
            max_denied_listed: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    pub dirs_enumerated: u64,
    pub entries_indexed: u64,
    pub logical_bytes_seen: u64,
    pub index_bytes_estimate: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Truncation {
    EntryLimit,
    Duration,
    MemoryBudget,
}

/// Coverage counters. Skipped categories are never attributed bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    pub files: u64,
    pub dirs: u64,
    pub denied_dirs: u64,
    pub denied_files: u64,
    pub reparse_skipped: u64,
    pub placeholders: u64,
    /// Remote size reported by placeholder metadata; not local allocation.
    pub placeholder_logical_bytes: u64,
    pub other_volume_skipped: u64,
    pub special_skipped: u64,
    pub vanished: u64,
    pub changed_during_scan: u64,
    pub errors: u64,
    pub hard_link_aliases: u64,
    pub allocation_unknown_files: u64,
    pub truncated: Option<Truncation>,
    pub cancelled: bool,
}

/// Seam for alternative backends (e.g. a future NTFS MFT/USN backend) behind the same index.
pub trait ScanBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn scan(
        &self,
        scope: &ScanScope,
        options: &ScanOptions,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(&ScanProgress),
    ) -> StorageIndex;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanSummary {
    pub scope_id: String,
    pub scan_id: String,
    pub backend: String,
    pub status: Status,
    pub coverage: Coverage,
    pub duration_ms: u64,
}

/// Strict arguments for `get_largest_items`; unknown fields are rejected.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LargestItemsArgs {
    pub scope_id: String,
    pub kind: ItemKind,
    #[serde(default)]
    pub offset: usize,
    pub limit: usize,
}
/// Strict arguments for `get_storage_overview` and `start_storage_scan`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ScopeArgs {
    pub scope_id: String,
}

struct ScopeState {
    scope: Arc<ScanScope>,
    index: Option<Arc<StorageIndex>>,
    running: Option<CancelToken>,
}

/// Registry of authorized scopes and their latest completed index.
pub struct StorageService {
    backend: Arc<dyn ScanBackend>,
    scopes: Mutex<HashMap<String, ScopeState>>,
}
impl Default for StorageService {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ScanJob {
    pub scope_id: String,
    cancel: CancelToken,
    progress: Arc<Mutex<ScanProgress>>,
    handle: Option<JoinHandle<Result<ScanSummary, StorageError>>>,
}
impl ScanJob {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
    pub fn progress(&self) -> ScanProgress {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }
    pub fn wait(mut self) -> Result<ScanSummary, StorageError> {
        self.handle
            .take()
            .expect("owned scan worker")
            .join()
            .unwrap_or_else(|_| Err(StorageError::Io("scan worker panicked".into())))
    }
}

impl Drop for ScanJob {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct RunningGuard<'a> {
    service: &'a StorageService,
    scope_id: String,
}
impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        if let Some(s) = self.service.lock().get_mut(&self.scope_id) {
            s.running = None;
        }
    }
}

impl StorageService {
    pub fn new() -> Self {
        Self::with_backend(Arc::new(ClassicBackend))
    }
    pub fn with_backend(backend: Arc<dyn ScanBackend>) -> Self {
        Self {
            backend,
            scopes: Mutex::new(HashMap::new()),
        }
    }
    fn lock(&self) -> MutexGuard<'_, HashMap<String, ScopeState>> {
        self.scopes.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Local, user-initiated authorization. Never call with a model-supplied path.
    pub fn authorize(&self, root: &Path) -> Result<ScopeInfo, StorageError> {
        let scope = ScanScope::authorize(root)?;
        let info = scope.info();
        self.lock().insert(
            info.scope_id.clone(),
            ScopeState {
                scope: Arc::new(scope),
                index: None,
                running: None,
            },
        );
        Ok(info)
    }
    pub fn revoke(&self, scope_id: &str) -> bool {
        let removed = self.lock().remove(scope_id);
        if let Some(token) = removed.as_ref().and_then(|s| s.running.as_ref()) {
            token.cancel();
        }
        removed.is_some()
    }
    pub fn scopes(&self) -> Vec<ScopeInfo> {
        self.lock().values().map(|s| s.scope.info()).collect()
    }
    fn begin(&self, scope_id: &str) -> Result<(Arc<ScanScope>, CancelToken), StorageError> {
        let mut scopes = self.lock();
        if scopes.values().filter(|s| s.running.is_some()).count() >= MAX_CONCURRENT_SCANS {
            return Err(StorageError::Busy);
        }
        let state = scopes.get_mut(scope_id).ok_or(StorageError::UnknownScope)?;
        if state.running.is_some() {
            return Err(StorageError::ScanInProgress);
        }
        let token = CancelToken::default();
        state.running = Some(token.clone());
        Ok((state.scope.clone(), token))
    }
    fn run(
        &self,
        scope: &ScanScope,
        options: &ScanOptions,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(&ScanProgress),
    ) -> Result<ScanSummary, StorageError> {
        let _guard = RunningGuard {
            service: self,
            scope_id: scope.id.clone(),
        };
        let index = self.backend.scan(scope, options, cancel, progress);
        let summary = index.summary();
        let mut scopes = self.lock();
        let state = scopes
            .get_mut(&scope.id)
            .ok_or(StorageError::UnknownScope)?;
        state.index = Some(Arc::new(index));
        Ok(summary)
    }
    /// Synchronous scan on the calling thread.
    pub fn scan_blocking(
        &self,
        scope_id: &str,
        options: &ScanOptions,
        progress: &mut dyn FnMut(&ScanProgress),
    ) -> Result<ScanSummary, StorageError> {
        let (scope, cancel) = self.begin(scope_id)?;
        self.run(&scope, options, &cancel, progress)
    }
    /// `start_storage_scan`: one background worker thread per scan, at most `MAX_CONCURRENT_SCANS`.
    pub fn start_storage_scan(
        self: &Arc<Self>,
        scope_id: &str,
        options: ScanOptions,
    ) -> Result<ScanJob, StorageError> {
        let (scope, cancel) = self.begin(scope_id)?;
        let progress = Arc::new(Mutex::new(ScanProgress::default()));
        let (service, token, shared) = (self.clone(), cancel.clone(), progress.clone());
        let handle = std::thread::Builder::new()
            .name("pcagent-storage-scan".into())
            .spawn(move || {
                service.run(&scope, &options, &token, &mut |p| {
                    *shared.lock().unwrap_or_else(|e| e.into_inner()) = *p;
                })
            });
        let handle = match handle {
            Ok(h) => h,
            Err(e) => {
                if let Some(s) = self.lock().get_mut(scope_id) {
                    s.running = None;
                }
                return Err(e.into());
            }
        };
        Ok(ScanJob {
            scope_id: scope_id.into(),
            cancel,
            progress,
            handle: Some(handle),
        })
    }
    pub fn cancel(&self, scope_id: &str) -> Result<bool, StorageError> {
        let scopes = self.lock();
        let state = scopes.get(scope_id).ok_or(StorageError::UnknownScope)?;
        Ok(state.running.as_ref().map(CancelToken::cancel).is_some())
    }
    pub fn index(&self, scope_id: &str) -> Result<Arc<StorageIndex>, StorageError> {
        let scopes = self.lock();
        let state = scopes.get(scope_id).ok_or(StorageError::UnknownScope)?;
        state.index.clone().ok_or(StorageError::NotScanned)
    }
    /// `get_storage_overview`: live volume space plus the latest completed scan, if any.
    pub fn get_storage_overview(&self, scope_id: &str) -> Result<StorageOverview, StorageError> {
        let (scope, index, running) = {
            let scopes = self.lock();
            let s = scopes.get(scope_id).ok_or(StorageError::UnknownScope)?;
            (s.scope.clone(), s.index.clone(), s.running.is_some())
        };
        let space = platform::volume_space(&scope.root);
        Ok(StorageOverview::build(
            &scope,
            index.as_deref(),
            space,
            running,
        ))
    }
    /// `get_largest_items`: paginated read of the latest completed index.
    pub fn get_largest_items(
        &self,
        scope_id: &str,
        kind: ItemKind,
        offset: usize,
        limit: usize,
    ) -> Result<LargestItemsPage, StorageError> {
        self.index(scope_id)?.largest_items(kind, offset, limit)
    }
}

#[cfg(test)]
mod tests;
