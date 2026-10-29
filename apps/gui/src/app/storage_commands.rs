//! Explicit folder selection, bounded scans and item reveal.
use super::selection_commands::{retain_storage_facts, retain_storage_page};
use super::{lock, AppState};
use crate::failure::{Code, Failure};
use crate::{failure, shell, view};
use pcagent_storage::{ItemKind, ScanOptions, MAX_PAGE};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

pub(super) const PAGE: usize = 50;

#[derive(Serialize)]
pub(super) struct QuickFolder {
    id: &'static str,
    path: String,
}

/// Common places, from the user's environment; only existing folders are offered.
#[tauri::command]
pub(super) fn quick_folders() -> Vec<QuickFolder> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let profile = var("USERPROFILE");
    // SystemDrive is "C:"; the folder to scan is its root, "C:\".
    let drive = std::env::var_os("SystemDrive").map(|mut d| {
        d.push("\\");
        PathBuf::from(d)
    });
    [
        ("downloads", profile.as_ref().map(|p| p.join("Downloads"))),
        ("desktop", profile.as_ref().map(|p| p.join("Desktop"))),
        ("documents", profile.as_ref().map(|p| p.join("Documents"))),
        ("temp", var("TEMP")),
        ("profile", profile.clone()),
        ("system_drive", drive),
    ]
    .into_iter()
    .filter_map(|(id, path)| {
        let path = path?;
        path.is_dir().then(|| QuickFolder {
            id,
            path: path.display().to_string(),
        })
    })
    .collect()
}

#[tauri::command]
pub(super) async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let texts = shell::shell_texts(pcagent_inspect::elevation::windows_ui_is_french());
        app.dialog()
            .file()
            .set_title(texts.folder_dialog)
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
pub(super) struct ScanStarted {
    scope_id: String,
    root: String,
}
/// `code` translates `message` for the page (failure.rs); the message stays for diagnosis.
#[derive(Clone, Serialize)]
pub(super) struct ScanFailed {
    scope_id: String,
    code: Code,
    message: String,
}

#[tauri::command]
pub(super) fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<ScanStarted, Failure> {
    let root = PathBuf::from(path.trim());
    let shown = root.display().to_string();
    if !root.is_absolute() {
        return Err(Failure::new(
            Code::NotAFolder,
            "Choose a complete folder path, such as C:\\Users\\you\\Downloads.",
        ));
    }
    if !root.exists() {
        return Err(Failure::new(
            Code::FolderMissing,
            format!("{shown} does not exist."),
        ));
    }
    let mut slot = lock(&state.scan);
    if slot.running {
        return Err(Failure::new(
            Code::ScanRunning,
            "A scan is already running.",
        ));
    }
    let info = state
        .storage
        .authorize(&root)
        .map_err(|e| failure::storage(&e, &shown))?;
    // Only the latest folder stays authorized and indexed; its memory is freed otherwise.
    if let Some(old) = slot.scope_id.replace(info.scope_id.clone()) {
        state.storage.revoke(&old);
        lock(&state.selections).invalidate_storage(&old);
    }
    slot.root = Some(info.root.clone());
    slot.running = true;
    drop(slot);
    let (storage, scan, selections) = (
        state.storage.clone(),
        state.scan.clone(),
        state.selections.clone(),
    );
    let started = ScanStarted {
        scope_id: info.scope_id.clone(),
        root: info.root.clone(),
    };
    thread::Builder::new()
        .name("storage-scan".into())
        .spawn(move || {
            let scope = info.scope_id;
            let mut last = Instant::now();
            let result = storage
                .scan_blocking(&scope, &ScanOptions::default(), &mut |p| {
                    if last.elapsed() >= Duration::from_millis(150) {
                        last = Instant::now();
                        let _ = app.emit("scan:progress", view::progress(&scope, p));
                    }
                })
                .and_then(|summary| {
                    let overview = storage.get_storage_overview(&scope)?;
                    let dirs = storage.get_largest_items(&scope, ItemKind::Dirs, 0, PAGE)?;
                    let files = storage.get_largest_items(&scope, ItemKind::Files, 0, PAGE)?;
                    let dir_source =
                        retain_storage_page(&selections, &storage, &scan, dirs.clone());
                    let file_source =
                        retain_storage_page(&selections, &storage, &scan, files.clone());
                    let mut shown = view::storage(&info.root, &summary, &overview, &dirs, &files);
                    shown.dirs.selection_source_id = dir_source;
                    shown.files.selection_source_id = file_source;
                    Ok(shown)
                });
            lock(&scan).running = false;
            let _ = match result {
                Ok(done) => app.emit("scan:done", done),
                Err(e) => {
                    let failed = failure::storage(&e, &info.root);
                    app.emit(
                        "scan:failed",
                        ScanFailed {
                            scope_id: scope,
                            code: failed.code,
                            message: failed.message,
                        },
                    )
                }
            };
        })
        .map_err(|e| {
            lock(&state.scan).running = false;
            Failure::new(Code::ScanFailed, format!("Cannot start the scan: {e}"))
        })?;
    Ok(started)
}

