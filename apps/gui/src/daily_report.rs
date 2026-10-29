//! The daily report seen from the desktop app: the windowless run started by the scheduled task
//! (`pcagent-gui.exe --scheduled-report`) and the views the page receives. Portable so its
//! tests run everywhere; the task itself exists on Windows only.
use pcagent_actions::scheduled_report::{self as task, ScheduleState};
use pcagent_diagnostics::{Classification, Support};
use pcagent_report::{HistoryStatus, Report, RunStatus};
use pcagent_shared::presentation::Language;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Only the exact fixed invocation the task uses starts a windowless run. Arguments are read as
/// OS strings: a launch with arguments that are not Unicode must not panic here.
pub fn requested(args: &[std::ffi::OsString]) -> bool {
    args.len() == 2 && args[1] == task::GUI_ARGUMENTS
}

/// The fixed invocation an uninstaller (or the person) runs to remove this account's task.
pub const REMOVE_ARGUMENT: &str = "--remove-scheduled-report";

pub fn remove_requested(args: &[std::ffi::OsString]) -> bool {
    args.len() == 2 && args[1] == REMOVE_ARGUMENT
}

/// The local action journal, the same folder the window's actions use.
pub fn journal_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pcagent")
        .join("journal")
}

/// Removes the daily task without a window; journaled like the switch in Settings. Exit codes:
/// 0 removed or absent, 1 failed, 2 refused (elevated).
pub fn remove_headless() -> i32 {
    if pcagent_inspect::elevation::ensure_unelevated_host().is_err() {
        return 2;
    }
    let scheduler = task::WindowsScheduler::for_current_user();
    match pcagent_actions::Journal::open(journal_dir())
        .and_then(|journal| task::disable(&journal, &scheduler))
    {
        Ok(_) => 0,
        Err(error) if error.kind() == std::io::ErrorKind::Unsupported => 0,
        Err(_) => 1,
    }
}

fn app_dir() -> Option<PathBuf> {
    Some(
        pcagent_agent::credentials::file_path()?
            .parent()?
            .to_path_buf(),
    )
}

pub fn reports_dir() -> Option<PathBuf> {
    app_dir().map(|dir| dir.join("reports"))
}

pub fn database() -> Option<PathBuf> {
    app_dir().map(|dir| dir.join("pcagent.sqlite"))
}

/// The language chosen for notifications in the app, else the language of Windows.
pub fn language(database: Option<&Path>) -> Language {
    database
        .filter(|path| path.is_file())
        .and_then(|path| pcagent_store::Store::open(path).ok())
        .and_then(|store| store.watch_language_saved().ok().flatten())
        .and_then(|code| code.parse().ok())
        .unwrap_or(if pcagent_inspect::elevation::windows_ui_is_french() {
            Language::Fr
        } else {
            Language::En
        })
}

/// Measures, writes today's report and returns it. Never elevated, never online.
pub fn run_once(scheduled: bool) -> Result<Report, String> {
    let dir = reports_dir().ok_or("The APPDATA folder is unknown.")?;
    let database = database();
    let report = pcagent_report::collect(database.as_deref(), pcagent_shared::unix_ms(), scheduled);
    pcagent_report::write(&dir, &report, language(database.as_deref()))
        .map_err(|error| error.to_string())?;
    Ok(report)
}

/// The toast text, only when something needs a look or the run failed: a quiet day stays quiet.
pub fn toast_text(report: &Report, language: Language) -> Option<(String, String)> {
    if report.status == RunStatus::Failed {
        return Some(match language {
            Language::En => (
                "PC Agent daily report".into(),
                "Today's check could not measure the PC. The next run tries again.".into(),
            ),
            Language::Fr => (
                "Rapport quotidien de PC Agent".into(),
                "La vérification du jour n’a pas pu mesurer le PC. La prochaine réessaiera.".into(),
            ),
        });
    }
    if report.points_to_review == 0 {
        return None;
    }
    let n = report.points_to_review;
    Some(match language {
        Language::En => (
            "PC Agent daily report".into(),
            format!("{n} point(s) to review. Open PC Agent to see the details."),
        ),
        Language::Fr => (
            "Rapport quotidien de PC Agent".into(),
            format!("{n} point(s) à voir. Ouvrez PC Agent pour le détail."),
        ),
    })
}

/// The scheduled task's entry point. Exit codes: 0 written, 1 failed, 2 refused (elevated).
pub fn run_headless() -> i32 {
    if pcagent_inspect::elevation::ensure_unelevated_host().is_err() {
        return 2;
    }
    let report = match run_once(true) {
        Ok(report) => report,
        Err(_) => return 1,
    };
    let notify = reports_dir().is_none_or(|dir| pcagent_report::settings(&dir).notify);
    if !notify {
        return 0;
    }
    if let Some((title, body)) = toast_text(&report, language(database().as_deref())) {
        let service = pcagent_actions::notifications::Service::start(std::sync::Arc::new(|_| {}));
        let _ = service.show(pcagent_actions::notifications::Notification {
            alert_id: format!("daily-report-{}", report.date),
            watch_id: "daily-report".into(),
            title,
            body,
        });
    }
    0
}

#[derive(Debug, Serialize)]
pub struct ReportItem {
    pub classification: Classification,
    /// English heading; the page shows its own French heading when it has one.
    pub label: &'static str,
    pub support: Support,
}

