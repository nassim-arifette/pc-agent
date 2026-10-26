// History: what the local sampler measured over time, gaps included, the anomalies it detected,
// a recording taken during a problem, and what the collection itself costs. Nothing here is sent
// anywhere; selecting an event or a finished recording shows the action bar under it, whose
// Explain and Investigate go through the selected-evidence preview (select.js).
import { h, s, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { findReference, runSelection } from "../select.js";
import { actionBar, keepFocus, selectionKeys } from "../actionbar.js";
import { subscribe } from "../store.js";
import { button, segmented, toast, toggle, dialog } from "../ui.js";
import { bytes, clock, date, duration, number, percent } from "../format.js";
import { hoursMinutes, watts } from "../live/battery.js";
import { failureDetail, failureText } from "../failure.js";

const PERIODS = [
  { id: "hour", ms: 3600 * 1000, label: "history.period.hour" },
  { id: "day", ms: 24 * 3600 * 1000, label: "history.period.day" },
  { id: "week", ms: 7 * 24 * 3600 * 1000, label: "history.period.week" },
];

// Each row is a 0..1 share of a resource; memory and disk are derived from two stored metrics.
const ROWS = [
  { id: "cpu", label: "metric.cpu", color: "res-cpu", metrics: ["cpu_busy_basis_points"], value: ([v]) => v / 10000 },
  {
    id: "memory",
    label: "metric.memory",
    color: "res-mem",
    metrics: ["physical_available_bytes", "physical_total_bytes"],
    value: ([available, total]) => (total ? 1 - available / total : null),
  },
  { id: "disk", label: "history.disk", color: "res-disk", metrics: ["disk_idle_basis_points"], value: ([idle]) => 1 - idle / 10000 },
  { id: "gpu", label: "history.gpu", color: "res-gpu", metrics: ["gpu_busiest_engine_basis_points"], value: ([v]) => v / 10000 },
];

const EVENT_ICONS = { cpu_busy: "cpu", memory_pressure: "memory", disk_latency: "drive", disk_busy: "drive" };

// Rounding must not say "100% not measured" next to measured values, nor "0%" for a real gap.
function gapsHint(share) {
  if (!(share > 0)) return t("history.noGaps");
  if (share < 0.005) return t("history.gapsFew");
  if (share >= 0.995 && share < 1) return t("history.gapsAlmostAll");
  return t("history.gaps", { share: percent(share) });
}

/**
 * Explains or investigates only the stored event or recording named by a host-issued source.
 * An expired capture must be read again before either action can use its measurements.
 */
async function runStoredSelection(match, { label, subject }, intent) {
  const reference = await findReference(match);
  if (reference) return runSelection({ reference, label, subject }, intent);
  toast(t("select.unavailable"), "bad");
}

/**
 * What the action bar and the shortcuts do for a stored event. The reference is checked when
 * invoked. The bar sits under the row's right end, away from the names on its left.
 */
function eventItem(e) {
  const when = clock(e.started_at_unix_ms);
  const run = (intent) =>
    runStoredSelection(
      (target) => target.kind === "event" && target.event_id === e.id,
      { label: `${eventLabel(e)} · ${when}`, subject: t("history.event.subject", { what: eventLabel(e), when }) },
      intent,
    );
  return { key: `event:${e.id}`, label: `${eventLabel(e)} · ${when}`, ready: true, explain: () => run("explain"), investigate: () => run("investigate"), align: "end" };
}

/** The same for a finished recording. */
function recordingItem(r) {
  const from = clock(r.started_at_unix_ms, { seconds: true });
  const to = r.ended_at_unix_ms ? clock(r.ended_at_unix_ms, { seconds: true }) : from;
  const label = t("history.recording.label", { from, to });
  const run = (intent) =>
    runStoredSelection(
      (target) => target.kind === "recording" && target.recording_id === r.id,
      { label, subject: t("history.recording.subject", { from, to }) },
      intent,
    );
  return { key: `recording:${r.id}`, label, ready: true, explain: () => run("explain"), investigate: () => run("investigate"), align: "end" };
}

// ---------- Battery during a recording ----------

/** The last usable value of `metric` in a battery's points, or null. */
function batteryValue(points, metric, { first = false } = {}) {
  const ordered = first ? points : [...points].reverse();
  for (const point of ordered) {
    const o = point.observations.find((x) => x.metric === metric && x.status === "ok" && x.value != null);
    if (o) return o.value;
  }
  return null;
}

/**
 * What the battery did during a finished recording (battery_recording): charge at the start and
 * end, the last measured rate, and the loss measured between capacity readings when Windows gave
 * them. Nothing is collected again. Null when the PC reported no battery.
 */
function batteryFacts(recording) {
  const batteries = (recording?.batteries || []).filter((b) => b.points?.length);
  if (!batteries.length) return null;
  return batteries.map((battery, index) => {
    const rows = [];
    const first = batteryValue(battery.points, "battery_charge_percent", { first: true });
    const last = batteryValue(battery.points, "battery_charge_percent");
    if (first != null && last != null) rows.push(["history.battery.charge", first === last ? percent(last / 100) : `${percent(first / 100)} → ${percent(last / 100)}`]);
    const state = batteryValue(battery.points, "battery_charge_state");
    const rate = batteryValue(battery.points, "battery_power_rate_mw");
    if (state === "discharging" || state === "charging") {
      const power = rate ?? batteryValue(battery.points, state === "charging" ? "battery_charge_power_mw" : "battery_discharge_power_mw");
      rows.push(["history.battery.rate", power != null ? t(`history.battery.${state}`, { power: watts(power) }) : t(`history.battery.${state}.only`)]);
    } else if (state === "idle") {
      // Idle while unplugged is not "plugged in": same rule as the Live view.
      const plugged = batteryValue(battery.points, "battery_on_ac") === true;
      rows.push(["history.battery.rate", t(plugged ? "history.battery.idle" : "history.battery.idleOnBattery")]);
    }
    // The longest continuous discharge segment carries the measured loss and its derived rate.
    const segment = [...(battery.evolution || [])].sort((a, b) => b.to_unix_ms - b.from_unix_ms - (a.to_unix_ms - a.from_unix_ms))[0];
    const derived = (metric) => segment?.observations.find((o) => o.metric === metric && o.status === "ok" && o.value != null)?.value ?? null;
    const lost = derived("battery_measured_capacity_decrease_mwh");
    const average = derived("battery_average_power_rate_mw");
    if (lost != null && segment) {
      const energy = `${number(lost / 1000, 2)} Wh`;
      const time = duration(segment.to_unix_ms - segment.from_unix_ms);
      rows.push(["history.battery.lost", average != null ? t("history.battery.lostRate", { energy, time, power: watts(average) }) : t("history.battery.lostValue", { energy, time })]);
    }
    const runtime = derived("battery_estimated_runtime_from_recording_seconds");
    if (runtime != null) rows.push(["history.battery.left", t("history.battery.leftValue", { time: hoursMinutes(runtime) })]);
    const windows = batteryValue(battery.points, "battery_estimated_runtime_windows_seconds");
    if (windows != null) rows.push(["history.battery.windows", t("history.battery.windowsValue", { time: hoursMinutes(windows) })]);
    return h(
      "div",
      { class: "rec-battery" },
      h("strong", null, batteries.length > 1 ? t("history.battery.titleN", { n: index + 1 }) : t("history.battery.title")),
      rows.length
        ? h("dl", { class: "rec-battery-facts" }, rows.map(([key, value]) => [h("dt", null, t(key)), h("dd", null, value)]))
        : h("span", null, t("history.battery.none")),
    );
  });
}

function eventLabel(e) {
  const key = `history.event.${e.kind}`;
  const label = t(key);
  return label === key ? e.kind : label;
}

function eventValue(e) {
  if (e.unit === "microseconds") return `${number(e.peak / 1000, 1)} ms`;
  if (e.unit.startsWith("basis_points")) return percent(e.peak / 10000);
  return number(e.peak);
}

/** Join derived measurements by timestamp, never by an unrelated position in another series. */
function rowPoints(row, series) {
  const inputs = series.map((one) => {
    const input = new Map();
    for (const point of one?.points || []) {
      if (!input.has(point.unix_ms)) input.set(point.unix_ms, []);
      input.get(point.unix_ms).push(point);
    }
    return input;
  });
  const times = [...new Set(inputs.flatMap((input) => [...input.keys()]))].sort((a, b) => a - b);
  const samples = times.flatMap((at) => Array.from({ length: Math.max(...inputs.map((input) => input.get(at)?.length || 0)) }, (_, index) => ({ at, index })));
  const gaps = series.flatMap((one) => one?.gaps || []).sort((a, b) => a.from_unix_ms - b.from_unix_ms);
  const mergedGaps = [];
  for (const gap of gaps) {
    const last = mergedGaps.at(-1);
    if (last && gap.from_unix_ms < last.to_unix_ms) last.to_unix_ms = Math.max(last.to_unix_ms, gap.to_unix_ms);
    else mergedGaps.push({ ...gap });
  }
  const bounded = (value) => value == null || !Number.isFinite(value) ? null : Math.max(0, Math.min(1, value));
  let previous = null;
  let gapIndex = 0;
  return samples.map(({ at, index }) => {
    const points = inputs.map((input) => input.get(at)?.[index]);
    const values = points.map((point) => point?.value ?? null);
    while (mergedGaps[gapIndex]?.to_unix_ms <= (previous?.at ?? -Infinity)) gapIndex += 1;
    const gap = mergedGaps[gapIndex];
    const inGap = gap && gap.from_unix_ms < at && gap.to_unix_ms > at;
    // Without a serialized session ID, coincident operands cannot be paired unambiguously.
    const ambiguous = inputs.length > 1 && inputs.some((input) => (input.get(at)?.length || 0) > 1);
    const known = !inGap && !ambiguous && values.every((value) => value != null && Number.isFinite(value));
    const value = known ? bounded(row.value(values)) : null;
    const resolution = Math.max(0, ...points.map((point) => point?.resolution_ms || 0));
    const breakBefore = Boolean(previous && ((gap && gap.from_unix_ms < at && gap.to_unix_ms > previous.at)
      || (resolution && previous.resolution && at - previous.at > Math.max(resolution, previous.resolution) * 2)));
    let min = null;
    let max = null;
    // Independent extrema cannot establish a memory-use range when total capacity changes.
    // A fixed denominator makes both endpoints exact; otherwise keep only the reported mean.
    const stableTotal = row.id !== "memory" || (points[1]?.min === values[1] && points[1]?.max === values[1]);
    if (value != null && stableTotal && points.every((point) => Number.isFinite(point?.min) && Number.isFinite(point?.max))) {
      const ends = [row.value(points.map((point) => point.min)), row.value(points.map((point) => point.max))].map(bounded);
      if (ends.every((end) => end != null)) { min = Math.min(...ends, value); max = Math.max(...ends, value); }
    }
    previous = { at, resolution };
    return { at, value, min, max, breakBefore, weight: points[0]?.observed_duration_ms > 0 ? points[0].observed_duration_ms : null };
  });
}

/** Reduce only the display, retaining extrema and every break in the measured timeline. */
function chartSegments(points, from, to, width) {
  const bins = Math.max(1, Math.floor(width / 2.5));
  const segments = [];
  let segment = [];
  let bucket = null;
  const flushBucket = () => {
    if (!bucket) return;
    segment.push({ at: bucket.at / (bucket.weight || 1), value: bucket.value / (bucket.weight || 1), min: bucket.min, max: bucket.max });
    bucket = null;
  };
  const flushSegment = () => {
    flushBucket();
    if (segment.length) segments.push(segment);
    segment = [];
  };
  for (const point of points) {
    if (point.value == null || point.breakBefore) flushSegment();
    if (point.value == null) continue;
    const bin = Math.floor(((point.at - from) / Math.max(1, to - from)) * bins);
    if (bucket && bucket.bin !== bin) flushBucket();
    const weight = point.weight;
    if (!bucket) bucket = { bin, at: point.at * (weight || 1), value: point.value * (weight || 1), weight, min: point.min, max: point.max };
    else {
      if (bucket.weight && weight) {
        bucket.at += point.at * weight;
        bucket.value += point.value * weight;
        bucket.weight += weight;
      } else {
        // Gauges expose no averaging weights. Keep an original mean from this pixel bin,
        // with the extrema of all its points, rather than inventing a mean across cadences.
        bucket.at = point.at;
        bucket.value = point.value;
        bucket.weight = null;
      }
      bucket.min = bucket.min == null || point.min == null ? null : Math.min(bucket.min, point.min);
      bucket.max = bucket.max == null || point.max == null ? null : Math.max(bucket.max, point.max);
    }
  }
  flushSegment();
  return segments;
}

/** Means and measured ranges share the same segments; neither crosses unknown coverage. */
function chart(points, from, to, events, cls, width = 1000) {
  const W = 1000;
  const H = 54;
  const x = (at) => ((at - from) / Math.max(1, to - from)) * W;
  const y = (v) => H - 3 - v * (H - 6);
  const segments = chartSegments(points, from, to, width);
  const coordinate = (point, value = point.value) => `${x(point.at).toFixed(1)},${y(value).toFixed(1)}`;
  const bands = [];
  for (const segment of segments) {
    let band = [];
    const flush = () => { if (band.length) bands.push(band); band = []; };
    for (const point of segment) {
      if (point.min == null || point.max == null) flush();
      else band.push(point);
    }
    flush();
  }
  return s(
    "svg",
    { class: ["history-chart", cls], viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" },
    events.map((e) => s("rect", { class: "history-mark", x: x(e.started_at_unix_ms).toFixed(1), y: 0, width: Math.max(2, x(e.ended_at_unix_ms) - x(e.started_at_unix_ms)).toFixed(1), height: H })),
    bands.map((band) => band.length === 1
      ? s("rect", { class: "history-range", x: x(band[0].at) - 0.8, y: y(band[0].max), width: 1.6, height: y(band[0].min) - y(band[0].max) })
      : s("polygon", { class: "history-range", points: [...band.map((point) => coordinate(point, point.max)), ...band.slice().reverse().map((point) => coordinate(point, point.min))].join(" ") })),
    segments.map((segment) => (segment.length === 1 ? s("circle", { cx: x(segment[0].at), cy: y(segment[0].value), r: 2 }) : s("polyline", { points: segment.map((point) => coordinate(point)).join(" ") }))),
  );
}

function axis(from, to) {
  const ticks = 6;
  return h(
    "div",
    { class: "history-axis" },
    Array.from({ length: ticks + 1 }, (_, i) => {
      const at = from + ((to - from) * i) / ticks;
      return h("span", null, to - from > 2 * 24 * 3600 * 1000 ? date(at) : clock(at));
    }),
  );
}

export function mount(page) {
  let period = "day";
  let disposed = false;
  let loadRequest = 0;
  let statusRequest = 0;
  let loading = false;
  let loaded = null;
  let chartWidth = 0;
  let status = null;
  let startup = null;
  // The selected event or finished recording: { kind: "event" | "recording", id }.
  let selected = null;
  // The events drawn last; null until the first load succeeds.
  let shownEvents = null;
  let poll = null;
  // The sampler reports the active recording; once it ends the view keeps showing it so the
  // person can ask about it.
  let lastRecording = null;
  let cancelledByUs = false;
  // The battery points of the finished recording shown, read once from the local store.
  let battery = { id: null, data: null };
  const head = h("div", { class: "page-head" });
  const recordHost = h("div");
  const select = (next) => {
    selected = next;
    if (shownEvents) renderEvents(shownEvents);
    renderRecording();
  };
  const chartsHost = h("section", { class: "card card-pad history-card" });
  const eventsHost = h("section", { class: "card card-pad" });
  const costHost = h("section", { class: "card card-pad" });
  fill(page, head, recordHost, chartsHost, eventsHost, costHost);

  const renderHead = () => {
    fill(
      head,
      h("div", null, h("h1", { class: "page-title" }, t("history.title.page")), h("p", { class: "page-sub" }, t("history.sub"))),
      h(
        "div",
        { class: "row" },
        segmented(
          PERIODS.map((p) => ({ value: p.id, label: t(p.label) })),
          period,
          (value) => {
            period = value;
            renderHead();
            load();
          },
          t("history.period"),
        ),
        status?.recording && status.recording.ended_at_unix_ms == null ? null : button(t("history.record"), { variant: "primary", iconName: "record", onClick: startRecording }),
      ),
    );
  };

  // ---------- Recording ----------

  const startRecording = async () => {
    const seconds = await dialog(
      (close) => [
        h("h2", { class: "dialog-title" }, t("history.record.title")),
        h("p", null, t("history.record.lead")),
        h(
          "div",
          { class: "row wrap" },
          [30, 60, 120].map((n) => button(t("history.record.seconds", { n }), { variant: n === 60 ? "primary" : "", onClick: () => close(n) })),
        ),
        h("div", { class: "dialog-actions" }, button(t("common.cancel"), { onClick: () => close(null) })),
      ],
      { label: t("history.record.title"), dismissValue: null },
    );
    if (!seconds || disposed) return;
    cancelledByUs = false;
    try {
      await call("record_activity_start", { durationSeconds: seconds });
      refreshStatus();
    } catch (error) {
      toast(failureText(error), "bad");
    }
  };

  const renderRecording = () => keepFocus(recordHost, drawRecording);
  const drawRecording = () => {
    const r = status?.recording ?? lastRecording;
    if (!r) {
      if (selected?.kind === "recording") selected = null;
      fill(recordHost);
      return;
    }
    const running = r.ended_at_unix_ms == null;
    if (!running && battery.id !== r.id) {
      battery = { id: r.id, data: null };
      call("battery_recording", { recordingId: r.id })
        .then((data) => {
          if (disposed || battery.id !== r.id) return;
          battery.data = data;
          renderRecording();
        })
        .catch(() => {});
    }
    const elapsed = Math.min(r.duration_seconds * 1000, Date.now() - r.started_at_unix_ms);
    const left = Math.max(0, r.duration_seconds * 1000 - elapsed);
    // A recording lasts seconds to minutes: minute precision would read "from 20:01 to 20:01".
    const from = clock(r.started_at_unix_ms, { seconds: true });
    const to = r.ended_at_unix_ms ? clock(r.ended_at_unix_ms, { seconds: true }) : null;
    const title = running ? t("history.recording", { left: duration(left) }) : t(`history.recording.${r.status}`, { from, to: to ?? "" });
    if (running) {
      if (selected?.kind === "recording") selected = null;
      fill(
        recordHost,
        h(
          "section",
          { class: ["callout", "recording"], style: { "margin-bottom": "16px" } },
          h("span", { class: "rec-dot", "aria-hidden": "true" }),
          h(
            "div",
            null,
            h("strong", null, title),
            h("span", null, t("history.recording.samples", { count: r.samples })),
            h("div", { class: "rec-bar" }, h("span", { style: { width: `${(elapsed / (r.duration_seconds * 1000)) * 100}%` } })),
          ),
          h("span", { class: "spacer" }),
          button(t("history.recording.stop"), {
            iconName: "stop",
            onClick: async () => {
              cancelledByUs = true;
              try {
                await call("record_activity_cancel", { recordingId: r.id });
              } catch (error) {
                toast(failureText(error), "bad");
              }
              refreshStatus();
            },
          }),
        ),
      );
      return;
    }
    // A finished recording is selected like an event: the whole callout is the target.
    const isSelected = selected?.kind === "recording" && selected.id === r.id;
    const item = recordingItem(r);
    fill(
      recordHost,
      h(
        "div",
        { class: ["actbar-anchor", "rec-anchor"] },
        h(
          "section",
          { class: ["callout", "info", "rec-done", isSelected ? "selected" : ""] },
          icon("check"),
          h(
            "div",
            null,
            h(
              "button",
              {
                type: "button",
                class: "rec-select",
                title: isSelected ? null : t("history.recording.select"),
                "aria-pressed": String(isSelected),
                "aria-keyshortcuts": isSelected ? "E Enter" : null,
                "data-key": `recording:${r.id}`,
                "data-actbar-item": isSelected,
                onclick: () => select(isSelected ? null : { kind: "recording", id: r.id }),
              },
              h("strong", null, title),
            ),
            h("span", null, t("history.recording.samplesDone", { count: r.samples })),
            battery.id !== r.id ? null : batteryFacts(battery.data),
          ),
          h("span", { class: "rec-chevron", "aria-hidden": "true" }, icon("chevron")),
        ),
        isSelected ? actionBar(item) : null,
      ),
    );
  };

  // ---------- Charts and events ----------

  const load = async () => {
    if (disposed) return;
    const request = ++loadRequest;
    const current = () => !disposed && request === loadRequest;
    loading = true;
    const span = PERIODS.find((p) => p.id === period).ms;
    const to = Date.now();
    const from = to - span;
    // Keep the last chart legible, but make its pending replacement explicit and its events inert.
    if (!chartsHost.querySelector(".hist-row")) fill(chartsHost, h("div", { class: "skeleton", style: { height: "260px" } }));
    chartsHost.querySelector('[role="status"]')?.remove();
    chartsHost.prepend(h("p", { class: "note row", role: "status" }, h("span", { class: "spinner", "aria-hidden": "true" }), t("history.loading", { period: t(PERIODS.find((p) => p.id === period).label) })));
    chartsHost.setAttribute("aria-busy", "true");
    eventsHost.inert = true;
    if (selected?.kind === "event") {
      selected = null;
      if (shownEvents) renderEvents(shownEvents);
    }
    const metrics = [...new Set(ROWS.flatMap((r) => r.metrics))];
    let series;
    try {
      series = Object.fromEntries(
        await Promise.all(metrics.map(async (metric) => [metric, await call("metric_history", { metric, fromUnixMs: from, toUnixMs: to })])),
      );
    } catch (error) {
      if (!current()) return;
      loading = false;
      loaded = null;
      chartsHost.setAttribute("aria-busy", "false");
      eventsHost.inert = false;
      fill(chartsHost, h("div", { class: "callout warn" }, icon("alert"), h("div", null, h("strong", null, t("history.unavailable")), h("span", { title: failureDetail(error) }, failureText(error)))));
      shownEvents = null;
      if (selected?.kind === "event") selected = null;
      fill(eventsHost);
      return;
    }
    if (!current()) return;
    const events = [];
    const seen = new Set();
    for (const one of Object.values(series)) {
      for (const e of one.events || []) {
        if (seen.has(e.id)) continue;
        seen.add(e.id);
        events.push({ ...e, contributor: e.contributor_ref ? one.process_names?.[e.contributor_ref] : null });
      }
    }
    events.sort((a, b) => b.started_at_unix_ms - a.started_at_unix_ms);
    loading = false;
    loaded = { series, from, to, events };
    chartsHost.setAttribute("aria-busy", "false");
    eventsHost.inert = false;
    drawCharts();
    renderEvents(events);
  };

  const drawCharts = () => {
    if (disposed || loading || !loaded) return;
    const { series, from, to, events } = loaded;
    // Account for the card padding and labels; resize only redraws cached data, never rereads it.
    chartWidth = Math.max(80, Math.round(chartsHost.getBoundingClientRect().width - 174));
    const gaps = series.cpu_busy_basis_points?.gaps || [];
    const missing = gaps.reduce((sum, g) => sum + (g.to_unix_ms - g.from_unix_ms), 0);
    fill(
      chartsHost,
      h(
        "div",
        { class: "card-head" },
        h("h2", { class: "card-title" }, t("history.activity")),
        h("span", { class: "hint" }, gapsHint(missing / (to - from))),
      ),
      ROWS.map((row) => {
        const points = rowPoints(row, row.metrics.map((m) => series[m]));
        const known = points.filter((p) => p.value != null);
        const latest = known.length ? known[known.length - 1].value : null;
        const rowEvents = events.filter((e) => (row.id === "cpu" && e.kind === "cpu_busy") || (row.id === "memory" && e.kind === "memory_pressure") || (row.id === "disk" && e.kind.startsWith("disk")));
        return h(
          "div",
          { class: "hist-row" },
          h("div", { class: "hist-row-label" }, h("span", null, t(row.label)), h("strong", null, latest == null ? "—" : percent(latest))),
          known.length ? chart(points, from, to, rowEvents, row.color, chartWidth) : h("div", { class: "history-empty-row" }, t("history.noData")),
        );
      }),
      axis(from, to),
      h("p", { class: "note", style: { "margin-top": "12px" } }, t("history.range.note")),
    );
  };

  const renderEvents = (events) => keepFocus(eventsHost, () => drawEvents(events));
  const drawEvents = (events) => {
    shownEvents = events;
    if (selected?.kind === "event" && !events.some((e) => e.id === selected.id)) selected = null;
    fill(
      eventsHost,
      h("div", { class: "card-head" }, h("h2", { class: "card-title" }, t("history.events")), h("span", { class: "hint" }, t("history.events.note"))),
      events.length
        ? h(
            "div",
            { class: "event-list" },
            events.map((e) => {
              const isSelected = selected?.kind === "event" && selected.id === e.id;
              const when = clock(e.started_at_unix_ms);
              return h(
                "div",
                { class: ["event-row", "actbar-anchor", isSelected ? "selected" : ""] },
                h(
                  "button",
                  {
                    type: "button",
                    class: "event-main",
                    "aria-pressed": String(isSelected),
                    "aria-keyshortcuts": isSelected ? "E Enter" : null,
                    "data-key": `event:${e.id}`,
                    "data-actbar-item": isSelected,
                    onclick: () => select(isSelected ? null : { kind: "event", id: e.id }),
                  },
                  h("span", { class: "event-icon" }, icon(EVENT_ICONS[e.kind] || "activity")),
                  h(
                    "span",
                    { class: "event-text" },
                    h("strong", null, eventLabel(e)),
                    h(
                      "span",
                      null,
                      [
                        t("history.event.peak", { value: eventValue(e) }),
                        e.ended_at_unix_ms > e.started_at_unix_ms ? t("history.event.during", { time: duration(e.ended_at_unix_ms - e.started_at_unix_ms) }) : t("history.event.single"),
                        e.contributor ? t("history.event.contributor", { name: e.contributor }) : null,
                        e.active ? t("history.event.active") : null,
                      ]
                        .filter(Boolean)
                        .join(" · "),
                    ),
                  ),
                  h("span", { class: "event-time" }, when),
                ),
                // The program measured at the peak is not the proven cause: said before acting.
                isSelected ? h("p", { class: "note event-note" }, t("history.event.note")) : null,
                isSelected ? actionBar(eventItem(e)) : null,
              );
            }),
          )
        : h("p", { class: "hint" }, t("history.events.none")),
    );
  };

  // ---------- Collection cost and startup ----------

  const renderCost = () => {
    const st = status;
    const cpu = st?.total_cpu_basis_points;
    const memory = st?.host_working_set_bytes;
    const cpuOk = cpu == null || cpu <= st.cpu_target_basis_points;
    const memoryOk = memory == null || memory <= st.memory_target_bytes;
    fill(
      costHost,
      h("div", { class: "card-head" }, h("h2", { class: "card-title" }, t("history.collection"))),
      st
        ? h(
            "dl",
            { class: "kv" },
            h("dt", null, t("history.collection.state")),
            h("dd", null, st.running ? t("history.collection.running", { count: number(st.samples) }) : t("history.collection.stopped")),
            h("dt", null, t("history.collection.cpu")),
            h("dd", { class: cpuOk ? "" : "warn-text" }, cpu == null ? t("history.collection.unknown") : t("history.collection.cpuValue", { value: percent(cpu / 10000, 2), target: percent(st.cpu_target_basis_points / 10000, 1) })),
            h("dt", null, t("history.collection.memory")),
            h("dd", { class: memoryOk ? "" : "warn-text" }, memory == null ? t("history.collection.unknown") : t("history.collection.memoryValue", { value: bytes(memory) })),
          )
        : h("p", { class: "hint" }, t("history.unavailable")),
      h("p", { class: "note" }, t("history.collection.note")),
      h(
        "div",
        { class: "setting-row" },
        h("div", { class: "setting-text" }, h("strong", null, t("startup.title")), h("span", { class: "hint" }, t("startup.note"))),
        startup == null
          ? h("span", { class: "hint" }, t("history.collection.unknown"))
          : toggle(startup, async (on) => {
              try {
                startup = await call("startup_set", { enabled: on });
              } catch (error) {
                toast(failureText(error), "bad");
              }
              if (!disposed) renderCost();
            }, t("startup.title")),
      ),
      h(
        "div",
        { class: "row", style: { "margin-top": "12px" } },
        button(t("history.clear"), {
          iconName: "x",
          variant: "danger",
          onClick: async () => {
            const yes = await dialog(
              (close) => [
                h("h2", { class: "dialog-title" }, t("history.clear.title")),
                h("p", null, t("history.clear.body")),
                h("div", { class: "dialog-actions" }, button(t("common.cancel"), { onClick: () => close(false) }), button(t("history.clear"), { variant: "danger", onClick: () => close(true) })),
              ],
              { label: t("history.clear.title"), dismissValue: false },
            );
            if (!yes || disposed) return;
            try {
              await call("history_clear");
              toast(t("history.cleared"));
              load();
            } catch (error) {
              toast(failureText(error), "bad");
            }
          },
        }),
      ),
    );
  };

  const refreshStatus = async () => {
    if (disposed) return;
    const request = ++statusRequest;
    window.clearTimeout(poll);
    let nextStatus;
    try {
      nextStatus = await call("history_status");
    } catch {
      nextStatus = null;
    }
    if (disposed || request !== statusRequest) return;
    status = nextStatus;
    const wasRecording = Boolean(recordHost.querySelector(".recording"));
    if (status?.recording) {
      lastRecording = status.recording;
    } else if (lastRecording && lastRecording.ended_at_unix_ms == null) {
      lastRecording = { ...lastRecording, ended_at_unix_ms: Date.now(), status: cancelledByUs ? "cancelled" : "completed" };
    }
    renderHead();
    renderRecording();
    renderCost();
    // While recording, refresh every second; the charts reload once it ends.
    const running = status?.recording && status.recording.ended_at_unix_ms == null;
    if (wasRecording && !running) load();
    poll = window.setTimeout(refreshStatus, running ? 1000 : 10000);
  };

  // E explains and Enter investigates the selected event or recording; Escape clears it.
  const stopKeys = selectionKeys({
    current: () => {
      if (selected?.kind === "event") {
        const e = shownEvents?.find((x) => x.id === selected.id);
        return e ? eventItem(e) : null;
      }
      const r = status?.recording ?? lastRecording;
      return selected?.kind === "recording" && r?.id === selected.id && r.ended_at_unix_ms != null ? recordingItem(r) : null;
    },
    clear: () => select(null),
  });

  renderHead();
  load();
  refreshStatus();
  call("startup_get")
    .then((on) => {
      startup = on;
      if (!disposed) renderCost();
    })
    .catch(() => {});
  const unsubscribe = subscribe((topic) => {
    if (topic === "info") renderCost();
  });
  const resize = new ResizeObserver(() => {
    const width = Math.max(80, Math.round(chartsHost.getBoundingClientRect().width - 174));
    if (width !== chartWidth) drawCharts();
  });
  resize.observe(chartsHost);
  return () => {
    disposed = true;
    loadRequest += 1;
    statusRequest += 1;
    resize.disconnect();
    window.clearTimeout(poll);
    unsubscribe();
    stopKeys();
  };
}
