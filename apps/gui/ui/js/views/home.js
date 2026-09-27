// Home: Iris, the question box and live measurements.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { mascotWithBubble } from "../mascot.js";
import { composer, suggestions, ask, cloudReady } from "../ask.js";
import { getPrefs } from "../prefs.js";
import { subscribe } from "../store.js";
import { bar, iconButton } from "../ui.js";
import { ago, basisPoints, bytes, number, percent } from "../format.js";

const REFRESH_MS = 3000;
const SLEEP_AFTER_MS = 3 * 60 * 1000;

// Values are formatted here from their raw numbers so the interface language decides the
// decimal mark; a missing value says why instead of showing zero.
const val = (measure) => (measure?.value != null ? measure.value : null);
const size = (measure) => (val(measure) != null ? bytes(measure.value) : null);
const ms = (measure) => (val(measure) != null ? `${number(measure.value / 1000, 1)} ms` : null);

function notMeasured(measure) {
  return h("span", { class: "unknown", title: measure?.note || "" }, t("metric.notMeasured"));
}

/** `pending` shows a quiet placeholder while a first sample is being taken. */
function metricCard({ title, iconName, tone, value, unit, ratio, barTone, lines, extra, pending = false, note }) {
  return h(
    "article",
    { class: "card metric" },
    h("div", { class: "metric-head" }, h("span", { class: ["metric-icon", tone] }, icon(iconName)), h("span", { class: "metric-name", title }, title)),
    h(
      "div",
      { class: "metric-value" },
      value ?? h("span", { class: pending ? "pending" : "unknown", title: note || "" }, pending ? "…" : t("metric.notMeasured")),
      value != null && unit ? h("small", null, unit) : null,
    ),
    ratio != null ? bar(ratio, barTone) : h("div", { class: "bar" }),
    h("div", { class: "metric-lines" }, lines.filter(Boolean)),
    extra,
  );
}

function memoryCard(m) {
  const lines = [];
  const total = val(m.total);
  const available = val(m.available);
  if (total != null && available != null) lines.push(h("span", null, t("metric.memory.used", { used: bytes(total - available), total: bytes(total) })));
  if (available != null) lines.push(h("span", null, t("metric.memory.available", { value: bytes(available) })));
  if (val(m.commit) != null && val(m.commit_limit) != null) lines.push(h("span", { class: "faint" }, t("metric.memory.commit", { commit: size(m.commit), limit: size(m.commit_limit) })));
  if (!lines.length) lines.push(notMeasured(m.total));
  return metricCard({
    title: t("metric.memory"),
    iconName: "memory",
    value: m.used_ratio != null ? percent(m.used_ratio) : null,
    unit: t("metric.memory.inUse"),
    ratio: m.used_ratio,
    barTone: m.used_ratio > 0.9 ? "warn" : "",
    lines,
    note: m.total.note,
  });
}

function cpuCard(c) {
  const busy = val(c.busy);
  const first = busy == null && c.busy.status === "partial";
  const lines = [];
  if (busy == null) lines.push(h("span", { class: "faint", title: c.busy.note || "" }, first ? t("metric.cpu.first") : t("metric.notMeasured")));
  if (val(c.busiest_core) != null) lines.push(h("span", null, t("metric.cpu.core", { value: basisPoints(c.busiest_core.value) })));
  return metricCard({
    title: t("metric.cpu"),
    iconName: "cpu",
    tone: "info",
    value: busy != null ? basisPoints(busy) : null,
    unit: t("metric.cpu.busy"),
    ratio: busy != null ? busy / 10000 : null,
    lines,
    pending: first,
    note: c.busy.note,
  });
}

function diskCard(d) {
  const lines = [];
  if (val(d.read_latency) != null || val(d.write_latency) != null) {
    lines.push(h("span", null, t("metric.disk.latency", { read: ms(d.read_latency) ?? "?", write: ms(d.write_latency) ?? "?" })));
  }
  if (val(d.hard_faults) != null) lines.push(h("span", { class: "faint" }, t("metric.disk.paging", { value: t("unit.pagesPerSecond", { value: number(d.hard_faults.value / 100, 1) }) })));
  if (!lines.length) lines.push(notMeasured(d.idle));
  return metricCard({
    title: t("metric.disk"),
    iconName: "disk",
    tone: "mint",
    value: d.busy_ratio != null ? percent(d.busy_ratio, 1) : null,
    unit: t("metric.disk.busy"),
    ratio: d.busy_ratio,
    barTone: "mint",
    lines,
    pending: d.idle.status === "partial",
    note: d.idle.note,
  });
}

