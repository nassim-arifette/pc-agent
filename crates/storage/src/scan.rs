//! Classic progressive walker: single-threaded, depth-first, iterative, metadata only.
use crate::{
    index::{
        Entry, StorageIndex, F_ALIAS, F_ALLOC_UNKNOWN, F_COMPRESSED, F_DENIED, F_INCOMPLETE,
        F_SPARSE,
    },
    platform, CancelToken, ScanBackend, ScanOptions, ScanProgress, ScanScope, Truncation,
};
use pcagent_shared::unix_ms;
use serde::Serialize;
use std::{collections::HashSet, ffi::OsString, io::ErrorKind, time::Instant};

pub const ATTR_DIRECTORY: u32 = 0x10;
pub const ATTR_REPARSE_POINT: u32 = 0x400;
pub const ATTR_SPARSE_FILE: u32 = 0x200;
pub const ATTR_COMPRESSED: u32 = 0x800;
pub const ATTR_OFFLINE: u32 = 0x1000;
pub const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
pub const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributeClass {
    Dir,
    File,
    /// Cloud/offline placeholder: must not be opened (opening may hydrate it).
    Placeholder,
    /// Symlink, junction, mount point or other reparse point: never followed.
    Reparse,
    Special,
}

/// Windows directory-enumeration attributes to class. Placeholder takes precedence over reparse.
pub fn classify_attributes(attributes: u32) -> AttributeClass {
    if attributes & (ATTR_RECALL_ON_DATA_ACCESS | ATTR_RECALL_ON_OPEN | ATTR_OFFLINE) != 0 {
        AttributeClass::Placeholder
    } else if attributes & ATTR_REPARSE_POINT != 0 {
        AttributeClass::Reparse
    } else if attributes & ATTR_DIRECTORY != 0 {
        AttributeClass::Dir
    } else {
        AttributeClass::File
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileFacts {
    pub logical: u64,
    pub allocated: Option<u64>,
    /// (volume serial or device, file ID or inode).
    pub key: Option<(u64, u128)>,
    pub links: Option<u32>,
    pub modified_ms: Option<u64>,
    pub sparse: bool,
    pub compressed: bool,
    pub other_volume: bool,
}

pub(crate) struct EntryFacts {
    pub class: AttributeClass,
    pub logical: u64,
    pub modified_ms: Option<u64>,
    pub facts: Option<FileFacts>,
    pub other_volume: bool,
}

struct Frame {
    index: usize,
    directory: platform::Directory,
}

pub struct ClassicBackend;
impl ScanBackend for ClassicBackend {
    fn name(&self) -> &'static str {
        "classic"
    }
    fn scan(
        &self,
        scope: &ScanScope,
        options: &ScanOptions,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(&ScanProgress),
    ) -> StorageIndex {
        Walk::new(scope, options, cancel).run(progress)
    }
}

const ENTRY_OVERHEAD: u64 =
    (std::mem::size_of::<Entry>() + 2 * std::mem::size_of::<usize>() + 16) as u64;
const LINK_OVERHEAD: u64 = 48;

struct Walk<'a> {
    scope: &'a ScanScope,
    options: &'a ScanOptions,
    cancel: &'a CancelToken,
    index: StorageIndex,
    links: HashSet<(u64, u128)>,
    clock: Instant,
    started_ms: u64,
    walk_bytes: u64,
    progress: ScanProgress,
}

enum Stop {
    Cancelled,
    Truncated(Truncation),
}

impl<'a> Walk<'a> {
    fn new(scope: &'a ScanScope, options: &'a ScanOptions, cancel: &'a CancelToken) -> Self {
        let started_ms = unix_ms();
        Self {
            scope,
            options,
            cancel,
            index: StorageIndex::new(scope, "classic", started_ms),
            links: HashSet::new(),
            clock: Instant::now(),
            started_ms,
            walk_bytes: 0,
            progress: ScanProgress::default(),
        }
    }

    fn check(&self) -> Result<(), Stop> {
        if self.cancel.is_cancelled() {
            return Err(Stop::Cancelled);
        }
        if self.clock.elapsed() > self.options.max_duration {
            return Err(Stop::Truncated(Truncation::Duration));
        }
        if self.index.entries.len() >= self.options.max_entries {
            return Err(Stop::Truncated(Truncation::EntryLimit));
        }
        if self
            .progress
            .index_bytes_estimate
            .saturating_add(self.walk_bytes)
            > self.options.index_budget_bytes
        {
            return Err(Stop::Truncated(Truncation::MemoryBudget));
        }
        Ok(())
    }

    fn push(&mut self, entry: Entry) -> usize {
        let extra = if entry.links.is_some_and(|n| n > 1) {
            LINK_OVERHEAD
        } else {
            0
        };
        self.progress.index_bytes_estimate = self
            .progress
            .index_bytes_estimate
            .saturating_add(ENTRY_OVERHEAD + entry.name.len() as u64 + extra);
        self.progress.entries_indexed += 1;
        self.progress.logical_bytes_seen = self
            .progress
            .logical_bytes_seen
            .saturating_add(entry.logical);
        self.index.entries.push(entry);
        self.index.entries.len() - 1
    }

    fn deny(&mut self, dir: usize) {
        self.index.entries[dir].flags |= F_DENIED;
        self.index.coverage.denied_dirs += 1;
        if self.index.denied.len() < self.options.max_denied_listed {
            self.index.denied.push(dir);
        } else {
            self.index.denied_truncated = true;
        }
    }

