// History: what the local sampler measured over time, gaps included, the anomalies it detected,
// a recording taken during a problem, and what the collection itself costs. Nothing here is sent
// anywhere; selecting an event or a finished recording shows the action bar under it, whose
// Explain and Investigate go through the selected-evidence preview.
import { h, s, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t, language } from "../i18n.js";
import { call } from "../api.js";
import { findReference, runSelection } from "../selection/actions.js";
import { actionBar, keepFocus, selectionKeys } from "../actionbar.js";
import { state, subscribe } from "../store.js";
import { button, segmented, toast, toggle, dialog, dialogOpen } from "../ui.js";
import { bytes, clock, date, duration, number, percent } from "../format.js";
import { hoursMinutes, watts } from "../live/battery.js";
import { failureDetail, failureText } from "../failure.js";
import { PERIODS, dayStart, periodWindow, decodeChart, axisLabels, shouldReload, missingDuration, watchRule, timelineGroups } from "./data.js";

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
  { id: "read", label: "history.readLatency", color: "res-disk", metrics: ["disk_read_latency_microseconds"], value: ([v]) => v / 1000, latency: true },
  { id: "write", label: "history.writeLatency", color: "res-disk", metrics: ["disk_write_latency_microseconds"], value: ([v]) => v / 1000, latency: true },
];

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
async function runStoredSelection(match, { label, subject, sourceId, target }, intent) {
  const reference = sourceId ? { source_id: sourceId, target } : await findReference(match);
  if (reference) return runSelection({ reference, label, subject }, intent);
  toast(t("select.unavailable"), "bad");
}

/**
 * What the action bar and the shortcuts do for a stored event. The reference is checked when
 * invoked. The bar sits under the row's right end, away from the names on its left.
 */
function eventItem(e, capture) {
  const when = clock(e.started_at_unix_ms);
  const run = (intent) =>
    runStoredSelection(
      (target) => target.kind === "event" && target.event_id === e.id,
      { label: `${eventLabel(e)} · ${when}`, subject: t("history.event.subject", { what: eventLabel(e), when }), sourceId: capture?.source_id, target: { kind: "event", event_id: e.id } },
      intent,
    );
  const rule = watchRule(e.kind);
  return { key: `event:${e.id}`, label: `${eventLabel(e)} · ${when}`, ready: Boolean(capture?.source_id), unavailable: t("history.selectionUnavailable"), explain: () => run("explain"), investigate: () => run("investigate"),
    monitor: rule ? async () => {
      try {
        const { requestWatch } = await import("../watches-controller.js");
        await requestWatch(rule);
      } catch (error) { toast(failureText(error), "bad"); }
    } : null,
    monitorUnavailable: t("history.watch.unsupported"), align: "end" };
}

