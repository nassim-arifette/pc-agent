import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { axisLabels, decodeChart, periodWindow, shouldReload, watchRule, missingDuration, timelineGroups } from "../ui/js/history/data.js";

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

test("dense timeline groups reserve space for event numbers and counts when the inspector narrows the view", () => {
  const events = Array.from({ length: 363 }, (_, index) => ({ started_at_unix_ms: index * 1000 }));
  const groups = timelineGroups(events, 0, 363000, 350).sort((a, b) => a.left - b.left);
  assert.equal(groups.length, 4);
  assert.equal(groups.reduce((count, group) => count + group.items.length, 0), 363);
  for (let i = 1; i < groups.length; i++) assert.ok((groups[i].left - groups[i - 1].left) * 350 / 100 >= 81);
  assert.equal(timelineGroups(events, 0, 363000, 72).length, 1);
});

test("dark-theme event markers use semantic warning colour and a visible outline", () => {
  const css = readFileSync(new URL("../ui/styles/app.css", import.meta.url), "utf8");
  const marker = css.match(/\.history-chart \.history-mark\s*\{([^}]+)\}/)[1];
  assert.match(marker, /stroke: var\(--warn\)/);
  assert.match(marker, /stroke-opacity: 0\.8/);
  assert.doesNotMatch(marker, /opacity: 0\.12/);
});
