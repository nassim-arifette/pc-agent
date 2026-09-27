//! Bounded local content comparison, independent of metadata scans and cloud permissions.
use crate::{
    index::Entry, platform::Directory, scan::FileFacts, CancelToken, ScanScope, StorageIndex,
};
use pcagent_shared::Status;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{self, Read},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy)]
pub struct ContentOptions {
    pub max_files: usize,
    pub max_bytes: u64,
    pub max_duration: Duration,
}
impl Default for ContentOptions {
    fn default() -> Self {
        Self {
            max_files: 256,
            max_bytes: 256 << 20,
            max_duration: Duration::from_secs(15),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentGroup {
    pub group_ref: String,
    pub item_refs: Vec<String>,
    pub logical_bytes_each: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContentComparison {
    pub scope_id: String,
    pub scan_id: String,
    pub status: Status,
    pub method: &'static str,
    pub indexed_files: usize,
    pub considered_files: usize,
    pub checked_refs: Vec<String>,
    pub unavailable_files: usize,
    pub bytes_read: u64,
    pub groups: Vec<ContentGroup>,
    pub limitations: Vec<String>,
}

impl ContentComparison {
    pub fn group_for(&self, item_ref: &str) -> Option<&ContentGroup> {
        self.groups
            .iter()
            .find(|g| g.item_refs.iter().any(|r| r == item_ref))
    }
}

struct Budget<'a> {
    options: &'a ContentOptions,
    cancel: &'a CancelToken,
    started: Instant,
    read: u64,
}
impl Budget<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancel.is_cancelled() || self.started.elapsed() >= self.options.max_duration {
            Err(io::Error::from(io::ErrorKind::Interrupted))
        } else {
            Ok(())
        }
    }
}

fn matches(entry: &Entry, facts: &FileFacts) -> bool {
    entry.identity.is_some()
        && entry.identity == facts.key
        && entry.logical == facts.logical
        && entry.modified_ms.is_some()
        && entry.modified_ms == facts.modified_ms
        && facts.links == Some(1)
        && !facts.other_volume
}

fn fingerprint(
    scope: &ScanScope,
    index: &StorageIndex,
    i: usize,
    budget: &mut Budget<'_>,
) -> io::Result<[u8; 32]> {
    budget.check()?;
    let entry = &index.entries[i];
    if entry.links != Some(1)
        || entry.logical > budget.options.max_bytes.saturating_sub(budget.read)
    {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    // All traversal is handle-relative and no-follow. Retain ancestors for the entire read;
    // never reopen a joined path. The authorized root and scanned file identities must match.
    let mut ancestors = Vec::new();
    let mut parent = entry.parent;
    while parent != 0 {
        budget.check()?;
        ancestors.push(parent);
        parent = index.entries[parent].parent;
    }
    let mut pins = vec![Directory::open_root(&scope.root, Some(scope.identity))?];
    for parent in ancestors.into_iter().rev() {
        budget.check()?;
        let pin = pins
            .last()
            .expect("root pin")
            .child(&index.entries[parent].name, scope.volume)?;
        pins.push(pin);
    }
    let mut file = pins
        .last_mut()
        .expect("root pin")
        .open_content(&entry.name, &mut || budget.check())?;
    if !matches(entry, &file.facts(scope.volume)?) {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = entry.logical;
    while remaining > 0 {
        budget.check()?;
        let length = remaining.min(buffer.len() as u64) as usize;
        let count = file.read(&mut buffer[..length])?;
        budget.read += count as u64;
        if count == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        hash.update(&buffer[..count]);
        remaining -= count as u64;
    }
    budget.check()?;
    if !matches(entry, &file.facts(scope.volume)?) {
        return Err(io::Error::from(io::ErrorKind::InvalidData));
    }
    Ok(hash.finalize().into())
}

pub(crate) fn compare(
    scope: &ScanScope,
    index: &StorageIndex,
    options: &ContentOptions,
    cancel: &CancelToken,
) -> ContentComparison {
    let mut result = ContentComparison {
        scope_id: scope.id().into(), scan_id: index.scan_id().into(), status: Status::Ok,
        method: "full_file_sha256", indexed_files: index.files_rank.len(), considered_files: 0,
        checked_refs: Vec::new(), unavailable_files: 0, bytes_read: 0, groups: Vec::new(),
        limitations: vec![
            "Only same-size files among the largest selected entries in this scope are compared; unchecked files are not known to be unique.".into(),
            "Matches mean equal full-file SHA-256 fingerprints at observation time, not permission to delete. No antivirus, corruption check or folder comparison is performed.".into(),
            "Hard links, redirected or unavailable files are excluded. Reads can update access times or warm filesystem caches. Individual OS calls cannot be interrupted by the time budget.".into(),
            "Files can change after comparison; another observed copy is not a verified backup or a restoration guarantee.".into(),
        ],
    };
    let mut sizes: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    // Hard cap also bounds result size and working memory when called outside the CLI.
    for &i in index.files_rank.iter().take(options.max_files.min(1024)) {
        result.considered_files += 1;
        sizes.entry(index.entries[i].logical).or_default().push(i);
    }
    let mut budget = Budget {
        options,
        cancel,
        started: Instant::now(),
        read: 0,
    };
    for (size, files) in sizes.into_iter().rev().filter(|(_, f)| f.len() > 1) {
        let mut hashes: BTreeMap<[u8; 32], Vec<String>> = BTreeMap::new();
        for i in files {
            match fingerprint(scope, index, i, &mut budget) {
                Ok(hash) => {
                    let reference = index.item_ref(i);
                    result.checked_refs.push(reference.clone());
                    hashes.entry(hash).or_default().push(reference);
                }
                Err(_) => result.unavailable_files += 1,
            }
        }
        for refs in hashes.into_values().filter(|refs| refs.len() > 1) {
            result.groups.push(ContentGroup {
                group_ref: format!("{}:copy-{}", index.scan_id(), result.groups.len() + 1),
                item_refs: refs,
                logical_bytes_each: size,
            });
        }
    }
    result.bytes_read = budget.read;
    if result.considered_files < result.indexed_files
        || result.unavailable_files > 0
        || index.status() != Status::Ok
    {
        result.status = Status::Partial;
        result.limitations.push("Comparison coverage is partial: scan coverage, file/byte/time limits, changed files or access restrictions excluded candidates.".into());
    }
    if cancel.is_cancelled() {
        result.status = Status::Cancelled;
    }
    result
}
