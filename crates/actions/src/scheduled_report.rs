//! The one optional daily task that writes the local report while PC Agent is closed
//! (plan, chantier 17). Created for the current user only, at the lowest run level, from a
//! fixed definition; turning it on, changing its time and removing it are journaled local
//! actions whose restoration is removal.
use crate::{ItemOutcome, Journal, JournalPhase, JournalRecord, PreState};
use pcagent_shared::{unix_ms, Status, SCHEMA_VERSION};
use serde::Serialize;
use std::io;
use std::path::Path;

/// Task Scheduler path of the single task this module may create or delete.
pub const TASK_NAME: &str = "PCAgent\\DailyReport";
/// Fixed arguments; nothing from the model or from free text reaches the task.
pub const GUI_ARGUMENTS: &str = "--scheduled-report";
pub const CLI_ARGUMENTS: &str = "report --scheduled";
pub const DEFAULT_TIME: (u8, u8) = (9, 0);
const FINGERPRINT: &str = "scheduled-report-v1";

/// What the settings page shows, read back from the Task Scheduler each time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScheduleState {
    pub supported: bool,
    pub installed: bool,
    /// `HH:MM` when the definition could be read back; `None` when only presence is known.
    pub time: Option<String>,
    pub task_name: &'static str,
}

/// The narrow surface of the Task Scheduler this module uses; faked in tests.
pub trait TaskScheduler {
    fn supported(&self) -> bool;
    /// `None` when absent; `Some(None)` when present but its time could not be read.
    fn query(&self) -> io::Result<Option<Option<(u8, u8)>>>;
    fn create(&self, definition: &str) -> io::Result<()>;
    fn delete(&self) -> io::Result<()>;
}

pub fn parse_time(value: &str) -> Result<(u8, u8), &'static str> {
    const HINT: &str = "Use HH:MM, for example 09:00.";
    let bytes = value.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return Err(HINT);
    }
    let hour: u8 = value[..2].parse().map_err(|_| HINT)?;
    let minute: u8 = value[3..].parse().map_err(|_| HINT)?;
    if hour > 23
        || minute > 59
        || !value[..2]
            .bytes()
            .chain(value[3..].bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(HINT);
    }
    Ok((hour, minute))
}

pub fn format_time((hour, minute): (u8, u8)) -> String {
    format!("{hour:02}:{minute:02}")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Daily at `hour:minute` from `start_date` (`YYYY-MM-DD`), catch up once when missed, only in
/// the user's interactive session, never elevated, five-minute limit, one instance, no wake.
pub fn task_xml(
    program: &Path,
    arguments: &str,
    (hour, minute): (u8, u8),
    start_date: &str,
    user: Option<&str>,
) -> String {
    let user = user
        .map(|u| format!("<UserId>{}</UserId>", escape(u)))
        .unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>PC Agent daily local report. Reads this PC only and sends nothing.</Description></RegistrationInfo>
  <Triggers><CalendarTrigger><StartBoundary>{start_date}T{hour:02}:{minute:02}:00</StartBoundary><Enabled>true</Enabled><ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay></CalendarTrigger></Triggers>
  <Principals><Principal id="Author">{user}<LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT5M</ExecutionTimeLimit>
    <Priority>7</Priority>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions>
</Task>
"#,
        escape(&program.to_string_lossy()),
        escape(arguments),
    )
}

/// Reads `HH:MM` back from an exported definition (`StartBoundary`), whatever its date.
pub fn time_from_definition(definition: &str) -> Option<(u8, u8)> {
    let start = definition.find("<StartBoundary>")? + "<StartBoundary>".len();
    let value = &definition[start..start + definition[start..].find('<')?];
    let time = value.split_once('T')?.1.get(..5)?;
    parse_time(time).ok()
}

pub fn state(scheduler: &dyn TaskScheduler) -> io::Result<ScheduleState> {
    if !scheduler.supported() {
        return Ok(ScheduleState {
            supported: false,
            installed: false,
            time: None,
            task_name: TASK_NAME,
        });
    }
    let found = scheduler.query()?;
    Ok(ScheduleState {
        supported: true,
        installed: found.is_some(),
        time: found.flatten().map(format_time),
        task_name: TASK_NAME,
    })
}

fn record(
    journal: &Journal,
    plan_id: &str,
    phase: JournalPhase,
    pre_state: Option<PreState>,
    outcome: Option<ItemOutcome>,
) -> io::Result<()> {
    journal.append(&JournalRecord {
        schema_version: SCHEMA_VERSION,
        plan_id: plan_id.into(),
        fingerprint: FINGERPRINT.into(),
        item_index: 0,
        target_ref: TASK_NAME.into(),
        phase,
        pre_state,
        outcome,
        at_unix_ms: unix_ms(),
    })
}

