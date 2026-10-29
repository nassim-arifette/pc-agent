# Daily report (chantier 17)

A once-a-day local check that runs even when PC Agent is closed, and leaves a dated report.
Contract: `docs/plan.md`, chantier 17 (plan PR #117).

## What runs

- The Windows Task Scheduler starts `pcagent-gui.exe --scheduled-report` (no window). The CLI
  `pcagent report --scheduled` writes the same report; `pcagent schedule enable` points the task
  at the desktop app when it is installed next to the CLI, otherwise at the CLI.
- One measurement (`Collector::snapshot`), the local rules (`pcagent-diagnostics`), and what the
  local history recorded in the last 24 hours: activity events and watch alerts
  (`pcagent-report::read_history`). A missing history file is reported, never created.
- The space of fixed drives (`hardware_inventory`); a drive almost full (under 5 % or 5 GiB free,
  the same limit as Live PC) is one more point to review.
- A run that cannot measure still writes a dated report with `status: failed` and a closed
  `failure` code, and its notification says so: a failed day is never silent.
- No network, no model, no administrator. An elevated process refuses to run it.

## Files

`%APPDATA%\pcagent\reports\rapport-YYYY-MM-DD.json` (schema 1) and `.md`, in the language the
window last reported (`watch_language_set`), else the language of Windows. A rerun on the same
local day replaces that day; the 30 newest days are kept. Writes are atomic. The Markdown renders
findings from closed phrases and alerts from closed rule targets: no user name, command line or
profile path.

## The task

`PCAgent DailyReport (<account>)` at the root of the Task Scheduler library (a standard
account cannot create a folder there; the account name keeps two accounts apart), from a fixed
XML definition (`pcagent_actions::scheduled_report`): daily
at the chosen local time, run once when missed, only in the user's interactive session,
`LeastPrivilege`, 5-minute limit, one instance, no wake, no network condition. `schtasks.exe`
is called from `System32` with fixed arguments and no console window. Turning it on, moving it
and removing it are journaled (`PreState::ScheduledReport`) in the local action journal, whose
restoration is removal. The settings row reads the state back from Windows every time,
including the command the task starts; a task that starts another copy (an older install, or the
CLI) is shown with a Repair button. The first day is yesterday in UTC, so the first run is the
next occurrence of the chosen local time. Report file names use the local day (Windows' time
zone, daylight saving included), so a report written just after local midnight never replaces
the previous day's.

## Surfaces

- Settings > Daily report: switch, time, the exact command, "Write a report now", "Show the
  report file".
- Home: the latest report (points to review first), "Ask PC Agent about it" (drafts a question
  in the composer; nothing is sent), and an invitation when no task exists.
- A toast at the end of a scheduled run, only when something needs review or the run failed.
  Settings > Daily report > Notify me turns it off (`reports\settings.json`, local only).
- Removal: turning the switch off, `pcagent schedule disable`, or
  `pcagent-gui.exe --remove-scheduled-report` (fixed argument, journaled, refused when elevated).
  The per-user installer's uninstall step runs the last command ([installer.md](installer.md)).
- Tauri commands: `report_schedule_get`, `report_schedule_set`, `report_latest`,
  `report_run_now`, `report_reveal`, `report_notify_set`. Mock:
  `?report=none|quiet|unsupported|stale|failed`.

## Validation

Linux: unit tests (task XML, time parsing, journal, rotation, bilingual Markdown, missing
history), Node tests of both cards over the mock, `cargo check`/Clippy for
`x86_64-pc-windows-gnu`, and captures of both cards in EN light and FR dark.

Not validated yet (Windows, standard account): creating the task, the run at the chosen time,
catch-up after the PC was off, the toast, moving and removing the task, and the report language.
