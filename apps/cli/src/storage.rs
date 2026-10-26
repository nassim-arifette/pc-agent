//! Explicitly authorized local folder scans and content comparisons.
use crate::local::write_json;
use crate::{args as options, text, Result};
use pcagent_agent::StorageContext;
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::Status;
use pcagent_storage::{ContentOptions, ItemKind, ScanOptions, StorageService};
use serde_json::json;
use std::path::Path;
use std::{fs, io};

/// Authorizes a folder for scanning, with messages that name the folder the user typed.
pub(super) fn authorize_folder(
    storage: &StorageService,
    root: &Path,
) -> Result<pcagent_storage::ScopeInfo> {
    let shown = text::path(root);
    match fs::metadata(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!("Folder not found: {shown}").into())
        }
        Ok(m) if !m.is_dir() => return Err(format!("Not a folder: {shown}").into()),
        _ => {}
    }
    storage
        .authorize(root)
        .map_err(|e| format!("Cannot scan {shown}: {e}").into())
}

pub(super) fn scan(options: options::Storage, json: bool) -> Result<u8> {
    let options::Storage {
        root,
        limit,
        dirs,
        compare_content,
    } = options;
    let storage = StorageService::new();
    let info = authorize_folder(&storage, &root)?;
    let scope = info.scope_id;
    let summary = storage.scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})?;
    let kind = if dirs {
        ItemKind::Dirs
    } else {
        ItemKind::Files
    };
    let overview = storage.get_storage_overview(&scope)?;
    let largest = storage.get_largest_items(&scope, kind, 0, usize::from(limit))?;
    let mut context = StorageContext::default();
    if compare_content {
        context.add_comparison(
            storage.compare_content(&scope, &ContentOptions::default())?,
            storage.index(&scope)?.as_ref(),
        );
    }
    let items = context.local_items(&largest.items, &Policy::from_env(), &Preferences::default());
    if json {
        write_json(
            &json!({"overview":overview,"largest":largest,"items":items,"content_comparisons":context.comparisons()}),
        )?;
    } else {
        text::storage(
            &mut io::stdout().lock(),
            &info.root,
            &summary,
            &overview,
            &largest,
            dirs,
        )?;
        text::storage_context(&mut io::stdout().lock(), &items)?;
        text::content_comparisons(&mut io::stdout().lock(), context.comparisons())?;
    }
    Ok(if summary.status == Status::Ok { 0 } else { 3 })
}
