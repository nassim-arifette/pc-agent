//! The daily report seen from the desktop app: the windowless run started by the scheduled task
//! (`pcagent-gui.exe --scheduled-report`) and the views the page receives. Portable so its
//! tests run everywhere; the task itself exists on Windows only.
use pcagent_actions::scheduled_report::{self as task, ScheduleState};
use pcagent_diagnostics::{Classification, Support};
use pcagent_report::{HistoryStatus, Report};
use pcagent_shared::presentation::Language;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Only the exact fixed invocation the task uses starts a windowless run.
pub fn requested(args: &[String]) -> bool {
    args.len() == 2 && args[1] == task::GUI_ARGUMENTS
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
        .and_then(|store| store.watch_language().ok())
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

/// The toast text, only when something needs a look: a quiet day stays quiet.
pub fn toast_text(report: &Report, language: Language) -> Option<(String, String)> {
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
    }
}

/// The settings row: the task as Windows reports it and exactly what it would start.
#[derive(Debug, Serialize)]
pub struct ScheduleView {
    #[serde(flatten)]
    pub state: ScheduleState,
    pub command: String,
    pub default_time: String,
}

pub fn schedule_view(state: ScheduleState, program: &Path) -> ScheduleView {
    ScheduleView {
        state,
        command: format!("\"{}\" {}", program.display(), task::GUI_ARGUMENTS),
        default_time: task::format_time(task::DEFAULT_TIME),
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
                .map(String::from)
                .collect::<Vec<_>>()
        };
        assert!(requested(&args(&["--scheduled-report"])));
        assert!(!requested(&args(&[])));
        assert!(!requested(&args(&["--scheduled-report", "--extra"])));
        assert!(!requested(&args(&["report"])));
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
    }

    #[test]
    fn the_settings_view_shows_the_exact_fixed_command() {
        let state = ScheduleState {
            supported: true,
            installed: true,
            time: Some("09:00".into()),
            task_name: task::TASK_NAME,
        };
        let view = schedule_view(state, Path::new(r"C:\Apps\pcagent-gui.exe"));
        assert_eq!(
            view.command,
            r#""C:\Apps\pcagent-gui.exe" --scheduled-report"#
        );
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["installed"], true);
        assert_eq!(json["time"], "09:00");
        assert_eq!(json["default_time"], "09:00");
    }
}
