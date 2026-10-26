// Live measurements: the one-line PC status and the "Live PC" tab of the panel. Values are
// formatted here from their raw numbers; a missing value says why instead of showing zero.
import { h, s } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { applicationName, missingReason, processName } from "../phrases.js";
import { ago, basisPoints, bytes, percent } from "../format.js";
import { batteryFacts, batteryItems, trendCaption } from "./battery.js";
import { failureDetail, failureText } from "../failure.js";
import { almostFull, driveName, gpuName } from "./model.js";

/** A small line chart of the last samples; nothing is drawn with fewer than two values. */
export function sparkline(values, tone) {
  const points = values.filter((v) => v != null);
  if (points.length < 2) return h("span", { class: "spark" });
  const max = 1;
  const step = 96 / (points.length - 1);
  const coords = points.map((v, i) => `${(i * step).toFixed(1)},${(26 - Math.max(0, Math.min(max, v)) * 24).toFixed(1)}`).join(" ");
  return s("svg", { class: ["spark", tone], viewBox: "0 0 96 28", width: 96, height: 28, "aria-hidden": "true" }, s("polyline", { points: coords }));
}

function unknown(measure) {
  return h("span", { class: "unknown", title: missingReason(measure) }, t("metric.notMeasured"));
}

/** A value the source did not give, in the same words as the deeper checks. */
export function notReported(title) {
  return h("span", { class: "unknown", title: title || null }, t("admin.notReported"));
}

function metric({ iconName, label, value, detail, tone, history }) {
  return h(
    "div",
    { class: "live-row" },
    h("span", { class: "live-icon" }, icon(iconName)),
    h(
      "div",
      { class: "live-text" },
      h("div", { class: "live-line" }, h("span", { class: "live-label" }, label), h("span", { class: ["live-value", tone] }, value)),
      h("span", { class: "live-detail" }, detail),
    ),
    sparkline(history || [], tone),
  );
}

/**
 * The "Live PC" tab: memory, processor, drives, graphics cards, each system battery and the
 * heaviest programs.
 */
