//! Portable tests on disposable temp trees. Windows-only cases are `#[cfg(windows)]`.
use super::*;
use std::{fs, io::Write};

struct Tree(PathBuf);
impl Tree {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(new_id("pcagent-storage-test"));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn file(&self, rel: &str, len: usize) -> PathBuf {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::File::create(&p)
            .unwrap()
            .write_all(&vec![7_u8; len])
            .unwrap();
        p
    }
}
impl Drop for Tree {
    fn drop(&mut self) {
        #[cfg(unix)]
        restore_permissions(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[cfg(unix)]
fn restore_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                restore_permissions(&e.path());
            }
        }
    }
}

fn scan_with(
    root: &Path,
    options: ScanOptions,
    progress: &mut dyn FnMut(&ScanProgress),
) -> StorageIndex {
    let scope = ScanScope::authorize(root).unwrap();
    ClassicBackend.scan(&scope, &options, &CancelToken::default(), progress)
}
fn scan(root: &Path) -> StorageIndex {
    scan_with(root, ScanOptions::default(), &mut |_| {})
}

#[test]
fn nested_dirs_aggregate_without_double_counting() {
    let t = Tree::new();
    t.file("a/b/c/deep.bin", 3000);
    t.file("a/b/mid.bin", 2000);
    t.file("a/top.bin", 1000);
    t.file("other/x.bin", 10);
    let ix = scan(&t.0);
    assert_eq!(ix.status(), Status::Ok);
    let totals = ix.totals();
    assert_eq!(totals.logical_bytes, 6010);
    assert_eq!(totals.files, 4);
    assert_eq!(ix.coverage().dirs, 5);
    let dirs = ix.largest_items(ItemKind::Dirs, 0, 100).unwrap();
    let a = dirs
        .items
        .iter()
        .find(|i| i.redacted_path == "<scope>/a")
        .unwrap();
    assert_eq!((a.logical_bytes, a.files), (6000, Some(3)));
    let files = ix.largest_items(ItemKind::Files, 0, 100).unwrap();
    assert_eq!(files.items[0].redacted_path, "<scope>/a/b/c/deep.bin");
    assert_eq!(files.items[0].last_used, LastUse::Unknown);
    assert!(files.items[0].modified_unix_ms.is_some());
    assert_eq!(files.items[0].estimated_reclaim_bytes, None);
    let json = serde_json::to_value(&files.items[0]).unwrap();
    assert_eq!(json["last_used"], "unknown");
    assert!(json["estimated_reclaim_bytes"].is_null());
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let t = Tree::new();
    let outside = Tree::new();
    outside.file("secret/huge.bin", 50_000);
    t.file("inside.bin", 100);
    std::os::unix::fs::symlink(outside.0.join("secret"), t.0.join("dirlink")).unwrap();
    std::os::unix::fs::symlink(outside.0.join("secret/huge.bin"), t.0.join("filelink")).unwrap();
    let ix = scan(&t.0);
    assert_eq!(ix.coverage().reparse_skipped, 2);
    assert_eq!(ix.totals().logical_bytes, 100);
    assert_eq!(ix.status(), Status::Ok);
    assert_eq!(
        ScanScope::authorize(&t.0.join("dirlink")).unwrap_err(),
        StorageError::ReparseRoot
    );
}

#[cfg(windows)]
#[test]
fn junctions_are_not_followed() {
    let t = Tree::new();
    let outside = Tree::new();
    outside.file("secret/huge.bin", 50_000);
    t.file("inside.bin", 100);
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(t.0.join("junction"))
        .arg(outside.0.join("secret"))
        .status()
        .unwrap();
    assert!(status.success());
    let ix = scan(&t.0);
    assert_eq!(ix.coverage().reparse_skipped, 1);
    assert_eq!(ix.totals().logical_bytes, 100);
    assert_eq!(
        ScanScope::authorize(&t.0.join("junction")).unwrap_err(),
        StorageError::ReparseRoot
    );
}

