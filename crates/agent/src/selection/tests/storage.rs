use super::*;
use pcagent_storage::{ItemKind, ItemView, LargestItemsPage, LastUse};

#[test]
fn storage_projection_keeps_allocations_and_aliases_but_no_scan_authorization_keys_or_paths() {
    let source = storage_source();
    let selection = frozen(
        &source,
        &SelectionTarget::Storage {
            scope_id: "scope-private".into(),
            item_ref: "scan-private:17".into(),
        },
    );
    let message = selection.preview_message(&[], "Explain this item.", &privacy());
    let item = &message["initial_observation"]["items"][0];
    assert_eq!(item["logical_bytes"], 2048);
    assert_eq!(item["allocated_bytes"], 4096);
    assert_eq!(item["item_ref"], "file-17");
    assert_eq!(item["last_used"], "unknown");
    assert!(item["estimated_reclaim_bytes"].is_null());
    for private in ["scan-private", "scope-private", "secret.pdf", "Alice", "C:"] {
        assert!(!message.to_string().contains(private));
    }
    assert!(FrozenSelection::freeze(
        &source,
        &SelectionTarget::Storage {
            scope_id: "scope-private".into(),
            item_ref: "scan-new:17".into()
        },
        SelectionIntent::Explain,
        6000,
        Sharing::Anonymized,
        false,
        &privacy()
    )
    .is_err());
}

fn storage_source() -> SelectionSource {
    let item = ItemView {
        item_ref: "scan-private:17".into(),
        alias: "file-17".into(),
        alias_path: "<scope>/file-17".into(),
        parent_ref: Some("scan-private:0".into()),
        kind: ItemKind::Files,
        path: Some(r"C:\Users\Alice\secret.pdf".into()),
        redacted_path: "%USERPROFILE%/secret.pdf".into(),
        logical_bytes: 2048,
        allocated_bytes: Some(4096),
        allocation_unknown_files: 0,
        estimated_reclaim_bytes: None,
        files: Some(1),
        link_count: Some(1),
        sparse: false,
        compressed: false,
        modified_unix_ms: Some(500),
        last_used: LastUse::Unknown,
        status: Status::Ok,
    };
    SelectionSource::Storage(StorageSource {
        page: LargestItemsPage {
            schema_version: 1,
            scope_id: "scope-private".into(),
            scan_id: "scan-private".into(),
            kind: ItemKind::Files,
            sort_basis: "allocated_bytes".into(),
            offset: 0,
            limit: 1,
            total: 1,
            next_offset: None,
            items: vec![item],
            scan_status: Status::Ok,
            limitations: vec![],
        },
        window: Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 2000,
            duration_ms: 1000,
        },
        origin: Origin::SyntheticFixture,
        context: StorageContext::default(),
    })
}