/// Runs one journaled change: the previous state is written before the effect, the outcome
/// after it, so an interrupted change is reported by [`crate::recover`].
fn journaled(
    journal: &Journal,
    scheduler: &dyn TaskScheduler,
    effect: impl FnOnce() -> io::Result<()>,
) -> io::Result<ScheduleState> {
    if !scheduler.supported() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The daily report task is available on Windows only.",
        ));
    }
    let before = state(scheduler)?;
    let plan_id = format!("scheduled-report-{}", uuid::Uuid::new_v4());
    record(
        journal,
        &plan_id,
        JournalPhase::Started,
        Some(PreState::ScheduledReport {
            installed: before.installed,
            time: before.time.clone(),
        }),
        None,
    )?;
    let result = effect();
    let outcome = match &result {
        Ok(()) => ItemOutcome::Completed {
            detail: None,
            bytes_removed: None,
        },
        Err(error) => ItemOutcome::Failed {
            status: Status::Error,
            detail: error.to_string(),
        },
    };
    record(
        journal,
        &plan_id,
        JournalPhase::Finished,
        None,
        Some(outcome),
    )?;
    result?;
    state(scheduler)
}

/// Creates or replaces the task at `time`, after an explicit local click.
pub fn enable(
    journal: &Journal,
    scheduler: &dyn TaskScheduler,
    program: &Path,
    arguments: &str,
    time: (u8, u8),
    start_date: &str,
    user: Option<&str>,
) -> io::Result<ScheduleState> {
    let definition = task_xml(program, arguments, time, start_date, user);
    journaled(journal, scheduler, || scheduler.create(&definition))
}

/// Removes the task; removing an absent task is not an error.
pub fn disable(journal: &Journal, scheduler: &dyn TaskScheduler) -> io::Result<ScheduleState> {
    journaled(journal, scheduler, || match scheduler.query()? {
        Some(_) => scheduler.delete(),
        None => Ok(()),
    })
}

/// The account that owns the task, as Windows names it (`DOMAIN\user`). Local only: it is
/// written into the task definition and never shown, logged or sent.
pub fn current_user() -> Option<String> {
    let user = std::env::var("USERNAME").ok().filter(|u| !u.is_empty())?;
    Some(
        match std::env::var("USERDOMAIN").ok().filter(|d| !d.is_empty()) {
            Some(domain) => format!("{domain}\\{user}"),
            None => user,
        },
    )
}

/// `schtasks.exe` with fixed arguments and no console window.
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsScheduler;

#[cfg(windows)]
impl WindowsScheduler {
    fn run(args: &[&std::ffi::OsStr]) -> io::Result<std::process::Output> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let system = std::env::var_os("SystemRoot")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
        std::process::Command::new(system.join("System32").join("schtasks.exe"))
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    }

    fn failed(output: &std::process::Output, what: &str) -> io::Error {
        io::Error::other(format!(
            "Windows refused to {what} the scheduled task (exit code {}).",
            output.status.code().unwrap_or(-1)
        ))
    }
}