/// What the home card shows; the full report stays in the local files.
#[derive(Debug, Serialize)]
pub struct ReportView {
    pub date: String,
    pub generated_at_unix_ms: u64,
    pub scheduled: bool,
    pub points_to_review: usize,
    pub items: Vec<ReportItem>,
    pub alerts_total: u64,
    pub history_status: HistoryStatus,
    pub events_last_day: Option<u64>,
    /// Fixed drives almost full, by root (`C:\\`).
    pub low_drives: Vec<String>,
    pub failed: bool,
}

pub fn view(report: &Report) -> ReportView {
    ReportView {
        date: report.date.clone(),
        generated_at_unix_ms: report.generated_at_unix_ms,
        scheduled: report.scheduled,
        points_to_review: report.points_to_review,
        items: report
            .findings
            .iter()
            .filter(|f| pcagent_report::needs_attention(f))
            .map(|f| ReportItem {
                classification: f.classification,
                label: f.classification.label(),
                support: f.support,
            })
            .collect(),
        alerts_total: report.alerts_total,
        history_status: report.history.status,
        events_last_day: report.history.events_last_day,
        low_drives: report
            .volumes
            .iter()
            .filter(|v| v.low)
            .map(|v| v.root.clone())
            .collect(),
        failed: report.status == RunStatus::Failed,
    }
}

/// The settings row: the task as Windows reports it, what turning it on would start, and
/// whether an installed task starts something else (an older copy, or the CLI).
#[derive(Debug, Serialize)]
pub struct ScheduleView {
    #[serde(flatten)]
    pub state: ScheduleState,
    pub command: String,
    pub stale: bool,
    pub default_time: String,
    /// Whether a scheduled run shows a notification when something needs review.
    pub notify: bool,
}

pub fn schedule_view(state: ScheduleState, program: &Path) -> ScheduleView {
    let command = task::command_line(&program.to_string_lossy(), task::GUI_ARGUMENTS);
    ScheduleView {
        stale: state.installed
            && state
                .task_command
                .as_deref()
                .is_some_and(|actual| !actual.eq_ignore_ascii_case(&command)),
        state,
        command,
        default_time: task::format_time(task::DEFAULT_TIME),
        notify: reports_dir().is_none_or(|dir| pcagent_report::settings(&dir).notify),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_report::{assemble, HistoryContext};

    fn report(findings: Vec<pcagent_diagnostics::Finding>) -> Report {
        assemble(
            findings,
            (
                HistoryContext {
                    status: HistoryStatus::Stale,
                    latest_sample_unix_ms: Some(1),
                    events_last_day: Some(0),
                },
                Vec::new(),
                0,
            ),
            1_793_232_000_000,
            true,
        )
    }

    #[test]
    fn only_the_exact_task_invocation_runs_without_a_window() {
        let args = |rest: &[&str]| {
            std::iter::once("pcagent-gui.exe")
                .chain(rest.iter().copied())
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };
        assert!(requested(&args(&["--scheduled-report"])));
        assert!(!requested(&args(&[])));
        assert!(!requested(&args(&["--scheduled-report", "--extra"])));
        assert!(!requested(&args(&["report"])));
        assert!(remove_requested(&args(&["--remove-scheduled-report"])));
        assert!(!remove_requested(&args(&["--scheduled-report"])));
        assert!(!requested(&args(&["--remove-scheduled-report"])));
    }

    #[test]
    fn a_quiet_report_raises_no_notification() {
        let quiet = report(pcagent_diagnostics::analyze(&pcagent_shared::Snapshot {
            schema_version: 1,
            id: "s".into(),
            session_id: "x".into(),
            origin: pcagent_shared::Origin::SyntheticFixture,
            evidence: Vec::new(),
            processes: Vec::new(),
            process_coverage: Default::default(),
            limitations: Vec::new(),
            gpus: Vec::new(),
        }));
        assert_eq!(quiet.points_to_review, 0);
        assert!(toast_text(&quiet, Language::En).is_none());
        let view = view(&quiet);
        assert!(view.items.is_empty());
        assert_eq!(view.history_status, HistoryStatus::Stale);
        let mut busy = quiet;
        busy.points_to_review = 2;
        let (title, body) = toast_text(&busy, Language::Fr).unwrap();
        assert_eq!(title, "Rapport quotidien de PC Agent");
        assert!(body.starts_with("2 point(s)"));
        // A failed run is never silent, and the home card says it failed.
        let failed = pcagent_report::failed(1, true, "measurement_failed");
        assert!(toast_text(&failed, Language::En)
            .unwrap()
            .1
            .contains("could not measure"));
        assert!(super::view(&failed).failed);
    }

    #[test]
    fn the_settings_view_shows_the_exact_fixed_command() {
        let state = |command: &str| ScheduleState {
            supported: true,
            installed: true,
            time: Some("09:00".into()),
            task_command: Some(command.into()),
            task_name: task::TASK_NAME.into(),
        };
        let program = Path::new(r"C:\Apps\pcagent-gui.exe");
        let view = schedule_view(
            state(r#""c:\apps\PCAGENT-GUI.exe" --scheduled-report"#),
            program,
        );
        assert_eq!(
            view.command,
            r#""C:\Apps\pcagent-gui.exe" --scheduled-report"#
        );
        assert!(!view.stale);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["installed"], true);
        assert_eq!(json["time"], "09:00");
        assert_eq!(json["default_time"], "09:00");
        assert_eq!(json["stale"], false);
        // A task left by an older copy, or created by the CLI, is shown for repair.
        assert!(
            schedule_view(
                state(r#""D:\Old\pcagent-gui.exe" --scheduled-report"#),
                program
            )
            .stale
        );
        assert!(
            schedule_view(
                state(r#""C:\Apps\pcagent.exe" report --scheduled"#),
                program
            )
            .stale
        );
    }
}
