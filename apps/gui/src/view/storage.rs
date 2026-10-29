//! Folder-relative storage results and scan progress for the desktop.
use pcagent_shared::{format_bytes, Status};
use pcagent_storage::{
    Coverage, ItemKind, ItemView, LargestItemsPage, ScanProgress, ScanSummary, StorageOverview,
    Totals,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub item_ref: String,
    pub parent_ref: Option<String>,
    pub kind: ItemKind,
    pub system: bool,
    pub file_type: Option<pcagent_storage::FileType>,
    pub name: String,
    /// Folder that contains the item, relative to the scanned folder ("" at its top level).
    pub parent: String,
    pub path: String,
    pub logical_bytes: u64,
    pub allocated_bytes: Option<u64>,
    /// Size on disk when known, otherwise the logical size; this is how items are ranked.
    pub size: u64,
    pub size_text: String,
    pub files: Option<u64>,
    pub modified_unix_ms: Option<u64>,
    pub status: Status,
}
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub selection_source_id: Option<String>,
    pub kind: ItemKind,
    pub items: Vec<Item>,
    pub total: usize,
    pub next_offset: Option<usize>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub root: Option<String>,
    pub fixed: bool,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub total_text: Option<String>,
    pub free_text: Option<String>,
    pub filesystem: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct StorageView {
    pub scope_id: String,
    pub root: String,
    pub status: Status,
    pub duration_ms: u64,
    pub scanned_at_unix_ms: Option<u64>,
    pub scanned: Option<Totals>,
    pub scanned_text: Option<String>,
    pub volume: Volume,
    pub coverage: Coverage,
    pub denied: Vec<String>,
    pub dirs: Page,
    pub files: Page,
    pub limitations: Vec<String>,
}
pub fn item(i: &ItemView, root: &str) -> Item {
    let path = i.path.clone().unwrap_or_default();
    let trimmed = path.trim_end_matches('\\');
    let (folder, name) = trimmed.rsplit_once('\\').unwrap_or(("", trimmed));
    let parent = folder
        .strip_prefix(root.trim_end_matches('\\'))
        .map(|rest| rest.trim_start_matches('\\').to_owned())
        .unwrap_or_else(|| folder.to_owned());
    let size = i.allocated_bytes.unwrap_or(i.logical_bytes);
    Item {
        item_ref: i.item_ref.clone(),
        parent_ref: i.parent_ref.clone(),
        kind: i.kind,
        system: i.kind == ItemKind::Dirs
            && folder.len() == 2
            && folder.ends_with(':')
            && [
                "windows",
                "program files",
                "program files (x86)",
                "programdata",
                "$recycle.bin",
                "system volume information",
            ]
            .contains(&name.to_ascii_lowercase().as_str()),
        file_type: (i.kind == ItemKind::Files)
            .then(|| pcagent_storage::FileType::of(std::path::Path::new(name))),
        name: if name.is_empty() {
            path.clone()
        } else {
            name.to_owned()
        },
        parent,
        path: path.clone(),
        logical_bytes: i.logical_bytes,
        allocated_bytes: i.allocated_bytes,
        size,
        size_text: format_bytes(size),
        files: i.files,
        modified_unix_ms: i.modified_unix_ms,
        status: i.status,
    }
}
pub fn page(p: &LargestItemsPage, root: &str) -> Page {
    Page {
        selection_source_id: None,
        kind: p.kind,
        items: p.items.iter().map(|i| item(i, root)).collect(),
        total: p.total,
        next_offset: p.next_offset,
    }
}
pub fn storage(
    root: &str,
    summary: &ScanSummary,
    overview: &StorageOverview,
    dirs: &LargestItemsPage,
    files: &LargestItemsPage,
) -> StorageView {
    let v = &overview.volume;
    let location = pcagent_storage::volume_location(std::path::Path::new(root));
    StorageView {
        scope_id: summary.scope_id.clone(),
        root: root.to_owned(),
        status: summary.status,
        duration_ms: summary.duration_ms,
        scanned_at_unix_ms: overview.window.as_ref().map(|w| w.finished_at_unix_ms),
        scanned_text: overview
            .scanned
            .as_ref()
            .map(|t| format_bytes(t.allocated_bytes.max(t.logical_bytes))),
        scanned: overview.scanned.clone(),
        volume: Volume {
            root: location.as_ref().map(|(root, _)| root.clone()),
            fixed: location.is_some_and(|(_, fixed)| fixed),
            total_bytes: v.total_bytes,
            free_bytes: v.free_bytes,
            total_text: v.total_bytes.map(format_bytes),
            free_text: v.free_bytes.map(format_bytes),
            filesystem: v.filesystem.clone(),
        },
        coverage: summary.coverage.clone(),
        denied: overview
            .denied
            .iter()
            .filter_map(|d| d.path.clone())
            .collect(),
        dirs: page(dirs, root),
        files: page(files, root),
        limitations: overview.limitations.clone(),
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub scope_id: String,
    pub entries: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub bytes_text: String,
    pub elapsed_ms: u64,
}
pub fn progress(scope_id: &str, p: &ScanProgress) -> Progress {
    Progress {
        scope_id: scope_id.to_owned(),
        entries: p.entries_indexed,
        dirs: p.dirs_enumerated,
        bytes: p.logical_bytes_seen,
        bytes_text: format_bytes(p.logical_bytes_seen),
        elapsed_ms: p.elapsed_ms,
    }
}
