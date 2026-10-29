// Pure local transport and navigation rules, shared with the History regression tests.
export const PERIODS = [
  { id: "hour", ms: 3600000, label: "history.period.hour" },
  { id: "day", ms: 86400000, label: "history.period.today" },
  { id: "week", ms: 604800000, label: "history.period.week" },
];

export function dayStart(now = Date.now()) {
  const day = new Date(now);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

export function periodWindow(period, now = Date.now()) {
  return { from: period === "day" ? dayStart(now) : now - PERIODS.find((p) => p.id === period).ms, to: now };
}

export function decodeChart(chart) {
  const series = Object.fromEntries(chart.series.map((series) => [series.metric, {
    ...series,
    gaps: chart.gaps,
    points: series.points.map(([unix_ms, value, min, max, measured_samples, expected_samples, resolution_ms, status, origin, observed_duration_ms, break_before]) =>
      ({ unix_ms, value, min, max, measured_samples, expected_samples, resolution_ms, status, origin, observed_duration_ms, break_before })),
  }]));
  const events = chart.events.map((e) => ({ ...e, contributor: chart.process_names[e.contributor_ref] ?? null }))
    .sort((a, b) => b.started_at_unix_ms - a.started_at_unix_ms || a.id.localeCompare(b.id));
  return { ...chart, series, events, from: chart.from_unix_ms, to: chart.to_unix_ms };
}

// h23 is deliberate: en-US's default 12-hour clock made 24-hour charts ambiguous.
export function axisLabels(from, to, locale) {
  const day = 86400000;
  const format = new Intl.DateTimeFormat(locale, {
    hour: "2-digit", minute: "2-digit", hourCycle: "h23",
    ...(to - from > 2 * day ? { month: "short", day: "numeric" } : {}),
  });
  return Array.from({ length: 7 }, (_, i) => format.format(from + (to - from) * i / 6));
}

export function shouldReload(previous, next, period) {
  if (!previous || period === "week") return false;
  const before = previous.periods.find((p) => p.id === period);
  const after = next.periods.find((p) => p.id === period);
  return before?.event_count !== after?.event_count
    || previous.latest_event_ended_at_unix_ms !== next.latest_event_ended_at_unix_ms
    || (period === "day" && before?.from_unix_ms !== after?.from_unix_ms);
}

export function watchRule(kind) {
  const resource = { cpu_busy: "cpu", memory_pressure: "memory", disk_busy: "disk_busy" }[kind];
  return resource ? { kind: "resource_above", resource, threshold_basis_points: resource === "disk_busy" ? 9500 : 9000, sustained_seconds: 60 } : null;
}

export function missingDuration(gaps, from, to) {
  const ranges = gaps.map((gap) => [Math.max(from, gap.from_unix_ms), Math.min(to, gap.to_unix_ms)])
    .filter(([start, end]) => end > start).sort((a, b) => a[0] - b[0]);
  let end = from;
  let duration = 0;
  for (const [start, next] of ranges) {
    duration += Math.max(0, next - Math.max(start, end));
    end = Math.max(end, next);
  }
  return duration;
}

export function timelineGroups(events, from, to, width) {
  // Reserve room for both the largest event number and its "+N" count.
  const spacing = Math.max(72, (String(events.length).length * 2 + 3) * 9);
  const bins = Math.max(1, Math.floor(width / spacing));
  const groups = new Map();
  events.forEach((event, index) => {
    const bin = Math.min(bins - 1, Math.max(0, Math.floor((event.started_at_unix_ms - from) / Math.max(1, to - from) * bins)));
    if (!groups.has(bin)) groups.set(bin, []);
    groups.get(bin).push({ event, index });
  });
  return [...groups].map(([bin, items]) => ({ left: (bin + 0.5) / bins * 100, items }));
}