#[test]
fn hard_links_are_counted_once() {
    let t = Tree::new();
    let original = t.file("a/original.bin", 4096);
    fs::create_dir_all(t.0.join("b")).unwrap();
    fs::hard_link(&original, t.0.join("b/alias.bin")).unwrap();
    let ix = scan(&t.0);
    assert_eq!(ix.coverage().hard_link_aliases, 1);
    assert_eq!(ix.coverage().files, 2);
    let totals = ix.totals();
    assert_eq!((totals.logical_bytes, totals.files), (4096, 1));
    let files = ix.largest_items(ItemKind::Files, 0, 10).unwrap();
    assert_eq!(files.total, 1);
    assert_eq!(files.items[0].link_count, Some(2));
}

#[cfg(unix)]
#[test]
fn permission_denied_directories_are_listed() {
    use std::os::unix::fs::PermissionsExt;
    let t = Tree::new();
    t.file("open/ok.bin", 10);
    t.file("locked/hidden.bin", 10);
    let locked = t.0.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        eprintln!("skipped: running with permission bypass (root)");
        return;
    }
    let ix = scan(&t.0);
    assert_eq!(ix.status(), Status::Partial);
    assert_eq!(ix.coverage().denied_dirs, 1);
    let overview = StorageOverview::build(
        &ScanScope::authorize(&t.0).unwrap(),
        Some(&ix),
        platform::volume_space(&t.0),
        false,
    );
    assert_eq!(overview.denied.len(), 1);
    assert_eq!(overview.denied[0].status, Status::PermissionDenied);
    assert_eq!(overview.denied[0].redacted_path, "<scope>/locked");
    assert!(overview.clone().without_paths().denied[0].path.is_none());
}

#[test]
fn unicode_names_round_trip() {
    let t = Tree::new();
    t.file("jeux é/日本語 🦀.bin", 42);
    let ix = scan(&t.0);
    let files = ix.largest_items(ItemKind::Files, 0, 1).unwrap();
    assert_eq!(files.items[0].redacted_path, "<scope>/jeux é/日本語 🦀.bin");
    assert!(files.items[0]
        .path
        .as_deref()
        .unwrap()
        .ends_with("日本語 🦀.bin"));
}