export function liveView(live, options) {
  const { onPause } = options;
  const dash = live.dash;
  const history = live.history;
  const head = h(
    "div",
    { class: "live-head" },
    h("span", { class: ["dot", live.paused ? "neutral" : "live"] }),
    h("span", null, live.paused ? t("live.paused") : dash ? `${t("live.updated", { ago: ago(dash.collected_at_unix_ms) })} · ${t("live.every")}` : t("health.measuring")),
    h("span", { class: "spacer" }),
    h("button", { type: "button", class: "icon-btn", title: t(live.paused ? "live.resume" : "live.pause"), "aria-label": t(live.paused ? "live.resume" : "live.pause"), onclick: onPause }, icon(live.paused ? "play" : "pause")),
  );
  if (!dash) return h("div", { class: "live" }, head, live.error ? h("p", { class: "note", title: failureDetail(live.error) }, failureText(live.error, "failure.live_unavailable")) : h("div", { class: "skeleton", style: { height: "180px" } }));

  const m = dash.memory;
  const rows = [];
  const total = m.total.value;
  const available = m.available.value;
  rows.push(
    metric({
      iconName: "memory",
      label: t("metric.memory"),
      value: m.used_ratio != null ? percent(m.used_ratio) : unknown(m.total),
      detail: total != null && available != null ? t("live.memory.detail", { used: bytes(total - available), total: bytes(total) }) : "",
      tone: m.used_ratio >= 0.9 ? "warn" : "",
      history: history.map((d) => d.memory.used_ratio),
    }),
  );
  const busy = dash.cpu.busy;
  rows.push(
    metric({
      iconName: "cpu",
      label: t("metric.cpu"),
      value: busy.value != null ? basisPoints(busy.value) : h("span", { class: "pending", title: missingReason(busy) }, "…"),
      detail: dash.cpu.busiest_core.value != null ? t("metric.cpu.core", { value: basisPoints(dash.cpu.busiest_core.value) }) : "",
      tone: busy.value >= 9000 ? "warn" : "",
      history: history.map((d) => (d.cpu.busy.value != null ? d.cpu.busy.value / 10000 : null)),
    }),
  );
  for (const v of dash.volumes || []) {
    const full = almostFull(v);
    rows.push(
      metric({
        iconName: "drive",
        label: t("live.drive", { drive: driveName(v.root) }),
        value: percent(1 - v.free_ratio, 1),
        detail: t("live.drive.detail", { free: bytes(v.free_bytes), total: bytes(v.total_bytes) }),
        tone: full ? "warn" : "",
        history: history.map((d) => {
          const same = (d.volumes || []).find((x) => x.root === v.root);
          return same ? 1 - same.free_ratio : null;
        }),
      }),
    );
  }
  for (const [index, g] of dash.gpus.entries()) {
    const load = g.utilization.value;
    const details = [
      g.memory_used.value != null && g.memory_total.value != null ? t("metric.gpu.memory", { used: bytes(g.memory_used.value), total: bytes(g.memory_total.value) }) : null,
      g.temperature.value != null ? `${g.temperature.value} °C` : null,
    ].filter(Boolean);
    rows.push(
      metric({
        iconName: "gpu",
        label: gpuName(g.name),
        value: load != null ? basisPoints(load, 0) : g.temperature.value != null ? `${g.temperature.value} °C` : unknown(g.utilization),
        detail: details.join(" · "),
        tone: g.slowdown.value ? "warn" : "",
        history: history.map((d) => (d.gpus[index]?.utilization.value != null ? d.gpus[index].utilization.value / 10000 : null)),
      }),
    );
  }
  for (const item of batteryItems(dash, history)) {
    rows.push(metric({ iconName: "battery", label: item.label, value: item.value ?? notReported(), detail: item.detail, tone: item.warn ? "warn" : "", history: item.history }));
  }

  const hidden = dash.process_coverage.permission_denied;
  return h(
    "div",
    { class: "live" },
    head,
    h("div", { class: "live-rows" }, rows),
    applicationsView(live, options),
    hidden ? h("p", { class: "note" }, t("procs.hidden", { count: hidden })) : null,
  );
}

/**
 * The inspector of a selected battery card: its facts (unknown ones as not reported), full versus
 * design capacity, the power over the reads this window made, and what the numbers mean.
 */
export function batteryInspector(card) {
  const b = card.battery;
  if (!b) return null;
  const { rows, capacity, notes } = batteryFacts(b);
  const trend = card.trend || [];
  // Zero at the bottom and the largest power at mid-height, so the line sits beside its caption.
  const scale = 2 * Math.max(1, ...trend.map((p) => Math.abs(p.power)));
  return [
    h("dl", { class: "insp-kv battery-facts" }, rows.map(([label, value]) => [h("dt", null, label), h("dd", null, value ?? notReported())])),
    capacity ? h("div", { class: "battery-capacity" }, h("span", null, capacity.text), h("span", { class: "meter" }, h("span", { style: { width: `${capacity.ratio * 100}%` } }))) : null,
    trend.length >= 2 ? h("div", { class: "battery-trend" }, h("span", null, trendCaption(trend)), sparkline(trend.map((p) => Math.abs(p.power) / scale))) : null,
    h("div", { class: "battery-notes" }, notes.map((note) => h("p", { class: "note" }, note))),
  ];
}

/** Value used to rank and size a row: memory in bytes or CPU in basis points; unknown last. */
function rankValue(app, sort) {
  return (sort === "cpu" ? app.cpu.value : app.working_set.value) ?? -1;
}

export function measureText(measure, kind) {
  if (measure?.value == null) return h("span", { class: "unknown", title: missingReason(measure) }, "—");
  return kind === "bytes" ? bytes(measure.value) : basisPoints(measure.value);
}

