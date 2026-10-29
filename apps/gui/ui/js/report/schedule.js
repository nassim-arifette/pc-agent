// Settings: the optional daily report (chantier 17). The state shown is read back from Windows
// each time; turning the task on, off or moving it is one local click, journaled on the Rust
// side. The exact command the task starts is shown before and after.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { button, toast, toggle } from "../ui.js";
import { failureText } from "../failure.js";
import { reportSummary } from "./summary.js";

// A change still running when Settings redraws (a new card replaces this one) is awaited by the
// next card before it reads the state, so it never shows the state from before the click.
let inflight = null;

export function dailyReportCard() {
  let view = null;
  let latest;
  let busy = false;
  let disposed = false;
  let move = null;
  const time = h("input", {
    type: "time",
    class: "input report-time",
    step: "60",
    "aria-label": t("report.schedule.time"),
    oninput: () => {
      if (move) move.disabled = busy || !time.value || time.value === view?.time;
    },
  });
  const status = h("p", { class: "hint", role: "status" }, t("report.schedule.loading"));
  const controls = h("div", { class: "report-controls" });
  const detail = h("div", { class: "report-detail" });
  const latestHost = h("div", { class: "report-latest" });

  const change = async (enabled) => {
    if (busy || !view) return;
    busy = true;
    render();
    try {
      inflight = call("report_schedule_set", { enabled, time: time.value || view.default_time });
      view = await inflight;
      toast(enabled ? t("report.schedule.enabled", { time: view.time ?? time.value }) : t("report.schedule.disabled"));
    } catch (error) {
      toast(failureText(error), "bad");
      view = await call("report_schedule_get").catch(() => view);
    } finally {
      inflight = null;
      busy = false;
      if (!disposed) render();
    }
  };

  const runNow = async () => {
    if (busy) return;
    busy = true;
    render();
    try {
      latest = await call("report_run_now");
      toast(t("report.run.done", { count: latest.points_to_review }));
    } catch (error) {
      toast(failureText(error), "bad");
    } finally {
      busy = false;
      if (!disposed) render();
    }
  };

  function notifySwitch() {
    const sw = toggle(view.notify !== false, async (next) => {
      if (busy) return;
      busy = true;
      try {
        view = await call("report_notify_set", { enabled: next });
      } catch (error) {
        toast(failureText(error), "bad");
      } finally {
        busy = false;
        if (!disposed) render();
      }
    }, t("report.notify.title"));
    sw.disabled = busy;
    return sw;
  }

  function render() {
    if (!view) return;
    if (!view.supported) {
      status.textContent = t("report.schedule.unsupported");
      fill(controls, button(t("report.run.now"), { iconName: "play", size: "sm", disabled: busy, onClick: runNow }));
      fill(detail);
    } else {
      status.textContent = view.installed
        ? (view.time ? t("report.schedule.on", { time: view.time }) : t("report.schedule.onUnknown"))
        : t("report.schedule.off");
      if (!time.value) time.value = view.time ?? view.default_time;
      time.disabled = busy;
      move = view.installed ? button(t("report.schedule.move"), { size: "sm", disabled: busy || time.value === view.time, onClick: () => change(true) }) : null;
      const sw = toggle(view.installed, (next) => change(next), t("report.schedule.title"));
      sw.disabled = busy;
      fill(
        controls,
        time,
        move,
        sw,
      );
      fill(
        detail,
        view.stale
          ? h(
            "div",
            { class: "report-stale", role: "alert" },
            h("span", null, icon("alert"), t("report.schedule.stale")),
            h("code", { class: "mono report-command" }, view.task_command),
            button(t("report.schedule.repair"), { size: "sm", disabled: busy, onClick: () => change(true) }),
          )
          : null,
        h("span", { class: "hint" }, t(view.stale ? "report.schedule.commandRepaired" : "report.schedule.command")),
        h("code", { class: "mono report-command" }, view.command),
        h("span", { class: "hint" }, t("report.schedule.guarantees")),
        h(
          "div",
          { class: "setting-row report-notify" },
          h("div", { class: "setting-text" }, h("strong", null, t("report.notify.title")), h("span", { class: "hint" }, t("report.notify.note"))),
          notifySwitch(),
        ),
      );
    }
    fill(
      latestHost,
      latest === undefined ? null : latest ? reportSummary(latest, { compact: true }) : h("p", { class: "hint" }, t("report.none")),
      h(
        "div",
        { class: "report-actions" },
        view.supported ? button(t("report.run.now"), { iconName: "play", size: "sm", disabled: busy, onClick: runNow }) : null,
        latest ? button(t("report.reveal"), { iconName: "reveal", size: "sm", variant: "ghost", onClick: reveal }) : null,
      ),
    );
  }

  const el = h(
    "section",
    { class: "card card-pad report-card" },
    h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("clock"), t("report.schedule.title"))),
    h(
      "div",
      { class: "card-rows" },
      h("div", { class: "setting-row" }, h("div", { class: "setting-text" }, h("strong", null, t("report.schedule.row")), h("span", { class: "hint" }, t("report.schedule.note")), status), controls),
    ),
    detail,
    latestHost,
  );

  Promise.resolve(inflight)
    .catch(() => {})
    .then(() => Promise.all([call("report_schedule_get"), call("report_latest").catch(() => null)]))
    .then(([schedule, report]) => {
      if (disposed) return;
      view = schedule;
      latest = report;
      render();
    })
    .catch((error) => {
      if (!disposed) status.textContent = failureText(error);
    });

  return { el, dispose: () => { disposed = true; } };
}

export async function reveal() {
  try {
    await call("report_reveal");
  } catch (error) {
    toast(failureText(error), "bad");
  }
}
