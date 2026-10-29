//! The optional daily task that writes the report while PC Agent is closed.
use crate::args::ScheduleAction;
use crate::local::local_data_dir;
use crate::{text, Result};
use pcagent_actions::scheduled_report::{self as task, WindowsScheduler};
use pcagent_actions::Journal;
use std::path::PathBuf;

/// The desktop app runs the report without a window; the CLI is the fallback when the app is
/// not installed next to it (a console then opens briefly once a day).
fn program() -> Result<(PathBuf, &'static str)> {
    let cli = std::env::current_exe()?;
    let gui = cli.with_file_name(if cfg!(windows) {
        "pcagent-gui.exe"
    } else {
        "pcagent-gui"
    });
    Ok(if gui.is_file() {
        (gui, task::GUI_ARGUMENTS)
    } else {
        (cli, task::CLI_ARGUMENTS)
    })
}

fn print(state: &task::ScheduleState) {
    let line = match (state.supported, state.installed, &state.time) {
        (false, ..) => text::label("cli.schedule.unsupported").to_owned(),
        (true, false, _) => text::label("cli.schedule.off").to_owned(),
        (true, true, Some(time)) => text::label("cli.schedule.on").replace("{time}", time),
        (true, true, None) => text::label("cli.schedule.on_unknown").to_owned(),
    };
    println!("{line}");
}

pub(super) fn run(action: ScheduleAction) -> Result<u8> {
    pcagent_inspect::elevation::ensure_unelevated_host()?;
    let scheduler = WindowsScheduler;
    let journal = || Journal::open(local_data_dir().join("journal"));
    let state = match action {
        ScheduleAction::Status => task::state(&scheduler)?,
        ScheduleAction::Enable { time } => {
            let time = task::parse_time(&time)?;
            let (program, arguments) = program()?;
            task::enable(
                &journal()?,
                &scheduler,
                &program,
                arguments,
                time,
                &pcagent_report::date_stamp(pcagent_shared::unix_ms()),
                task::current_user().as_deref(),
            )?
        }
        ScheduleAction::Disable => task::disable(&journal()?, &scheduler)?,
    };
    print(&state);
    Ok(if state.supported { 0 } else { 3 })
}
