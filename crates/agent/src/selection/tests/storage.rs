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
        facts: None,
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

#[test]
fn facts_projection_is_frozen_anonymous_and_verifiable_in_both_sharing_modes() {
    use pcagent_storage::{FileType, FileTypeTotal, ItemFacts, MeasuredFiles, StorageItemEvidence};
    let mut source = storage_source();
    let SelectionSource::Storage(capture) = &mut source else {
        unreachable!()
    };
    let mut largest = capture.page.items[0].clone();
    largest.path = Some(r"C:\Users\Alice\private-largest.pdf".into());
    largest.redacted_path = "<scope>/private-largest.pdf".into();
    capture.facts = Some(Box::new(StorageItemEvidence {
        parent_ref: "scan-private:0".into(),
        parent_alias: "dir-0".into(),
        rank_scope: "siblings_of_parent".into(),
        volume_total_bytes: Some(8192),
        item_facts: ItemFacts {
            scanned_at_unix_ms: 2000,
            untouched_before_unix_ms: 1,
            files_count: 2341,
            untouched: MeasuredFiles {
                files_count: 2341,
                logical_bytes: 2048,
                allocated_bytes: Some(4096),
                allocation_unknown_files_count: 0,
            },
            undated_files_count: 7,
            largest_file: Some(largest),
            file_types: vec![FileTypeTotal {
                file_type: FileType::Documents,
                totals: MeasuredFiles {
                    files_count: 2341,
                    logical_bytes: 2048,
                    allocated_bytes: Some(4096),
                    allocation_unknown_files_count: 0,
                },
            }],
            status: Status::Partial,
            limitations: vec!["Synthetic partial scan.".into()],
        },
    }));
    for (sharing, names) in [(Sharing::Anonymized, false), (Sharing::Complete, true)] {
        let selection = FrozenSelection::freeze(
            &source,
            &SelectionTarget::Storage {
                scope_id: "scope-private".into(),
                item_ref: "scan-private:17".into(),
            },
            SelectionIntent::Explain,
            6000,
            sharing,
            names,
            &privacy(),
        )
        .unwrap();
        let message = selection.preview_message(&[], "Explain this item.", &privacy());
        let evidence = &message["initial_observation"];
        assert_eq!(evidence["parent_ref"], "dir-0");
        assert_eq!(evidence["items"][0]["parent_ref"], "dir-0");
        assert_eq!(evidence["rank_scope"], "siblings_of_parent");
        assert_eq!(evidence["volume_total_bytes"], 8192);
        assert_eq!(evidence["item_facts"]["files_count"], 2341);
        assert_eq!(
            evidence["item_facts"]["largest_file"]["item_ref"],
            "file-17"
        );
        assert_eq!(evidence["item_facts"]["status"], "partial");
        assert_eq!(evidence["measured_at_unix_ms"], 2000);
        for private in [
            "scan-private",
            "scope-private",
            "private-largest.pdf",
            "Alice",
            "C:",
        ] {
            assert!(!message.to_string().contains(private), "{private}");
        }
        for text in [
            "The scan recorded 2341 files [O0].",
            "L'analyse avait mesuré 2 341 fichiers [O0].",
            "The scan recorded 50 % of the volume [O0].",
        ] {
            assert!(checked(text, message.clone()), "{text}");
        }
        assert!(!checked(
            "The scan recorded 9000 files [O0].",
            message.clone()
        ));
        assert!(!checked("There are currently 2341 files [O0].", message));
    }
}
