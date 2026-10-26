//! Bounded local filename/parent index. Paths are private helper data, never cloud evidence.
use super::*;
use std::fmt;
#[cfg(test)]
use std::time::Duration;

pub const MAX_TREE_NODES: usize = 250_000;
pub const MAX_TREE_RETAINED_BYTES: usize = 32 * 1024 * 1024;
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
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSummary {
    /// Largest attributed subtrees; a parent and its child overlap and must not be summed.
    pub directories: Vec<TreeItem>,
    /// Each attributed file contributes at most once, regardless of its filename aliases.
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
            || self.retained_bytes < self.indexed_nodes.saturating_mul(384)
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
                if item.path.len() > MAX_TREE_PATH_BYTES
                    || !item.path.starts_with('\\')
                    || !item.path.split('\\').skip(1).all(component_ok)
                    || !paths.insert(&item.path)
                    || item.files == 0
                    || item.files > self.attributed_files
                    || item.logical_bytes_known > self.attributed_logical_bytes_known
                    || item.unknown_logical_files > item.files
                    || item.unknown_allocation_files > item.files
                    || item.hard_linked_files > item.files
                    || item.unknown_logical_files > summary.unknown_logical_files as u64
                    || item.unknown_allocation_files > summary.unknown_data_allocation_files as u64
                    || item.hard_linked_files > summary.hard_linked_files as u64
                    || item.data_cluster_bytes_known.is_some() != allocation_additive
                    || item.data_cluster_bytes_known.is_some_and(|bytes| {
                        self.attributed_data_cluster_bytes_known
                            .is_none_or(|total| bytes > total)
                    })
                {
                    return Err(ParseError::InvalidAttribute);
                }
            }
        }
        let mut returned_files = Numbers::default();
        for item in &self.files {
            if item.files != 1 {
                return Err(ParseError::InvalidAttribute);
            }
            returned_files.add(&Numbers::from_item(item))?;
        }
        if returned_files.files > self.attributed_files
            || returned_files.logical > self.attributed_logical_bytes_known
            || self
                .attributed_data_cluster_bytes_known
                .is_some_and(|bytes| returned_files.allocated > bytes)
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

pub(super) fn parsed_bytes(parsed: &Parsed) -> usize {
    parsed.names.capacity() * std::mem::size_of::<NameLink>()
        + parsed
            .names
            .iter()
            .map(|link| link.name.capacity())
            .sum::<usize>()
}

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
            for identity in record
                .attributes
                .iter()
                .filter(|id| id.key.kind == FILE_NAME)
            {
                observed.insert(*identity);
            }
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
    fn bytes(&self) -> usize {
        // Covers BTreeMap/Vec capacity and a reserved per-directory rollup entry.
        let overhead = if self.directory { 640 } else { 384 };
        overhead
            + self.names.capacity() * std::mem::size_of::<NameLink>()
            + self
                .names
                .iter()
                .map(|link| link.name.capacity())
                .sum::<usize>()
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
    fn from_summary(summary: &Summary) -> Self {
        Self {
            files: summary.base_files as u64,
            logical: summary.logical_bytes_known,
            allocated: summary.data_cluster_bytes_known,
            unknown_logical: summary.unknown_logical_files as u64,
            unknown_allocation: summary.unknown_data_allocation_files as u64,
            hard_links: summary.hard_linked_files as u64,
        }
    }
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
    fn add(&mut self, other: &Self) -> Result<(), ParseError> {
        self.files = add(self.files, other.files)?;
        self.logical = add(self.logical, other.logical)?;
        self.allocated = add(self.allocated, other.allocated)?;
        self.unknown_logical = add(self.unknown_logical, other.unknown_logical)?;
        self.unknown_allocation = add(self.unknown_allocation, other.unknown_allocation)?;
        self.hard_links = add(self.hard_links, other.hard_links)?;
        Ok(())
    }
    fn item(self, path: String, additive: bool) -> TreeItem {
        TreeItem {
            path,
            files: self.files,
            logical_bytes_known: self.logical,
            data_cluster_bytes_known: additive.then_some(self.allocated),
            unknown_logical_files: self.unknown_logical,
            unknown_allocation_files: self.unknown_allocation,
            hard_linked_files: self.hard_links,
        }
    }
}

