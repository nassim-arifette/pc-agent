use crate::{FileIdentity, ItemOutcome, ProcessIdentity, SettingsPage};
use pcagent_shared::{unix_ms, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
};

const MAX_JOURNALS: usize = 4096;
const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PreState {
    File {
        path: PathBuf,
        identity: FileIdentity,
    },
    Process {
        identity: ProcessIdentity,
    },
    SettingsPage {
        page: SettingsPage,
    },
    CleanupAdapter {
        files: u64,
        bytes: Option<u64>,
    },
    UserStartup {
        target: crate::StartupTarget,
    },
    RecycleCleanup {
        target: crate::RecycleCleanupTarget,
    },
    /// The daily report task before the change; restoring means removing it.
    ScheduledReport {
        installed: bool,
        time: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalPhase {
    Started,
    Finished,
    NeedsInspection,
}

/// One append-only line. Contains no approval token or other secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRecord {
    pub schema_version: u32,
    pub plan_id: String,
    pub fingerprint: String,
    pub item_index: usize,
    pub target_ref: String,
    pub phase: JournalPhase,
    pub pre_state: Option<PreState>,
    pub outcome: Option<ItemOutcome>,
    pub at_unix_ms: u64,
}

/// Per-plan JSON-lines files in a caller-provided local directory; each append is fsync'd.
#[derive(Debug)]
pub struct Journal {
    dir: PathBuf,
}
impl Journal {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        if !fs::symlink_metadata(&dir)?.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal path is not a directory",
            ));
        }
        Ok(Self { dir })
    }
    fn path(&self, plan_id: &str) -> io::Result<PathBuf> {
        if plan_id.is_empty()
            || plan_id.len() > 128
            || !plan_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid plan id",
            ));
        }
        Ok(self.dir.join(format!("{plan_id}.jsonl")))
    }
    pub(crate) fn append(&self, record: &JournalRecord) -> io::Result<()> {
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        if line.len() > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "journal record too large",
            ));
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(&record.plan_id)?)?;
        file.write_all(&line)?;
        file.sync_all()
    }
    /// Reads one plan's records; malformed or oversized lines are counted, not trusted.
    pub fn read(&self, plan_id: &str) -> io::Result<(Vec<JournalRecord>, usize)> {
        read_file(self.path(plan_id)?)
    }
}

fn read_file(path: PathBuf) -> io::Result<(Vec<JournalRecord>, usize)> {
    let file = File::open(path)?;
    if file.metadata()?.len() > MAX_JOURNAL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "journal too large",
        ));
    }
    let mut reader = BufReader::new(file.take(MAX_JOURNAL_BYTES));
    let (mut records, mut malformed) = (Vec::new(), 0);
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        match serde_json::from_slice::<JournalRecord>(&line) {
            Ok(r) if line.len() <= MAX_LINE_BYTES => records.push(r),
            _ => malformed += 1,
        }
    }
    Ok((records, malformed))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InterruptedItem {
    pub plan_id: String,
    pub fingerprint: String,
    pub item_index: usize,
    pub target_ref: String,
    pub pre_state: Option<PreState>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RecoveryReport {
    /// Items that started without a recorded result. State is unknown; inspect, never replay.
    pub needs_inspection: Vec<InterruptedItem>,
    pub journals_read: usize,
    pub journals_unreadable: usize,
    pub malformed_lines: usize,
    pub truncated: bool,
}

/// Post-crash inspection. Interrupted items are marked `needs_inspection`; nothing is re-executed.
pub fn recover(journal: &Journal) -> io::Result<RecoveryReport> {
    let mut report = RecoveryReport::default();
    let mut paths = Vec::new();
    for entry in fs::read_dir(&journal.dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            if paths.len() == MAX_JOURNALS {
                report.truncated = true;
                break;
            }
            paths.push(path);
        }
    }
    paths.sort();
    for path in paths {
        let (records, malformed) = match read_file(path) {
            Ok(r) => r,
            Err(_) => {
                report.journals_unreadable += 1;
                continue;
            }
        };
        report.journals_read += 1;
        report.malformed_lines += malformed;
        let mut last: BTreeMap<(String, usize), JournalRecord> = BTreeMap::new();
        for r in records {
            last.insert((r.plan_id.clone(), r.item_index), r);
        }
        for r in last.into_values().filter(|r| {
            matches!(
                r.phase,
                JournalPhase::Started | JournalPhase::NeedsInspection
            )
        }) {
            if r.phase == JournalPhase::Started {
                journal.append(&JournalRecord {
                    schema_version: SCHEMA_VERSION,
                    phase: JournalPhase::NeedsInspection,
                    outcome: None,
                    at_unix_ms: unix_ms(),
                    ..r.clone()
                })?;
            }
            report.needs_inspection.push(InterruptedItem {
                plan_id: r.plan_id,
                fingerprint: r.fingerprint,
                item_index: r.item_index,
                target_ref: r.target_ref,
                pre_state: r.pre_state,
            });
        }
    }
    Ok(report)
}
