//! The daily report (chantier 17): the task's state read back from Windows, turning it on or off
//! by a local click (journaled), the latest report for the home card, and a run on demand.
use crate::daily_report::{self, ReportView, ScheduleView};
use crate::failure::{self, Code, Failure};
use pcagent_actions::scheduled_report::{self as task, WindowsScheduler};
use pcagent_actions::Journal;

fn program() -> Result<std::path::PathBuf, Failure> {
    std::env::current_exe().map_err(failure::schedule)
}

#[tauri::command]
pub(super) async fn report_schedule_get() -> Result<ScheduleView, Failure> {
    tauri::async_runtime::spawn_blocking(|| {
        let state =
            task::state(&WindowsScheduler::for_current_user()).map_err(failure::schedule)?;
        Ok(daily_report::schedule_view(state, &program()?))
    })
    .await
    .map_err(failure::schedule)?
}

/// `time` is `HH:MM`; it is ignored when turning the task off.
#[tauri::command]
pub(super) async fn report_schedule_set(
    enabled: bool,
    time: String,
) -> Result<ScheduleView, Failure> {
    let parsed = if enabled {
        Some(task::parse_time(&time).map_err(|e| Failure::new(Code::ScheduleFailed, e))?)
    } else {
        None
    };
    tauri::async_runtime::spawn_blocking(move || {
        let program = program()?;
        let journal = Journal::open(crate::actions::journal_dir()).map_err(failure::schedule)?;
        let scheduler = WindowsScheduler::for_current_user();
        let state = match parsed {
            Some(time) => task::enable(
                &journal,
                &scheduler,
                &program,
                task::GUI_ARGUMENTS,
                time,
                &task::start_date(pcagent_shared::unix_ms()),
                task::current_user().as_deref(),
            ),
            None => task::disable(&journal, &scheduler),
        }
        .map_err(failure::schedule)?;
        Ok(daily_report::schedule_view(state, &program))
    })
    .await
    .map_err(failure::schedule)?
}

#[tauri::command]
pub(super) async fn report_latest() -> Result<Option<ReportView>, Failure> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(dir) = daily_report::reports_dir() else {
            return Ok(None);
        };
        pcagent_report::latest(&dir)
            .map(|report| report.as_ref().map(daily_report::view))
            .map_err(failure::report)
    })
    .await
    .map_err(failure::report)?
}

/// The same report the task writes, now, at the person's click.
#[tauri::command]
pub(super) async fn report_run_now() -> Result<ReportView, Failure> {
    tauri::async_runtime::spawn_blocking(|| {
        daily_report::run_once(false)
            .map(|report| daily_report::view(&report))
            .map_err(failure::report)
    })
    .await
    .map_err(failure::report)?
}

/// Shows the newest report's Markdown file in Explorer. The path is built here, never taken
/// from the page.
#[tauri::command]
pub(super) fn report_reveal() -> Result<(), Failure> {
    let dir = daily_report::reports_dir()
        .ok_or_else(|| Failure::new(Code::ReportFailed, "The APPDATA folder is unknown."))?;
    let (path, _) = pcagent_report::latest_file(&dir)
        .map_err(failure::report)?
        .ok_or_else(|| Failure::new(Code::ReportFailed, "No report has been written yet."))?;
    pcagent_actions::show_in_explorer(&path).map_err(|e| Failure::new(Code::OpenFailed, e.detail))
}
