// Live measurements: the one-line PC status and the "Live PC" tab of the panel. Values are
// formatted here from their raw numbers; a missing value says why instead of showing zero.
import { h, s } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { ago, basisPoints, bytes, percent } from "./format.js";

const GiB = 1024 ** 3;

/** A drive is "almost full" under 5 % or 5 GiB free: a hint for the status line, not a finding. */
function almostFull(volume) {
  return volume.free_ratio < 0.05 || volume.free_bytes < 5 * GiB;
}

export function driveName(root) {
  return (root || "").replace(/[\\/]+$/, "");
}

/** The most important thing to know right now, in a few words. */
export function health(dash) {
  if (!dash) return { tone: "neutral", text: t("health.measuring") };
  const full = (dash.volumes || []).filter(almostFull).sort((a, b) => a.free_ratio - b.free_ratio)[0];
  if (full) return { tone: "warn", text: t("health.diskFull", { drive: driveName(full.root) }), drive: driveName(full.root) };
  if (dash.memory.used_ratio != null && dash.memory.used_ratio >= 0.9) return { tone: "warn", text: t("health.memory") };
  if (dash.cpu.busy.value != null && dash.cpu.busy.value >= 9000) return { tone: "warn", text: t("health.cpu") };
  if (dash.gpus.some((g) => g.slowdown.value != null && g.slowdown.value !== 0)) return { tone: "warn", text: t("health.gpu") };
  return { tone: "good", text: t("health.ok") };
}

/** A small line chart of the last samples; nothing is drawn with fewer than two values. */
function sparkline(values, tone) {
  const points = values.filter((v) => v != null);
  if (points.length < 2) return h("span", { class: "spark" });
  const max = 1;
  const step = 96 / (points.length - 1);
  const coords = points.map((v, i) => `${(i * step).toFixed(1)},${(26 - Math.max(0, Math.min(max, v)) * 24).toFixed(1)}`).join(" ");
  return s("svg", { class: ["spark", tone], viewBox: "0 0 96 28", width: 96, height: 28, "aria-hidden": "true" }, s("polyline", { points: coords }));
}

function unknown(measure) {
  return h("span", { class: "unknown", title: measure?.note || "" }, t("metric.notMeasured"));
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

function gpuName(name) {
  return name.replace(/^NVIDIA\s+/i, "").replace(/\s+GPU$/i, "");
}

/** The "Live PC" tab: memory, processor, drives, graphics cards and the heaviest programs. */
export function liveView(live, { onPause }) {
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
  if (!dash) return h("div", { class: "live" }, head, live.error ? h("p", { class: "note" }, t("live.error", { message: live.error })) : h("div", { class: "skeleton", style: { height: "180px" } }));

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
      value: busy.value != null ? basisPoints(busy.value) : h("span", { class: "pending", title: busy.note || "" }, "…"),
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

  const procs = dash.processes.filter((p) => p.working_set.value != null).slice(0, 6);
  const max = Math.max(1, ...procs.map((p) => p.working_set.value));
  const hidden = dash.process_coverage.permission_denied;
  return h(
    "div",
    { class: "live" },
    head,
    h("div", { class: "live-rows" }, rows),
    h("h3", { class: "caption" }, t("live.procs")),
    h(
      "div",
      { class: "live-procs" },
      procs.map((p) =>
        h(
          "div",
          { class: "proc" },
          h("span", { class: "proc-name", title: `${p.name} · PID ${p.pid}` }, p.name),
          h("span", { class: "proc-value" }, bytes(p.working_set.value)),
          h("div", { class: "meter" }, h("span", { style: { width: `${(p.working_set.value / max) * 100}%` } })),
        ),
      ),
    ),
    hidden ? h("p", { class: "note" }, t("procs.hidden", { count: hidden })) : null,
  );
}