/// `schtasks /XML` prints UTF-16 when redirected on some systems and the console code page on
/// others; both are accepted.
pub fn decode_output(bytes: &[u8]) -> String {
    let utf16 = bytes.len() >= 2
        && bytes.len().is_multiple_of(2)
        && (bytes[..2] == [0xFF, 0xFE] || bytes[1] == 0);
    if utf16 {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
            .trim_start_matches('\u{feff}')
            .to_owned()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

impl TaskScheduler for WindowsScheduler {
    fn supported(&self) -> bool {
        cfg!(windows)
    }

    #[cfg(windows)]
    fn query(&self) -> io::Result<Option<Option<(u8, u8)>>> {
        let output = Self::run(&[
            "/Query".as_ref(),
            "/TN".as_ref(),
            TASK_NAME.as_ref(),
            "/XML".as_ref(),
        ])?;
        if !output.status.success() {
            // schtasks reports an absent task with exit code 1; nothing else is distinguishable.
            return Ok(None);
        }
        Ok(Some(time_from_definition(&decode_output(&output.stdout))))
    }

    #[cfg(windows)]
    fn create(&self, definition: &str) -> io::Result<()> {
        let path =
            std::env::temp_dir().join(format!("pcagent-daily-report-{}.xml", uuid::Uuid::new_v4()));
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(definition.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&path, bytes)?;
        let output = Self::run(&[
            "/Create".as_ref(),
            "/F".as_ref(),
            "/TN".as_ref(),
            TASK_NAME.as_ref(),
            "/XML".as_ref(),
            path.as_os_str(),
        ]);
        let _ = std::fs::remove_file(&path);
        let output = output?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Self::failed(&output, "create"))
        }
    }

    #[cfg(windows)]
    fn delete(&self) -> io::Result<()> {
        let output = Self::run(&[
            "/Delete".as_ref(),
            "/F".as_ref(),
            "/TN".as_ref(),
            TASK_NAME.as_ref(),
        ])?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Self::failed(&output, "delete"))
        }
    }

    #[cfg(not(windows))]
    fn query(&self) -> io::Result<Option<Option<(u8, u8)>>> {
        Ok(None)
    }

    #[cfg(not(windows))]
    fn create(&self, _: &str) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    #[cfg(not(windows))]
    fn delete(&self) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Default)]
    struct Fake {
        task: RefCell<Option<String>>,
        refuse: bool,
    }

    impl TaskScheduler for Fake {
        fn supported(&self) -> bool {
            true
        }
        fn query(&self) -> io::Result<Option<Option<(u8, u8)>>> {
            Ok(self.task.borrow().as_deref().map(time_from_definition))
        }
        fn create(&self, definition: &str) -> io::Result<()> {
            if self.refuse {
                return Err(io::Error::other("refused"));
            }
            *self.task.borrow_mut() = Some(definition.into());
            Ok(())
        }
        fn delete(&self) -> io::Result<()> {
            *self.task.borrow_mut() = None;
            Ok(())
        }
    }

    fn journal() -> (Journal, PathBuf) {
        let dir = std::env::temp_dir().join(format!("pcagent-sched-{}", uuid::Uuid::new_v4()));
        (Journal::open(&dir).unwrap(), dir)
    }

    #[test]
    fn time_must_be_a_two_digit_clock_time() {
        assert_eq!(parse_time("09:00"), Ok((9, 0)));
        assert_eq!(parse_time("23:59"), Ok((23, 59)));
        for bad in [
            "9:00", "24:00", "12:60", "noon", "", "09:0", "+9:00", "09:+5", "0900 ",
        ] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn definition_is_unelevated_bounded_and_escaped() {
        let xml = task_xml(
            Path::new(r"C:\Program Files\A&B\pcagent-gui.exe"),
            GUI_ARGUMENTS,
            (9, 5),
            "2026-10-29",
            Some(r"PC\o'neil"),
        );
        for part in [
            "<RunLevel>LeastPrivilege</RunLevel>",
            "<LogonType>InteractiveToken</LogonType>",
            "<ExecutionTimeLimit>PT5M</ExecutionTimeLimit>",
            "<StartWhenAvailable>true</StartWhenAvailable>",
            "<WakeToRun>false</WakeToRun>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
            "2026-10-29T09:05:00",
            "A&amp;B",
            "o&apos;neil",
            "<Arguments>--scheduled-report</Arguments>",
        ] {
            assert!(xml.contains(part), "{part}");
        }
        assert_eq!(time_from_definition(&xml), Some((9, 5)));
    }

    #[test]
    fn exported_definitions_decode_from_utf16_and_utf8() {
        let xml = task_xml(
            Path::new("p.exe"),
            CLI_ARGUMENTS,
            (7, 30),
            "2026-01-01",
            None,
        );
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(time_from_definition(&decode_output(&utf16)), Some((7, 30)));
        assert_eq!(
            time_from_definition(&decode_output(xml.as_bytes())),
            Some((7, 30))
        );
    }

    #[test]
    fn enable_and_disable_are_journaled_with_the_previous_state() {
        let (journal, dir) = journal();
        let fake = Fake::default();
        let on = enable(
            &journal,
            &fake,
            Path::new("p.exe"),
            GUI_ARGUMENTS,
            (8, 15),
            "2026-10-29",
            None,
        )
        .unwrap();
        assert!(on.installed);
        assert_eq!(on.time.as_deref(), Some("08:15"));
        let off = disable(&journal, &fake).unwrap();
        assert!(!off.installed);
        // Removing an absent task succeeds and is still journaled.
        assert!(!disable(&journal, &fake).unwrap().installed);
        let report = crate::recover(&journal).unwrap();
        assert_eq!(report.journals_read, 3);
        assert!(report.needs_inspection.is_empty());
        let mut pre = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let id = entry
                .unwrap()
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let (records, malformed) = journal.read(&id).unwrap();
            assert_eq!(malformed, 0);
            assert_eq!(records.len(), 2);
            pre.push(records[0].pre_state.clone().unwrap());
        }
        assert!(pre.contains(&PreState::ScheduledReport {
            installed: true,
            time: Some("08:15".into())
        }));
        assert!(pre.contains(&PreState::ScheduledReport {
            installed: false,
            time: None
        }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_refused_creation_is_reported_and_journaled_as_failed() {
        let (journal, dir) = journal();
        let fake = Fake {
            refuse: true,
            ..Fake::default()
        };
        assert!(enable(
            &journal,
            &fake,
            Path::new("p.exe"),
            GUI_ARGUMENTS,
            (9, 0),
            "2026-10-29",
            None
        )
        .is_err());
        let id = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let (records, _) = journal
            .read(&id.file_stem().unwrap().to_string_lossy())
            .unwrap();
        assert!(matches!(
            records[1].outcome,
            Some(ItemOutcome::Failed { .. })
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unsupported_systems_refuse_without_journaling() {
        let (journal, dir) = journal();
        let scheduler = WindowsScheduler;
        if !scheduler.supported() {
            assert!(!state(&scheduler).unwrap().supported);
            assert!(disable(&journal, &scheduler).is_err());
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
