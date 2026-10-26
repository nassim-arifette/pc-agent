use super::*;

#[test]
fn storage_items_show_their_folder_relative_to_the_scanned_root() {
    let view = ItemView {
        item_ref: "scan-1:7".into(),
        kind: ItemKind::Files,
        path: Some(r"C:\Users\me\Downloads\iso\disk.img".into()),
        redacted_path: "<scope>/*/*.img".into(),
        alias: "file-7.img".into(),
        alias_path: "<scope>/folder-2/file-7.img".into(),
        parent_ref: Some("scan-1:2".into()),
        logical_bytes: 1 << 30,
        allocated_bytes: None,
        allocation_unknown_files: 1,
        estimated_reclaim_bytes: None,
        files: None,
        link_count: Some(1),
        sparse: false,
        compressed: false,
        modified_unix_ms: Some(1),
        last_used: pcagent_storage::LastUse::Unknown,
        status: Status::Partial,
    };
    let i = item(&view, r"C:\Users\me\Downloads");
    assert_eq!((i.name.as_str(), i.parent.as_str()), ("disk.img", "iso"));
    // Unknown allocation: the logical size ranks and is shown.
    assert_eq!((i.size, i.size_text.as_str()), (1 << 30, "1.00 GiB"));
    let top = item(
        &ItemView {
            path: Some(r"C:\Users\me\Downloads\a.zip".into()),
            allocated_bytes: Some(4096),
            ..view
        },
        r"C:\Users\me\Downloads\",
    );
    assert_eq!((top.name.as_str(), top.parent.as_str()), ("a.zip", ""));
    assert_eq!(top.size, 4096);
}
