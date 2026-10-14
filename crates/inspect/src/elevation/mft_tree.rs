//! Bounded local filename/parent index. Paths are private helper data, never cloud evidence.
//!
//! The index keeps fixed-size nodes in chunked arrays and every stored filename once, as UTF-8,
//! in a chunked arena: nothing is allocated per file. A file node takes 40 bytes plus its name,
//! a directory node 24 bytes plus its name, and both reserve the working memory their share of
//! `finish` needs, so the accounted total bounds the index for its whole lifetime.
//! `tests::memory_*` compare this accounting with the real heap.
use super::memory::{Arena, Chunked};
use super::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fmt;
use std::mem::size_of;
#[cfg(test)]
use std::time::Duration;

/// One node per examined record at most.
pub const MAX_TREE_NODES: usize = collection::MAX_NATIVE_RECORDS;
/// Index share of the collector's bound (`MAX_RETAINED_BYTES`), finish working memory included.
pub const MAX_TREE_RETAINED_BYTES: usize = 192 * 1024 * 1024;
pub const MAX_TREE_ITEMS: usize = 16;
pub const MAX_TREE_PATH_BYTES: usize = 512;
pub const MAX_TREE_RESPONSE_BYTES: usize = 48 * 1024;
const MAX_LINKS: usize = 16;
const MAX_DEPTH: usize = 128;
const FILE_NAME: u32 = 0x30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeLimit {
    IndexMemoryLimit,
    IndexNodeLimit,
    NameLinksTruncated,
    InvalidFileName,
    MissingFileName,
    ParentMissing,
    ParentSequenceChanged,
    ParentMetadataIncomplete,
    ParentCycle,
    PathConflict,
    PathLimit,
    DepthLimit,
    FilenameMembershipUnresolved,
    CollectionIncomplete,
    Interrupted,
    AllocationNotAdditive,
    TopDirectoriesTruncated,
    TopFilesTruncated,
    /// Some returned items start below a folder that was not read (`TreeItem::partial_path`).
    PartialPaths,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeItem {
    /// Volume-relative, local-only metadata path. The host must project an opaque reference.
    pub path: String,
    pub files: u64,
    pub logical_bytes_known: u64,
    pub data_cluster_bytes_known: Option<u64>,
    pub unknown_logical_files: u64,
    pub unknown_allocation_files: u64,
    pub hard_linked_files: u64,
    /// The folders above `path` were not read: the path starts below an unknown folder and the
    /// amounts belong to the unattributed share. Never a complete location or total.
    #[serde(default)]
    pub partial_path: bool,
}
impl fmt::Debug for TreeItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TreeItem")
            .field("path", &"<local-metadata-path>")
            .field("files", &self.files)
            .field("logical_bytes_known", &self.logical_bytes_known)
            .field("data_cluster_bytes_known", &self.data_cluster_bytes_known)
            .field("unknown_logical_files", &self.unknown_logical_files)
            .field("unknown_allocation_files", &self.unknown_allocation_files)
            .field("hard_linked_files", &self.hard_linked_files)
            .field("partial_path", &self.partial_path)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSummary {
    /// Largest subtrees; a parent and its child overlap and must not be summed.
    pub directories: Vec<TreeItem>,
    /// Each file contributes at most once, regardless of its filename aliases.
    pub files: Vec<TreeItem>,
    pub indexed_nodes: u64,
    pub indexed_name_links: u64,
    pub retained_bytes: u64,
    pub attributed_files: u64,
    pub unattributed_files: u64,
    pub attributed_logical_bytes_known: u64,
    pub unattributed_logical_bytes_known: u64,
    pub attributed_data_cluster_bytes_known: Option<u64>,
    pub unattributed_data_cluster_bytes_known: Option<u64>,
    pub limitations: BTreeSet<TreeLimit>,
}
impl Default for TreeSummary {
    fn default() -> Self {
        Self {
            directories: Vec::new(),
            files: Vec::new(),
            indexed_nodes: 0,
            indexed_name_links: 0,
            retained_bytes: 0,
            attributed_files: 0,
            unattributed_files: 0,
            attributed_logical_bytes_known: 0,
            unattributed_logical_bytes_known: 0,
            attributed_data_cluster_bytes_known: Some(0),
            unattributed_data_cluster_bytes_known: Some(0),
            limitations: BTreeSet::new(),
        }
    }
}

fn component_ok(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '/' | ':'))
}

