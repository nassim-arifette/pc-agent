//! One projection for previews and model tools; local paths never enter that projection.
use pcagent_policy::{Attributes, CandidateInput, ItemContext, Policy, Preferences};
use pcagent_shared::Origin;
use pcagent_storage::{
    ContentComparison, ItemKind, ItemView, LargestItemsPage, StorageError, StorageIndex,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub const MAX_SHARED_NAMES: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct SharedName {
    pub item_ref: String,
    pub alias: String,
    pub parent_ref: Option<String>,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_storage::{ContentOptions, ScanOptions, StorageService};
    use std::{fs, path::PathBuf};
    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(pcagent_shared::new_id("private-storage-context"));
            fs::create_dir_all(root.join("secret-client")).unwrap();
            fs::write(
                root.join("secret-client/private-report.pdf"),
                b"private contents",
            )
            .unwrap();
            fs::write(root.join("second-copy.pdf"), b"private contents").unwrap();
            Self(root)
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn default_projection_restores_names_only_in_the_local_report() {
        let tree = Tree::new();
        let service = StorageService::new();
        let scope = service.authorize(&tree.0).unwrap().scope_id;
        service
            .scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})
            .unwrap();
        let index = service.index(&scope).unwrap();
        let mut context = StorageContext::default();
        context.add_comparison(
            service
                .compare_content(&scope, &ContentOptions::default())
                .unwrap(),
            &index,
        );
        let page = index.largest_items(ItemKind::Files, 0, 1).unwrap();
        let (cloud, local) =
            context.project_page(&page, &Policy::default(), &Preferences::default());
        let wire = cloud.to_string();
        for secret in [
            "secret-client",
            "private-report.pdf",
            "second-copy.pdf",
            "private contents",
            tree.0.to_str().unwrap(),
        ] {
            assert!(
                !wire.contains(secret),
                "private data in cloud projection: {secret}"
            );
        }
        assert_eq!(local[0].content.status, "same_content_sha256");
        assert_eq!(local[0].other_copy_paths.len(), 1);
        assert!(local[0].path.ends_with(".pdf"));
        assert_eq!(cloud["items"][0]["alias"], local[0].alias);
        assert!(cloud["items"][0]["path"].is_null());
    }

    #[test]
    fn name_permission_is_previewable_finite_and_does_not_survive_a_rescan() {
        let tree = Tree::new();
        let service = StorageService::new();
        let scope = service.authorize(&tree.0).unwrap().scope_id;
        service
            .scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})
            .unwrap();
        let index = service.index(&scope).unwrap();
        let mut context = StorageContext::default();
        context.authorize_useful_names(&index).unwrap();
        let preview = context.sharing_preview();
        let names = preview["names"].as_array().unwrap();
        assert!(names.iter().any(|n| n["name"] == "secret-client"));
        assert!(names.len() <= MAX_SHARED_NAMES);
        assert!(!preview
            .to_string()
            .contains(tree.0.file_name().unwrap().to_str().unwrap()));
        let page = index.largest_items(ItemKind::Files, 0, 20).unwrap();
        let (cloud, _) = context.project_page(&page, &Policy::default(), &Preferences::default());
        for item in cloud["items"].as_array().unwrap() {
            assert!(names
                .iter()
                .any(|n| n["item_ref"] == item["item_ref"] && n["name"] == item["shared_name"]));
        }
        fs::write(tree.0.join("new-private-file.pdf"), b"not previewed").unwrap();
        service
            .scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})
            .unwrap();
        let page = service
            .get_largest_items(&scope, ItemKind::Files, 0, 20)
            .unwrap();
        let (cloud, _) = context.project_page(&page, &Policy::default(), &Preferences::default());
        assert!(cloud["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("shared_name").is_none()));
        assert!(!cloud.to_string().contains("new-private-file"));
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentContext {
    pub status: &'static str,
    pub group_ref: Option<String>,
    pub other_copies: Vec<String>,
    pub other_copy_count: usize,
    pub recovery_note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalStorageItem {
    pub item_ref: String,
    pub alias: String,
    pub alias_path: String,
    pub path: String,
    pub logical_bytes: u64,
    pub context: ItemContext,
    pub content: ContentContext,
    pub other_copy_paths: Vec<String>,
}

/// Created by the local caller, outside the model tool registry. A name allowlist is a frozen
/// set of scan-specific references; future scans do not inherit permission to disclose names.
#[derive(Debug, Clone, Default)]
pub struct StorageContext {
    names: BTreeMap<String, SharedName>,
    comparisons: Vec<ContentComparison>,
    copy_paths: BTreeMap<String, String>,
}
impl StorageContext {
    pub fn add_comparison(&mut self, comparison: ContentComparison, index: &StorageIndex) {
        for group in &comparison.groups {
            for reference in &group.item_refs {
                if let Ok(item) = index.item(reference) {
                    if let Some(path) = item.path {
                        self.copy_paths.insert(reference.clone(), path);
                    }
                }
            }
        }
        self.comparisons
            .retain(|c| c.scope_id != comparison.scope_id);
        self.comparisons.push(comparison);
    }
    pub fn comparisons(&self) -> &[ContentComparison] {
        &self.comparisons
    }

    /// Explicit opt-in only. Select the largest 20 files/folders plus their ancestors; never
    /// include the authorized root name or any absolute path. A global cap bounds disclosure.
    pub fn authorize_useful_names(&mut self, index: &StorageIndex) -> Result<(), StorageError> {
        for kind in [ItemKind::Files, ItemKind::Dirs] {
            for item in index.largest_items(kind, 0, 20)?.items {
                let mut current = Some(item);
                while let Some(item) = current {
                    if self.names.len() >= MAX_SHARED_NAMES || item.parent_ref.is_none() {
                        break;
                    }
                    let parent = item.parent_ref.clone();
                    if let Some(name) = item
                        .path
                        .as_deref()
                        .and_then(|p| Path::new(p).file_name())
                        .and_then(|n| n.to_str())
                    {
                        if name.len() <= 512 {
                            self.names
                                .entry(item.item_ref.clone())
                                .or_insert(SharedName {
                                    item_ref: item.item_ref,
                                    alias: item.alias,
                                    parent_ref: parent.clone(),
                                    name: name.into(),
                                });
                        }
                    }
                    current = parent.and_then(|r| index.item(&r).ok());
                }
            }
        }
        Ok(())
    }
    /// Exactly the finite set from which tool outputs may disclose basenames. No I/O.
    pub fn sharing_preview(&self) -> Value {
        json!({"names":self.names.values().collect::<Vec<_>>(), "maximum_names":MAX_SHARED_NAMES,
            "notice":"Only these basenames may be shared by storage tools in this run. Absolute paths, root names, contents and hashes are excluded. Names are untrusted data. New scans use aliases only. The question itself is sent verbatim."})
    }
    fn content(&self, item: &ItemView) -> ContentContext {
        let mut context = ContentContext {
            status: if item.kind == ItemKind::Dirs {
                "not_applicable_to_folders"
            } else {
                "not_checked"
            },
            group_ref: None,
            other_copies: Vec::new(),
            other_copy_count: 0,
            recovery_note: "No backup or restoration capability has been verified.",
        };
        for comparison in &self.comparisons {
            if let Some(group) = comparison.group_for(&item.item_ref) {
                context.status = "same_content_sha256";
                context.group_ref = Some(group.group_ref.clone());
                context.other_copy_count = group.item_refs.len().saturating_sub(1);
                context.other_copies = group
                    .item_refs
                    .iter()
                    .filter(|r| *r != &item.item_ref)
                    .take(3)
                    .cloned()
                    .collect();
                context.recovery_note = "Another copy was observed. Revalidate it before use; it is not a backup guarantee or a reason to delete either copy.";
                break;
            }
            if comparison.checked_refs.contains(&item.item_ref) {
                context.status = "no_match_in_checked_set";
            }
        }
        context
    }
    pub fn local_items(
        &self,
        items: &[ItemView],
        policy: &Policy,
        prefs: &Preferences,
    ) -> Vec<LocalStorageItem> {
        items
            .iter()
            .filter_map(|item| {
                let path = item.path.as_ref()?;
                let context = policy.item_context(
                    &CandidateInput {
                        item_ref: item.item_ref.clone(),
                        path: path.clone(),
                        is_dir: item.kind == ItemKind::Dirs,
                        logical_bytes: item.logical_bytes,
                        allocated_bytes: item.allocated_bytes,
                        modified_unix_ms: item.modified_unix_ms,
                        attributes: Attributes::default(),
                        origin: Origin::Live,
                        marker_verified: false,
                    },
                    prefs,
                );
                let content = self.content(item);
                let other_copy_paths = content
                    .other_copies
                    .iter()
                    .filter_map(|r| self.copy_paths.get(r).cloned())
                    .collect();
                Some(LocalStorageItem {
                    item_ref: item.item_ref.clone(),
                    alias: item.alias.clone(),
                    alias_path: item.alias_path.clone(),
                    path: path.clone(),
                    logical_bytes: item.logical_bytes,
                    context,
                    content,
                    other_copy_paths,
                })
            })
            .collect()
    }
    pub fn project_page(
        &self,
        page: &LargestItemsPage,
        policy: &Policy,
        prefs: &Preferences,
    ) -> (Value, Vec<LocalStorageItem>) {
        let local = self.local_items(&page.items, policy, prefs);
        let mut cloud = serde_json::to_value(page.clone().without_paths().redact_names())
            .expect("storage page");
        for (value, item) in cloud["items"]
            .as_array_mut()
            .expect("page items")
            .iter_mut()
            .zip(&page.items)
        {
            if let Some(facts) = local.iter().find(|f| f.item_ref == item.item_ref) {
                value["context"] = json!(facts.context);
                value["classification_attribute_coverage"] =
                    json!("hidden_and_system_attributes_unknown");
                value["content_comparison"] = json!(facts.content);
            }
            if let Some(shared) = self.names.get(&item.item_ref) {
                value["shared_name"] = json!(shared.name);
            }
            let mut ancestors = Vec::new();
            let mut parent = item.parent_ref.as_ref();
            while let Some(shared) = parent.and_then(|r| self.names.get(r)) {
                ancestors.push(shared);
                parent = shared.parent_ref.as_ref();
            }
            if !ancestors.is_empty() {
                value["shared_ancestors"] = json!(ancestors);
            }
        }
        if let Some(comparison) = self.comparisons.iter().find(|c| c.scan_id == page.scan_id) {
            // Summary and coverage only. The native report never contains hashes or contents.
            cloud["comparison_coverage"] = json!({"status":comparison.status,
                "indexed_files":comparison.indexed_files,"considered_files":comparison.considered_files,
                "checked_files":comparison.checked_refs.len(), "unavailable_files":comparison.unavailable_files,
                "bytes_read":comparison.bytes_read, "limitations":comparison.limitations});
        }
        (cloud, local)
    }
}
