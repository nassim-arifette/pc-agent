//! Local hierarchy and scan-time facts. No filesystem reads or caller-provided paths.
use crate::index::{F_ALIAS, F_ALLOC_UNKNOWN};
use crate::{CancelToken, ItemKind, ItemView, LargestItemsPage, StorageError, StorageIndex};
use pcagent_shared::{Status, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

const PAGE: usize = 50;
const OLD_MS: u64 = 182 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    DiskImages,
    VirtualMachines,
    Videos,
    Pictures,
    Music,
    Archives,
    ProgramsInstallers,
    Documents,
    MlModels,
    LogsDumps,
    Other,
    NoExtension,
}

impl FileType {
    pub fn of(path: &Path) -> Self {
        let Some(extension) = path.extension().filter(|e| !e.is_empty()) else {
            return Self::NoExtension;
        };
        match extension.to_string_lossy().to_ascii_lowercase().as_str() {
            "iso" | "img" | "dmg" => Self::DiskImages,
            "vhd" | "vhdx" | "vmdk" | "qcow" | "qcow2" | "vdi" | "ova" | "ovf" => {
                Self::VirtualMachines
            }
            "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "m4v" | "mpeg" => Self::Videos,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "tif" | "tiff" | "bmp" | "svg"
            | "raw" => Self::Pictures,
            "mp3" | "flac" | "wav" | "aac" | "ogg" | "opus" | "m4a" | "wma" => Self::Music,
            "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "bz2" | "zst" => Self::Archives,
            "exe" | "msi" | "msix" | "appx" | "dll" | "sys" | "deb" | "rpm" | "apk" => {
                Self::ProgramsInstallers
            }
            "pdf" | "txt" | "md" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt"
            | "ods" | "csv" | "rtf" | "epub" => Self::Documents,
            "gguf" | "ggml" | "safetensors" | "onnx" | "pt" | "pth" | "ckpt" => Self::MlModels,
            "log" | "dmp" | "dump" | "etl" => Self::LogsDumps,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasuredFiles {
    pub files_count: u64,
    pub logical_bytes: u64,
    /// Unknown if any file's allocation could not be read; never replaced by zero.
    pub allocated_bytes: Option<u64>,
    pub allocation_unknown_files_count: u64,
}

impl Default for MeasuredFiles {
    fn default() -> Self {
        Self {
            files_count: 0,
            logical_bytes: 0,
            allocated_bytes: Some(0),
            allocation_unknown_files_count: 0,
        }
    }
}

impl MeasuredFiles {
    fn add(&mut self, logical: u64, allocated: Option<u64>) {
        self.files_count += 1;
        self.logical_bytes = self.logical_bytes.saturating_add(logical);
        self.allocated_bytes = self
            .allocated_bytes
            .zip(allocated)
            .and_then(|(a, b)| a.checked_add(b));
        self.allocation_unknown_files_count += u64::from(allocated.is_none());
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTypeTotal {
    pub file_type: FileType,
    #[serde(flatten)]
    pub totals: MeasuredFiles,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemFacts {
    pub scanned_at_unix_ms: u64,
    pub untouched_before_unix_ms: u64,
    pub files_count: u64,
    pub untouched: MeasuredFiles,
    pub undated_files_count: u64,
    pub largest_file: Option<ItemView>,
    pub file_types: Vec<FileTypeTotal>,
    pub status: Status,
    pub limitations: Vec<String>,
}

impl ItemFacts {
    /// The largest file never participates in the finite file-name consent list.
    pub fn without_names(&self) -> Self {
        let mut facts = self.clone();
        if let Some(file) = &mut facts.largest_file {
            file.path = None;
            file.redacted_path = file.alias_path.clone();
            file.item_ref = file.alias.clone();
            file.parent_ref = file
                .parent_ref
                .as_ref()
                .and_then(|r| r.rsplit_once(':').map(|(_, i)| format!("dir-{i}")));
        }
        facts
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageItemEvidence {
    pub item_facts: ItemFacts,
    pub parent_ref: String,
    pub parent_alias: String,
    pub rank_scope: String,
    pub volume_total_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildrenPage {
    #[serde(flatten)]
    pub page: LargestItemsPage,
    pub parent: ItemView,
    /// Remainder of this page, not free disk space. A UI combining pages recomputes it.
    pub other_bytes: u64,
    pub parent_partial: bool,
}

impl StorageIndex {
    pub(crate) fn validate_fact_item(&self, item_ref: &str) -> Result<usize, StorageError> {
        self.resolve(item_ref)
            .filter(|&i| i != 0 && self.entries[i].flags & F_ALIAS == 0)
            .ok_or(StorageError::UnknownItem)
    }

    fn children_rank(&self, parent: usize, kind: ItemKind) -> Vec<usize> {
        let mut rank = Vec::new();
        let mut next = self.entries[parent].first_child;
        while let Some(i) = next {
            let e = &self.entries[i];
            if e.flags & F_ALIAS == 0 && e.dir == (kind == ItemKind::Dirs) {
                rank.push(i);
            }
            next = e.next_sibling;
        }
        rank.sort_unstable_by_key(|&i| (std::cmp::Reverse(self.entries[i].size_key()), i));
        rank
    }

    pub fn children(
        &self,
        item_ref: Option<&str>,
        kind: ItemKind,
        offset: usize,
    ) -> Result<ChildrenPage, StorageError> {
        let parent = match item_ref {
            Some(r) => self.resolve(r).ok_or(StorageError::UnknownItem)?,
            None if !self.entries.is_empty() => 0,
            None => return Err(StorageError::UnknownItem),
        };
        if !self.entries[parent].dir {
            return Err(StorageError::NotADirectory);
        }
        let rank = self.children_rank(parent, kind);
        let items: Vec<_> = rank
            .iter()
            .skip(offset)
            .take(PAGE)
            .map(|&i| self.view(i))
            .collect();
        let shown = items.iter().fold(0u64, |total, item| {
            total.saturating_add(item.allocated_bytes.unwrap_or(item.logical_bytes))
        });
        let parent = self.view(parent);
        let size = parent.allocated_bytes.unwrap_or(parent.logical_bytes);
        let end = offset.saturating_add(items.len());
        Ok(ChildrenPage {
            other_bytes: size.saturating_sub(shown),
            parent_partial: shown > size
                || parent.allocation_unknown_files > 0
                || parent.status != Status::Ok
                || self.status != Status::Ok,
            parent,
            page: LargestItemsPage {
                schema_version: SCHEMA_VERSION,
                scope_id: self.scope_id.clone(),
                scan_id: self.scan_id.clone(),
                kind,
                sort_basis: "allocated_bytes_when_known_else_logical_bytes_then_logical_bytes"
                    .into(),
                offset,
                limit: PAGE,
                total: rank.len(),
                next_offset: (end < rank.len()).then_some(end),
                items,
                scan_status: self.status,
                limitations: self.limitations(),
            },
        })
    }

    pub fn item_page(&self, item_ref: &str) -> Result<(LargestItemsPage, ItemView), StorageError> {
        let i = self.validate_fact_item(item_ref)?;
        let parent = self.entries[i].parent;
        let kind = if self.entries[i].dir {
            ItemKind::Dirs
        } else {
            ItemKind::Files
        };
        let rank = self.children_rank(parent, kind);
        let offset = rank
            .iter()
            .position(|&r| r == i)
            .ok_or(StorageError::UnknownItem)?;
        let mut children = self.children(Some(&self.item_ref(parent)), kind, offset)?;
        children.page.items.truncate(1);
        children.page.limit = 1;
        children.page.next_offset = (offset + 1 < rank.len()).then_some(offset + 1);
        Ok((children.page, children.parent))
    }

    pub fn item_facts(
        &self,
        item_ref: &str,
        cancel: &CancelToken,
    ) -> Result<Arc<ItemFacts>, StorageError> {
        let root = self.validate_fact_item(item_ref)?;
        if cancel.is_cancelled() {
            return Err(StorageError::FactsCancelled);
        }
        if let Some(facts) = self
            .facts_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&root)
        {
            return Ok(facts.clone());
        }
        let mut facts = ItemFacts {
            scanned_at_unix_ms: self.window.finished_at_unix_ms,
            untouched_before_unix_ms: self.window.finished_at_unix_ms.saturating_sub(OLD_MS),
            files_count: 0,
            untouched: MeasuredFiles::default(),
            undated_files_count: 0,
            largest_file: None,
            file_types: vec![],
            status: self.view(root).status,
            limitations: self.limitations(),
        };
        if self.status != Status::Ok || self.view(root).allocation_unknown_files > 0 {
            facts.status = Status::Partial;
        }
        facts.limitations.push("Modification is not last use. These facts describe the scan, not the present. Missing dates are excluded from untouched totals.".into());
        let mut types = BTreeMap::<FileType, MeasuredFiles>::new();
        let mut largest = None::<usize>;
        let mut cursor = root;
        loop {
            if cancel.is_cancelled() {
                return Err(StorageError::FactsCancelled);
            }
            let entry = &self.entries[cursor];
            if !entry.dir && entry.flags & F_ALIAS == 0 {
                facts.files_count += 1;
                let allocated = (entry.flags & F_ALLOC_UNKNOWN == 0).then_some(entry.allocated);
                types
                    .entry(FileType::of(Path::new(&entry.name)))
                    .or_default()
                    .add(entry.logical, allocated);
                match entry.modified_ms {
                    Some(at) if at <= facts.untouched_before_unix_ms => {
                        facts.untouched.add(entry.logical, allocated)
                    }
                    None => facts.undated_files_count += 1,
                    _ => {}
                }
                if largest.is_none_or(|i| {
                    (entry.size_key(), std::cmp::Reverse(cursor))
                        > (self.entries[i].size_key(), std::cmp::Reverse(i))
                }) {
                    largest = Some(cursor);
                }
            }
            if let Some(child) = entry.first_child {
                cursor = child;
                continue;
            }
            while cursor != root && self.entries[cursor].next_sibling.is_none() {
                cursor = self.entries[cursor].parent;
            }
            if cursor == root {
                break;
            }
            cursor = self.entries[cursor].next_sibling.expect("sibling checked");
        }
        facts.largest_file = largest.map(|i| self.view(i));
        facts.file_types = types
            .into_iter()
            .map(|(file_type, totals)| FileTypeTotal { file_type, totals })
            .collect();
        let facts = Arc::new(facts);
        cancel.complete(|| {
            self.facts_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(root, facts.clone());
            facts
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Entry, F_INCOMPLETE};
    use crate::{ScanScope, StorageService};

    fn fixture() -> StorageIndex {
        let scope = ScanScope::authorize(&std::env::temp_dir()).unwrap();
        let mut index = StorageIndex::new(&scope, "synthetic", 1);
        index.entries = vec![
            Entry::dir(usize::MAX, "".into()),
            Entry::dir(0, "private".into()),
            Entry::file(0, "root.iso".into(), 100),
            Entry::dir(1, "nested".into()),
            Entry::file(1, "archive.ZIP".into(), 200),
            Entry::file(3, "report.pdf".into(), 300),
            Entry::file(3, "alias.pdf".into(), 300),
            Entry::file(1, "README".into(), 10),
        ];
        for entry in &mut index.entries {
            entry.allocated = entry.logical;
            entry.modified_ms = Some(1);
        }
        index.entries[6].flags = F_ALIAS;
        index.entries[7].modified_ms = None;
        index.finalize(10);
        index.window.finished_at_unix_ms = OLD_MS + 1;
        index
    }

    #[test]
    fn hierarchy_uses_direct_children_and_deduplicated_subtrees() {
        let index = fixture();
        assert_eq!(index.entries[0].first_child, Some(1));
        assert_eq!(index.entries[1].next_sibling, Some(2));
        assert_eq!(index.entries[1].first_child, Some(3));
        assert_eq!(index.entries[3].next_sibling, Some(4));
        let root = index.children(None, ItemKind::Dirs, 0).unwrap();
        assert_eq!(root.page.items.len(), 1);
        assert_eq!(root.page.items[0].logical_bytes, 510);
        assert_eq!(root.other_bytes, 100);
        let nested = index
            .children(Some(&index.item_ref(3)), ItemKind::Files, 0)
            .unwrap();
        assert_eq!(nested.page.items.len(), 1);
        assert_eq!(nested.page.items[0].logical_bytes, 300);
        assert_eq!(nested.other_bytes, 0);
        assert!(index
            .children(Some(&index.item_ref(4)), ItemKind::Dirs, 0)
            .is_err());
        assert!(index
            .children(Some("other-scan:1"), ItemKind::Dirs, 0)
            .is_err());
    }

    #[test]
    fn child_pages_and_selected_rank_have_no_total_item_cap() {
        let mut index = fixture();
        for i in 0..123 {
            let mut entry = Entry::file(1, format!("file-{i}.txt").into(), i + 1000);
            entry.allocated = entry.logical;
            index.entries.push(entry);
        }
        index.finalize(10);
        let parent = index.item_ref(1);
        let first = index.children(Some(&parent), ItemKind::Files, 0).unwrap();
        assert_eq!(first.page.total, 125);
        assert_eq!(first.page.next_offset, Some(50));
        assert_eq!(first.page.items.len(), 50);
        let last = index.children(Some(&parent), ItemKind::Files, 100).unwrap();
        assert_eq!(last.page.items.len(), 25);
        assert_eq!(last.page.next_offset, None);
        let (selected, p) = index.item_page(&last.page.items[3].item_ref).unwrap();
        assert_eq!(selected.offset, 103);
        assert_eq!(selected.total, 125);
        assert_eq!(selected.items.len(), 1);
        assert_eq!(p.item_ref, parent);
        let empty = index
            .children(Some(&parent), ItemKind::Files, usize::MAX)
            .unwrap();
        assert!(empty.page.items.is_empty());
        assert_eq!(empty.page.next_offset, None);
    }

    #[test]
    fn facts_use_scan_date_exclude_aliases_and_cache_per_item() {
        let index = fixture();
        let facts = index
            .item_facts(&index.item_ref(1), &CancelToken::default())
            .unwrap();
        assert_eq!(facts.files_count, 3);
        assert_eq!(facts.untouched.files_count, 2);
        assert_eq!(facts.untouched.logical_bytes, 500);
        assert_eq!(facts.untouched.allocated_bytes, Some(500));
        assert_eq!(facts.undated_files_count, 1);
        assert_eq!(
            facts.largest_file.as_ref().unwrap().item_ref,
            index.item_ref(5)
        );
        assert_eq!(facts.file_types.len(), 3);
        assert!(Arc::ptr_eq(
            &facts,
            &index
                .item_facts(&index.item_ref(1), &CancelToken::default())
                .unwrap()
        ));
        let file = index
            .item_facts(&index.item_ref(7), &CancelToken::default())
            .unwrap();
        assert_eq!(file.files_count, 1);
        assert_eq!(file.undated_files_count, 1);
        assert_eq!(file.file_types[0].file_type, FileType::NoExtension);
        assert!(index
            .item_facts(&index.item_ref(0), &CancelToken::default())
            .is_err());
        assert!(index
            .item_facts(&index.item_ref(6), &CancelToken::default())
            .is_err());
    }

    #[test]
    fn unknown_allocations_stay_unknown_and_overlarge_children_mark_partial() {
        let mut index = fixture();
        index.entries[4].flags = F_ALLOC_UNKNOWN;
        index.entries[3].flags = F_INCOMPLETE;
        index.finalize(10);
        let page = index
            .children(Some(&index.item_ref(1)), ItemKind::Files, 0)
            .unwrap();
        assert!(page.parent_partial);
        let facts = index
            .item_facts(&index.item_ref(1), &CancelToken::default())
            .unwrap();
        assert_eq!(facts.status, Status::Partial);
        assert_eq!(facts.untouched.allocated_bytes, None);
        let zip = facts
            .file_types
            .iter()
            .find(|f| f.file_type == FileType::Archives)
            .unwrap();
        assert_eq!(zip.totals.allocated_bytes, None);
        assert_eq!(zip.totals.allocation_unknown_files_count, 1);
        index.entries[4].logical = 10_000;
        index.finalize(10);
        let page = index
            .children(Some(&index.item_ref(1)), ItemKind::Files, 0)
            .unwrap();
        assert_eq!(page.other_bytes, 0);
        assert!(page.parent_partial);
    }

    #[test]
    fn cancelled_facts_never_enter_cache_and_new_requests_cancel_old_jobs() {
        let index = fixture();
        let token = CancelToken::default();
        token.cancel();
        assert!(matches!(
            index.item_facts(&index.item_ref(1), &token),
            Err(StorageError::FactsCancelled)
        ));
        assert!(index.facts_cache.lock().unwrap().is_empty());

        let service = StorageService::new();
        let scope = service.authorize(&std::env::temp_dir()).unwrap();
        let first_ref = index.item_ref(1);
        let second_ref = index.item_ref(3);
        service.lock().get_mut(&scope.scope_id).unwrap().index = Some(Arc::new(index));
        let first = service
            .prepare_item_facts(&scope.scope_id, &first_ref)
            .unwrap();
        let second = service
            .prepare_item_facts(&scope.scope_id, &second_ref)
            .unwrap();
        assert!(matches!(first.run(), Err(StorageError::FactsCancelled)));
        assert_eq!(second.run().unwrap().files_count, 1);
        let index = service.index(&scope.scope_id).unwrap();
        assert!(!index.facts_cache.lock().unwrap().contains_key(&1));
        assert!(index.facts_cache.lock().unwrap().contains_key(&3));
        let job = service
            .prepare_item_facts(&scope.scope_id, &first_ref)
            .unwrap();
        assert!(service
            .prepare_item_facts(&scope.scope_id, "invalid")
            .is_err());
        assert!(job.run().is_ok());
        let job = service
            .prepare_item_facts(&scope.scope_id, &first_ref)
            .unwrap();
        service.revoke(&scope.scope_id);
        assert!(matches!(job.run(), Err(StorageError::FactsCancelled)));
    }

    #[test]
    fn extension_groups_are_closed_and_projection_never_leaks_largest_file_names() {
        for (name, expected) in [
            ("a.ISO", FileType::DiskImages),
            ("b.vhdx", FileType::VirtualMachines),
            ("c.mkv", FileType::Videos),
            ("d.jpeg", FileType::Pictures),
            ("e.flac", FileType::Music),
            ("f.7z", FileType::Archives),
            ("g.msi", FileType::ProgramsInstallers),
            ("h.pdf", FileType::Documents),
            ("i.gguf", FileType::MlModels),
            ("j.dmp", FileType::LogsDumps),
            ("k.unknown", FileType::Other),
            ("README", FileType::NoExtension),
        ] {
            assert_eq!(FileType::of(Path::new(name)), expected);
        }
        let index = fixture();
        let facts = index
            .item_facts(&index.item_ref(1), &CancelToken::default())
            .unwrap();
        let projected = facts.without_names();
        let json = serde_json::to_string(&projected).unwrap();
        for private in ["private", "nested", "report.pdf", &index.scan_id] {
            assert!(!json.contains(private), "{private}");
        }
        assert_eq!(
            projected.largest_file.unwrap().parent_ref.as_deref(),
            Some("dir-3")
        );
    }
}