    fn run(mut self, progress: &mut dyn FnMut(&ScanProgress)) -> StorageIndex {
        self.push(Entry::dir(usize::MAX, OsString::new()));
        self.index.coverage.dirs += 1;
        let mut stack = Vec::new();
        match platform::Directory::open_root(self.scope.root(), Some(self.scope.identity)) {
            Ok(directory) => stack.push(Frame {
                index: 0,
                directory,
            }),
            Err(e) => self.directory_error(0, e),
        }
        let stop = loop {
            self.walk_bytes = stack.iter().map(|f| f.directory.estimated_bytes()).sum();
            if stack.is_empty() {
                break None;
            }
            if let Err(stop) = self.check() {
                break Some(stop);
            }
            let frame = stack.last_mut().expect("active frame");
            let name = match frame.directory.next() {
                Some(Ok(name)) => name,
                Some(Err(e)) => {
                    let dir = frame.index;
                    self.directory_error(dir, e);
                    stack.pop();
                    continue;
                }
                None => {
                    self.progress.dirs_enumerated += 1;
                    stack.pop();
                    continue;
                }
            };
            // Names are streamed one at a time. Check the deadline/cancellation before the
            // metadata access, including when almost every entry is skipped.
            self.progress.elapsed_ms = self.clock.elapsed().as_millis() as u64;
            progress(&self.progress);
            if let Err(stop) = self.check() {
                break Some(stop);
            }
            let frame = stack.last().expect("active frame");
            if let Some(child) = self.entry(frame, name) {
                stack.push(child);
            }
        };
        for frame in &stack {
            self.index.entries[frame.index].flags |= F_INCOMPLETE;
        }
        match stop {
            Some(Stop::Cancelled) => self.index.coverage.cancelled = true,
            Some(Stop::Truncated(t)) => self.index.coverage.truncated = Some(t),
            None => {}
        }
        self.index.finalize(self.clock.elapsed().as_millis() as u64);
        self.index
    }

    fn directory_error(&mut self, dir: usize, e: std::io::Error) {
        if e.kind() == ErrorKind::PermissionDenied {
            self.deny(dir);
        } else {
            self.index.entries[dir].flags |= F_INCOMPLETE;
            if e.kind() == ErrorKind::NotFound && dir != 0 {
                self.index.coverage.vanished += 1;
            } else {
                self.index.coverage.errors += 1;
            }
        }
        if dir == 0 {
            self.index.root_error = Some(e.kind());
        }
    }

    fn entry(&mut self, parent: &Frame, child: platform::Child) -> Option<Frame> {
        let dir = parent.index;
        let name = platform::child_name(&child);
        let meta = match parent.directory.inspect(&child, self.scope.volume) {
            Ok(meta) => meta,
            Err(e) => {
                match e.kind() {
                    ErrorKind::NotFound => self.index.coverage.vanished += 1,
                    ErrorKind::PermissionDenied => self.index.coverage.denied_files += 1,
                    _ => self.index.coverage.errors += 1,
                }
                self.index.entries[dir].flags |= F_INCOMPLETE;
                return None;
            }
        };
        match meta.class {
            AttributeClass::Reparse => self.index.coverage.reparse_skipped += 1,
            AttributeClass::Special => self.index.coverage.special_skipped += 1,
            AttributeClass::Placeholder => {
                self.index.coverage.placeholders += 1;
                self.index.coverage.placeholder_logical_bytes = self
                    .index
                    .coverage
                    .placeholder_logical_bytes
                    .saturating_add(meta.logical);
            }
            AttributeClass::Dir => {
                if meta.other_volume {
                    self.index.coverage.other_volume_skipped += 1;
                    return None;
                }
                self.index.coverage.dirs += 1;
                let mut entry = Entry::dir(dir, name.clone());
                entry.modified_ms = meta.modified_ms;
                let index = self.push(entry);
                match parent.directory.child(&name, self.scope.volume) {
                    Ok(directory) => return Some(Frame { index, directory }),
                    Err(e) => self.directory_error(index, e),
                }
            }
            AttributeClass::File => self.file(dir, name, meta),
        }
        None
    }

    fn file(&mut self, dir: usize, name: OsString, meta: EntryFacts) {
        let coverage = &mut self.index.coverage;
        let mut entry = Entry::file(dir, name, meta.logical);
        entry.modified_ms = meta.modified_ms;
        if let Some(f) = meta.facts {
            if f.other_volume {
                coverage.other_volume_skipped += 1;
                return;
            }
            entry.logical = f.logical;
            entry.modified_ms = f.modified_ms.or(entry.modified_ms);
            entry.links = f.links;
            match f.allocated {
                Some(a) => entry.allocated = a,
                None => entry.flags |= F_ALLOC_UNKNOWN,
            }
            if f.sparse {
                entry.flags |= F_SPARSE;
            }
            if f.compressed {
                entry.flags |= F_COMPRESSED;
            }
            if let (Some(key), Some(n)) = (f.key, f.links) {
                if n > 1 && !self.links.insert(key) {
                    entry.flags |= F_ALIAS;
                    coverage.hard_link_aliases += 1;
                }
            }
        } else {
            entry.flags |= F_ALLOC_UNKNOWN;
        }
        if entry.modified_ms.is_some_and(|m| m > self.started_ms) {
            coverage.changed_during_scan += 1;
        }
        coverage.files += 1;
        self.push(entry);
    }
}
