import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { axisLabels, decodeChart, periodWindow, shouldReload, watchRule, missingDuration } from "../ui/js/history/data.js";
import { createHistoryFixture } from "./mock/history.js";
import { virtualClock } from "./mock-cases.mjs";

test("compact chart decodes nulls, bands, coverage, durations and disconnects without duplicating metadata", () => {
  const chart = { from_unix_ms: 1, to_unix_ms: 3000, gaps: [], events: [{ id: "event-1", contributor_ref: "proc-1", started_at_unix_ms: 2 }],
    process_names: { "proc-1": "browser.exe" }, series: [{ metric: "cpu_busy_basis_points", points: [
      [1, 42, 20, 80, 3, 4, 1000, "partial", "live", 2500, false],
      [1001, null, null, null, 0, 1, 1000, "unsupported", "live", 0, true],
    ] }] };
  const decoded = decodeChart(chart);
  assert.equal(decoded.from, 1);
  assert.equal(decoded.events[0].contributor, "browser.exe");
  const [known, missing] = decoded.series.cpu_busy_basis_points.points;
  assert.deepEqual([known.value, known.min, known.max, known.observed_duration_ms], [42, 20, 80, 2500]);
  assert.equal(missing.value, null);
  assert.equal(missing.break_before, true);
  assert.equal(decoded.series.cpu_busy_basis_points.events, undefined);
  assert.equal(decoded.series.cpu_busy_basis_points.process_names, undefined);
});

test("24-hour axes use 00–23 hours even in English; week axes include dates", () => {
  const from = new Date(2026, 9, 27, 0, 0).getTime();
  const labels = axisLabels(from, from + 86400000, "en-US");
  assert.equal(labels[0], "00:00");
  assert.equal(labels[3], "12:00");
  assert.equal(labels[5], "20:00");
  assert.ok(labels.every((label) => !/AM|PM/.test(label)));
  assert.match(axisLabels(from, from + 604800000, "en-US")[0], /Oct/);
  assert.equal(periodWindow("day", from + 500000).from, from);
});

test("polling reloads changed events, not new samples or an unchanged/week view", () => {
  const before = { periods: [{ id: "day", from_unix_ms: 0, event_count: 150 }], latest_event_ended_at_unix_ms: 1000 };
  assert.equal(shouldReload(before, { ...before, latest_sample_unix_ms: 2000 }, "day"), false);
  const next = { ...before, latest_event_ended_at_unix_ms: 2000 };
  assert.equal(shouldReload(before, next, "day"), true);
  assert.equal(shouldReload(before, next, "week"), false);
  assert.equal(shouldReload(null, next, "day"), false);
});

test("watch proposals are closed C7 resource DTOs; latency and recordings stay unsupported", () => {
  for (const [kind, resource, threshold] of [["cpu_busy", "cpu", 9000], ["memory_pressure", "memory", 9000], ["disk_busy", "disk_busy", 9500]]) {
    assert.deepEqual(watchRule(kind), { kind: "resource_above", resource, threshold_basis_points: threshold, sustained_seconds: 60 });
  }
  assert.equal(watchRule("disk_latency"), null);
  assert.equal(watchRule("recording"), null);
});

test("coverage clips and unions gaps rather than double counting them", () => {
  assert.equal(missingDuration([{ from_unix_ms: -5, to_unix_ms: 4 }, { from_unix_ms: 2, to_unix_ms: 7 }, { from_unix_ms: 9, to_unix_ms: 15 }], 0, 10), 8);
});

function fixture(t, query = "history=fast") {
  const clock = virtualClock();
  t.mock.method(Date, "now", clock.now);
  globalThis.window = { setTimeout: clock.setTimeout };
  const publications = [];
  const { commands } = createHistoryFixture({ params: new URLSearchParams(query), batteryMode: "none",
    publishSource: (targets) => { publications.push(targets); return `selection-source-${publications.length}`; } });
  const call = async (name, args) => {
    const result = await clock.settle(Promise.resolve().then(() => commands[name](args)));
    if (result.error) throw result.error;
    return result.value;
  };
  return { call, clock, publications };
}

test("realistic mock returns all events, seven bucketed curves, and one source per load", async (t) => {
  const { call, clock, publications } = fixture(t);
  const chart = await call("history_chart", { fromUnixMs: clock.now() - 86400000, toUnixMs: clock.now(), buckets: 300 });
  assert.equal(chart.series.length, 7);
  assert.ok(chart.series.every((s) => s.points.length <= 300 && s.points.every((p) => p.length === 11)));
  assert.ok(chart.events.length > 256);
  assert.equal(publications.length, 1);
  assert.equal(publications[0].length, chart.events.length + 4);
  assert.equal(JSON.stringify(chart).match(/"events":/g).length, 1);
  assert.ok(chart.gaps.length);
  assert.ok(chart.series.some((s) => s.points.some((p) => p[1] === null)));
});

test("overview and paginated recordings expose all records; details retain local peaks", async (t) => {
  const { call, clock } = fixture(t);
  const overview = await call("history_overview", { dayStartUnixMs: clock.now() - 12 * 3600000 });
  assert.ok(overview.periods.find((p) => p.id === "day").event_count > 256);
  assert.equal(overview.recordings.length, 20);
  assert.equal(overview.more_recordings, true);
  const page = await call("history_recordings", { offset: 20 });
  assert.equal(page.recordings.length, 7);
  assert.equal(page.more, false);
  const detail = await call("recording_detail", { recordingId: page.recordings[0].id });
  assert.equal(detail.resource_peaks.observations.length, 3);
  assert.equal(detail.minute_summaries, true);
  assert.ok(detail.source_id);
  assert.equal(await call("recording_detail", { recordingId: "recording-missing" }), null);
});

test("budget reports protected overflow without deleting recordings; explicit clear removes them", async (t) => {
  const { call } = fixture(t);
  const small = await call("history_collection_set_budget", { bytes: 1048576 });
  assert.equal(small.over_budget, true);
  assert.ok(small.protected_samples > 0);
  assert.equal((await call("history_recordings", { offset: 0 })).recordings.length, 20);
  await call("history_clear");
  assert.equal((await call("history_collection")).oldest_unix_ms, null);
  assert.equal((await call("history_recordings", { offset: 0 })).recordings.length, 0);
});

test("mock explicitly rejects unavailable History rather than silently fabricating empty charts", async (t) => {
  const { call, clock } = fixture(t, "history=unavailable");
  await assert.rejects(call("history_chart", { fromUnixMs: clock.now() - 1000, toUnixMs: clock.now(), buckets: 300 }), (e) => e.code === "history_unavailable");
});

test("dark-theme event markers use semantic warning colour and a visible outline", () => {
  const css = readFileSync(new URL("../ui/styles/app.css", import.meta.url), "utf8");
  const marker = css.match(/\.history-chart \.history-mark\s*\{([^}]+)\}/)[1];
  assert.match(marker, /stroke: var\(--warn\)/);
  assert.match(marker, /stroke-opacity: 0\.8/);
  assert.doesNotMatch(marker, /opacity: 0\.12/);
});