/** The same for a finished recording. */
function recordingItem(r, detail) {
  const from = clock(r.started_at_unix_ms, { seconds: true });
  const to = r.ended_at_unix_ms ? clock(r.ended_at_unix_ms, { seconds: true }) : from;
  const label = t("history.recording.label", { from, to });
  const run = (intent) =>
    runStoredSelection(
      (target) => target.kind === "recording" && target.recording_id === r.id,
      { label, subject: t("history.recording.subject", { from, to }), sourceId: detail?.source_id, target: { kind: "recording", recording_id: r.id } },
      intent,
    );
  return { key: `recording:${r.id}`, label, ready: Boolean(detail?.source_id), unavailable: t("history.selectionUnavailable"), explain: () => run("explain"), investigate: () => run("investigate"), monitorUnavailable: t("history.watch.recording"), align: "end" };
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
  const bounded = (value) => value == null || !Number.isFinite(value) ? null : Math.max(0, Math.min(row.latency ? Infinity : 1, value));
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
    const breakBefore = points.some((point) => point?.break_before) || Boolean(previous && ((gap && gap.from_unix_ms < at && gap.to_unix_ms > previous.at)
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
function chart(points, from, to, events, cls, width = 1000, scale = 1) {
  const W = 1000;
  const H = 54;
  const x = (at) => ((at - from) / Math.max(1, to - from)) * W;
  const y = (v) => H - 3 - (v / scale) * (H - 6);
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

function axis(from, to, width = 1000) {
  const labels = axisLabels(from, to, language() === "fr" ? "fr-FR" : "en-US")
    .filter((_, index) => width >= 400 || index % 2 === 0);
  return h(
    "div",
    { class: "history-axis", style: { "grid-template-columns": `repeat(${labels.length}, minmax(0, 1fr))` } },
    labels.map((label) => h("span", null, label)),
  );
}

export function mount(page, shell = {}) {
  let period = "day";
  let disposed = false;
  let loadRequest = 0;
  let statusRequest = 0;
  let loading = false;
  let loaded = null;
  let chartWidth = 0;
  let detailWidth = 0;
  let status = null;
  let area = "period";
  let overview = null;
  let overviewRequest = 0;
  let overviewTimer = null;
  let recordingRows = [];
  let moreRecordings = false;
  let recordsLoading = false;
  let detail = null;
  let detailRequest = 0;
  let detailLoading = false;
  let collection = null;
  let collectionRequest = 0;
  let eventLimit = 20;
  let startup = null;
  // The selected event or finished recording: { kind: "event" | "recording", id }.
  let selected = null;
  // The events drawn last; null until the first load succeeds.
  let shownEvents = null;
  let poll = null;
  // The sampler reports the active recording; once it ends the view keeps showing it so the
  // person can ask about it.
  let lastRecording = null;
  const head = h("div", { class: "page-head" });
  const recordHost = h("div");
  const listHost = h("section", { class: "card card-pad history-card" });
  const detailHost = h("section", { class: "card card-pad history-card" });
  const select = (next) => {
    selected = next;
    if (next?.kind === "recording") {
      area = "recordings";
      renderHead();
      renderRecordings();
    }
    if (shownEvents) renderEvents(shownEvents);
    renderRecording();
    if (next?.kind === "recording" && detail?.recording.id !== next.id) loadDetail(next.id);
    renderDetail();
    shell.refreshViewBar?.();
  };
  const chartsHost = h("section", { class: "card card-pad history-card" });
  const eventsHost = h("section", { class: "card card-pad" });
  const costHost = h("section", { class: "card card-pad" });
  fill(page, head, recordHost, listHost, detailHost, chartsHost, eventsHost, costHost);

  const visibility = () => {
    chartsHost.hidden = area !== "period";
    eventsHost.hidden = area !== "period";
    listHost.hidden = area !== "recordings";
    costHost.hidden = area !== "collection";
    detailHost.hidden = area !== "recordings" || selected?.kind !== "recording";
  };
  const choose = (nextArea, nextPeriod = period) => {
    const changed = period !== nextPeriod;
    area = nextArea;
    period = nextPeriod;
    selected = null;
    renderHead();
    renderRecording();
    if (shownEvents) renderEvents(shownEvents);
    visibility();
    if (area === "period" && (changed || !loaded)) load();
    if (area === "recordings") renderRecordings();
    if (area === "collection") refreshCollection();
    shell.refreshViewBar?.();
    shell.closeViewBar?.();
  };

  const navigationItem = (key, title, subtitle, active, run) => h("button", {
    type: "button", class: "viewbar-item", "data-key": `history:${key}`, "aria-current": String(active), onclick: run,
  }, h("span", { class: "viewbar-item-text" }, h("span", { class: "viewbar-item-title" }, title), h("span", { class: "viewbar-item-sub" }, subtitle)));
  const renderNavigation = () => h("div", { class: "viewbar" },
    h("div", { class: "viewbar-group" }, h("h2", { class: "viewbar-label" }, t("history.period")),
      PERIODS.map((p) => navigationItem(p.id, t(p.label), t("history.events.total", { count: overview?.periods.find((one) => one.id === p.id)?.event_count ?? "—" }), area === "period" && period === p.id, () => choose("period", p.id)))),
    h("div", { class: "viewbar-group" },
      navigationItem("recordings", t("history.recordings"), t("history.recordings.note"), area === "recordings", () => choose("recordings")),
      navigationItem("collection", t("history.collection"), t("history.collection.local"), area === "collection", () => choose("collection"))));

  const renderHead = () => {
    fill(
      head,
      h("div", null, h("h1", { class: "page-title" }, t("history.title.page")), h("p", { class: "page-sub" }, t("history.sub"))),
      h(
        "div",
        { class: "row wrap" },
        segmented(
          [
            { value: "period", label: t("history.activity") },
            { value: "recordings", label: t("history.recordings") },
            { value: "collection", label: t("history.collection") },
          ],
          area,
          (value) => {
            choose(value);
          },
          t("history.title.page"),
        ),
        button(t("history.refresh"), { iconName: "refresh", onClick: async () => { await refreshOverview(); if (area === "period") load(); else if (area === "recordings" && selected?.kind === "recording") loadDetail(selected.id); else if (area === "collection") refreshCollection(); } }),
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
      fill(recordHost);
      return;
    }
    const running = r.ended_at_unix_ms == null;
    const elapsed = Math.min(r.duration_seconds * 1000, Date.now() - r.started_at_unix_ms);
    const left = Math.max(0, r.duration_seconds * 1000 - elapsed);
    // A recording lasts seconds to minutes: minute precision would read "from 20:01 to 20:01".
    const from = clock(r.started_at_unix_ms, { seconds: true });
    const to = r.ended_at_unix_ms ? clock(r.ended_at_unix_ms, { seconds: true }) : null;
    const title = running ? t("history.recording", { left: duration(left) }) : t(`history.recording.${r.status}`, { from, to: to ?? "" });
    if (running) {
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
          ),
          h("span", { class: "rec-chevron", "aria-hidden": "true" }, icon("chevron")),
        ),
      ),
    );
  };

  // ---------- Charts and events ----------

  const load = async () => {
    if (disposed) return;
    const request = ++loadRequest;
    const current = () => !disposed && request === loadRequest;
    loading = true;
    const { from, to } = periodWindow(period);
    eventLimit = 20;
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
    let capture;
    try {
      capture = decodeChart(await call("history_chart", { fromUnixMs: from, toUnixMs: Math.max(from + 1, to), buckets: 300 }));
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
    loading = false;
    loaded = capture;
    chartsHost.setAttribute("aria-busy", "false");
    eventsHost.inert = false;
    drawCharts();
    renderEvents(capture.events);
  };

  const drawCharts = () => {
    if (disposed || loading || !loaded) return;
    const { series, from, to } = loaded;
    // Account for the card padding and labels; resize only redraws cached data, never rereads it.
    chartWidth = Math.max(80, Math.round(chartsHost.getBoundingClientRect().width - 174));
    const gaps = series.cpu_busy_basis_points?.gaps || [];
    const missing = missingDuration(gaps, from, to);
    fill(
      chartsHost,
      h(
        "div",
        { class: "card-head" },
        h("h2", { class: "card-title" }, t("history.activity")),
        h("span", { class: "hint" }, gapsHint(missing / (to - from))),
      ),
      segmented(PERIODS.map((p) => ({ value: p.id, label: t(p.label) })), period, (value) => choose("period", value), t("history.period")),
      chartRows(loaded),
      axis(from, to, chartWidth),
      h("p", { class: "note", style: { "margin-top": "12px" } }, t("history.range.note")),
      h("p", { class: "note" }, t("history.updated", { time: clock(to, { seconds: true }) })),
      loaded.selection_unavailable ? h("p", { class: "callout warn" }, t("history.selectionUnavailable")) : null,
      gaps.length ? h("details", null, h("summary", null, t("history.gapDetails", { count: gaps.length })),
        h("ul", { class: "history-gaps" }, gaps.map((gap) => {
          const key = `history.gap.${gap.reason}`;
          const reason = t(key);
          return h("li", null, `${date(gap.from_unix_ms, true)} → ${date(gap.to_unix_ms, true)} · ${reason === key ? t("history.gap.unknown") : reason}`);
        }))) : null,
    );
  };

  const chartRows = (capture, width = chartWidth) => ROWS.map((row) => {
    const points = rowPoints(row, row.metrics.map((m) => capture.series[m]));
    const known = points.filter((p) => p.value != null);
    const latest = known.at(-1)?.value ?? null;
    const scale = row.latency ? Math.max(1, ...known.map((p) => p.max ?? p.value)) : 1;
    const rowEvents = capture.events.filter((e) => (row.id === "cpu" && e.kind === "cpu_busy") || (row.id === "memory" && e.kind === "memory_pressure") || (["disk", "read", "write"].includes(row.id) && e.kind.startsWith("disk")));
    return h("div", { class: "hist-row" },
      h("div", { class: "hist-row-label" }, h("span", null, t(row.label)), h("strong", null, latest == null ? "—" : row.latency ? `${number(latest, 1)} ms` : percent(latest)),
        row.latency ? h("small", { class: "hint" }, t("history.scale", { value: number(scale, 1) })) : null),
      known.length ? chart(points, capture.from, capture.to, rowEvents, row.color, width || 600, scale) : h("div", { class: "history-empty-row" }, t("history.noData")));
  });

  const timeline = (events) => {
    if (!loaded || !events.length) return null;
    const width = Math.max(72, eventsHost.getBoundingClientRect().width - 76);
    const groups = timelineGroups(events, loaded.from, loaded.to, width);
    return h("div", { class: "history-timeline", role: "group", "aria-label": t("history.timeline") }, groups.map(({ left, items }) =>
      h("button", { type: "button", class: "history-timeline-mark", style: { left: `${left}%` },
        title: items.map(({ event, index }) => `${index + 1}. ${eventLabel(event)} · ${clock(event.started_at_unix_ms)}`).join("\n"),
        onclick: () => { eventLimit = Math.max(eventLimit, items.at(-1).index + 1); select({ kind: "event", id: items[0].event.id }); },
      }, `${items[0].index + 1}${items.length > 1 ? ` +${items.length - 1}` : ""}`)));
  };

  const renderEvents = (events) => keepFocus(eventsHost, () => drawEvents(events));
  const drawEvents = (events) => {
    shownEvents = events;
    if (selected?.kind === "event" && !events.some((e) => e.id === selected.id)) selected = null;
    fill(
      eventsHost,
      h("div", { class: "card-head" }, h("h2", { class: "card-title" }, t("history.events")), h("span", { class: "hint" }, t("history.events.total", { count: number(events.length) }))),
      timeline(events),
      events.length
        ? h(
            "div",
            { class: "event-list" },
            events.slice(0, eventLimit).map((e, index) => {
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
                  h("span", { class: "event-number" }, index + 1),
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
                        e.contributor ? t("history.event.contributor", { name: e.contributor }) : e.contributor_ref ? t("history.programUnknown") : null,
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
                isSelected ? actionBar(eventItem(e, loaded)) : null,
              );
            }),
          )
        : h("p", { class: "hint" }, t("history.events.none")),
      events.length > eventLimit ? button(t("history.moreEvents", { count: number(events.length - eventLimit) }), { onClick: () => { eventLimit += 50; renderEvents(events); } }) : null,
    );
  };

  // ---------- Finished recordings and their immutable local evidence ----------

  const renderRecordings = () => {
    fill(listHost, h("div", { class: "card-head" }, h("h2", { class: "card-title" }, t("history.recordings"))),
      recordingRows.length ? h("div", { class: "event-list" }, recordingRows.map((r) =>
        h("button", { type: "button", class: ["event-main", selected?.id === r.id && "selected"], "data-key": `recording-list:${r.id}`, disabled: r.ended_at_unix_ms == null,
          onclick: () => select({ kind: "recording", id: r.id }),
        }, icon("record"), h("span", { class: "event-text" }, h("strong", null, date(r.started_at_unix_ms, true)),
          h("span", null, r.ended_at_unix_ms == null ? t("history.recording.running") : `${duration(r.ended_at_unix_ms - r.started_at_unix_ms)} · ${t(`history.status.${r.status}`)} · ${number(r.samples)} ${t("history.samples")}`)))))
        : h("p", { class: "hint" }, t(overview ? "history.recordings.empty" : "history.recordings.loading")),
      moreRecordings ? button(t("history.moreRecordings"), { disabled: recordsLoading, onClick: async () => {
        recordsLoading = true;
        renderRecordings();
        const request = overviewRequest;
        try {
          const next = await call("history_recordings", { offset: recordingRows.length });
          if (disposed || request !== overviewRequest) return;
          recordingRows = [...new Map([...recordingRows, ...next.recordings].map((r) => [r.id, r])).values()];
          moreRecordings = next.more;
        } catch (error) { if (!disposed) toast(failureText(error), "bad"); }
        finally { recordsLoading = false; if (!disposed) renderRecordings(); }
      } }) : null);
  };

  const loadDetail = async (id) => {
    const request = ++detailRequest;
    detailLoading = true;
    detail = null;
    renderDetail();
    try {
      const next = await call("recording_detail", { recordingId: id });
      if (disposed || request !== detailRequest) return;
      detail = next ? { ...next, chart: decodeChart(next.chart) } : null;
    } catch (error) { if (!disposed && request === detailRequest) toast(failureText(error), "bad"); }
    finally {
      if (!disposed && request === detailRequest) {
        detailLoading = false;
        renderDetail();
        renderRecording();
      }
    }
  };

  const renderDetail = () => {
    visibility();
    if (selected?.kind !== "recording") return;
    if (detailLoading) {
      fill(detailHost, h("p", { role: "status", class: "row" }, h("span", { class: "spinner" }), t("history.detail.loading")));
      return;
    }
    if (!detail || detail.recording.id !== selected.id) {
      fill(detailHost, h("p", { class: "hint" }, t("history.detail.missing")));
      return;
    }
    const r = detail.recording;
    detailWidth = Math.max(80, Math.round(detailHost.getBoundingClientRect().width - 174));
    const peaks = [
      ["metric.cpu", detail.cpu_peaks, "process_cpu_basis_points", (v) => percent(v / 10000)],
      ["history.gpu", detail.resource_peaks, "process_gpu_basis_points", (v) => percent(v / 10000)],
      ["history.read", detail.resource_peaks, "process_io_read_bytes_per_second", (v) => `${bytes(v)}/s`],
      ["history.write", detail.resource_peaks, "process_io_write_bytes_per_second", (v) => `${bytes(v)}/s`],
    ];
    fill(detailHost,
      h("div", { class: "card-head" }, h("h2", { class: "card-title" }, t("history.detail.title")), h("span", { class: "hint" }, date(r.started_at_unix_ms, true))),
      h("div", { class: "actbar-anchor history-detail-actions" }, h("strong", null, `${clock(r.started_at_unix_ms, { seconds: true })} → ${clock(r.ended_at_unix_ms, { seconds: true })}`), actionBar(recordingItem(r, detail))),
      detail.minute_summaries ? h("p", { class: "callout warn" }, t("history.minuteSummaries")) : null,
      chartRows(detail.chart, detailWidth), axis(r.started_at_unix_ms, r.ended_at_unix_ms, detailWidth),
      h("p", { class: "note" }, gapsHint(missingDuration(detail.chart.gaps, r.started_at_unix_ms, r.ended_at_unix_ms) / Math.max(1, r.ended_at_unix_ms - r.started_at_unix_ms))),
      h("h3", null, t("history.peaks")), h("p", { class: "note" }, t("history.peaks.note")),
      peaks.map(([label, result, metric, format]) => {
        const observations = result?.observations.filter((o) => o.metric === metric && o.value != null) || [];
        return h("section", { class: "history-peaks" }, h("h4", null, t(label)), observations.length
          ? h("table", null, h("thead", null, h("tr", null, h("th", null, t("history.program")), h("th", null, t("history.peak")), h("th", null, t("history.measuredAt")))),
            h("tbody", null, observations.map((o) => h("tr", null, h("td", null, result.process_names[o.entity_ref] || t("history.programUnknown")), h("td", null, format(o.value)), h("td", null, clock(o.measured_at_unix_ms, { seconds: true }))))))
          : h("p", { class: "hint" }, t("history.noData")));
      }),
      detail.battery?.batteries?.length ? batteryFacts(detail.battery) : h("p", { class: "hint" }, t("history.battery.none")),
      h("p", { class: "note" }, t("history.localNames")));
  };

  const refreshOverview = async () => {
    const request = ++overviewRequest;
    try {
      const next = await call("history_overview", { dayStartUnixMs: dayStart() });
      if (disposed || request !== overviewRequest) return false;
      const reload = shouldReload(overview, next, period);
      overview = next;
      // Keep already paged records; refresh their state without discarding navigation.
      recordingRows = [...new Map([...next.recordings, ...recordingRows.filter((r) => !next.recordings.some((n) => n.id === r.id))].map((r) => [r.id, r])).values()];
      moreRecordings = recordingRows.length <= next.recordings.length ? next.more_recordings : moreRecordings;
      renderRecordings();
      shell.refreshViewBar?.();
      return reload;
    } catch (error) {
      if (!disposed && !overview) toast(failureText(error), "bad");
      return false;
    }
  };
  const autoRefresh = async () => {
    if (disposed) return;
    // Never publish new selection sources while evidence is being read or prepared for sharing.
    if (!dialogOpen() && !state.preparing && !state.panel.open && !selected && !loading && !detailLoading) {
      const changed = await refreshOverview();
      if (changed && area === "period") await load();
      if (area === "collection") await refreshCollection();
    }
    if (!disposed) overviewTimer = window.setTimeout(autoRefresh, 60000);
  };

  const refreshCollection = async () => {
    const request = ++collectionRequest;
    try {
      const next = await call("history_collection");
      if (disposed || request !== collectionRequest) return;
      collection = next;
      renderCost();
    } catch (error) { if (!disposed) toast(failureText(error), "bad"); }
  };

  // ---------- Collection cost and startup ----------

  const editBudget = async () => {
    const value = await dialog((close) => {
      const input = h("input", { type: "number", min: 1, step: 1, value: Math.ceil(collection.budget_bytes / 1048576), id: "history-budget", required: true });
      return [h("h2", { class: "dialog-title" }, t("history.budget.edit")),
        h("p", null, t("history.budget.warning")),
        h("form", { onsubmit: (e) => {
          e.preventDefault();
          const bytes = Number(input.value) * 1048576;
          if (Number.isSafeInteger(bytes) && bytes >= 1048576 && input.checkValidity()) close(bytes);
          else input.reportValidity();
        } }, h("label", { for: "history-budget" }, t("history.budget.mib")), input,
        h("div", { class: "dialog-actions" }, button(t("common.cancel"), { onClick: () => close(null) }), button(t("history.budget.apply"), { variant: "primary", type: "submit" })))];
    }, { label: t("history.budget.edit") });
    if (value == null || disposed) return;
    const request = ++collectionRequest;
    try {
      const next = await call("history_collection_set_budget", { bytes: value });
      if (disposed || request !== collectionRequest) return;
      collection = next;
      renderCost();
      loaded = null;
      ++loadRequest;
      loading = false;
      await refreshOverview();
    } catch (error) { if (!disposed) toast(failureText(error), "bad"); }
  };

  const renderCost = () => {
    const st = status;
    const cpu = st?.total_cpu_basis_points;
    const memory = st?.host_working_set_bytes;
    const cpuOk = cpu == null || cpu <= st.cpu_target_basis_points;
    const memoryOk = memory == null || memory <= st.memory_target_bytes;
    fill(
      costHost,
      h("div", { class: "card-head" }, h("h2", { class: "card-title" }, t("history.collection"))),
      collection ? [
        collection.over_budget ? h("p", { class: "callout warn" }, t("history.budget.over")) : null,
        h("dl", { class: "kv" },
          h("dt", null, t("history.budget")), h("dd", null, `${bytes(collection.allocated_bytes)} / ${bytes(collection.budget_bytes)}`),
          h("dt", null, t("history.budget.sqlite")), h("dd", null, bytes(collection.sqlite_bytes)),
          h("dt", null, t("history.budget.wal")), h("dd", null, bytes(collection.wal_bytes)),
          h("dt", null, t("history.budget.reusable")), h("dd", null, bytes(collection.reusable_bytes)),
          h("dt", null, t("history.budget.protected")), h("dd", null, t("history.budget.protectedValue", { count: number(collection.protected_samples), size: bytes(collection.protected_payload_bytes) })),
          h("dt", null, t("history.retention")), h("dd", null, collection.oldest_unix_ms == null ? t("history.noData") : `${date(collection.oldest_unix_ms, true)} → ${date(collection.latest_unix_ms, true)}`),
          h("dt", null, t("history.budget.evicted")), h("dd", null, number(collection.budget_removed_samples)),
          h("dt", null, t("history.budget.lastEviction")), h("dd", null, collection.last_budget_eviction_unix_ms == null ? "—" : date(collection.last_budget_eviction_unix_ms, true))),
        h("p", { class: "note" }, t("history.budget.note")),
        button(t("history.budget.edit"), { onClick: editBudget }),
      ] : h("p", { class: "hint" }, t("history.collection.unknown")),
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
              ++detailRequest;
              ++loadRequest;
              ++overviewRequest;
              detail = null;
              selected = null;
              loaded = null;
              recordingRows = [];
              overview = null;
              lastRecording = null;
              toast(t("history.cleared"));
              renderRecording();
              renderDetail();
              await Promise.all([refreshOverview(), refreshCollection(), load()]);
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
      await refreshOverview();
      if (disposed || request !== statusRequest) return;
      // Use the host's exact completion time and sample count, never the UI polling time.
      lastRecording = recordingRows.find((r) => r.id === lastRecording.id) ?? null;
    }
    renderHead();
    renderRecording();
    renderCost();
    // While recording, refresh every second. A finished week view still requires a manual reload.
    const running = status?.recording && status.recording.ended_at_unix_ms == null;
    if (wasRecording && !running && !dialogOpen() && !state.panel.open && !state.preparing && period !== "week") load();
    poll = window.setTimeout(refreshStatus, running ? 1000 : 10000);
  };

  // E explains and Enter investigates the selected event or recording; Escape clears it.
  const stopKeys = selectionKeys({
    current: () => {
      if (selected?.kind === "event") {
        const e = shownEvents?.find((x) => x.id === selected.id);
        return e && !loading ? eventItem(e, loaded) : null;
      }
      const r = detail?.recording;
      return selected?.kind === "recording" && r?.id === selected.id && r.ended_at_unix_ms != null ? recordingItem(r, detail) : null;
    },
    clear: () => select(null),
  });

  renderHead();
  visibility();
  shell.setViewBar?.({ label: "history.title.page", render: renderNavigation });
  load();
  refreshOverview();
  refreshCollection();
  overviewTimer = window.setTimeout(autoRefresh, 60000);
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
    if (width !== chartWidth) {
      drawCharts();
      if (shownEvents) renderEvents(shownEvents);
    }
    const nextDetailWidth = Math.max(80, Math.round(detailHost.getBoundingClientRect().width - 174));
    if (!detailHost.hidden && nextDetailWidth !== detailWidth) renderDetail();
  });
  resize.observe(chartsHost);
  resize.observe(detailHost);
  return () => {
    disposed = true;
    loadRequest += 1;
    statusRequest += 1;
    detailRequest += 1;
    overviewRequest += 1;
    collectionRequest += 1;
    resize.disconnect();
    window.clearTimeout(poll);
    window.clearTimeout(overviewTimer);
    unsubscribe();
    stopKeys();
  };
}