impl TreeSummary {
    pub fn validate(&self, summary: &Summary, allocation_additive: bool) -> Result<(), ParseError> {
        if self.directories.len() > MAX_TREE_ITEMS
            || self.files.len() > MAX_TREE_ITEMS
            || self.indexed_nodes > MAX_TREE_NODES as u64
            || self.indexed_name_links > self.indexed_nodes.saturating_mul(MAX_LINKS as u64)
            || self.retained_bytes > MAX_TREE_RETAINED_BYTES as u64
            || self.retained_bytes < self.indexed_nodes.saturating_mul(MIN_NODE_BYTES)
            || self.attributed_files.checked_add(self.unattributed_files)
                != Some(summary.base_files as u64)
            || self.attributed_files > self.indexed_nodes
            || self
                .attributed_logical_bytes_known
                .checked_add(self.unattributed_logical_bytes_known)
                != Some(summary.logical_bytes_known)
            || (self.attributed_files == 0 && self.attributed_logical_bytes_known != 0)
            || (self.unattributed_files == 0 && self.unattributed_logical_bytes_known != 0)
        {
            return Err(ParseError::InvalidAttribute);
        }
        if allocation_additive {
            let known = self
                .attributed_data_cluster_bytes_known
                .zip(self.unattributed_data_cluster_bytes_known)
                .and_then(|(a, b)| a.checked_add(b));
            if known != Some(summary.data_cluster_bytes_known) {
                return Err(ParseError::InvalidAttribute);
            }
            if (self.attributed_files == 0 && self.attributed_data_cluster_bytes_known != Some(0))
                || (self.unattributed_files == 0
                    && self.unattributed_data_cluster_bytes_known != Some(0))
            {
                return Err(ParseError::InvalidAttribute);
            }
        } else if self.attributed_data_cluster_bytes_known.is_some()
            || self.unattributed_data_cluster_bytes_known.is_some()
            || !self.limitations.contains(&TreeLimit::AllocationNotAdditive)
        {
            return Err(ParseError::InvalidAttribute);
        }
        for list in [&self.directories, &self.files] {
            let mut paths = BTreeSet::new();
            for item in list {
                // A partial item belongs to the unattributed share; only complete paths are
                // unique (two unread folders can hold equally named subfolders).
                let (files, logical, allocated) = if item.partial_path {
                    (
                        self.unattributed_files,
                        self.unattributed_logical_bytes_known,
                        self.unattributed_data_cluster_bytes_known,
                    )
                } else {
                    (
                        self.attributed_files,
                        self.attributed_logical_bytes_known,
                        self.attributed_data_cluster_bytes_known,
                    )
                };
                if item.path.len() > MAX_TREE_PATH_BYTES
                    || !item.path.starts_with('\\')
                    || !item.path.split('\\').skip(1).all(component_ok)
                    || (!item.partial_path && !paths.insert(&item.path))
                    || (item.partial_path && !self.limitations.contains(&TreeLimit::PartialPaths))
                    || item.files == 0
                    || item.files > files
                    || item.logical_bytes_known > logical
                    || item.unknown_logical_files > item.files
                    || item.unknown_allocation_files > item.files
                    || item.hard_linked_files > item.files
                    || item.unknown_logical_files > summary.unknown_logical_files as u64
                    || item.unknown_allocation_files > summary.unknown_data_allocation_files as u64
                    || item.hard_linked_files > summary.hard_linked_files as u64
                    || item.data_cluster_bytes_known.is_some() != allocation_additive
                    || item
                        .data_cluster_bytes_known
                        .is_some_and(|bytes| allocated.is_none_or(|total| bytes > total))
                {
                    return Err(ParseError::InvalidAttribute);
                }
            }
        }
        let mut complete = Numbers::default();
        let mut partial = Numbers::default();
        for item in &self.files {
            if item.files != 1 {
                return Err(ParseError::InvalidAttribute);
            }
            let share = if item.partial_path {
                &mut partial
            } else {
                &mut complete
            };
            share.add(&Numbers::from_item(item))?;
        }
        if complete.files > self.attributed_files
            || complete.logical > self.attributed_logical_bytes_known
            || self
                .attributed_data_cluster_bytes_known
                .is_some_and(|bytes| complete.allocated > bytes)
            || partial.files > self.unattributed_files
            || partial.logical > self.unattributed_logical_bytes_known
            || self
                .unattributed_data_cluster_bytes_known
                .is_some_and(|bytes| partial.allocated > bytes)
            || serde_json::to_vec(self)
                .map_err(|_| ParseError::InvalidAttribute)?
                .len()
                > MAX_TREE_RESPONSE_BYTES
        {
            return Err(ParseError::InvalidAttribute);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct NameLink {
    identity: AttributeIdentity,
    parent: Key,
    namespace: u8,
    name: String,
}
impl NameLink {
    fn rank(&self) -> (Key, u8, &str, u16) {
        let kind = match self.namespace {
            1 | 3 => 0,
            0 => 1,
            _ => 2,
        };
        (self.parent, kind, &self.name, self.identity.instance)
    }
}

pub(super) fn parse_name(raw: &[u8], identity: AttributeIdentity) -> Result<NameLink, TreeLimit> {
    if raw.len() < 68 {
        return Err(TreeLimit::InvalidFileName);
    }
    let length = usize::from(raw[64]);
    if length == 0 || raw[65] > 3 || raw.len() != 66 + length * 2 {
        return Err(TreeLimit::InvalidFileName);
    }
    let parent = u64::from_le_bytes(raw[..8].try_into().unwrap());
    if parent >> 48 == 0 {
        return Err(TreeLimit::InvalidFileName);
    }
    let words: Vec<_> = raw[66..]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes(pair.try_into().unwrap()))
        .collect();
    let name = String::from_utf16(&words).map_err(|_| TreeLimit::InvalidFileName)?;
    // Root's special dot name need not be decoded to establish its directory identity.
    if !(component_ok(&name) || identity.record.0 == 5 && name == ".") {
        return Err(TreeLimit::InvalidFileName);
    }
    Ok(NameLink {
        identity,
        parent: Key(parent & ((1_u64 << 48) - 1), (parent >> 48) as u16),
        namespace: raw[65],
        name,
    })
}

pub(super) fn retain_link(links: &mut Vec<NameLink>, link: NameLink) -> bool {
    let position = links
        .binary_search_by(|other| other.rank().cmp(&link.rank()))
        .unwrap_or_else(|at| at);
    links.insert(position, link);
    if links.len() > MAX_LINKS {
        links.pop();
        return false;
    }
    true
}

/// Heap bytes of a parsed record's filename links (vector capacity and decoded names).
pub(super) fn parsed_bytes(parsed: &Parsed) -> usize {
    parsed.names.capacity() * size_of::<NameLink>()
        + parsed
            .names
            .iter()
            .map(|link| link.name.capacity())
            .sum::<usize>()
}

/// Filename metadata of one joined record group, before it is packed into the index.
pub(super) struct Metadata {
    key: Key,
    directory: bool,
    unsupported_parent: bool,
    names: Vec<NameLink>,
    limits: BTreeSet<TreeLimit>,
}
impl Metadata {
    pub(super) fn from_group(key: Key, group: &[Parsed]) -> Option<Self> {
        let base = group
            .iter()
            .find(|record| record.is_base && record.key == key)?;
        let mut out = Self {
            key,
            directory: base.directory,
            unsupported_parent: base.directory && base.excluded,
            names: Vec::new(),
            limits: base.tree_limits.clone(),
        };
        if base.unresolved_list {
            out.limits.insert(TreeLimit::FilenameMembershipUnresolved);
        }
        let mut observed = BTreeSet::new();
        for record in group {
            out.limits.extend(&record.tree_limits);
            observed.extend(record.file_names.iter().copied());
            for link in &record.names {
                if record.is_base
                    || base
                        .attribute_list
                        .as_ref()
                        .is_some_and(|list| list.contains(&link.identity))
                {
                    if !retain_link(&mut out.names, link.clone()) {
                        out.limits.insert(TreeLimit::NameLinksTruncated);
                    }
                } else {
                    out.limits.insert(TreeLimit::FilenameMembershipUnresolved);
                }
            }
        }
        if let Some(list) = &base.attribute_list {
            let expected: BTreeSet<_> = list
                .iter()
                .filter(|id| id.key.kind == FILE_NAME)
                .copied()
                .collect();
            if observed != expected {
                out.limits.insert(TreeLimit::FilenameMembershipUnresolved);
            }
        }
        let win32_parents: BTreeSet<_> = out
            .names
            .iter()
            .filter(|link| matches!(link.namespace, 1 | 3))
            .map(|link| link.parent)
            .collect();
        out.names
            .retain(|link| link.namespace != 2 || !win32_parents.contains(&link.parent));
        if out.names.is_empty() && key.0 != 5 {
            out.limits.insert(TreeLimit::MissingFileName);
        }
        Some(out)
    }
}

#[derive(Clone, Copy, Default)]
struct Numbers {
    files: u64,
    logical: u64,
    allocated: u64,
    unknown_logical: u64,
    unknown_allocation: u64,
    hard_links: u64,
}
impl Numbers {
    fn from_item(item: &TreeItem) -> Self {
        Self {
            files: item.files,
            logical: item.logical_bytes_known,
            allocated: item.data_cluster_bytes_known.unwrap_or(0),
            unknown_logical: item.unknown_logical_files,
            unknown_allocation: item.unknown_allocation_files,
            hard_links: item.hard_linked_files,
        }
    }
    fn from_file(node: &FileNode) -> Self {
        let flag = |bit: u8| u64::from(node.link.flags & bit != 0);
        Self {
            files: 1,
            logical: node.logical,
            allocated: node.allocated,
            unknown_logical: flag(UNKNOWN_LOGICAL),
            unknown_allocation: flag(UNKNOWN_ALLOCATION),
            hard_links: flag(HARD_LINKED),
        }
    }
    fn add(&mut self, other: &Self) -> Result<(), ParseError> {
        self.files = add(self.files, other.files)?;
        self.logical = add(self.logical, other.logical)?;
        self.allocated = add(self.allocated, other.allocated)?;
        self.unknown_logical = add(self.unknown_logical, other.unknown_logical)?;
        self.unknown_allocation = add(self.unknown_allocation, other.unknown_allocation)?;
        self.hard_links = add(self.hard_links, other.hard_links)?;
        Ok(())
    }
    /// Ranking amounts: known allocation (only when additive), then known logical bytes.
    fn rank(&self, additive: bool) -> (u64, u64) {
        (if additive { self.allocated } else { 0 }, self.logical)
    }
    fn item(self, path: String, additive: bool, partial_path: bool) -> TreeItem {
        TreeItem {
            path,
            files: self.files,
            logical_bytes_known: self.logical,
            data_cluster_bytes_known: additive.then_some(self.allocated),
            unknown_logical_files: self.unknown_logical,
            unknown_allocation_files: self.unknown_allocation,
            hard_linked_files: self.hard_links,
            partial_path,
        }
    }
}

/// A subtree rollup during `finish`; counts fit in u32 (at most `MAX_TREE_NODES` files).
#[derive(Clone, Copy, Default)]
struct Rollup {
    files: u32,
    unknown_logical: u32,
    unknown_allocation: u32,
    hard_links: u32,
    logical: u64,
    allocated: u64,
}
impl Rollup {
    fn add(&mut self, other: &Numbers) -> Result<(), ParseError> {
        let count = |value: u32, more: u64| {
            u32::try_from(more)
                .ok()
                .and_then(|more| value.checked_add(more))
                .ok_or(ParseError::ArithmeticOverflow)
        };
        self.files = count(self.files, other.files)?;
        self.unknown_logical = count(self.unknown_logical, other.unknown_logical)?;
        self.unknown_allocation = count(self.unknown_allocation, other.unknown_allocation)?;
        self.hard_links = count(self.hard_links, other.hard_links)?;
        self.logical = add(self.logical, other.logical)?;
        self.allocated = add(self.allocated, other.allocated)?;
        Ok(())
    }
    fn numbers(&self) -> Numbers {
        Numbers {
            files: u64::from(self.files),
            logical: self.logical,
            allocated: self.allocated,
            unknown_logical: u64::from(self.unknown_logical),
            unknown_allocation: u64::from(self.unknown_allocation),
            hard_links: u64::from(self.hard_links),
        }
    }
}

/// A key packed so that integer order is `Key` order: record number above, sequence below.
fn pack(key: Key) -> u64 {
    key.0 << 16 | u64::from(key.1)
}
fn unpack(packed: u64) -> Key {
    Key(packed >> 16, packed as u16)
}

/// One filename: packed parent key and the name's place in the arena. On a node's first link,
/// `flags` carries the node's flags.
#[derive(Clone, Copy)]
struct Link {
    parent: u64,
    name: u32,
    len: u16,
    flags: u8,
}
const UNKNOWN_LOGICAL: u8 = 1;
const UNKNOWN_ALLOCATION: u8 = 2;
const HARD_LINKED: u8 = 4;
const EXTRA_LINKS: u8 = 8;
const UNSUPPORTED_PARENT: u8 = 16;
const NO_NAME: u8 = 32;

/// 40 bytes: the file's key, its supported-data amounts and its first-ranked name.
#[derive(Clone, Copy)]
struct FileNode {
    key: u64,
    logical: u64,
    allocated: u64,
    link: Link,
}
/// 24 bytes: a directory needs only its first-ranked name to form paths.
#[derive(Clone, Copy)]
struct DirNode {
    key: u64,
    link: Link,
}
/// Further names of a hard-linked file, in rank order, stored after its node.
#[derive(Clone, Copy)]
struct ExtraLink {
    file: u32,
    link: Link,
}

type Nodes<T> = Chunked<T, 64, 8192>;

/// `finish` working memory per file: chosen link (1 byte), canonical parent (4) and a
/// conflict-sort index (4), rounded up.
pub(super) const FILE_FINISH_BYTES: usize = 12;
/// `finish` working memory per directory: key order, a second index order, parent, canonical
/// id and resolution stack (5 x 4 bytes), state, depth and path length (4), path fingerprint
/// (16) and subtree rollup (32).
pub(super) const DIR_FINISH_BYTES: usize = 72;
/// Root anchor, limitation set, top-N heaps, path scratch and one record's packed names.
const FIXED_BYTES: usize = 2048;
/// Smallest accounted node, for `TreeSummary::validate`.
const MIN_NODE_BYTES: u64 = size_of::<DirNode>() as u64;

pub(super) struct TreeBuilder {
    root: Option<Key>,
    dirs: Nodes<DirNode>,
    files: Nodes<FileNode>,
    extra: Nodes<ExtraLink>,
    names: Arena,
    links: u64,
    capacity: usize,
    /// Kept for directories: a full file index cannot block the parents read later.
    dir_reserve: usize,
    limits: BTreeSet<TreeLimit>,
}
impl TreeBuilder {
    pub(super) fn new(global_bound: usize) -> Self {
        let capacity = MAX_TREE_RETAINED_BYTES.min(global_bound / 4 * 3);
        let dir_reserve = capacity / 3;
        Self {
            root: None,
            dirs: Nodes::with_bound(capacity),
            files: Nodes::with_bound(capacity - dir_reserve),
            extra: Nodes::with_bound(capacity / 4),
            names: Arena::with_bound(capacity),
            links: 0,
            capacity,
            dir_reserve,
            limits: BTreeSet::new(),
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        FIXED_BYTES
            + self.dirs.bytes()
            + self.files.bytes()
            + self.extra.bytes()
            + self.names.bytes()
            + self.dirs.len() * DIR_FINISH_BYTES
            + self.files.len() * FILE_FINISH_BYTES
    }
    pub(super) fn has_root(&self) -> bool {
        self.root.is_some()
    }
    pub(super) fn record(&mut self, metadata: Option<Metadata>, delta: &Summary, available: usize) {
        let Some(metadata) = metadata else {
            return;
        };
        self.limits.extend(&metadata.limits);
        if metadata.directory && metadata.key.0 == 5 {
            if metadata.unsupported_parent {
                self.limits.insert(TreeLimit::ParentMetadataIncomplete);
            } else {
                self.root = Some(metadata.key);
            }
            return;
        }
        if !metadata.directory && (metadata.names.is_empty() || delta.base_files == 0) {
            return;
        }
        // A directory forms paths with its first-ranked name only.
        let names = if metadata.directory {
            &metadata.names[..metadata.names.len().min(1)]
        } else {
            &metadata.names[..]
        };
        let extra = names.len().saturating_sub(1);
        let growth = self
            .names
            .growth(names.iter().map(|link| link.name.len()))
            .saturating_add(if metadata.directory {
                self.dirs.growth(1).saturating_add(DIR_FINISH_BYTES)
            } else {
                self.files
                    .growth(1)
                    .saturating_add(self.extra.growth(extra))
                    .saturating_add(FILE_FINISH_BYTES)
            });
        let limit = if metadata.directory {
            self.capacity
        } else {
            self.capacity - self.dir_reserve
        };
        if self.retained_bytes().saturating_add(growth) > limit || growth > available {
            self.limits.insert(TreeLimit::IndexMemoryLimit);
            return;
        }
        if self.dirs.len() + self.files.len() >= MAX_TREE_NODES {
            self.limits.insert(TreeLimit::IndexNodeLimit);
            return;
        }
        let mut stored = Vec::with_capacity(names.len());
        for link in names {
            let (Some(at), Ok(len)) = (
                self.names.push(link.name.as_bytes()),
                u16::try_from(link.name.len()),
            ) else {
                self.limits.insert(TreeLimit::IndexMemoryLimit);
                return;
            };
            stored.push(Link {
                parent: pack(link.parent),
                name: at,
                len,
                flags: 0,
            });
        }
        let key = pack(metadata.key);
        let pushed = if metadata.directory {
            let mut link = stored.first().copied().unwrap_or(Link {
                parent: 0,
                name: 0,
                len: 0,
                flags: NO_NAME,
            });
            if metadata.unsupported_parent {
                link.flags |= UNSUPPORTED_PARENT;
            }
            self.dirs.push(DirNode { key, link })
        } else {
            let mut link = stored[0];
            for (bit, set) in [
                (UNKNOWN_LOGICAL, delta.unknown_logical_files != 0),
                (UNKNOWN_ALLOCATION, delta.unknown_data_allocation_files != 0),
                (HARD_LINKED, delta.hard_linked_files != 0),
                (EXTRA_LINKS, stored.len() > 1),
            ] {
                if set {
                    link.flags |= bit;
                }
            }
            let file = self.files.len() as u32;
            self.files.push(FileNode {
                key,
                logical: delta.logical_bytes_known,
                allocated: delta.data_cluster_bytes_known,
                link,
            }) && stored[1..]
                .iter()
                .all(|&link| self.extra.push(ExtraLink { file, link }))
        };
        if pushed {
            self.links += stored.len() as u64;
        } else {
            self.limits.insert(TreeLimit::IndexMemoryLimit);
        }
    }

    fn name(&self, link: Link) -> &str {
        // Only names decoded from valid UTF-16 are stored, so this never falls back.
        std::str::from_utf8(self.names.get(link.name, link.len)).unwrap_or("")
    }

    /// The file's names in rank order: its node's link, then its extra links.
    fn file_links(&self, index: u32) -> impl Iterator<Item = Link> + '_ {
        let node = self.files.get(index as usize);
        let first_extra = if node.link.flags & EXTRA_LINKS != 0 {
            let (mut low, mut high) = (0, self.extra.len());
            while low < high {
                let middle = (low + high) / 2;
                if self.extra.get(middle).file < index {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            low
        } else {
            self.extra.len()
        };
        std::iter::once(node.link).chain(
            (first_extra..self.extra.len())
                .map(|i| self.extra.get(i))
                .take_while(move |entry| entry.file == index)
                .map(|entry| entry.link),
        )
    }

    pub(super) fn finish(
        self,
        summary: &Summary,
        additive: bool,
        incomplete: bool,
        stop: &mut dyn FnMut() -> Result<Option<CollectionEnd>, CollectError>,
    ) -> Result<(TreeSummary, Option<CollectionEnd>), CollectError> {
        let mut out = TreeSummary {
            indexed_nodes: (self.dirs.len() + self.files.len()) as u64,
            indexed_name_links: self.links,
            retained_bytes: self.retained_bytes() as u64,
            limitations: self.limits.clone(),
            ..TreeSummary::default()
        };
        if incomplete {
            out.limitations.insert(TreeLimit::CollectionIncomplete);
        }
        if !additive {
            out.limitations.insert(TreeLimit::AllocationNotAdditive);
        }
        let ending = Finish::run(&self, additive, &mut out, stop)?;
        if ending.is_some() {
            // Nothing half-computed is returned: every file stays unattributed.
            out.directories.clear();
            out.files.clear();
            out.limitations.insert(TreeLimit::Interrupted);
            out.limitations.remove(&TreeLimit::PartialPaths);
            out.attributed_files = 0;
            out.attributed_logical_bytes_known = 0;
            out.attributed_data_cluster_bytes_known = additive.then_some(0);
        }
        out.unattributed_files = (summary.base_files as u64)
            .checked_sub(out.attributed_files)
            .ok_or(ParseError::ArithmeticOverflow)?;
        out.unattributed_logical_bytes_known = summary
            .logical_bytes_known
            .checked_sub(out.attributed_logical_bytes_known)
            .ok_or(ParseError::ArithmeticOverflow)?;
        out.unattributed_data_cluster_bytes_known = if additive {
            Some(
                summary
                    .data_cluster_bytes_known
                    .checked_sub(out.attributed_data_cluster_bytes_known.unwrap_or(0))
                    .ok_or(ParseError::ArithmeticOverflow)?,
            )
        } else {
            None
        };
        out.validate(summary, additive)?;
        Ok((out, ending))
    }
}

/// Directory resolution states during `finish`; errors keep the limitation that stopped them.
const UNVISITED: u8 = 0;
const VISITING: u8 = 1;
const COMPLETE: u8 = 2;
const PARTIAL: u8 = 3;
const ERRORS: [TreeLimit; 5] = [
    TreeLimit::ParentMetadataIncomplete,
    TreeLimit::ParentSequenceChanged,
    TreeLimit::ParentCycle,
    TreeLimit::DepthLimit,
    TreeLimit::PathLimit,
];
fn error_state(limit: TreeLimit) -> u8 {
    16 + ERRORS.iter().position(|known| *known == limit).unwrap_or(0) as u8
}
fn state_error(state: u8) -> Option<TreeLimit> {
    (state & !CONFLICT)
        .checked_sub(16)
        .and_then(|index| ERRORS.get(usize::from(index)).copied())
}
/// Marks a resolvable directory whose path an earlier directory already has.
const CONFLICT: u8 = 0x80;
/// Parent sentinels: the chain reaches the read root, or ends at an unread folder or an error.
const ROOT: u32 = u32::MAX - 1;
const NONE: u32 = u32::MAX;
/// Canonical parents of files directly below the root, read or not.
const ROOT_CANON: u32 = u32::MAX - 1;
const UNREAD_ROOT_CANON: u32 = u32::MAX - 2;
/// A chosen filename whose path starts below an unread folder.
const PARTIAL_CHOICE: u8 = 0x80;
/// How often the loops of `finish` honour cancellation and the deadline.
const STOP_EVERY: usize = 1 << 16;

enum Top {
    Root,
    Unread,
    Error(TreeLimit),
    Resolved(u32),
}

enum Placement {
    Complete(u32),
    Partial(u32),
    Failed(TreeLimit),
}

type Stop<'s> = &'s mut dyn FnMut() -> Result<Option<CollectionEnd>, CollectError>;

/// Local path identity; it only compares directories and never leaves `finish`.
fn fingerprint(tag: u8, parent: &[u8], name: &[u8]) -> [u8; 16] {
    let mut hash = Sha256::new();
    hash.update([tag]);
    hash.update(parent);
    hash.update((name.len() as u16).to_le_bytes());
    hash.update(name);
    hash.finalize()[..16].try_into().unwrap()
}

fn vec_of<T: Clone>(len: usize, value: T) -> Result<Vec<T>, CollectError> {
    let mut out = Vec::new();
    // Accounted in the per-node finish reserve; failure is reported, never an abort.
    out.try_reserve_exact(len)
        .map_err(|_| CollectError::InvalidBounds)?;
    out.resize(len, value);
    Ok(out)
}

/// Working state of `TreeBuilder::finish`, sized by `DIR_FINISH_BYTES` and `FILE_FINISH_BYTES`.
struct Finish<'a> {
    tree: &'a TreeBuilder,
    by_key: Vec<u32>,
    state: Vec<u8>,
    parent: Vec<u32>,
    depth: Vec<u8>,
    length: Vec<u16>,
    canon: Vec<u32>,
    ticks: usize,
}
impl<'a> Finish<'a> {
    /// Fills `out` and returns `None`, or returns the stop reason (the caller clears `out`).
    fn run(
        tree: &'a TreeBuilder,
        additive: bool,
        out: &mut TreeSummary,
        stop: Stop<'_>,
    ) -> Result<Option<CollectionEnd>, CollectError> {
        let dirs = tree.dirs.len();
        let mut by_key: Vec<u32> = vec_of(dirs, 0)?;
        for (index, slot) in by_key.iter_mut().enumerate() {
            *slot = index as u32;
        }
        by_key.sort_unstable_by_key(|&index| tree.dirs.get(index as usize).key);
        let mut finish = Finish {
            tree,
            by_key,
            state: vec_of(dirs, UNVISITED)?,
            parent: vec_of(dirs, NONE)?,
            depth: vec_of(dirs, 0)?,
            length: vec_of(dirs, 0)?,
            canon: vec_of(dirs, NONE)?,
            ticks: 0,
        };
        if let Some(end) = stop()? {
            return Ok(Some(end));
        }
        let fingerprints = match finish.resolve(&mut out.limitations, stop)? {
            Ok(fingerprints) => fingerprints,
            Err(end) => return Ok(Some(end)),
        };
        if let Some(end) = finish.canonicalize(fingerprints, &mut out.limitations, stop)? {
            return Ok(Some(end));
        }
        let mut choice: Vec<u8> = vec_of(tree.files.len(), 0)?;
        let mut canon: Vec<u32> = vec_of(tree.files.len(), NONE)?;
        if let Some(end) =
            finish.place_files(&mut choice, &mut canon, &mut out.limitations, stop)?
        {
            return Ok(Some(end));
        }
        if let Some(end) = finish.drop_conflicts(&mut choice, &canon, &mut out.limitations, stop)? {
            return Ok(Some(end));
        }
        drop(canon);
        finish.summarize(&choice, additive, out, stop)
    }

    fn tick(&mut self, stop: Stop<'_>) -> Result<Option<CollectionEnd>, CollectError> {
        self.ticks += 1;
        if self.ticks.is_multiple_of(STOP_EVERY) {
            return stop();
        }
        Ok(None)
    }

    fn find(&self, key: u64) -> Option<u32> {
        let dirs = &self.tree.dirs;
        self.by_key
            .binary_search_by_key(&key, |&index| dirs.get(index as usize).key)
            .ok()
            .map(|position| self.by_key[position])
    }

    /// Whether a read directory has this record number under another sequence.
    fn number_read(&self, number: u64) -> bool {
        let dirs = &self.tree.dirs;
        let at = self
            .by_key
            .partition_point(|&index| dirs.get(index as usize).key >> 16 < number);
        at < self.by_key.len() && dirs.get(self.by_key[at] as usize).key >> 16 == number
    }

    /// Resolves every directory chain once: complete up to the read root, partial up to an
    /// unread folder, or failed with the limitation that stopped it. Returns path fingerprints.
    fn resolve(
        &mut self,
        limits: &mut BTreeSet<TreeLimit>,
        stop: Stop<'_>,
    ) -> Result<Result<Vec<[u8; 16]>, CollectionEnd>, CollectError> {
        let tree = self.tree;
        let mut fingerprints: Vec<[u8; 16]> = vec_of(tree.dirs.len(), [0; 16])?;
        let mut stack: Vec<u32> = Vec::new();
        for start in 0..tree.dirs.len() {
            if let Some(end) = self.tick(stop)? {
                return Ok(Err(end));
            }
            if self.state[start] != UNVISITED {
                continue;
            }
            stack.clear();
            let mut current = start as u32;
            let top = loop {
                match self.state[current as usize] {
                    UNVISITED => {}
                    VISITING => break Top::Error(TreeLimit::ParentCycle),
                    _ => break Top::Resolved(current),
                }
                self.state[current as usize] = VISITING;
                // At most one entry per directory, which DIR_FINISH_BYTES accounts.
                stack.push(current);
                let node = tree.dirs.get(current as usize);
                if node.link.flags & (UNSUPPORTED_PARENT | NO_NAME) != 0 {
                    break Top::Error(TreeLimit::ParentMetadataIncomplete);
                }
                let parent = unpack(node.link.parent);
                if parent.0 == 5 {
                    break match tree.root {
                        Some(root) if root == parent => Top::Root,
                        Some(_) => Top::Error(TreeLimit::ParentSequenceChanged),
                        None => Top::Unread,
                    };
                }
                match self.find(node.link.parent) {
                    Some(index) => {
                        self.parent[current as usize] = index;
                        current = index;
                    }
                    None if self.number_read(parent.0) => {
                        break Top::Error(TreeLimit::ParentSequenceChanged)
                    }
                    None => break Top::Unread,
                }
            };
            let mut result = top;
            while let Some(index) = stack.pop() {
                let at = index as usize;
                let node = tree.dirs.get(at);
                let name = tree.names.get(node.link.name, node.link.len);
                let resolved = match result {
                    Top::Root => {
                        self.parent[at] = ROOT;
                        Ok((COMPLETE, 1, 0, fingerprint(b'R', &[], name)))
                    }
                    Top::Unread => {
                        self.parent[at] = NONE;
                        limits.insert(TreeLimit::ParentMissing);
                        let anchor = node.key.to_le_bytes();
                        Ok((PARTIAL, 1, 0, fingerprint(b'A', &anchor, name)))
                    }
                    Top::Error(limit) => Err(limit),
                    Top::Resolved(parent) => {
                        let p = parent as usize;
                        match state_error(self.state[p]) {
                            Some(limit) => Err(limit),
                            None => Ok((
                                self.state[p],
                                usize::from(self.depth[p]) + 1,
                                usize::from(self.length[p]),
                                fingerprint(b'D', &fingerprints[p], name),
                            )),
                        }
                    }
                };
                let resolved = resolved.and_then(|(state, depth, base, print)| {
                    let length = base + 1 + name.len();
                    if depth >= MAX_DEPTH {
                        Err(TreeLimit::DepthLimit)
                    } else if length > MAX_TREE_PATH_BYTES {
                        Err(TreeLimit::PathLimit)
                    } else {
                        Ok((state, depth, length, print))
                    }
                });
                match resolved {
                    Ok((state, depth, length, print)) => {
                        self.state[at] = state;
                        self.depth[at] = depth as u8;
                        self.length[at] = length as u16;
                        fingerprints[at] = print;
                    }
                    Err(limit) => {
                        self.state[at] = error_state(limit);
                        self.parent[at] = NONE;
                    }
                }
                result = Top::Resolved(index);
            }
        }
        Ok(Ok(fingerprints))
    }

    /// Gives directories with one path one canonical id. The lowest key keeps the path; later
    /// ones (a live rename or replacement) are path conflicts and are not listed.
    fn canonicalize(
        &mut self,
        fingerprints: Vec<[u8; 16]>,
        limits: &mut BTreeSet<TreeLimit>,
        stop: Stop<'_>,
    ) -> Result<Option<CollectionEnd>, CollectError> {
        let tree = self.tree;
        let mut order: Vec<u32> = Vec::new();
        order
            .try_reserve_exact(tree.dirs.len())
            .map_err(|_| CollectError::InvalidBounds)?;
        order.extend(
            (0..tree.dirs.len() as u32)
                .filter(|&index| matches!(self.state[index as usize], COMPLETE | PARTIAL)),
        );
        order.sort_unstable_by(|&a, &b| {
            fingerprints[a as usize]
                .cmp(&fingerprints[b as usize])
                .then_with(|| {
                    tree.dirs
                        .get(a as usize)
                        .key
                        .cmp(&tree.dirs.get(b as usize).key)
                })
        });
        let mut first = NONE;
        for position in 0..order.len() {
            if let Some(end) = self.tick(stop)? {
                return Ok(Some(end));
            }
            let index = order[position] as usize;
            if position == 0 || fingerprints[order[position - 1] as usize] != fingerprints[index] {
                first = index as u32;
            } else {
                self.state[index] |= CONFLICT;
                limits.insert(TreeLimit::PathConflict);
            }
            self.canon[index] = first;
        }
        Ok(None)
    }

    /// Where one filename places its file, from its parent directory's resolution.
    fn place(&self, link: Link) -> Placement {
        let tree = self.tree;
        let parent = unpack(link.parent);
        let fits = |base: usize| base + 1 + usize::from(link.len) <= MAX_TREE_PATH_BYTES;
        if parent.0 == 5 {
            return match tree.root {
                Some(root) if root == parent => Placement::Complete(ROOT_CANON),
                Some(_) => Placement::Failed(TreeLimit::ParentSequenceChanged),
                None => Placement::Partial(UNREAD_ROOT_CANON),
            };
        }
        match self.find(link.parent) {
            Some(index) => {
                let at = index as usize;
                let base = usize::from(self.length[at]);
                match self.state[at] & !CONFLICT {
                    COMPLETE if fits(base) => Placement::Complete(self.canon[at]),
                    PARTIAL if fits(base) => Placement::Partial(self.canon[at]),
                    COMPLETE | PARTIAL => Placement::Failed(TreeLimit::PathLimit),
                    state => Placement::Failed(
                        state_error(state).unwrap_or(TreeLimit::ParentMetadataIncomplete),
                    ),
                }
            }
            None if self.number_read(parent.0) => {
                Placement::Failed(TreeLimit::ParentSequenceChanged)
            }
            // Its own folder was not read: listed by name alone, below an unknown location.
            None => {
                if fits(0) {
                    Placement::Partial(NONE)
                } else {
                    Placement::Failed(TreeLimit::PathLimit)
                }
            }
        }
    }

    /// Chooses each file's name: the first complete one in rank order, else the first partial.
    /// `choice` is the link position plus one (0: none), with `PARTIAL_CHOICE` for a partial.
    fn place_files(
        &mut self,
        choice: &mut [u8],
        canon: &mut [u32],
        limits: &mut BTreeSet<TreeLimit>,
        stop: Stop<'_>,
    ) -> Result<Option<CollectionEnd>, CollectError> {
        for file in 0..self.tree.files.len() {
            if let Some(end) = self.tick(stop)? {
                return Ok(Some(end));
            }
            let mut partial = None;
            for (position, link) in self.tree.file_links(file as u32).enumerate() {
                match self.place(link) {
                    Placement::Complete(parent) => {
                        partial = None;
                        choice[file] = position as u8 + 1;
                        canon[file] = parent;
                        break;
                    }
                    Placement::Partial(parent) => {
                        limits.insert(TreeLimit::ParentMissing);
                        partial = partial.or(Some((position, parent)));
                    }
                    Placement::Failed(limit) => {
                        limits.insert(limit);
                    }
                }
            }
            if let Some((position, parent)) = partial {
                choice[file] = (position as u8 + 1) | PARTIAL_CHOICE;
                canon[file] = parent;
            }
        }
        Ok(None)
    }

    fn chosen(&self, file: u32, choice: u8) -> Link {
        let position = usize::from(choice & !PARTIAL_CHOICE).saturating_sub(1);
        self.tree
            .file_links(file)
            .nth(position)
            .unwrap_or_else(|| self.tree.files.get(file as usize).link)
    }

    /// Two files with one path (same canonical folder and name): the lowest key keeps it.
    /// Files whose own folder was not read have no comparable location and are all kept.
    fn drop_conflicts(
        &mut self,
        choice: &mut [u8],
        canon: &[u32],
        limits: &mut BTreeSet<TreeLimit>,
        stop: Stop<'_>,
    ) -> Result<Option<CollectionEnd>, CollectError> {
        let tree = self.tree;
        let mut order: Vec<u32> = Vec::new();
        order
            .try_reserve_exact(tree.files.len())
            .map_err(|_| CollectError::InvalidBounds)?;
        order.extend(
            (0..tree.files.len() as u32)
                .filter(|&file| choice[file as usize] != 0 && canon[file as usize] != NONE),
        );
        {
            let choice = &*choice;
            let name = |file: u32| {
                let link = self.chosen(file, choice[file as usize]);
                tree.names.get(link.name, link.len)
            };
            order.sort_unstable_by(|&a, &b| {
                canon[a as usize]
                    .cmp(&canon[b as usize])
                    .then_with(|| name(a).cmp(name(b)))
                    .then_with(|| {
                        tree.files
                            .get(a as usize)
                            .key
                            .cmp(&tree.files.get(b as usize).key)
                    })
            });
        }
        let mut head = 0;
        for position in 1..order.len() {
            if let Some(end) = self.tick(stop)? {
                return Ok(Some(end));
            }
            let (first, file) = (order[head], order[position]);
            let same = canon[first as usize] == canon[file as usize] && {
                let a = self.chosen(first, choice[first as usize]);
                let b = self.chosen(file, choice[file as usize]);
                tree.names.get(a.name, a.len) == tree.names.get(b.name, b.len)
            };
            if same {
                choice[file as usize] = 0;
                limits.insert(TreeLimit::PathConflict);
            } else {
                head = position;
            }
        }
        Ok(None)
    }

    fn parent_of(&self, link: Link) -> u32 {
        if unpack(link.parent).0 == 5 {
            ROOT
        } else {
            self.find(link.parent).unwrap_or(NONE)
        }
    }

    fn dir_path(&self, index: u32) -> String {
        let mut chain = Vec::new();
        let mut current = index;
        while current != ROOT && current != NONE && chain.len() < MAX_DEPTH {
            chain.push(current);
            current = self.parent[current as usize];
        }
        let mut path = String::new();
        for &index in chain.iter().rev() {
            path.push('\\');
            path.push_str(self.tree.name(self.tree.dirs.get(index as usize).link));
        }
        path
    }

    /// Rolls files up into their folders, then picks and names the largest items.
    fn summarize(
        &mut self,
        choice: &[u8],
        additive: bool,
        out: &mut TreeSummary,
        stop: Stop<'_>,
    ) -> Result<Option<CollectionEnd>, CollectError> {
        let tree = self.tree;
        let mut rollups: Vec<Rollup> = vec_of(tree.dirs.len(), Rollup::default())?;
        let mut attributed = Numbers::default();
        let mut top_files = BinaryHeap::new();
        let mut file_candidates = 0_usize;
        for (file, &chosen) in choice.iter().enumerate() {
            if let Some(end) = self.tick(stop)? {
                return Ok(Some(end));
            }
            if chosen == 0 {
                continue;
            }
            let node = tree.files.get(file);
            let numbers = Numbers::from_file(&node);
            if chosen & PARTIAL_CHOICE == 0 {
                attributed.add(&numbers)?;
            }
            let parent = self.parent_of(self.chosen(file as u32, chosen));
            if parent != ROOT && parent != NONE {
                rollups[parent as usize].add(&numbers)?;
            }
            file_candidates += 1;
            keep(
                &mut top_files,
                numbers.rank(additive),
                node.key,
                file as u32,
            );
        }
        // Children before parents: depth only decreases along a resolved chain.
        let mut order: Vec<u32> = Vec::new();
        order
            .try_reserve_exact(tree.dirs.len())
            .map_err(|_| CollectError::InvalidBounds)?;
        order.extend(
            (0..tree.dirs.len() as u32).filter(|&index| {
                matches!(self.state[index as usize] & !CONFLICT, COMPLETE | PARTIAL)
            }),
        );
        order.sort_unstable_by_key(|&index| Reverse(self.depth[index as usize]));
        for &index in &order {
            if let Some(end) = self.tick(stop)? {
                return Ok(Some(end));
            }
            let parent = self.parent[index as usize];
            if parent != ROOT && parent != NONE {
                let child = rollups[index as usize].numbers();
                rollups[parent as usize].add(&child)?;
            }
        }
        let mut top_dirs = BinaryHeap::new();
        let mut dir_candidates = 0_usize;
        for &index in &order {
            let rollup = rollups[index as usize];
            if self.state[index as usize] & CONFLICT != 0 || rollup.files == 0 {
                continue;
            }
            dir_candidates += 1;
            let key = tree.dirs.get(index as usize).key;
            keep(&mut top_dirs, rollup.numbers().rank(additive), key, index);
        }
        if dir_candidates > MAX_TREE_ITEMS {
            out.limitations.insert(TreeLimit::TopDirectoriesTruncated);
        }
        if file_candidates > MAX_TREE_ITEMS {
            out.limitations.insert(TreeLimit::TopFilesTruncated);
        }
        for Reverse((_, _, index)) in top_dirs {
            let partial = self.state[index as usize] & !CONFLICT == PARTIAL;
            let path = self.dir_path(index);
            let item = rollups[index as usize]
                .numbers()
                .item(path, additive, partial);
            out.directories.push(item);
        }
        for Reverse((_, _, file)) in top_files {
            let at = file as usize;
            let link = self.chosen(file, choice[at]);
            let parent = self.parent_of(link);
            let mut path = if parent == ROOT || parent == NONE {
                String::new()
            } else {
                self.dir_path(parent)
            };
            path.push('\\');
            path.push_str(tree.name(link));
            let partial = choice[at] & PARTIAL_CHOICE != 0;
            let item = Numbers::from_file(&tree.files.get(at)).item(path, additive, partial);
            out.files.push(item);
        }
        for list in [&mut out.directories, &mut out.files] {
            list.sort_by(|left, right| {
                right
                    .data_cluster_bytes_known
                    .cmp(&left.data_cluster_bytes_known)
                    .then_with(|| right.logical_bytes_known.cmp(&left.logical_bytes_known))
                    .then_with(|| left.path.cmp(&right.path))
            });
        }
        if out
            .directories
            .iter()
            .chain(&out.files)
            .any(|item| item.partial_path)
        {
            out.limitations.insert(TreeLimit::PartialPaths);
        }
        out.attributed_files = attributed.files;
        out.attributed_logical_bytes_known = attributed.logical;
        out.attributed_data_cluster_bytes_known = additive.then_some(attributed.allocated);
        Ok(None)
    }
}

type Ranked = Reverse<((u64, u64), Reverse<u64>, u32)>;

/// Keeps the `MAX_TREE_ITEMS` best entries: larger amounts first, then the lower key.
fn keep(heap: &mut BinaryHeap<Ranked>, rank: (u64, u64), key: u64, index: u32) {
    heap.push(Reverse((rank, Reverse(key), index)));
    if heap.len() > MAX_TREE_ITEMS {
        heap.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{geometry, listed_data, nonresident, record, resident};
    use super::*;
    use crate::test_alloc::measure;

    fn filename(parent: Key, name: &str, namespace: u8, id: u16) -> Vec<u8> {
        let words: Vec<_> = name.encode_utf16().collect();
        let mut attr = resident(FILE_NAME, 66 + words.len() * 2, id);
        attr[24..32].copy_from_slice(&(parent.0 | (u64::from(parent.1) << 48)).to_le_bytes());
        attr[24 + 64] = words.len() as u8;
        attr[24 + 65] = namespace;
        for (i, word) in words.iter().enumerate() {
            attr[24 + 66 + i * 2..24 + 68 + i * 2].copy_from_slice(&word.to_le_bytes());
        }
        attr
    }
    fn directory(sequence: u16, parent: Key, name: &str) -> Vec<u8> {
        let mut raw = record(1024, sequence, 0, 1, &[filename(parent, name, 1, 0)]);
        raw[22..24].copy_from_slice(&3_u16.to_le_bytes());
        raw
    }
    fn file(names: &[Vec<u8>], lcn: u8, size: u64, links: u16) -> Vec<u8> {
        let mut attrs = names.to_vec();
        attrs.push(nonresident(0, 1, size, 4096, 4096, 0, &[0x11, 1, lcn, 0]));
        let last = attrs.last_mut().unwrap();
        last[14..16].copy_from_slice(&99_u16.to_le_bytes());
        record(4096, 1, 0, links, &attrs)
    }
    struct Fixtures {
        records: BTreeMap<u64, Vec<u8>>,
        slots: u64,
        size: usize,
    }
    impl Source for Fixtures {
        fn snapshot(&mut self) -> Result<VolumeSnapshot, CollectError> {
            let mut geometry = geometry(self.size);
            geometry.volume_clusters = geometry.volume_clusters.max(self.slots * 2);
            Ok(VolumeSnapshot {
                geometry,
                record_slots: self.slots,
                journal: None,
            })
        }
        fn record_at_or_below(&mut self, ordinal: u64) -> Result<RecordData, CollectError> {
            let (&number, bytes) = self
                .records
                .range(..=ordinal)
                .next_back()
                .ok_or(CollectError::Unavailable)?;
            Ok(RecordData {
                number,
                bytes: bytes.clone(),
                returned_sequence: None,
            })
        }
    }
    fn scan(records: impl IntoIterator<Item = (u64, Vec<u8>)>) -> Collection {
        scan_bound(
            records,
            CollectionBounds::for_budget(Duration::from_secs(30)),
        )
    }
    fn scan_bound(
        records: impl IntoIterator<Item = (u64, Vec<u8>)>,
        bounds: CollectionBounds,
    ) -> Collection {
        scan_slots(records, bounds, 128)
    }
    fn scan_slots(
        records: impl IntoIterator<Item = (u64, Vec<u8>)>,
        bounds: CollectionBounds,
        slots: u64,
    ) -> Collection {
        let mut records: BTreeMap<_, _> = records
            .into_iter()
            .map(|(id, mut bytes)| {
                if bytes.len() == 1024 {
                    // Rebuild directory metadata rather than padding a record with stale fixups.
                    let parsed = parse(
                        &Record {
                            number: id,
                            bytes: &bytes,
                        },
                        geometry(1024),
                        &mut 0,
                    )
                    .unwrap()
                    .unwrap();
                    let attrs: Vec<_> = parsed
                        .names
                        .iter()
                        .enumerate()
                        .map(|(index, link)| {
                            filename(link.parent, &link.name, link.namespace, index as u16)
                        })
                        .collect();
                    bytes = record(4096, parsed.key.1, 0, 1, &attrs);
                    bytes[22..24].copy_from_slice(&3_u16.to_le_bytes());
                }
                (id, bytes)
            })
            .collect();
        records
            .entry(0)
            .or_insert_with(|| record(4096, 1, 0, 1, &[resident(DATA, 0, 0)]));
        let mut source = Fixtures {
            records,
            slots,
            size: 4096,
        };
        collect(&mut source, bounds, &mut || Ok(false)).unwrap()
    }

    #[test]
    fn hard_links_are_attributed_once_and_dos_aliases_do_not_win() {
        let root = Key(5, 1);
        let collection = scan([
            (5, directory(1, root, ".")),
            (20, directory(2, root, "alpha")),
            (21, directory(3, root, "beta")),
            (
                70,
                file(
                    &[
                        filename(Key(21, 3), "second.bin", 1, 1),
                        filename(Key(20, 2), "FIRST~1.BIN", 2, 2),
                        filename(Key(20, 2), "first.bin", 1, 3),
                    ],
                    10,
                    100,
                    2,
                ),
            ),
        ]);
        assert_eq!(collection.tree.attributed_files, 1);
        assert_eq!(collection.tree.attributed_logical_bytes_known, 100);
        assert_eq!(
            collection.tree.attributed_data_cluster_bytes_known,
            Some(4096)
        );
        assert_eq!(collection.tree.files[0].path, "\\alpha\\first.bin");
        assert_eq!(collection.tree.directories.len(), 1);
        assert_eq!(collection.tree.directories[0].path, "\\alpha");
        assert!(!format!("{:?}", collection.tree).contains("first.bin"));
        collection.validate().unwrap();
    }

    #[test]
    fn missing_changed_and_cyclic_parents_preserve_unattributed_bytes() {
        let root = Key(5, 1);
        let collection = scan([
            (5, directory(1, root, ".")),
            (20, directory(2, root, "renamed")),
            (21, directory(3, Key(22, 4), "loop-a")),
            (22, directory(4, Key(21, 3), "loop-b")),
            (
                70,
                file(&[filename(Key(20, 1), "stale.bin", 1, 1)], 10, 100, 1),
            ),
            (
                71,
                file(&[filename(Key(30, 1), "missing.bin", 1, 1)], 20, 200, 1),
            ),
            (
                72,
                file(&[filename(Key(21, 3), "cycle.bin", 1, 1)], 30, 300, 1),
            ),
        ]);
        assert_eq!(collection.tree.attributed_files, 0);
        assert_eq!(collection.tree.unattributed_files, 3);
        assert_eq!(collection.tree.unattributed_logical_bytes_known, 600);
        assert_eq!(
            collection.tree.unattributed_data_cluster_bytes_known,
            Some(12288)
        );
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::ParentSequenceChanged));
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::ParentMissing));
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::ParentCycle));
        // The file whose folder was never read is still listed, by name, as partial.
        assert_eq!(collection.tree.files.len(), 1);
        assert_eq!(collection.tree.files[0].path, "\\missing.bin");
        assert!(collection.tree.files[0].partial_path);
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::PartialPaths));
        collection.validate().unwrap();
    }

    #[test]
    fn invalid_filename_metadata_does_not_destroy_valid_data_aggregates() {
        let root = Key(5, 1);
        let mut invalid = filename(root, "ok.bin", 1, 1);
        invalid[24 + 66..24 + 68].copy_from_slice(&0xd800_u16.to_le_bytes());
        let collection = scan([
            (5, directory(1, root, ".")),
            (70, file(&[invalid], 10, 100, 1)),
        ]);
        assert_eq!(collection.summary.logical_bytes_known, 100);
        assert_eq!(collection.tree.unattributed_logical_bytes_known, 100);
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::InvalidFileName));
        collection.validate().unwrap();
    }

    #[test]
    fn extension_filenames_require_listed_record_sequence_instance_and_vcn() {
        let root = Key(5, 1);
        let extension = record(
            4096,
            8,
            70 | (3_u64 << 48),
            0,
            &[filename(root, "extended.bin", 1, 1)],
        );
        for (sequence, instance, vcn, resolves) in [
            (8, 1, 0, true),
            (9, 1, 0, false),
            (8, 2, 0, false),
            (8, 1, 1, false),
        ] {
            let mut list = listed_data(&[(70, 3, 0, 0), (71, sequence, vcn, instance)]);
            list[24 + 32..24 + 36].copy_from_slice(&FILE_NAME.to_le_bytes());
            let head = record(
                4096,
                3,
                0,
                1,
                &[
                    list,
                    nonresident(0, 1, 100, 4096, 4096, 0, &[0x11, 1, 10, 0]),
                ],
            );
            let collection = scan([
                (5, directory(1, root, ".")),
                (70, head),
                (71, extension.clone()),
            ]);
            assert_eq!(collection.summary.logical_bytes_known, 100);
            assert_eq!(collection.summary.data_cluster_bytes_known, 4096);
            if resolves {
                assert_eq!(collection.tree.files[0].path, "\\extended.bin");
                assert_eq!(collection.tree.attributed_files, 1);
                assert!(!collection
                    .tree
                    .limitations
                    .contains(&TreeLimit::FilenameMembershipUnresolved));
            } else {
                assert!(collection.tree.files.is_empty());
                assert_eq!(collection.tree.unattributed_files, 1);
                assert_eq!(
                    collection.tree.unattributed_data_cluster_bytes_known,
                    Some(4096)
                );
                assert!(collection
                    .tree
                    .limitations
                    .contains(&TreeLimit::FilenameMembershipUnresolved));
            }
            collection.validate().unwrap();
        }
    }

    #[test]
    fn top_n_is_bounded_and_parent_subtrees_are_overlapping_rollups() {
        let root = Key(5, 1);
        let mut records = vec![(5, directory(1, root, "."))];
        for i in 0..20_u8 {
            records.push((
                20 + u64::from(i),
                directory(2, root, &format!("dir-{i:02}")),
            ));
            records.push((
                70 + u64::from(i),
                file(
                    &[filename(Key(20 + u64::from(i), 2), "file.bin", 1, 1)],
                    i + 1,
                    u64::from(i + 1) * 10,
                    1,
                ),
            ));
        }
        let collection = scan(records);
        assert_eq!(collection.tree.files.len(), MAX_TREE_ITEMS);
        assert_eq!(collection.tree.directories.len(), MAX_TREE_ITEMS);
        assert_eq!(collection.tree.files[0].path, "\\dir-19\\file.bin");
        assert_eq!(collection.tree.attributed_files, 20);
        assert_eq!(collection.tree.unattributed_files, 0);
        assert_eq!(collection.tree.attributed_logical_bytes_known, 2100);
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::TopDirectoriesTruncated));
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::TopFilesTruncated));
        assert!(serde_json::to_vec(&collection.tree).unwrap().len() < MAX_TREE_RESPONSE_BYTES);
        collection.validate().unwrap();

        let nested = scan([
            (5, directory(1, root, ".")),
            (20, directory(2, root, "parent")),
            (21, directory(3, Key(20, 2), "child")),
            (
                70,
                file(&[filename(Key(21, 3), "inside.bin", 1, 1)], 10, 100, 1),
            ),
            (
                71,
                file(&[filename(Key(20, 2), "beside.bin", 1, 1)], 20, 200, 1),
            ),
        ]);
        assert_eq!(nested.tree.attributed_logical_bytes_known, 300);
        assert_eq!(nested.tree.directories[0].path, "\\parent");
        assert_eq!(nested.tree.directories[0].logical_bytes_known, 300);
        assert_eq!(nested.tree.directories[1].logical_bytes_known, 100);
        assert_eq!(
            nested
                .tree
                .directories
                .iter()
                .map(|item| item.logical_bytes_known)
                .sum::<u64>(),
            400
        );
        nested.validate().unwrap();
    }

    #[test]
    fn index_saturation_preserves_aggregate_and_reserved_parent_capacity() {
        let root = Key(5, 1);
        let mut records = vec![
            (5, directory(1, root, ".")),
            (20, directory(2, root, "parent")),
        ];
        // Resident files keep the run list empty, so only the index meets the small bound.
        for i in 0..1_500_u64 {
            records.push((
                50 + i,
                record(
                    4096,
                    1,
                    0,
                    1,
                    &[
                        filename(Key(20, 2), &format!("file-{i:04}.bin"), 1, 1),
                        resident(DATA, 100, 99),
                    ],
                ),
            ));
        }
        let mut bounds = CollectionBounds::for_budget(Duration::from_secs(30));
        bounds.max_retained_bytes = 64 * 1024;
        let collection = scan_slots(records, bounds, 2048);
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.summary.base_files, 1_500);
        assert_eq!(collection.summary.logical_bytes_known, 150_000);
        assert!(collection.tree.attributed_files > 0);
        assert!(collection.tree.unattributed_files > 0);
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::IndexMemoryLimit));
        assert_eq!(collection.tree.directories[0].path, "\\parent");
        assert!(!collection.tree.directories[0].partial_path);
        assert!(collection.retained_bytes_high_water <= bounds.max_retained_bytes as u64);
        collection.validate().unwrap();
    }

    #[test]
    fn nonresident_attribute_list_is_partial_and_invalid_allocation_is_masked() {
        let root = Key(5, 1);
        let mut list = nonresident(0, 1, 100, 4096, 4096, 0, &[0x11, 1, 40, 0]);
        list[..4].copy_from_slice(&0x20_u32.to_le_bytes());
        list[14..16].copy_from_slice(&9_u16.to_le_bytes());
        let collection = scan([
            (5, directory(1, root, ".")),
            (
                70,
                file(&[filename(root, "partial.bin", 1, 1), list], 10, 100, 1),
            ),
        ]);
        assert_eq!(collection.summary.logical_bytes_known, 100);
        assert_eq!(collection.summary.unknown_data_allocation_files, 1);
        assert_eq!(collection.tree.attributed_logical_bytes_known, 100);
        assert_eq!(collection.tree.files[0].unknown_allocation_files, 1);
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::FilenameMembershipUnresolved));
        collection.validate().unwrap();

        let overlapping = scan([
            (5, directory(1, root, ".")),
            (70, file(&[filename(root, "one.bin", 1, 1)], 10, 100, 1)),
            (71, file(&[filename(root, "two.bin", 1, 1)], 10, 200, 1)),
        ]);
        assert!(!overlapping.allocation_additive);
        assert_eq!(overlapping.tree.attributed_data_cluster_bytes_known, None);
        assert_eq!(overlapping.tree.unattributed_data_cluster_bytes_known, None);
        assert!(overlapping
            .tree
            .files
            .iter()
            .all(|item| item.data_cluster_bytes_known.is_none()));
        assert_eq!(overlapping.tree.attributed_logical_bytes_known, 300);
        overlapping.validate().unwrap();
    }

    #[test]
    fn sparse_and_compressed_tree_amounts_use_data_mapping_not_filename_size_copies() {
        let root = Key(5, 1);
        let mut sparse = nonresident(
            0,
            3,
            10_000,
            12_288,
            4096,
            0x8000,
            &[0x11, 1, 10, 0x01, 2, 0],
        );
        sparse[14..16].copy_from_slice(&99_u16.to_le_bytes());
        let mut compressed =
            nonresident(0, 16, 60_000, 65_536, 8192, 1, &[0x11, 2, 20, 0x01, 14, 0]);
        compressed[14..16].copy_from_slice(&99_u16.to_le_bytes());
        let collection = scan([
            (5, directory(1, root, ".")),
            (
                70,
                record(4096, 1, 0, 1, &[filename(root, "sparse.bin", 1, 1), sparse]),
            ),
            (
                71,
                record(
                    4096,
                    1,
                    0,
                    1,
                    &[filename(root, "compressed.bin", 1, 1), compressed],
                ),
            ),
        ]);
        assert_eq!(collection.tree.attributed_logical_bytes_known, 70_000);
        assert_eq!(
            collection.tree.attributed_data_cluster_bytes_known,
            Some(12_288)
        );
        assert_eq!(collection.tree.files[0].path, "\\compressed.bin");
        assert_eq!(
            collection.tree.files[0].data_cluster_bytes_known,
            Some(8192)
        );
        assert_eq!(
            collection.tree.files[1].data_cluster_bytes_known,
            Some(4096)
        );
        collection.validate().unwrap();
    }

    #[test]
    fn path_length_alias_and_live_path_conflicts_remain_explicit() {
        let root = Key(5, 1);
        let long = "d".repeat(250);
        let collection = scan([
            (5, directory(1, root, ".")),
            (20, directory(2, root, &long)),
            (21, directory(3, Key(20, 2), &long)),
            (
                70,
                file(
                    &[filename(Key(21, 3), "twenty-chars-name.bin", 1, 1)],
                    10,
                    100,
                    1,
                ),
            ),
        ]);
        assert_eq!(collection.tree.unattributed_logical_bytes_known, 100);
        assert!(collection.tree.limitations.contains(&TreeLimit::PathLimit));
        collection.validate().unwrap();

        let aliases: Vec<_> = (0..20_u16)
            .map(|i| filename(root, &format!("alias-{i:02}.bin"), 1, i + 1))
            .collect();
        let aliased = scan([
            (5, directory(1, root, ".")),
            (70, file(&aliases, 10, 100, 20)),
        ]);
        assert_eq!(aliased.tree.files[0].path, "\\alias-00.bin");
        assert_eq!(aliased.tree.attributed_files, 1);
        assert!(aliased
            .tree
            .limitations
            .contains(&TreeLimit::NameLinksTruncated));
        aliased.validate().unwrap();

        let collision = scan([
            (5, directory(1, root, ".")),
            (
                70,
                file(&[filename(root, "replaced.bin", 1, 1)], 10, 100, 1),
            ),
            (
                71,
                file(&[filename(root, "replaced.bin", 1, 1)], 20, 200, 1),
            ),
        ]);
        assert_eq!(collision.tree.attributed_logical_bytes_known, 100);
        assert_eq!(collision.tree.unattributed_logical_bytes_known, 200);
        assert!(collision
            .tree
            .limitations
            .contains(&TreeLimit::PathConflict));
        collision.validate().unwrap();
    }

    #[test]
    fn local_tree_protocol_rejects_extra_fields_unsafe_paths_and_impossible_totals() {
        let root = Key(5, 1);
        let collection = scan([
            (5, directory(1, root, ".")),
            (70, file(&[filename(root, "private.bin", 1, 1)], 10, 100, 1)),
        ]);
        let value = serde_json::to_value(&collection.tree).unwrap();
        let mut extra = value.clone();
        extra["files"][0]["content"] = serde_json::json!("secret");
        assert!(serde_json::from_value::<TreeSummary>(extra).is_err());
        for path in [
            "C:\\Users\\private.bin",
            "\\..\\private.bin",
            "\\private.bin:stream",
            "\\bad\nname",
        ] {
            let mut corrupt = collection.tree.clone();
            corrupt.files[0].path = path.into();
            assert!(corrupt.validate(&collection.summary, true).is_err());
        }
        let mut corrupt = collection.tree.clone();
        corrupt.unattributed_logical_bytes_known = 1;
        assert!(corrupt.validate(&collection.summary, true).is_err());
        let mut corrupt = collection.tree.clone();
        corrupt.files[0].data_cluster_bytes_known = Some(8192);
        assert!(corrupt.validate(&collection.summary, true).is_err());
        let mut corrupt = collection.tree.clone();
        corrupt.files = vec![corrupt.files[0].clone(); MAX_TREE_ITEMS + 1];
        assert!(corrupt.validate(&collection.summary, true).is_err());
        let output = serde_json::to_string(&collection.tree).unwrap();
        assert!(output.contains("private.bin")); // Private IPC retains metadata for local exploration.
        assert!(!format!("{:?}", collection).contains("private.bin"));
    }

    /// Downloads (90) below alex (21) below Users (20) below the root; files above them.
    fn downloads() -> Vec<(u64, Vec<u8>)> {
        let root = Key(5, 1);
        vec![
            (5, directory(1, root, ".")),
            (20, directory(2, root, "Users")),
            (21, directory(3, Key(20, 2), "alex")),
            (90, directory(4, Key(21, 3), "Downloads")),
            (
                95,
                file(&[filename(Key(21, 3), "notes.txt", 1, 1)], 30, 200, 1),
            ),
            (
                100,
                file(&[filename(Key(90, 4), "big.iso", 1, 1)], 10, 300, 1),
            ),
            (
                101,
                file(&[filename(Key(90, 4), "small.txt", 1, 1)], 20, 100, 1),
            ),
        ]
    }

    #[test]
    fn an_early_end_still_lists_the_largest_items_read_and_marks_them_partial() {
        let complete = scan(downloads());
        assert_eq!(complete.end, CollectionEnd::Enumerated);
        assert_eq!(
            complete.tree.files[0].path,
            "\\Users\\alex\\Downloads\\big.iso"
        );
        assert_eq!(complete.tree.directories[0].path, "\\Users");
        assert!(complete
            .tree
            .directories
            .iter()
            .chain(&complete.tree.files)
            .all(|item| !item.partial_path));
        assert!(!complete.tree.limitations.contains(&TreeLimit::PartialPaths));
        complete.validate().unwrap();

        // The walk stops after Downloads (90): alex, Users and the root are not walked.
        let mut bounds = CollectionBounds::for_budget(Duration::from_secs(30));
        bounds.max_records = 4;
        let partial = scan_bound(downloads(), bounds);
        assert_eq!(partial.end, CollectionEnd::RecordLimit);
        assert_eq!(partial.summary.base_files, 3);
        assert_eq!(partial.tree.attributed_files, 0);
        assert_eq!(partial.tree.unattributed_files, 3);
        let files: Vec<_> = partial
            .tree
            .files
            .iter()
            .map(|item| item.path.as_str())
            .collect();
        assert_eq!(
            files,
            [
                "\\Downloads\\big.iso",
                "\\notes.txt",
                "\\Downloads\\small.txt"
            ]
        );
        assert_eq!(partial.tree.directories.len(), 1);
        let folder = &partial.tree.directories[0];
        assert_eq!(folder.path, "\\Downloads");
        assert_eq!((folder.files, folder.logical_bytes_known), (2, 400));
        assert!(partial
            .tree
            .directories
            .iter()
            .chain(&partial.tree.files)
            .all(|item| item.partial_path));
        for limit in [
            TreeLimit::PartialPaths,
            TreeLimit::ParentMissing,
            TreeLimit::CollectionIncomplete,
        ] {
            assert!(partial.tree.limitations.contains(&limit), "{limit:?}");
        }
        partial.validate().unwrap();

        // A partial item can never pass for a complete one or exceed the unattributed share.
        let mut forged = partial.tree.clone();
        forged.files[0].partial_path = false;
        assert!(forged.validate(&partial.summary, true).is_err());
        let mut forged = partial.tree.clone();
        forged.limitations.remove(&TreeLimit::PartialPaths);
        assert!(forged.validate(&partial.summary, true).is_err());
        let mut forged = partial.tree.clone();
        forged.directories[0].logical_bytes_known = 601;
        assert!(forged.validate(&partial.summary, true).is_err());
    }

    /// A synthetic volume generated on demand: the root, a folder tree, then one file per
    /// record with one data run each. Names have realistic lengths (13 to 36 bytes).
    struct Volume {
        dirs: u64,
        files: u64,
        delay: Duration,
    }
    const LETTERS: &str = "abcdefghijklmnopqrstuvwxyz";
    impl Volume {
        fn new(dirs: u64, files: u64) -> Self {
            Self {
                dirs,
                files,
                delay: Duration::ZERO,
            }
        }
        fn slots(&self) -> u64 {
            16 + self.dirs + self.files
        }
        fn geometry() -> Geometry {
            let mut geometry = geometry(1024);
            geometry.volume_clusters = 1 << 24;
            geometry
        }
        fn bytes(&self, n: u64) -> Vec<u8> {
            if n == 5 {
                let mut raw = record(1024, 1, 0, 1, &[filename(Key(5, 1), ".", 1, 0)]);
                raw[22..24].copy_from_slice(&3_u16.to_le_bytes());
                return raw;
            }
            if n < 16 {
                return record(1024, 1, 0, 1, &[resident(DATA, 0, 0)]);
            }
            let i = n - 16;
            if i < self.dirs {
                let parent = if i < 16 { 5 } else { 16 + i / 16 };
                let name = format!("Folder-{i:06}-{}", &LETTERS[..(i % 12) as usize]);
                let mut raw = record(1024, 1, 0, 1, &[filename(Key(parent, 1), &name, 1, 0)]);
                raw[22..24].copy_from_slice(&3_u16.to_le_bytes());
                return raw;
            }
            let j = i - self.dirs;
            let lcn = 1000 + j;
            let mut data = nonresident(
                0,
                1,
                100 + j % 1000,
                4096,
                4096,
                0,
                &[0x31, 1, lcn as u8, (lcn >> 8) as u8, (lcn >> 16) as u8, 0],
            );
            data[14..16].copy_from_slice(&99_u16.to_le_bytes());
            let name = format!("file-{j:07}-{}.bin", &LETTERS[..(j % 20) as usize]);
            let parent = Key(16 + j % self.dirs, 1);
            record(1024, 1, 0, 1, &[filename(parent, &name, 1, 1), data])
        }
    }
    impl Source for Volume {
        fn snapshot(&mut self) -> Result<VolumeSnapshot, CollectError> {
            Ok(VolumeSnapshot {
                geometry: Self::geometry(),
                record_slots: self.slots(),
                journal: None,
            })
        }
        fn record_at_or_below(&mut self, ordinal: u64) -> Result<RecordData, CollectError> {
            std::thread::sleep(self.delay);
            Ok(RecordData {
                number: ordinal,
                returned_sequence: None,
                bytes: self.bytes(ordinal),
            })
        }
    }

    #[test]
    fn a_timed_out_walk_keeps_time_to_list_what_it_read() {
        let mut volume = Volume::new(4, 10_000);
        volume.delay = Duration::from_millis(3);
        let mut bounds = CollectionBounds::for_budget(Duration::from_secs(30));
        bounds.duration = Duration::from_millis(600);
        let collection = collect(&mut volume, bounds, &mut || Ok(false)).unwrap();
        assert_eq!(collection.end, CollectionEnd::TimedOut);
        assert!(collection.records_examined > 0);
        assert!(collection.record_slots_not_walked > 0);
        assert!(!collection.tree.files.is_empty());
        assert!(collection.tree.files.iter().all(|item| item.partial_path));
        assert!(!collection
            .tree
            .limitations
            .contains(&TreeLimit::Interrupted));
        assert!(collection
            .summary
            .limitations
            .contains(&Limit::DurationLimit));
        collection.validate().unwrap();
    }

    #[test]
    fn memory_accounting_covers_the_real_heap_of_index_nodes() {
        let volume = Volume::new(2_000, 20_000);
        let parsed: Vec<_> = (5..16 + 22_000)
            .filter(|&n| n == 5 || n >= 16)
            .map(|n| {
                let bytes = volume.bytes(n);
                let record = Record {
                    number: n,
                    bytes: &bytes,
                };
                parse(&record, Volume::geometry(), &mut 0).unwrap().unwrap()
            })
            .collect();
        let (dirs, files): (Vec<_>, Vec<_>) = parsed.into_iter().partition(|p| p.directory);
        let delta = Summary {
            base_files: 1,
            data_streams: 1,
            logical_bytes_known: 100,
            data_cluster_bytes_known: 4096,
            ..Summary::default()
        };
        let mut tree = TreeBuilder::new(collection::MAX_RETAINED_BYTES);
        let start = tree.retained_bytes();
        let (_, directories) = measure(|| {
            for parsed in &dirs {
                let metadata = Metadata::from_group(parsed.key, std::slice::from_ref(parsed));
                tree.record(metadata, &Summary::default(), usize::MAX);
            }
        });
        let after_dirs = tree.retained_bytes();
        let (_, nodes) = measure(|| {
            for parsed in &files {
                let metadata = Metadata::from_group(parsed.key, std::slice::from_ref(parsed));
                tree.record(metadata, &delta, usize::MAX);
            }
        });
        let accounted = tree.retained_bytes();
        // The root (record 5) is an anchor, not a node.
        let (dir_count, file_count) = (dirs.len() - 1, files.len());
        assert_eq!(tree.dirs.len(), dir_count);
        assert_eq!(tree.files.len(), file_count);
        // Index storage is exact: what is allocated is what is accounted, apart from the
        // finish reserve that `finish` allocates later.
        let storage = |bytes: usize, dirs: usize, files: usize| {
            bytes - dirs * DIR_FINISH_BYTES - files * FILE_FINISH_BYTES
        };
        assert!(directories.retained <= storage(after_dirs - start, dir_count, 0));
        assert!(nodes.retained <= storage(accounted - after_dirs, 0, file_count));
        let summary = Summary {
            base_files: file_count,
            data_streams: file_count,
            logical_bytes_known: 100 * file_count as u64,
            data_cluster_bytes_known: 4096 * file_count as u64,
            ..Summary::default()
        };
        let (result, finish) = measure(|| tree.finish(&summary, true, false, &mut || Ok(None)));
        let (inventory, ending) = result.unwrap();
        assert!(ending.is_none());
        assert_eq!(inventory.attributed_files, file_count as u64);
        // `finish` stays within its reserve; the bounded response is its output.
        let reserve = dir_count * DIR_FINISH_BYTES + file_count * FILE_FINISH_BYTES + FIXED_BYTES;
        assert!(finish.peak <= reserve + MAX_TREE_RESPONSE_BYTES);
        eprintln!(
            "index node bytes: directory real {:.1} accounted {:.1}; file real {:.1} accounted {:.1}; finish peak {} for reserve {}",
            directories.retained as f64 / dir_count as f64,
            (after_dirs - start) as f64 / dir_count as f64,
            nodes.retained as f64 / file_count as f64,
            (accounted - after_dirs) as f64 / file_count as f64,
            finish.peak,
            reserve,
        );
    }

    #[test]
    fn memory_accounting_bounds_a_whole_collection_and_scales_to_large_volumes() {
        let mut volume = Volume::new(4_000, 40_000);
        let records = volume.slots();
        let (collection, usage) = measure(|| {
            collect(
                &mut volume,
                CollectionBounds::for_budget(Duration::from_secs(120)),
                &mut || Ok(false),
            )
            .unwrap()
        });
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.tree.attributed_files, 40_000);
        assert!(collection.allocation_additive);
        // The real heap stays within the accounted high water plus one record's transient
        // buffers and the returned response.
        let transient = 64 * 1024;
        assert!(
            usage.peak as u64 <= collection.retained_bytes_high_water + transient,
            "peak {} high water {}",
            usage.peak,
            collection.retained_bytes_high_water
        );
        // At this size chunk and arena slack weigh more per record than on a large volume
        // (1.2 M generated records measure 98 accounted bytes each, 112 MiB in all), so this
        // projection overestimates; it must still leave 40 % of the bound free.
        let per_record = collection.retained_bytes_high_water as f64 / records as f64;
        let projected = per_record * 1_200_000.0;
        eprintln!(
            "collection of {records} records: real peak {} ({:.1} B/record), accounted high water {} ({:.1} B/record); 1.2 M records projected {:.1} MiB of {} MiB",
            usage.peak,
            usage.peak as f64 / records as f64,
            collection.retained_bytes_high_water,
            per_record,
            projected / 1048576.0,
            collection::MAX_RETAINED_BYTES >> 20,
        );
        assert!(projected <= collection::MAX_RETAINED_BYTES as f64 * 0.6);
    }

    /// Run with `cargo test --release -p pcagent-inspect -- --ignored --nocapture`.
    #[test]
    #[ignore = "1.2 million generated records: run in release to measure"]
    fn memory_peak_of_a_1_2_million_record_volume() {
        let mut volume = Volume::new(150_000, 1_050_000 - 16);
        let records = volume.slots();
        let started = std::time::Instant::now();
        let (collection, usage) = measure(|| {
            collect(
                &mut volume,
                CollectionBounds::for_budget(Duration::from_secs(120)),
                &mut || Ok(false),
            )
            .unwrap()
        });
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.tree.attributed_files, 1_050_000 - 16);
        eprintln!(
            "{records} records in {:?}: real peak {:.1} MiB, accounted high water {:.1} MiB, index {:.1} MiB, collector {} ms",
            started.elapsed(),
            usage.peak as f64 / 1048576.0,
            collection.retained_bytes_high_water as f64 / 1048576.0,
            collection.tree.retained_bytes as f64 / 1048576.0,
            collection.duration_ms,
        );
    }
}
