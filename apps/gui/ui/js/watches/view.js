// Surveillances (C7): the local rules with their live state, the alerts of the last seven days
// and a local weekly summary. Everything here stays on this PC; only "Explain" on an alert goes
// through the frozen selection and its exact preview, after the person's click.
import { nox } from "../brand.js";
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { badge, button, dialog, toast, toggle } from "../ui.js";
import { date } from "../format.js";
import { failureText } from "../failure.js";
import { runSelection } from "../selection/actions.js";
import { editWatch, requestWatch } from "../watches-controller.js";
import { defaultRule } from "./model.js";
import { describe, measured, subject } from "./text.js";
import { refreshWatches } from "./runtime.js";

const WEEK_MS = 7 * 86_400_000;
const PAGE = 50;
const PHASE_TONE = { normal: "good", pending: "warn", alert: "bad", missing: "warn", disabled: "", paused: "" };

export function mount(page) {
  let list = null;
  let alerts = [];
  let total = 0;
  let summary = null;
  let disposed = false;
  let loading = false;

  const statusHost = h("div", { class: "watch-status" });
  const rulesHost = h("div", { class: "watch-rules" });
  const alertsHost = h("div", { class: "watch-alerts" });
  const summaryHost = h("div", { class: "watch-summary" });

  async function load({ more = false } = {}) {
    if (loading) return;
    loading = true;
    try {
      const now = Date.now();
      const [nextList, page] = await Promise.all([
        refreshWatches(),
        call("watch_alerts", { fromUnixMs: now - WEEK_MS + 1, toUnixMs: now, limit: PAGE, offset: more ? alerts.length : 0 }).catch(() => null),
      ]);
      if (disposed) return;
      list = nextList;
      if (page) {
        alerts = more ? [...alerts, ...page.alerts] : page.alerts;
        total = page.total;
      }
      render();
    } finally {
      loading = false;
    }
  }

  const changed = () => load();
  window.addEventListener("watches:changed", changed);
  // A paused window re-reads its state; the evaluating one is also told by events.
  const timer = window.setInterval(() => load(), 10_000);

  function live(id) {
    return list?.engine?.rules?.find((entry) => entry.watch.id === id) ?? null;
  }

  function renderStatus() {
    const engine = list?.engine;
    const notes = [];
    if (!list) notes.push(h("p", { class: "hint" }, t("watches.unavailable")));
    else if (!engine) notes.push(h("p", { class: "hint" }, t("watches.engine.off")));
    else if (!engine.owns_lease) notes.push(h("p", { class: "hint" }, icon("pause"), t("watches.engine.paused")));
    if (engine?.error) notes.push(h("p", { class: "hint" }, icon("alert"), t("watches.engine.error")));
    if (list?.notification_status === "disabled") notes.push(h("p", { class: "hint" }, icon("bell"), t("watches.notifications.disabled")));
    if (list?.notification_status === "unsupported") notes.push(h("p", { class: "hint" }, icon("info"), t("watches.notifications.unsupported")));
    fill(statusHost, ...notes);
  }

  function ruleRow(watch) {
    const state = live(watch.id);
    const phase = watch.enabled ? state?.phase ?? "pending" : "disabled";
    const observation = state?.observation;
    const reading = observation
      ? t(state.lower_bound ? "watches.reading.lowerBound" : "watches.reading", {
        value: measured(observation.value, observation.unit),
        when: date(observation.measured_at_unix_ms, true),
      })
      : phase === "missing" && state?.reason ? t("watches.reading.missing", { reason: state.reason }) : null;
    const enabled = toggle(watch.enabled, async (next) => {
      try {
        await call("watch_set_enabled", { id: watch.id, enabled: next });
      } catch (error) {
        toast(failureText(error), "bad");
      }
      load();
    }, t("watches.rule.enabled"));
    return h(
      "div",
      { class: "setting-row watch-rule" },
      h(
        "div",
        { class: "setting-text" },
        h("strong", null, describe(watch.rule)),
        h("span", { class: "watch-phase" }, badge(t(`watches.phase.${phase}`), PHASE_TONE[phase] ?? ""), reading ? h("span", { class: "hint" }, reading) : null),
        watch.last_notified_unix_ms ? h("span", { class: "hint" }, t("watches.rule.notified", { when: date(watch.last_notified_unix_ms, true) })) : null,
      ),
      h(
        "div",
        { class: "watch-rule-actions" },
        button(t("watches.rule.edit"), { size: "sm", variant: "ghost", onClick: () => editWatch(watch) }),
        button(t("watches.rule.delete"), { size: "sm", variant: "ghost", onClick: () => remove(watch) }),
        enabled,
      ),
    );
  }

  async function remove(watch) {
    const yes = await dialog(
      (close) => [
        h("h2", { class: "dialog-title" }, t("watches.delete.title")),
        h("p", null, t("watches.delete.body", { rule: describe(watch.rule) })),
        h("div", { class: "dialog-actions" }, button(t("common.cancel"), { onClick: () => close(false) }), button(t("watches.rule.delete"), { variant: "danger", onClick: () => close(true) })),
      ],
      { label: t("watches.delete.title"), dismissValue: false },
    );
    if (!yes) return;
    try {
      await call("watch_delete", { id: watch.id });
      toast(t("watches.deleted"));
    } catch (error) {
      toast(failureText(error), "bad");
    }
    load();
  }

  async function explain(alert) {
    try {
      const detail = await call("watch_alert_detail", { id: alert.id });
      if (!detail?.source_id) {
        toast(t("select.unavailable"), "bad");
        return;
      }
      const when = date(alert.started_at_unix_ms, true);
      await runSelection(
        { reference: { source_id: detail.source_id, target: { kind: "watch_alert", alert_id: alert.id } }, label: `${describe(alert.rule)} · ${when}`, subject: subject(alert, when) },
        "explain",
      );
    } catch (error) {
      toast(failureText(error), "bad");
    }
  }

  function alertRow(alert) {
    const open = alert.ended_at_unix_ms == null;
    const end = open ? t("watches.alert.open") : t(`watches.close.${alert.close_reason ?? "returned"}`);
    const extreme = alert.extreme_observation;
    return h(
      "li",
      { class: `watch-alert${alert.seen ? "" : " unseen"}` },
      h("span", { class: "watch-alert-icon" }, icon(open ? "alert" : "check")),
      h(
        "div",
        { class: "watch-alert-text" },
        h("strong", null, describe(alert.rule)),
        h("span", { class: "hint" }, t("watches.alert.when", { start: date(alert.started_at_unix_ms, true), end })),
        h("span", { class: "hint" }, t(alert.lower_bound ? "watches.alert.extremeLowerBound" : "watches.alert.extreme", { value: measured(extreme?.value, extreme?.unit) }),
          ["rule_cooldown", "hourly_limit", "disabled", "failed"].includes(alert.notification) ? ` · ${t(`watches.notification.${alert.notification}`)}` : ""),
      ),
      alert.seen ? null : badge(t("watches.alert.new"), "warn"),
      button(t("watches.alert.explain"), { size: "sm", iconName: "sparkle", onClick: () => explain(alert) }),
    );
  }

  function renderRules() {
    const rules = list?.rules ?? [];
    fill(
      rulesHost,
      rules.length
        ? h("div", { class: "card-rows" }, rules.map(ruleRow))
        : h("div", { class: "watch-empty" }, nox("watch", 96), h("p", null, t("watches.empty")), h("p", { class: "hint" }, t("watches.empty.hint"))),
    );
  }

  function renderAlerts() {
    const unseen = list?.unseen_alerts ?? 0;
    fill(
      alertsHost,
      h(
        "div",
        { class: "card-head" },
        h("h2", { class: "card-title" }, icon("bell"), t("watches.alerts.title")),
        unseen > 0 ? button(t("watches.alerts.seen", { count: unseen }), { size: "sm", variant: "ghost", onClick: async () => {
          try {
            await call("watch_alerts_seen", { upToUnixMs: Date.now() });
          } catch (error) {
            toast(failureText(error), "bad");
          }
          load();
        } }) : null,
      ),
      alerts.length ? h("ul", { class: "watch-alert-list" }, alerts.map(alertRow)) : h("p", { class: "hint" }, t("watches.alerts.none")),
      alerts.length < total ? button(t("watches.alerts.more", { count: total - alerts.length }), { size: "sm", variant: "ghost", onClick: () => load({ more: true }) }) : null,
    );
  }

  function renderSummary() {
    const content = summary
      ? [
        h("p", null, t("watches.summary.line", { total: summary.total, open: summary.open, returned: summary.returned, interrupted: summary.interrupted })),
        Object.keys(summary.by_measure).length
          ? h("ul", { class: "watch-summary-list" }, Object.entries(summary.by_measure).map(([kind, count]) => h("li", null, t("watches.summary.measure", { what: t(`watch.kind.${kind}`), count }))))
          : null,
        h("p", { class: "hint" }, t("watches.summary.period", { from: date(summary.from_unix_ms), to: date(summary.to_unix_ms) }), " · ", t("watches.summary.local")),
      ]
      : [h("p", { class: "hint" }, t("watches.summary.hint"))];
    fill(
      summaryHost,
      h(
        "div",
        { class: "card-head" },
        h("h2", { class: "card-title" }, icon("list"), t("watches.summary.title")),
        button(t(summary ? "watches.summary.refresh" : "watches.summary.show"), { size: "sm", variant: "ghost", onClick: async () => {
          try {
            summary = await call("watch_weekly_summary");
          } catch (error) {
            toast(failureText(error), "bad");
          }
          if (!disposed) renderSummary();
        } }),
      ),
      ...content,
    );
  }

  async function testNotification() {
    try {
      const result = await call("watch_notification_test");
      toast(t(`watches.test.${result}`), result === "sent" ? "good" : "bad");
    } catch (error) {
      toast(failureText(error), "bad");
    }
  }

  function render() {
    renderStatus();
    renderRules();
    renderAlerts();
    renderSummary();
  }

  fill(
    page,
    h(
      "div",
      { class: "page-head" },
      h("div", null, h("h1", { class: "page-title" }, t("watches.title")), h("p", { class: "page-sub" }, t("watches.sub"))),
      h(
        "div",
        { class: "watch-head-actions" },
        button(t("watches.test"), { size: "sm", variant: "ghost", iconName: "bell", onClick: testNotification }),
        button(t("watches.new"), { iconName: "plus", variant: "primary", onClick: () => requestWatch(defaultRule()) }),
      ),
    ),
    h(
      "div",
      { class: "stack watch-column" },
      statusHost,
      h("section", { class: "card card-pad" }, h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("activity"), t("watches.rules.title"))), rulesHost),
      h("section", { class: "card card-pad" }, alertsHost),
      h("section", { class: "card card-pad" }, summaryHost),
    ),
  );
  render();
  load();

  return () => {
    disposed = true;
    window.clearInterval(timer);
    window.removeEventListener("watches:changed", changed);
  };
}