struct Node {
    metadata: Metadata,
    numbers: Numbers,
}
pub(super) struct TreeBuilder {
    root: Option<Key>,
    directories: BTreeMap<Key, Node>,
    files: BTreeMap<Key, Node>,
    directory_bytes: usize,
    file_bytes: usize,
    capacity: usize,
    limits: BTreeSet<TreeLimit>,
}
impl TreeBuilder {
    pub(super) fn new(global_bound: usize) -> Self {
        Self {
            root: None,
            directories: BTreeMap::new(),
            files: BTreeMap::new(),
            directory_bytes: 0,
            file_bytes: 0,
            capacity: MAX_TREE_RETAINED_BYTES
                .min(global_bound / 3)
                .saturating_sub(32),
            limits: BTreeSet::new(),
        }
    }
    pub(super) fn retained_bytes(&self) -> usize {
        // Root identity has a permanent small reservation, even before its late ordinal is read.
        self.directory_bytes + self.file_bytes + 32
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
        let key = metadata.key;
        let bytes = metadata.bytes();
        let nodes = self.directories.len() + self.files.len();
        let (map, used) = if metadata.directory {
            (&mut self.directories, &mut self.directory_bytes)
        } else {
            (&mut self.files, &mut self.file_bytes)
        };
        let previous = map.get(&key).map_or(0, |node| node.metadata.bytes());
        let growth = bytes.saturating_sub(previous);
        if *used - previous + bytes > self.capacity / 2 || growth > available {
            self.limits.insert(TreeLimit::IndexMemoryLimit);
            return;
        }
        if !map.contains_key(&key) && nodes >= MAX_TREE_NODES {
            self.limits.insert(TreeLimit::IndexNodeLimit);
            return;
        }
        *used = *used - previous + bytes;
        map.insert(
            key,
            Node {
                metadata,
                numbers: Numbers::from_summary(delta),
            },
        );
    }