function gpuName(name) {
  // "NVIDIA GeForce RTX 4060 Laptop GPU" reads well as "GeForce RTX 4060 Laptop".
  return name.replace(/^NVIDIA\s+/i, "").replace(/\s+GPU$/i, "");
}

function gpuCards(dash) {
  if (dash.gpus.length) {
    return dash.gpus.map((g) => {
      const lines = [];
      if (val(g.memory_used) != null && val(g.memory_total) != null) lines.push(h("span", null, t("metric.gpu.memory", { used: size(g.memory_used), total: size(g.memory_total) })));
      // Hybrid laptops can refuse the load while the GPU sleeps: show its temperature instead.
      const load = val(g.utilization);
      const temperature = val(g.temperature);
      const headlineIsTemperature = load == null && temperature != null;
      if (load == null) lines.push(h("span", { class: "faint", title: g.utilization.note || "" }, t("metric.gpu.loadUnknown")));
      const details = [
        temperature != null && !headlineIsTemperature ? `${temperature} °C` : null,
        val(g.sm_clock) != null ? `${number(g.sm_clock.value)} MHz` : null,
        val(g.power) != null ? `${number(g.power.value / 1000, 1)} W` : null,
      ].filter(Boolean);
      if (details.length) lines.push(h("span", { class: "faint" }, details.join(" · ")));
      let slowdown = null;
      if (g.slowdown.value === 0) slowdown = h("span", { class: "badge good" }, icon("check"), t("metric.gpu.noSlowdown"));
      else if (g.slowdown.value != null) slowdown = h("span", { class: "badge warn" }, icon("thermo"), t("metric.gpu.slowdown", { reasons: g.slowdown_reasons.join(", ") }));
      return metricCard({
        title: gpuName(g.name),
        iconName: "gpu",
        tone: "warn",
        value: load != null ? basisPoints(load, 0) : headlineIsTemperature ? `${temperature} °C` : null,
        unit: load != null ? t("metric.cpu.busy") : t("metric.gpu.temperature"),
        ratio: g.memory_ratio,
        lines,
        extra: slowdown ? h("div", null, slowdown) : null,
        note: g.utilization.note,
      });
    });
  }
  const busiest = dash.gpu_busiest_engine;
  const top = dash.gpu_engines.filter((e) => e.usage.value != null).slice(0, 2);
  return [
    metricCard({
      title: t("metric.gpu"),
      iconName: "gpu",
      tone: "warn",
      value: val(busiest) != null ? basisPoints(busiest.value) : null,
      unit: t("metric.cpu.busy"),
      ratio: val(busiest) != null ? busiest.value / 10000 : null,
      lines: top.length ? top.map((e) => h("span", null, `${e.kind} ${basisPoints(e.usage.value)}`)) : [h("span", { class: "faint" }, t("metric.gpu.none"))],
      note: busiest.note,
    }),
  ];
}

function processesCard(dash) {
  const rows = dash.processes.filter((p) => p.working_set.value != null).slice(0, 7);
  const max = Math.max(1, ...rows.map((p) => p.working_set.value));
  const hidden = dash.process_coverage.permission_denied;
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h3", { class: "card-title" }, icon("list"), t("procs.title")), h("span", { class: "hint", title: t("procs.note") }, icon("info"))),
    rows.length
      ? h(
          "div",
          { class: "list" },
          rows.map((p) =>
            h(
              "div",
              { class: "list-row" },
              h("div", { class: "list-name" }, icon("file"), h("div", { class: "text" }, h("strong", { title: p.name }, p.name), h("span", null, `PID ${p.pid}`))),
              bar(p.working_set.value / max),
              h("div", { class: "list-value" }, bytes(p.working_set.value)),
            ),
          ),
        )
      : h("p", { class: "muted" }, t("procs.empty")),
    hidden ? h("p", { class: "note", style: { "margin-top": "10px" } }, t("procs.hidden", { count: hidden })) : null,
  );
}