/**
 * Programs grouped by application, with their own sums, members and services. A group is never
 * presented as one process: its members stay one click away with their own values.
 */
function applicationsView(live, { sort = "memory", onSort, expanded, onToggle }) {
  const activity = live.activity;
  const head = h(
    "div",
    { class: "apps-head" },
    h("h3", { class: "caption" }, t("live.apps")),
    h("span", { class: "spacer" }),
    h(
      "div",
      { class: "mini-tabs", role: "group", "aria-label": t("live.apps.sort") },
      ["memory", "cpu"].map((key) =>
        h("button", { type: "button", "aria-pressed": String(sort === key), onclick: () => onSort?.(key) }, t(`live.apps.by.${key}`)),
      ),
    ),
  );
  if (!activity) {
    return h("div", { class: "apps" }, head, h("div", { class: "skeleton", style: { height: "120px" } }));
  }
  const apps = [...activity.applications].sort((a, b) => rankValue(b, sort) - rankValue(a, sort)).slice(0, 8);
  const max = Math.max(1, ...apps.map((a) => rankValue(a, sort)));
  return h(
    "div",
    { class: "apps" },
    head,
    apps.map((app) => {
      const open = expanded === app.entity_ref;
      const share = Math.max(0, rankValue(app, sort)) / max;
      return h(
        "div",
        { class: ["app-group", open ? "open" : ""] },
        h(
          "button",
          { type: "button", class: "app-main", "aria-expanded": String(open), onclick: () => onToggle?.(open ? null : app.entity_ref) },
          h("span", { class: "app-name", title: applicationName(app) }, app.readable_name || applicationName(app)),
          h("span", { class: "app-count" }, t(app.process_count === 1 ? "live.apps.one" : "live.apps.many", { count: app.process_count })),
          h("span", { class: "app-value" }, sort === "cpu" ? measureText(app.cpu, "cpu") : measureText(app.working_set, "bytes")),
          h("div", { class: "meter" }, h("span", { style: { width: `${share * 100}%` } })),
        ),
        open
          ? h(
              "div",
              { class: "app-detail" },
              h(
                "dl",
                { class: "kv" },
                h("dt", null, t("metric.memory")),
                h("dd", null, measureText(app.working_set, "bytes")),
                h("dt", null, t("metric.cpu")),
                h("dd", null, measureText(app.cpu, "cpu")),
                app.working_set_increase?.value ? h("dt", null, t("live.apps.grew")) : null,
                app.working_set_increase?.value ? h("dd", null, `+${bytes(app.working_set_increase.value)}`) : null,
                h("dt", null, t("live.apps.gpu")),
                h("dd", null, measureText(app.gpu, "cpu")),
              ),
              app.services.length
                ? h("div", { class: "app-services" }, h("span", { class: "caption" }, t("live.apps.services")), app.services.map((sv) => h("span", { class: "tag", title: sv.name }, sv.display_name || sv.name)))
                : null,
              h(
                "div",
                { class: "app-members" },
                h("span", { class: "caption" }, t("live.apps.members")),
                app.members.map((m) =>
                  h(
                    "div",
                    { class: "member" },
                    h("span", { class: "member-name", title: processName(m) }, `${m.readable_name || processName(m)} · PID ${m.pid}`),
                    h("span", null, measureText(m.cpu, "cpu")),
                    h("span", null, measureText(m.working_set, "bytes")),
                  ),
                ),
                app.process_count > app.members.length ? h("span", { class: "note" }, t("live.apps.others", { count: app.process_count - app.members.length })) : null,
              ),
            )
          : null,
      );
    }),
    activity.applications_total > apps.length ? h("p", { class: "note" }, t("live.apps.total", { shown: apps.length, total: activity.applications_total })) : null,
    h("p", { class: "note" }, t("live.apps.note")),
  );
}