#[tauri::command]
pub(super) fn cancel_scan(state: State<'_, AppState>) {
    state.storage.cancel_item_facts();
    if let Some(scope) = lock(&state.scan).scope_id.clone() {
        let _ = state.storage.cancel(&scope);
    }
}

#[derive(Serialize)]
pub(super) struct Children {
    #[serde(flatten)]
    page: view::Page,
    parent: view::StorageItem,
    other_bytes: u64,
    parent_partial: bool,
}

#[tauri::command]
pub(super) async fn scan_children(
    state: State<'_, AppState>,
    scope_id: String,
    item_ref: Option<String>,
    kind: ItemKind,
    offset: usize,
) -> Result<Children, Failure> {
    let storage = state.storage.clone();
    let root = lock(&state.scan).root.clone().unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let p = storage
            .index(&scope_id)
            .and_then(|i| i.children(item_ref.as_deref(), kind, offset))
            .map_err(|e| failure::storage(&e, &root))?;
        Ok(Children {
            page: view::page(&p.page, &root),
            parent: view::storage_item(&p.parent, &root),
            other_bytes: p.other_bytes,
            parent_partial: p.parent_partial,
        })
    })
    .await
    .map_err(|_| Failure::new(Code::ScanFailed, "Cannot read scan children."))?
}

#[derive(Serialize)]
pub(super) struct FactsView {
    item: view::StorageItem,
    #[serde(flatten)]
    evidence: pcagent_storage::StorageItemEvidence,
    selection_source_id: String,
}

#[tauri::command]
pub(super) async fn scan_item_facts(
    state: State<'_, AppState>,
    scope_id: String,
    item_ref: String,
) -> Result<FactsView, Failure> {
    let (storage, selections, scan) = (
        state.storage.clone(),
        state.selections.clone(),
        state.scan.clone(),
    );
    let root = lock(&scan).root.clone().unwrap_or_default();
    let job = storage
        .prepare_item_facts(&scope_id, &item_ref)
        .map_err(|e| failure::storage(&e, &root))?;
    tauri::async_runtime::spawn_blocking(move || {
        let unavailable = || {
            Failure::new(
                Code::ItemUnavailable,
                "The scan item is no longer available.",
            )
        };
        let index = storage.index(&scope_id).map_err(|_| unavailable())?;
        let facts = job.run().map_err(|e| failure::storage(&e, &root))?;
        let (page, parent) = index.item_page(&item_ref).map_err(|_| unavailable())?;
        let volume = storage
            .get_storage_overview(&scope_id)
            .map_err(|_| unavailable())?
            .volume;
        let evidence = pcagent_storage::StorageItemEvidence {
            item_facts: (*facts).clone(),
            parent_ref: parent.item_ref,
            parent_alias: parent.alias,
            rank_scope: "siblings_of_parent".into(),
            volume_total_bytes: volume.total_bytes,
        };
        let item = view::storage_item(&page.items[0], &root);
        let source =
            retain_storage_facts(&selections, &storage, &scan, page, Some(evidence.clone()))
                .ok_or_else(unavailable)?;
        Ok(FactsView {
            item,
            evidence,
            selection_source_id: source,
        })
    })
    .await
    .map_err(|_| Failure::new(Code::ScanFailed, "Cannot read item facts."))?
}

#[tauri::command]
pub(super) fn scan_page(
    state: State<'_, AppState>,
    scope_id: String,
    kind: ItemKind,
    offset: usize,
) -> Result<view::Page, Failure> {
    let root = lock(&state.scan).root.clone().unwrap_or_default();
    state
        .storage
        .get_largest_items(&scope_id, kind, offset, PAGE.min(MAX_PAGE))
        .map(|p| {
            let selection_source_id =
                retain_storage_page(&state.selections, &state.storage, &state.scan, p.clone());
            let mut shown = view::page(&p, &root);
            shown.selection_source_id = selection_source_id;
            shown
        })
        .map_err(|e| failure::storage(&e, &root))
}

/// Opens Explorer on an item of the latest scan. The page names the item by reference only.
#[tauri::command]
pub(super) fn reveal(
    state: State<'_, AppState>,
    scope_id: String,
    item_ref: String,
) -> Result<(), Failure> {
    let item = state
        .storage
        .index(&scope_id)
        .and_then(|index| index.item(&item_ref))
        .map_err(|_| {
            Failure::new(
                Code::ItemUnavailable,
                "This item is no longer in the latest scan.",
            )
        })?;
    let path = item
        .path
        .ok_or_else(|| Failure::new(Code::ItemUnavailable, "This item has no local path."))?;
    pcagent_actions::show_in_explorer(Path::new(&path))
        .map_err(|e| Failure::new(Code::OpenFailed, e.detail))
}