export function mount(page) {
  const iris = mascotWithBubble({ size: 150, label: t("mascot.label") });
  iris.say(t("say.idle"));
  const box = composer({
    onFocusChange: (focused) => {
      iris.set(focused ? "listening" : "idle");
      iris.say(t(focused ? "say.listening" : "say.idle"));
    },
  });
  const pick = (question) => ask(question, { cloud: getPrefs().mode === "cloud" && cloudReady() });

  const updated = h("span", { class: "tabular" });
  const liveDot = h("span", { class: "dot live" });
  let paused = false;
  const pauseButton = iconButton("pause", t("live.pause"), () => {
    paused = !paused;
    fill(pauseButton, icon(paused ? "play" : "pause"));
    pauseButton.title = t(paused ? "live.resume" : "live.pause");
    pauseButton.setAttribute("aria-label", pauseButton.title);
    liveDot.classList.toggle("live", !paused);
    if (!paused) tick();
    else updated.textContent = t("live.paused");
  });
  const grid = h(
    "div",
    { class: "grid grid-metrics" },
    [0, 1, 2, 3].map(() => h("div", { class: "card metric" }, h("div", { class: "skeleton", style: { height: "22px", width: "50%" } }), h("div", { class: "skeleton", style: { height: "30px", width: "40%" } }), h("div", { class: "skeleton", style: { height: "8px" } }))),
  );
  const procs = h("div", { class: "section" });

  fill(
    page,
    h(
      "section",
      { class: "card hero" },
      iris.el,
      h("div", null, h("p", { class: "muted", style: { "font-weight": "600" } }, t("home.hello")), h("h1", { class: "hero-title" }, t("home.title")), h("p", { class: "hero-sub" }, t("home.sub")), box.el, suggestions(pick)),
    ),
    h(
      "section",
      { class: "section" },
      h("div", { class: "section-head" }, h("h2", { class: "section-title" }, t("live.title")), h("div", { class: "section-meta" }, liveDot, updated, pauseButton)),
      grid,
    ),
    procs,
  );

  let last = null;
  let timer = null;
  let stamp = null;
  let disposed = false;
  const render = (dash) => {
    fill(grid, memoryCard(dash.memory), cpuCard(dash.cpu), diskCard(dash.disk), ...gpuCards(dash));
    fill(procs, processesCard(dash));
  };
  const tick = async () => {
    window.clearTimeout(timer);
    if (disposed || paused) return;
    if (!document.hidden) {
      try {
        last = await call("dashboard");
        if (disposed) return;
        render(last);
      } catch (error) {
        updated.textContent = t("live.error", { message: error.message });
      }
    }
    if (!disposed && !paused) timer = window.setTimeout(tick, REFRESH_MS);
  };
  stamp = window.setInterval(() => {
    if (last && !paused) updated.textContent = t("live.updated", { ago: ago(last.collected_at_unix_ms) });
  }, 1000);
  tick();

  // Iris dozes off when nobody uses the window for a while.
  let sleeper = null;
  const wake = () => {
    window.clearTimeout(sleeper);
    if (iris.el.querySelector(".iris")?.dataset.state === "sleeping") {
      iris.set("idle");
      iris.say(t("say.idle"));
    }
    sleeper = window.setTimeout(() => {
      iris.set("sleeping");
      iris.say(t("say.sleeping"));
    }, SLEEP_AFTER_MS);
  };
  const activity = ["pointermove", "keydown", "wheel"];
  activity.forEach((type) => window.addEventListener(type, wake, { passive: true }));
  wake();
  const onVisible = () => {
    if (!document.hidden) tick();
  };
  document.addEventListener("visibilitychange", onVisible);
  const unsubscribe = subscribe((topic) => {
    if (topic === "info") box.refresh();
  });

  return () => {
    disposed = true;
    window.clearTimeout(timer);
    window.clearInterval(stamp);
    window.clearTimeout(sleeper);
    activity.forEach((type) => window.removeEventListener(type, wake));
    document.removeEventListener("visibilitychange", onVisible);
    unsubscribe();
  };
}
