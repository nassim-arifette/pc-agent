//! In-memory index (P0) and agent-facing query views.
use crate::{Coverage, ScanScope, ScanSummary, StorageError, MAX_PAGE};
use pcagent_shared::{Origin, Status, Window, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    ffi::OsString,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

pub(crate) const F_ALIAS: u8 = 1;
pub(crate) const F_DENIED: u8 = 2;
pub(crate) const F_INCOMPLETE: u8 = 4;
pub(crate) const F_SPARSE: u8 = 8;
pub(crate) const F_COMPRESSED: u8 = 16;
pub(crate) const F_ALLOC_UNKNOWN: u8 = 32;
const ROOT_PARENT: usize = usize::MAX;
const SORT_BASIS: &str = "allocated_bytes_when_known_else_logical_bytes_then_logical_bytes";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Agg {
    logical: u64,
    allocated: u64,
    allocation_unknown: u64,
    files: u64,
    incomplete: bool,
}
impl Agg {
    fn add(&mut self, o: &Agg) -> bool {
        let sums = (
            self.logical.checked_add(o.logical),
            self.allocated.checked_add(o.allocated),
        );
        self.allocation_unknown = self.allocation_unknown.saturating_add(o.allocation_unknown);
        self.files = self.files.saturating_add(o.files);
        self.incomplete |= o.incomplete;
        match sums {
            (Some(l), Some(a)) => {
                (self.logical, self.allocated) = (l, a);
                true
            }
            _ => {
                (self.logical, self.allocated) = (u64::MAX, u64::MAX);
                false
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub parent: usize,
    pub name: OsString,
    pub dir: bool,
    pub logical: u64,
    pub allocated: u64,
    pub modified_ms: Option<u64>,
    pub links: Option<u32>,
    pub flags: u8,
    agg: Agg,
}
impl Entry {
    pub fn dir(parent: usize, name: OsString) -> Self {
        Self {
            parent,
            name,
            dir: true,
            logical: 0,
            allocated: 0,
            modified_ms: None,
            links: None,
            flags: 0,
            agg: Agg::default(),
        }
    }
    pub fn file(parent: usize, name: OsString, logical: u64) -> Self {
        Self {
            dir: false,
            logical,
            ..Self::dir(parent, name)
        }
    }
    fn own(&self) -> Agg {
        if self.dir || self.flags & F_ALIAS != 0 {
            return Agg {
                incomplete: self.flags & (F_DENIED | F_INCOMPLETE) != 0,
                ..Agg::default()
            };
        }
        let unknown = self.flags & F_ALLOC_UNKNOWN != 0;
        Agg {
            logical: self.logical,
            allocated: if unknown { 0 } else { self.allocated },
            allocation_unknown: u64::from(unknown),
            files: 1,
            incomplete: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Files,
    Dirs,
}

/// Spec 9.4: no activity source is collected, so last use is always unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LastUse {
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    pub logical_bytes: u64,
    /// Sum of known allocations only; see `allocation_unknown_files`.
    pub allocated_bytes: u64,
    pub allocation_unknown_files: u64,
    pub files: u64,
}
impl From<Agg> for Totals {
    fn from(a: Agg) -> Self {
        Self {
            logical_bytes: a.logical,
            allocated_bytes: a.allocated,
            allocation_unknown_files: a.allocation_unknown,
            files: a.files,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemView {
    pub item_ref: String,
    pub kind: ItemKind,
    /// Full local path; `None` in cloud-bound views.
    pub path: Option<String>,
    pub redacted_path: String,
    /// Files: own size. Dirs: deduplicated subtree sum.
    pub logical_bytes: u64,
    /// Files: `None` when unknown. Dirs: sum of known allocations in the subtree.
    pub allocated_bytes: Option<u64>,
    pub allocation_unknown_files: u64,
    /// Always `None` until a local rule computes it (spec 9.3).
    pub estimated_reclaim_bytes: Option<u64>,
    pub files: Option<u64>,
    pub link_count: Option<u32>,
    pub sparse: bool,
    pub compressed: bool,
    /// Modification time, not a last-use time.
    pub modified_unix_ms: Option<u64>,
    pub last_used: LastUse,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LargestItemsPage {
    pub schema_version: u32,
    pub scope_id: String,
    pub scan_id: String,
    pub kind: ItemKind,
    pub sort_basis: String,
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub next_offset: Option<usize>,
    pub items: Vec<ItemView>,
    pub scan_status: Status,
    pub limitations: Vec<String>,
}
impl LargestItemsPage {
    pub fn without_paths(mut self) -> Self {
        self.items.iter_mut().for_each(|i| i.path = None);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSpace {
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub occupied_bytes: Option<u64>,
    pub filesystem: Option<String>,
    pub source: String,
    pub status: Status,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageOverview {
    pub schema_version: u32,
    pub origin: Origin,
    pub scope_id: String,
    pub redacted_root: String,
    pub scan_running: bool,
    pub scan: Option<ScanSummary>,
    pub window: Option<Window>,
    pub volume: VolumeSpace,
    pub scanned: Option<Totals>,
    /// Occupied volume bytes minus allocation attributed by this scan: includes everything
    /// outside the scope, skipped/denied areas and file-system metadata. Never fully explained.
    pub unattributed_bytes: Option<u64>,
    pub estimated_reclaim_bytes: Option<u64>,
    pub denied: Vec<ItemView>,
    pub denied_list_truncated: bool,
    pub status: Status,
    pub limitations: Vec<String>,
}
impl StorageOverview {
    pub(crate) fn build(
        scope: &ScanScope,
        index: Option<&StorageIndex>,
        volume: VolumeSpace,
        scan_running: bool,
    ) -> Self {
        let mut limitations = vec![
            "Modification times are not last-use times; last use is unknown.".into(),
            "Estimated reclaimable space is unknown until local cleanup rules evaluate items."
                .into(),
        ];
        let Some(ix) = index else {
            limitations.push("No completed scan for this scope.".into());
            return Self {
                schema_version: SCHEMA_VERSION,
                origin: Origin::Live,
                scope_id: scope.id().into(),
                redacted_root: redact_root(scope.root(), scope.profile.as_deref()),
                scan_running,
                scan: None,
                window: None,
                volume,
                scanned: None,
                unattributed_bytes: None,
                estimated_reclaim_bytes: None,
                denied: Vec::new(),
                denied_list_truncated: false,
                status: Status::Partial,
                limitations,
            };
        };
        let scanned = ix.totals();
        let unattributed = volume
            .occupied_bytes
            .and_then(|o| o.checked_sub(scanned.allocated_bytes));
        if volume.occupied_bytes.is_some() && unattributed.is_none() {
            limitations.push(
                "Attributed allocation exceeds occupied volume space (volume changed since scan?)."
                    .into(),
            );
        }
        if scanned.allocation_unknown_files > 0 {
            limitations.push(
                "Some files have unknown allocation; attributed bytes are a lower bound.".into(),
            );
        }
        limitations.extend(ix.limitations());
        let status = if volume.status != Status::Ok && ix.status == Status::Ok {
            Status::Partial
        } else {
            ix.status
        };
        Self {
            schema_version: SCHEMA_VERSION,
            origin: Origin::Live,
            scope_id: ix.scope_id.clone(),
            redacted_root: redact_root(&ix.root, ix.profile.as_deref()),
            scan_running,
            scan: Some(ix.summary()),
            window: Some(ix.window.clone()),
            volume,
            scanned: Some(scanned),
            unattributed_bytes: unattributed,
            estimated_reclaim_bytes: None,
            denied: ix.denied.iter().map(|&i| ix.view(i)).collect(),
            denied_list_truncated: ix.denied_truncated,
            status,
            limitations,
        }
    }
    pub fn without_paths(mut self) -> Self {
        self.denied.iter_mut().for_each(|i| i.path = None);
        self
    }
}

/// Completed (possibly partial) scan result. Item refs are only valid for this scan.
#[derive(Debug)]
pub struct StorageIndex {
    pub(crate) scan_id: String,
    pub(crate) scope_id: String,
    pub(crate) root: PathBuf,
    pub(crate) profile: Option<PathBuf>,
    pub(crate) backend: &'static str,
    pub(crate) window: Window,
    pub(crate) entries: Vec<Entry>,
    pub(crate) coverage: Coverage,
    pub(crate) denied: Vec<usize>,
    pub(crate) denied_truncated: bool,
    pub(crate) root_error: Option<ErrorKind>,
    files_rank: Vec<usize>,
    dirs_rank: Vec<usize>,
    pub(crate) status: Status,
}

impl StorageIndex {
    pub(crate) fn new(scope: &ScanScope, backend: &'static str, started_ms: u64) -> Self {
        Self {
            scan_id: pcagent_shared::new_id("scan"),
            scope_id: scope.id().into(),
            root: scope.root().to_path_buf(),
            profile: scope.profile.clone(),
            backend,
            window: Window {
                started_at_unix_ms: started_ms,
                finished_at_unix_ms: started_ms,
                duration_ms: 0,
            },
            entries: Vec::new(),
            coverage: Coverage::default(),
            denied: Vec::new(),
            denied_truncated: false,
            root_error: None,
            files_rank: Vec::new(),
            dirs_rank: Vec::new(),
            status: Status::Ok,
        }
    }

    /// Bottom-up aggregation: pre-order indices guarantee children follow parents.
    pub(crate) fn finalize(&mut self, duration_ms: u64) {
        for i in 0..self.entries.len() {
            self.entries[i].agg = self.entries[i].own();
        }
        let mut overflow = false;
        for i in (1..self.entries.len()).rev() {
            let (parent, agg) = (self.entries[i].parent, self.entries[i].agg);
            overflow |= !self.entries[parent].agg.add(&agg);
        }
        self.coverage.allocation_unknown_files =
            self.entries.first().map_or(0, |r| r.agg.allocation_unknown);
        if overflow {
            self.coverage.errors += 1;
        }
        let key = |e: &Entry| {
            if e.dir {
                (e.agg.allocated, e.agg.logical)
            } else if e.flags & F_ALLOC_UNKNOWN != 0 {
                (e.logical, e.logical)
            } else {
                (e.allocated, e.logical)
            }
        };
        let (mut files, mut dirs): (Vec<usize>, Vec<usize>) = (1..self.entries.len())
            .filter(|&i| self.entries[i].flags & F_ALIAS == 0)
            .partition(|&i| !self.entries[i].dir);
        files.sort_by_key(|&i| (std::cmp::Reverse(key(&self.entries[i])), i));
        dirs.sort_by_key(|&i| (std::cmp::Reverse(key(&self.entries[i])), i));
        (self.files_rank, self.dirs_rank) = (files, dirs);
        self.window.duration_ms = duration_ms;
        self.window.finished_at_unix_ms = pcagent_shared::unix_ms();
        let c = &self.coverage;
        self.status = if c.cancelled {
            Status::Cancelled
        } else if self.root_error == Some(ErrorKind::PermissionDenied) {
            Status::PermissionDenied
        } else if self.root_error.is_some() {
            Status::Error
        } else if c.truncated.is_some()
            || c.denied_dirs + c.denied_files + c.vanished + c.changed_during_scan + c.errors > 0
            || self.entries.first().is_some_and(|r| r.agg.incomplete)
        {
            Status::Partial
        } else {
            Status::Ok
        };
    }

    pub fn scan_id(&self) -> &str {
        &self.scan_id
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }
    pub fn summary(&self) -> ScanSummary {
        ScanSummary {
            scope_id: self.scope_id.clone(),
            scan_id: self.scan_id.clone(),
            backend: self.backend.into(),
            status: self.status,
            coverage: self.coverage.clone(),
            duration_ms: self.window.duration_ms,
        }
    }
    /// Deduplicated totals of the whole scope (hard-link aliases counted once).
    pub fn totals(&self) -> Totals {
        self.entries
            .first()
            .map(|r| r.agg.into())
            .unwrap_or_default()
    }

    pub(crate) fn limitations(&self) -> Vec<String> {
        let c = &self.coverage;
        let mut out = vec![
            "Links, junctions and other reparse points are not followed; their targets are not attributed.".to_string(),
            "Directory metadata allocation is not attributed.".into(),
        ];
        if c.placeholders > 0 {
            out.push(
                "Cloud placeholders were not opened; their remote size is not local allocation."
                    .into(),
            );
        }
        if c.hard_link_aliases > 0 {
            out.push("Hard-linked files are attributed once, to the first path seen.".into());
        }
        if c.changed_during_scan + c.vanished > 0 {
            out.push("Files changed or disappeared during the scan.".into());
        }
        if let Some(t) = c.truncated {
            out.push(format!(
                "Scan stopped early ({t:?}); totals are a lower bound."
            ));
        }
        if c.cancelled {
            out.push("Scan was cancelled; totals are a lower bound.".into());
        }
        out
    }

    pub(crate) fn path(&self, mut i: usize) -> PathBuf {
        let mut names = Vec::new();
        while i != 0 && i < self.entries.len() {
            names.push(&self.entries[i].name);
            i = self.entries[i].parent;
        }
        let mut path = self.root.clone();
        path.extend(names.into_iter().rev());
        path
    }
    pub fn item_ref(&self, i: usize) -> String {
        format!("{}:{i}", self.scan_id)
    }
    /// Resolves an untrusted reference to an index entry of this scan.
    pub fn resolve(&self, item_ref: &str) -> Option<usize> {
        let (scan, idx) = item_ref.rsplit_once(':')?;
        if scan != self.scan_id || idx.is_empty() || !idx.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        idx.parse().ok().filter(|&i| i < self.entries.len())
    }
    /// Local-only full path of a referenced item.
    pub fn local_path(&self, item_ref: &str) -> Option<PathBuf> {
        self.resolve(item_ref).map(|i| self.path(i))
    }
    pub fn item(&self, item_ref: &str) -> Result<ItemView, StorageError> {
        self.resolve(item_ref)
            .map(|i| self.view(i))
            .ok_or(StorageError::UnknownItem)
    }

    pub(crate) fn view(&self, i: usize) -> ItemView {
        let e = &self.entries[i];
        let path = self.path(i);
        let unknown = e.flags & F_ALLOC_UNKNOWN != 0;
        let status = if e.flags & F_DENIED != 0 {
            Status::PermissionDenied
        } else if e.agg.incomplete || (!e.dir && unknown) {
            Status::Partial
        } else {
            Status::Ok
        };
        ItemView {
            item_ref: self.item_ref(i),
            kind: if e.dir {
                ItemKind::Dirs
            } else {
                ItemKind::Files
            },
            redacted_path: redact_path(&self.root, &path, self.profile.as_deref()),
            path: Some(display(&path)),
            logical_bytes: if e.dir { e.agg.logical } else { e.logical },
            allocated_bytes: match (e.dir, unknown) {
                (true, _) => Some(e.agg.allocated),
                (false, true) => None,
                (false, false) => Some(e.allocated),
            },
            allocation_unknown_files: e.agg.allocation_unknown,
            estimated_reclaim_bytes: None,
            files: e.dir.then_some(e.agg.files),
            link_count: e.links,
            sparse: e.flags & F_SPARSE != 0,
            compressed: e.flags & F_COMPRESSED != 0,
            modified_unix_ms: e.modified_ms,
            last_used: LastUse::Unknown,
            status,
        }
    }

    pub fn largest_items(
        &self,
        kind: ItemKind,
        offset: usize,
        limit: usize,
    ) -> Result<LargestItemsPage, StorageError> {
        if !(1..=MAX_PAGE).contains(&limit) {
            return Err(StorageError::InvalidPage);
        }
        let rank = match kind {
            ItemKind::Files => &self.files_rank,
            ItemKind::Dirs => &self.dirs_rank,
        };
        let items: Vec<ItemView> = rank
            .iter()
            .skip(offset)
            .take(limit)
            .map(|&i| self.view(i))
            .collect();
        let end = offset.saturating_add(items.len());
        let mut limitations = self.limitations();
        if kind == ItemKind::Dirs {
            limitations.push(
                "Directory sizes include their children; do not add a parent and its child.".into(),
            );
        }
        Ok(LargestItemsPage {
            schema_version: SCHEMA_VERSION,
            scope_id: self.scope_id.clone(),
            scan_id: self.scan_id.clone(),
            kind,
            sort_basis: SORT_BASIS.into(),
            offset,
            limit,
            total: rank.len(),
            next_offset: (end < rank.len()).then_some(end),
            items,
            scan_status: self.status,
            limitations,
        })
    }

    /// Totals of a set of items with parent/child overlap removed (an item whose
    /// ancestor is also selected is ignored). Unknown refs are rejected.
    pub fn deduplicated_totals(&self, item_refs: &[&str]) -> Result<Totals, StorageError> {
        let selected: HashSet<usize> = item_refs
            .iter()
            .map(|r| self.resolve(r).ok_or(StorageError::UnknownItem))
            .collect::<Result<_, _>>()?;
        let mut total = Agg::default();
        for &i in &selected {
            let mut p = self.entries[i].parent;
            let mut covered = false;
            while p != ROOT_PARENT {
                if selected.contains(&p) {
                    covered = true;
                    break;
                }
                p = self.entries[p].parent;
            }
            if !covered {
                total.add(&self.entries[i].agg);
            }
        }
        Ok(total.into())
    }
}

/// Removes any path that has an ancestor (component-wise) in the same set; keeps input order.
pub fn dedup_parent_child(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let covered = paths
            .iter()
            .enumerate()
            .any(|(j, q)| j != i && p.starts_with(q) && (p != q || j < i));
        if !covered {
            out.push(p.clone());
        }
    }
    out
}

pub(crate) fn display(path: &Path) -> String {
    let s = path.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => rest.to_string(),
        _ => s.into_owned(),
    }
}

fn user_name(profile: Option<&Path>) -> Option<&std::ffi::OsStr> {
    profile.and_then(Path::file_name)
}

fn join_redacted(prefix: &str, rest: &Path, user: Option<&std::ffi::OsStr>) -> String {
    let mut out = prefix.to_string();
    for c in rest.components() {
        if let Component::Normal(name) = c {
            out.push('/');
            if Some(name) == user {
                out.push_str("<user>");
            } else {
                out.push_str(&name.to_string_lossy());
            }
        }
    }
    out
}

/// Path relative to the authorized root, e.g. `<scope>/Games/big.bin`; any component equal to
/// the user profile name becomes `<user>`. Paths outside the root become `<outside-scope>`.
pub fn redact_path(root: &Path, path: &Path, profile: Option<&Path>) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => join_redacted("<scope>", rel, user_name(profile)),
        Err(_) => "<outside-scope>".into(),
    }
}

pub(crate) fn redact_root(root: &Path, profile: Option<&Path>) -> String {
    let profile_verbatim = profile.and_then(|p| std::fs::canonicalize(p).ok());
    for p in [profile.map(Path::to_path_buf), profile_verbatim]
        .into_iter()
        .flatten()
    {
        if let Ok(rel) = root.strip_prefix(&p) {
            return join_redacted("<profile>", rel, user_name(profile));
        }
    }
    join_redacted("<volume>", root, user_name(profile))
}