    pub(super) fn finish(
        self,
        summary: &Summary,
        additive: bool,
        incomplete: bool,
        stop: &mut dyn FnMut() -> Result<Option<CollectionEnd>, CollectError>,
    ) -> Result<(TreeSummary, Option<CollectionEnd>), CollectError> {
        let mut out = TreeSummary {
            indexed_nodes: (self.directories.len() + self.files.len()) as u64,
            indexed_name_links: self
                .directories
                .values()
                .chain(self.files.values())
                .map(|node| node.metadata.names.len() as u64)
                .sum(),
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
        let mut totals = Numbers::default();
        let mut directory_totals = BTreeMap::<Key, Numbers>::new();
        let mut attributed_paths = BTreeSet::new();
        let mut ending = None;
        for node in self.files.values() {
            if let Some(end) = stop()? {
                ending = Some(end);
                break;
            }
            let mut selected = None;
            for link in &node.metadata.names {
                match directory_path(link.parent, &self.directories, self.root) {
                    Ok((mut path, ancestors)) => {
                        if path != "\\" {
                            path.push('\\');
                        }
                        path.push_str(&link.name);
                        if path.len() <= MAX_TREE_PATH_BYTES {
                            selected = Some((path, ancestors));
                            break;
                        }
                        out.limitations.insert(TreeLimit::PathLimit);
                    }
                    Err(limit) => {
                        out.limitations.insert(limit);
                    }
                }
            }
            let Some((path, ancestors)) = selected else {
                continue;
            };
            // A rename/replacement during this non-atomic scan can reuse a metadata path.
            // Retain one observed attribution and leave later conflicting files unattributed.
            let path_key: [u8; 32] = Sha256::digest(path.as_bytes()).into();
            if !attributed_paths.insert(path_key) {
                out.limitations.insert(TreeLimit::PathConflict);
                continue;
            }
            totals.add(&node.numbers)?;
            for ancestor in ancestors {
                if ancestor.0 != 5 {
                    directory_totals
                        .entry(ancestor)
                        .or_default()
                        .add(&node.numbers)?;
                }
            }
            keep_top(&mut out.files, node.numbers.item(path, additive));
            if totals.files > MAX_TREE_ITEMS as u64 {
                out.limitations.insert(TreeLimit::TopFilesTruncated);
            }
        }
        if ending.is_none() {
            for (key, numbers) in directory_totals {
                if let Some(end) = stop()? {
                    ending = Some(end);
                    break;
                }
                match directory_path(key, &self.directories, self.root) {
                    Ok((path, _)) => {
                        if out.directories.iter().any(|item| item.path == path) {
                            out.limitations.insert(TreeLimit::PathConflict);
                            continue;
                        }
                        if out.directories.len() == MAX_TREE_ITEMS {
                            out.limitations.insert(TreeLimit::TopDirectoriesTruncated);
                        }
                        keep_top(&mut out.directories, numbers.item(path, additive));
                    }
                    Err(limit) => {
                        out.limitations.insert(limit);
                    }
                }
            }
        }
        if ending.is_some() {
            out.limitations.insert(TreeLimit::Interrupted);
        }
        out.attributed_files = totals.files;
        out.unattributed_files = (summary.base_files as u64)
            .checked_sub(totals.files)
            .ok_or(ParseError::ArithmeticOverflow)?;
        out.attributed_logical_bytes_known = totals.logical;
        out.unattributed_logical_bytes_known = summary
            .logical_bytes_known
            .checked_sub(totals.logical)
            .ok_or(ParseError::ArithmeticOverflow)?;
        out.attributed_data_cluster_bytes_known = additive.then_some(totals.allocated);
        out.unattributed_data_cluster_bytes_known = if additive {
            Some(
                summary
                    .data_cluster_bytes_known
                    .checked_sub(totals.allocated)
                    .ok_or(ParseError::ArithmeticOverflow)?,
            )
        } else {
            None
        };
        out.validate(summary, additive)?;
        Ok((out, ending))
    }
}

fn keep_top(items: &mut Vec<TreeItem>, item: TreeItem) {
    items.push(item);
    items.sort_by(|left, right| {
        right
            .data_cluster_bytes_known
            .cmp(&left.data_cluster_bytes_known)
            .then_with(|| right.logical_bytes_known.cmp(&left.logical_bytes_known))
            .then_with(|| left.path.cmp(&right.path))
    });
    if items.len() > MAX_TREE_ITEMS {
        items.pop();
    }
}

fn directory_path(
    key: Key,
    directories: &BTreeMap<Key, Node>,
    root: Option<Key>,
) -> Result<(String, Vec<Key>), TreeLimit> {
    let mut current = key;
    let mut seen = BTreeSet::new();
    let mut chain = Vec::new();
    let mut names = Vec::new();
    loop {
        if chain.len() >= MAX_DEPTH {
            return Err(TreeLimit::DepthLimit);
        }
        if !seen.insert(current) {
            return Err(TreeLimit::ParentCycle);
        }
        if current.0 == 5 {
            if root == Some(current) {
                chain.push(current);
                break;
            }
            return Err(if root.is_some() {
                TreeLimit::ParentSequenceChanged
            } else {
                TreeLimit::ParentMissing
            });
        }
        let Some(node) = directories.get(&current) else {
            return Err(
                if directories
                    .range(Key(current.0, 0)..=Key(current.0, u16::MAX))
                    .next()
                    .is_some()
                {
                    TreeLimit::ParentSequenceChanged
                } else {
                    TreeLimit::ParentMissing
                },
            );
        };
        if node.metadata.unsupported_parent {
            return Err(TreeLimit::ParentMetadataIncomplete);
        }
        chain.push(current);
        let link = node
            .metadata
            .names
            .first()
            .ok_or(TreeLimit::ParentMetadataIncomplete)?;
        names.push(link.name.as_str());
        current = link.parent;
    }
    let mut path = String::from("\\");
    for name in names.into_iter().rev() {
        if path.len() > 1 {
            path.push('\\');
        }
        path.push_str(name);
        if path.len() > MAX_TREE_PATH_BYTES {
            return Err(TreeLimit::PathLimit);
        }
    }
    Ok((path, chain))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{geometry, listed_data, nonresident, record, resident};
    use super::*;

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
            Ok(VolumeSnapshot {
                geometry: geometry(self.size),
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
            slots: 128,
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
        for i in 0..30_u8 {
            records.push((
                50 + u64::from(i),
                file(
                    &[filename(Key(20, 2), &format!("file-{i:02}.bin"), 1, 1)],
                    i + 1,
                    100,
                    1,
                ),
            ));
        }
        let mut bounds = CollectionBounds::for_budget(Duration::from_secs(30));
        bounds.max_retained_bytes = MAX_RECORD_BYTES;
        let collection = scan_bound(records, bounds);
        assert_eq!(collection.end, CollectionEnd::Enumerated);
        assert_eq!(collection.summary.base_files, 30);
        assert_eq!(collection.summary.logical_bytes_known, 3000);
        assert_eq!(collection.summary.data_cluster_bytes_known, 30 * 4096);
        assert!(collection.tree.attributed_files > 0);
        assert!(collection.tree.unattributed_files > 0);
        assert!(collection
            .tree
            .limitations
            .contains(&TreeLimit::IndexMemoryLimit));
        assert_eq!(collection.tree.directories[0].path, "\\parent");
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
}