#[cfg(windows)]
#[test]
fn long_paths_are_scanned() {
    let t = Tree::new();
    let segment = "a".repeat(60);
    let rel = (0..6)
        .map(|_| segment.as_str())
        .collect::<Vec<_>>()
        .join("\\");
    let deep = t.file(&format!("{rel}\\long.bin"), 77);
    assert!(deep.as_os_str().len() > 300);
    let ix = scan(&t.0);
    assert_eq!(ix.status(), Status::Ok);
    let files = ix.largest_items(ItemKind::Files, 0, 1).unwrap();
    assert_eq!(files.items[0].logical_bytes, 77);
    assert!(files.items[0].allocated_bytes.is_some());
    assert!(!files.items[0].path.as_deref().unwrap().starts_with(r"\\?\"));
}

#[test]
fn cancellation_stops_with_cancelled_status() {
    let t = Tree::new();
    for i in 0..5 {
        t.file(&format!("d{i}/f.bin"), 10);
    }
    let scope = ScanScope::authorize(&t.0).unwrap();
    let token = CancelToken::default();
    let inner = token.clone();
    let ix = ClassicBackend.scan(&scope, &ScanOptions::default(), &token, &mut |_| {
        inner.cancel()
    });
    assert_eq!(ix.status(), Status::Cancelled);
    assert!(ix.coverage().cancelled);
    assert!(ix.totals().files < 5);
}

#[test]
fn entry_and_memory_limits_stop_as_partial() {
    let t = Tree::new();
    for i in 0..20 {
        t.file(&format!("d{}/f{i}.bin", i % 4), 10);
    }
    let limited = ScanOptions {
        max_entries: 6,
        ..ScanOptions::default()
    };
    let ix = scan_with(&t.0, limited, &mut |_| {});
    assert_eq!(ix.status(), Status::Partial);
    assert_eq!(ix.coverage().truncated, Some(Truncation::EntryLimit));
    assert!(ix.entries.len() <= 6);
    let tiny = ScanOptions {
        index_budget_bytes: 1,
        ..ScanOptions::default()
    };
    let ix = scan_with(&t.0, tiny, &mut |_| {});
    assert_eq!(ix.status(), Status::Partial);
    assert_eq!(ix.coverage().truncated, Some(Truncation::MemoryBudget));
}

#[test]
fn files_changed_or_removed_during_scan_are_partial() {
    let t = Tree::new();
    let gone = t.file("gone.bin", 10);
    let changed = t.file("changed.bin", 10);
    t.file("stable.bin", 10);
    let mut first = true;
    let ix = scan_with(&t.0, ScanOptions::default(), &mut |_| {
        if std::mem::take(&mut first) {
            fs::remove_file(&gone).unwrap();
            let f = fs::OpenOptions::new().write(true).open(&changed).unwrap();
            f.set_modified(std::time::SystemTime::now() + Duration::from_secs(3600))
                .unwrap();
        }
    });
    assert_eq!(ix.status(), Status::Partial);
    assert_eq!(ix.coverage().vanished, 1);
    assert_eq!(ix.coverage().changed_during_scan, 1);
    assert_eq!(ix.totals().files, 2);
}

#[test]
fn parent_child_sets_are_deduplicated() {
    let paths: Vec<PathBuf> = ["/a/b", "/a", "/a/b/c", "/x", "/xy", "/a"]
        .iter()
        .map(PathBuf::from)
        .collect();
    let kept = dedup_parent_child(&paths);
    assert_eq!(kept, [PathBuf::from("/a"), "/x".into(), "/xy".into()]);

    let t = Tree::new();
    t.file("p/c/f1.bin", 100);
    t.file("p/f2.bin", 50);
    t.file("q.bin", 7);
    let ix = scan(&t.0);
    let dirs = ix.largest_items(ItemKind::Dirs, 0, 10).unwrap();
    let files = ix.largest_items(ItemKind::Files, 0, 10).unwrap();
    let find = |items: &[ItemView], p: &str| {
        items
            .iter()
            .find(|i| i.redacted_path == p)
            .unwrap()
            .item_ref
            .clone()
    };
    let (p, c) = (
        find(&dirs.items, "<scope>/p"),
        find(&dirs.items, "<scope>/p/c"),
    );
    let (f1, q) = (
        find(&files.items, "<scope>/p/c/f1.bin"),
        find(&files.items, "<scope>/q.bin"),
    );
    let totals = ix.deduplicated_totals(&[&p, &c, &f1, &q]).unwrap();
    assert_eq!((totals.logical_bytes, totals.files), (157, 3));
    assert_eq!(
        ix.deduplicated_totals(&["scan-forged:1"]),
        Err(StorageError::UnknownItem)
    );
}

#[test]
fn pagination_is_bounded() {
    let t = Tree::new();
    for i in 0..7 {
        t.file(&format!("f{i}.bin"), 100 * (i + 1));
    }
    let ix = scan(&t.0);
    assert_eq!(
        ix.largest_items(ItemKind::Files, 0, 0).unwrap_err(),
        StorageError::InvalidPage
    );
    assert_eq!(
        ix.largest_items(ItemKind::Files, 0, 101).unwrap_err(),
        StorageError::InvalidPage
    );
    let p1 = ix.largest_items(ItemKind::Files, 0, 3).unwrap();
    assert_eq!((p1.total, p1.items.len(), p1.next_offset), (7, 3, Some(3)));
    let sizes: Vec<u64> = p1.items.iter().map(|i| i.logical_bytes).collect();
    assert_eq!(sizes, [700, 600, 500]);
    let p3 = ix.largest_items(ItemKind::Files, 6, 3).unwrap();
    assert_eq!((p3.items.len(), p3.next_offset), (1, None));
    let beyond = ix.largest_items(ItemKind::Files, usize::MAX, 100).unwrap();
    assert!(beyond.items.is_empty() && beyond.next_offset.is_none());
}

#[test]
fn item_refs_are_opaque_and_validated() {
    let t = Tree::new();
    t.file("a.bin", 1);
    let ix = scan(&t.0);
    let r = ix.largest_items(ItemKind::Files, 0, 1).unwrap().items[0]
        .item_ref
        .clone();
    assert!(!r.contains("a.bin"));
    assert!(ix.local_path(&r).unwrap().ends_with("a.bin"));
    for forged in [
        "",
        "x",
        &format!("{}:99", ix.scan_id()),
        &format!("{}:+1", ix.scan_id()),
        "scan-other:1",
    ] {
        assert!(ix.resolve(forged).is_none(), "{forged}");
    }
}

#[test]
fn attributes_classify_placeholders_before_reparse_points() {
    for attr in [
        ATTR_RECALL_ON_DATA_ACCESS,
        ATTR_RECALL_ON_OPEN,
        ATTR_OFFLINE,
    ] {
        assert_eq!(
            classify_attributes(attr | ATTR_REPARSE_POINT),
            AttributeClass::Placeholder
        );
        assert_eq!(
            classify_attributes(attr | ATTR_DIRECTORY),
            AttributeClass::Placeholder
        );
    }
    assert_eq!(
        classify_attributes(ATTR_REPARSE_POINT | ATTR_DIRECTORY),
        AttributeClass::Reparse
    );
    assert_eq!(classify_attributes(ATTR_DIRECTORY), AttributeClass::Dir);
    assert_eq!(
        classify_attributes(ATTR_SPARSE_FILE | ATTR_COMPRESSED),
        AttributeClass::File
    );
}

#[cfg(windows)]
#[test]
fn attribute_constants_match_windows_sdk() {
    use windows_sys::Win32::Storage::FileSystem as fs;
    assert_eq!(ATTR_DIRECTORY, fs::FILE_ATTRIBUTE_DIRECTORY);
    assert_eq!(ATTR_REPARSE_POINT, fs::FILE_ATTRIBUTE_REPARSE_POINT);
    assert_eq!(ATTR_SPARSE_FILE, fs::FILE_ATTRIBUTE_SPARSE_FILE);
    assert_eq!(ATTR_COMPRESSED, fs::FILE_ATTRIBUTE_COMPRESSED);
    assert_eq!(ATTR_OFFLINE, fs::FILE_ATTRIBUTE_OFFLINE);
    assert_eq!(ATTR_RECALL_ON_OPEN, fs::FILE_ATTRIBUTE_RECALL_ON_OPEN);
    assert_eq!(
        ATTR_RECALL_ON_DATA_ACCESS,
        fs::FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
    );
}

#[test]
fn redaction_is_relative_and_hides_the_profile_name() {
    let profile = Path::new("/home/alice");
    let root = Path::new("/home/alice/Games");
    assert_eq!(
        redact_path(
            root,
            Path::new("/home/alice/Games/alice/x.bin"),
            Some(profile)
        ),
        "<scope>/<user>/x.bin"
    );
    assert_eq!(
        redact_path(root, Path::new("/etc/passwd"), Some(profile)),
        "<outside-scope>"
    );
    assert_eq!(index::redact_root(root, Some(profile)), "<profile>/Games");
    assert_eq!(
        index::redact_root(Path::new("/srv/alice"), Some(profile)),
        "<volume>/srv/<user>"
    );
}

#[cfg(unix)]
#[test]
fn allocation_and_logical_size_are_distinct() {
    let t = Tree::new();
    let f = fs::File::create(t.0.join("sparse.bin")).unwrap();
    f.set_len(64 << 20).unwrap();
    t.file("dense.bin", 1);
    let ix = scan(&t.0);
    let files = ix.largest_items(ItemKind::Files, 0, 10).unwrap();
    let sparse = files
        .items
        .iter()
        .find(|i| i.redacted_path.ends_with("sparse.bin"))
        .unwrap();
    assert_eq!(sparse.logical_bytes, 64 << 20);
    assert!(sparse.allocated_bytes.unwrap() < sparse.logical_bytes);
    let dense = files
        .items
        .iter()
        .find(|i| i.redacted_path.ends_with("dense.bin"))
        .unwrap();
    assert!(dense.allocated_bytes.unwrap() >= dense.logical_bytes);
}

#[cfg(not(windows))]
#[test]
fn non_windows_volume_space_is_unknown_not_zero() {
    let t = Tree::new();
    let service = StorageService::new();
    let scope = service.authorize(&t.0).unwrap();
    let o = service.get_storage_overview(&scope.scope_id).unwrap();
    assert_eq!(o.volume.status, Status::Unsupported);
    assert!(o.volume.total_bytes.is_none() && o.unattributed_bytes.is_none());
    assert!(o.scan.is_none());
    let json = serde_json::to_value(&o).unwrap();
    assert!(json["volume"]["free_bytes"].is_null());
}

#[cfg(windows)]
#[test]
fn native_volume_space_and_allocation_smoke_test() {
    let t = Tree::new();
    t.file("a.bin", 5000);
    let service = StorageService::new();
    let scope = service.authorize(&t.0).unwrap();
    service
        .scan_blocking(&scope.scope_id, &ScanOptions::default(), &mut |_| {})
        .unwrap();
    let o = service.get_storage_overview(&scope.scope_id).unwrap();
    assert_eq!(o.volume.status, Status::Ok);
    assert!(o.volume.total_bytes.unwrap() > 0);
    assert!(o.unattributed_bytes.is_some());
    let files = service
        .get_largest_items(&scope.scope_id, ItemKind::Files, 0, 1)
        .unwrap();
    assert!(files.items[0].allocated_bytes.unwrap() >= 4096);
    assert_eq!(files.items[0].link_count, Some(1));
}

#[test]
fn service_runs_background_scans_and_rejects_unknown_scopes() {
    let t = Tree::new();
    t.file("a/b.bin", 123);
    let service = Arc::new(StorageService::new());
    assert_eq!(
        service.get_storage_overview("scope-x").unwrap_err(),
        StorageError::UnknownScope
    );
    let scope = service.authorize(&t.0).unwrap();
    assert!(scope.redacted_root.starts_with('<'));
    assert_eq!(
        service
            .get_largest_items(&scope.scope_id, ItemKind::Files, 0, 10)
            .unwrap_err(),
        StorageError::NotScanned
    );
    let job = service
        .start_storage_scan(&scope.scope_id, ScanOptions::default())
        .unwrap();
    let summary = job.wait().unwrap();
    assert_eq!(summary.status, Status::Ok);
    assert_eq!(summary.coverage.files, 1);
    let page = service
        .get_largest_items(&scope.scope_id, ItemKind::Files, 0, 10)
        .unwrap()
        .without_paths();
    assert!(page.items[0].path.is_none());
    assert_eq!(page.items[0].logical_bytes, 123);
    let overview = service.get_storage_overview(&scope.scope_id).unwrap();
    assert_eq!(overview.scanned.unwrap().logical_bytes, 123);
    assert!(!overview.scan_running);
    assert!(service.revoke(&scope.scope_id));
    assert_eq!(
        service.index(&scope.scope_id).unwrap_err(),
        StorageError::UnknownScope
    );
    let args: LargestItemsArgs =
        serde_json::from_str(r#"{"scope_id":"s","kind":"dirs","limit":5}"#).unwrap();
    assert_eq!((args.kind, args.offset), (ItemKind::Dirs, 0));
    assert!(serde_json::from_str::<LargestItemsArgs>(
        r#"{"scope_id":"s","kind":"dirs","limit":5,"path":"/"}"#
    )
    .is_err());
}

#[test]
fn authorization_rejects_files_and_missing_paths() {
    let t = Tree::new();
    let f = t.file("x.bin", 1);
    assert_eq!(
        ScanScope::authorize(&f).unwrap_err(),
        StorageError::NotADirectory
    );
    assert!(matches!(
        ScanScope::authorize(&t.0.join("missing")).unwrap_err(),
        StorageError::Io(_)
    ));
}

#[test]
fn redact_names_keeps_only_generic_components() {
    use crate::index::redact_names;
    assert_eq!(redact_names("<scope>/CV Nassim.pdf"), "<scope>/*.pdf");
    assert_eq!(
        redact_names("<scope>/client-x/app/node_modules"),
        "<scope>/*/*/node_modules"
    );
    assert_eq!(
        redact_names("<scope>/<user>/Temp/日本語 🦀.bin"),
        "<scope>/<user>/Temp/*.bin"
    );
    assert_eq!(redact_names("<scope>/secret.folder/x"), "<scope>/*/*");
}
