// Synthetic daily report (chantier 17): the Windows task and the latest report file.
// ?report=none (no report written yet), ?report=quiet (nothing to review),
// ?report=unsupported (not Windows: no task, a report can still be written on request),
// ?report=stale (the installed task starts an older copy of the app: Settings offers a repair).
import { fail } from "./common.js";

const TIME = /^([01]\d|2[0-3]):[0-5]\d$/;

export function createDailyReportFixture(params) {
  const mode = params.get("report");
  const supported = mode !== "unsupported";
  const COMMAND = "\"C:\\Program Files\\PC Agent\\pcagent-gui.exe\" --scheduled-report";
  let installed = mode === "stale";
  let time = installed ? "08:00" : null;
  let taskCommand = installed ? "\"D:\\Old\\PC Agent\\pcagent-gui.exe\" --scheduled-report" : null;
  const report = (points, scheduled) => ({
    date: new Date(Date.now()).toISOString().slice(0, 10),
    generated_at_unix_ms: Date.now() - (scheduled ? 3 * 3_600_000 : 0),
    scheduled,
    points_to_review: points,
    items: points === 0 ? [] : [
      { classification: "low_available_memory", label: "Low available memory", support: "plausible" },
      { classification: "high_disk_latency", label: "High disk latency", support: "plausible" },
    ].slice(0, Math.max(0, points - 1)),
    alerts_total: points === 0 ? 0 : 1,
    history_status: scheduled ? "stale" : "recent",
    events_last_day: scheduled ? null : 4,
  });
  let latest = mode === "none" ? null : report(mode === "quiet" ? 0 : 3, true);
  const view = () => ({
    supported,
    installed,
    time: installed ? time : null,
    task_command: installed ? taskCommand : null,
    task_name: "PCAgent DailyReport (PC-Demo)",
    command: COMMAND,
    stale: installed && taskCommand !== COMMAND,
    default_time: "09:00",
  });
  return {
    commands: {
      report_schedule_get: () => view(),
      report_schedule_set: ({ enabled, time: requested }) => {
        if (typeof enabled !== "boolean") fail("unknown", "enabled must be a boolean");
        if (!supported) fail("schedule_failed", "The daily report task is available on Windows only.");
        if (enabled) {
          if (typeof requested !== "string" || !TIME.test(requested)) fail("schedule_failed", "Use HH:MM, for example 09:00.");
          installed = true;
          time = requested;
          taskCommand = COMMAND;
        } else {
          installed = false;
          time = null;
        }
        return view();
      },
      report_latest: () => latest,
      report_run_now: () => {
        latest = report(mode === "quiet" ? 0 : 2, false);
        return latest;
      },
      report_reveal: () => {
        if (!latest) fail("report_failed", "No report has been written yet.");
        return null;
      },
    },
  };
}
