import { wait, fail, GiB, MiB } from "./common.js";

export function createHistoryFixture({ params, batteryMode: BATTERY_MODE, publishSource }) {
  const fixtureTime = Date.now();
  const historyStart = fixtureTime - 7 * 24 * 3600 * 1000;

  const historyEvents = [
    { id: "ev-1", kind: "cpu_busy", hoursAgo: 30, minutes: 3, peak: 8700, metric: "cpu_busy_basis_points", unit: "basis_points_of_total_cpu", contributor: "proc-11aa22bb", name: "firefox.exe" },
    { id: "ev-2", kind: "disk_latency", hoursAgo: 52, minutes: 12, peak: 64000, metric: "disk_latency_microseconds", unit: "microseconds", contributor: "proc-33cc44dd", name: "TiWorker.exe" },
    { id: "ev-3", kind: "memory_pressure", hoursAgo: 3, minutes: 6, peak: 9700, metric: "physical_used_basis_points", unit: "basis_points", contributor: "proc-11aa22bb", name: "firefox.exe" },
  ];
  // A busy real day can contain hundreds of episodes, not three hand-picked examples.
  for (let i = 0; i < 360; i++) {
    const cpu = i % 3 === 0;
    const memory = i % 3 === 1;
    historyEvents.push({
      id: `event-busy-${i}`, kind: cpu ? "cpu_busy" : memory ? "memory_pressure" : "disk_busy",
      hoursAgo: (i + 1) / 36, minutes: 0.2, peak: 9500 + i % 400,
      metric: cpu ? "cpu_busy_basis_points" : memory ? "physical_used_basis_points" : "disk_busy_basis_points",
      unit: "basis_points", contributor: cpu || memory ? `proc-${i % 8}` : null, name: i % 7 ? `program-${i % 8}.exe` : null,
    });
  }

  let historyCleared = false;

  let recording = null;
  let records = Array.from({ length: 27 }, (_, i) => {
    const end = Date.now() - (i + 1) * 7200000;
    return { id: `recording-fixture-${i}`, started_at_unix_ms: end - 60000, ended_at_unix_ms: end,
      duration_seconds: 60, samples: 60, status: "completed", markers: [] };
  });
  let budget = 256 * MiB;

  let startupEnabled = false;

  function historyValue(metric, at) {
    const hours = (fixtureTime - at) / 3600000;
    const wave = Math.sin(at / 900000) * 0.5 + Math.sin(at / 170000) * 0.3;
    const near = (e) => Math.abs(hours - e.hoursAgo) < e.minutes / 60;
    const ev = (kind) => historyEvents.find((e) => e.kind === kind && near(e));
    switch (metric) {
      case "cpu_busy_basis_points":
        return ev("cpu_busy") ? 8500 : Math.round(1800 + wave * 900);
      case "physical_total_bytes":
        return Math.round(15.73 * GiB);
      case "physical_available_bytes":
        return ev("memory_pressure") ? Math.round(0.45 * GiB) : Math.round((3.2 + wave * 0.9) * GiB);
      case "disk_idle_basis_points":
        return ev("disk_latency") ? 2500 : Math.round(9300 + wave * 300);
      case "gpu_busiest_engine_basis_points":
        return Math.round(700 + wave * 500);
      default:
        return Math.round(1000 + wave * 500);
    }
  }

  function historySeries(metric, from, to, count = 300) {
    // The host transports display buckets, not the smaller model projection.
    const step = Math.max(5000, Math.ceil((to - from) / count));
    const gapFrom = fixtureTime - 20 * 3600 * 1000;
    const gapTo = gapFrom + 7 * 3600 * 1000;
    const points = [];
    for (let at = from; at < to; at += step) {
      const inGap = (at >= gapFrom && at < gapTo) || at < historyStart || historyCleared;
      const value = historyValue(metric, at);
      const spread = metric === "physical_total_bytes" ? 0 : metric === "physical_available_bytes" ? 0.15 * GiB : 500;
      points.push({ unix_ms: at, value: inGap ? null : value, min: inGap ? null : Math.round(Math.max(0, value - spread)), max: inGap ? null : Math.round(value + spread), status: inGap ? "unsupported" : "ok", measured_samples: inGap ? 0 : 1, expected_samples: 1, resolution_ms: step, observed_duration_ms: inGap ? 0 : step, origin: "synthetic_fixture" });
    }
    const gaps = historyCleared ? [{ from_unix_ms: from, to_unix_ms: to, reason: "not_sampled" }] : gapTo > from && gapFrom < to ? [{ from_unix_ms: Math.max(from, gapFrom), to_unix_ms: Math.min(to, gapTo), reason: "not_sampled" }] : [];
    const events = historyCleared
      ? []
      : historyEvents
          .map((e) => {
            const end = fixtureTime - e.hoursAgo * 3600000;
            return { id: e.id, kind: e.kind, entity_ref: "system", started_at_unix_ms: end - e.minutes * 60000, ended_at_unix_ms: end, peak: e.peak, metric: e.metric, unit: e.unit, active: false, contributor_ref: e.contributor, contributor_value: null, contributor_metric: null, contributor_unit: null, origin: "synthetic_fixture" };
          })
          .filter((e) => e.ended_at_unix_ms >= from && e.started_at_unix_ms <= to);
    const names = Object.fromEntries(historyEvents.map((e) => [e.contributor, e.name]));
    return { metric, entity_ref: "system", unit: "", status: gaps.length ? "partial" : "ok", origin: "synthetic_fixture", from_unix_ms: from, to_unix_ms: to, points, gaps, events, process_names: names, sources: ["synthetic"], limitations: [] };
  }

  function samplerStatus() {
    if (recording && recording.ended_at_unix_ms == null) {
      const elapsed = Date.now() - recording.started_at_unix_ms;
      recording.samples = Math.floor(elapsed / 1000);
      if (elapsed >= recording.duration_seconds * 1000) {
        recording.ended_at_unix_ms = recording.started_at_unix_ms + recording.duration_seconds * 1000;
        recording.status = "completed";
        records = [recording, ...records.filter((r) => r.id !== recording.id)];
      }
    }
    const active = recording && recording.ended_at_unix_ms == null ? recording : null;
    return { running: true, samples: historyCleared ? 12 : 7800, last_sample_unix_ms: Date.now() - 2000, recording: active, error: null, thread_cpu_basis_points: 160, total_cpu_basis_points: 20, logical_processors: 8, resource_window_ms: 60000, host_working_set_bytes: 96 * MiB, cpu_target_basis_points: 50, memory_target_bytes: 50 * MiB };
  }

  function groupedChart(from, to, buckets) {
    const metrics = ["cpu_busy_basis_points", "physical_available_bytes", "physical_total_bytes", "disk_idle_basis_points", "gpu_busiest_engine_basis_points", "disk_read_latency_microseconds", "disk_write_latency_microseconds"];
    const histories = metrics.map((metric) => historySeries(metric, from, to, buckets));
    const first = histories[0];
    return { from_unix_ms: from, to_unix_ms: to, buckets,
      series: histories.map((one) => ({ metric: one.metric, unit: one.unit, status: one.status,
        points: one.points.map((p) => [p.unix_ms, p.value, p.min, p.max, p.measured_samples, p.expected_samples, p.resolution_ms, p.status, p.origin, p.observed_duration_ms, false]) })),
      events: first.events, process_names: first.process_names, gaps: first.gaps, limitations: [] };
  }

  function checkAvailable() {
    if (params.get("history") === "unavailable") fail("history_unavailable", "Local history is unavailable.");
  }

  function collection() {
    const protectedBytes = historyCleared ? 0 : 24 * MiB;
    return { budget_bytes: budget, allocated_bytes: historyCleared ? 65536 : Math.max(protectedBytes, Math.min(budget, 132 * MiB)),
      sqlite_bytes: 150 * MiB, wal_bytes: 2 * MiB, reusable_bytes: 18 * MiB,
      samples: historyCleared ? 0 : 98213, protected_samples: historyCleared ? 0 : 1620, protected_payload_bytes: protectedBytes,
      oldest_unix_ms: historyCleared ? null : historyStart, latest_unix_ms: historyCleared ? null : Date.now() - 2000,
      budget_removed_samples: budget < 132 * MiB ? 3500 : 0, last_budget_eviction_unix_ms: budget < 132 * MiB ? Date.now() : null, over_budget: protectedBytes > budget };
  }

  function batteryRecording(rec) {
    const end = rec.ended_at_unix_ms ?? Date.now();
    const ticks = Math.max(1, Math.min(128, Math.floor((end - rec.started_at_unix_ms) / 1000)));
    const ref = "battery-00000000-0000-4000-8000-00000000b001";
    const window = (at) => ({ started_at_unix_ms: at - 40, finished_at_unix_ms: at, duration_ms: 40 });
    const source = "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS";
    const o = (at, metric, value, unit) => ({ observation_id: `${ref}-measurement-${at}-${metric}`, measured_at_unix_ms: at, window: window(at), entity_ref: ref, metric, value, unit, status: "ok", source, origin: "synthetic_fixture" });
    const startCapacity = 41250;
    const rate = -8400;
    const points = BATTERY_MODE === "none" ? [] : Array.from({ length: ticks }, (_, i) => {
      const at = rec.started_at_unix_ms + (i + 1) * 1000;
      const capacity = Math.round(startCapacity + (rate * (i + 1)) / 3600);
      return {
        recorded_at_unix_ms: at, recording_elapsed_ms: (i + 1) * 1000, clock_consistent: true, status: "ok",
        observations: [
          o(at, "battery_kind", "system", "category"), o(at, "battery_capacity_units", "milliwatt_hours", "category"), o(at, "battery_charge_state", "discharging", "category"), o(at, "battery_on_ac", false, "boolean"),
          o(at, "battery_remaining_capacity_mwh", capacity, "milliwatt_hours"), o(at, "battery_charge_percent", Math.round((capacity / 52800) * 100), "percent"),
          o(at, "battery_power_rate_mw", Math.round(rate + (Math.random() - 0.5) * 600), "milliwatts"), o(at, "battery_estimated_runtime_windows_seconds", 17400 - i, "seconds"),
        ],
      };
    });
    const first = points[0];
    const last = points[points.length - 1];
    const capacityOf = (p) => p.observations.find((x) => x.metric === "battery_remaining_capacity_mwh");
    const lost = points.length > 1 ? capacityOf(first).value - capacityOf(last).value : null;
    const elapsed = points.length > 1 ? last.recording_elapsed_ms - first.recording_elapsed_ms : 0;
    const derived = (metric, value, unit) => ({ ...o(last.recorded_at_unix_ms, metric, value, unit), source: "derived/recording_battery_capacity_endpoints", derivation: "synthetic", inputs: [capacityOf(first), capacityOf(last)] });
    const evolution = lost > 0 && elapsed > 0
      ? [{ from_unix_ms: first.recorded_at_unix_ms, to_unix_ms: last.recorded_at_unix_ms, measured_points: points.length, observations: [
          derived("battery_measured_capacity_decrease_mwh", lost, "milliwatt_hours"),
          derived("battery_average_power_rate_mw", (-lost * 3600000) / elapsed, "milliwatts"),
          derived("battery_estimated_runtime_from_recording_seconds", (capacityOf(last).value * elapsed) / lost / 1000, "seconds"),
        ] }]
      : [];
    return {
      status: points.length ? "ok" : "partial", started_at_unix_ms: rec.started_at_unix_ms, ended_at_unix_ms: rec.ended_at_unix_ms, interval_ms: 1000,
      coverage: { expected_ticks: rec.duration_seconds, recorded_ticks: ticks, ticks_without_battery: points.length ? 0 : ticks, unavailable_observations: 0, rejected_observations: 0, points_returned: points.length, points_omitted_from_response: 0, batteries_omitted_from_response: 0, battery_tick_points_omitted_during_collection: 0 },
      gaps: [], batteries: points.length ? [{ battery_ref: ref, status: "ok", points, gaps: [], evolution }] : [], limitations: [],
    };
  }

  const commands = {
    history_status: () => samplerStatus(),
    history_chart: async ({ fromUnixMs, toUnixMs, buckets }) => {
      // Deliberately reorder rapid period requests. ?history=fast is for deterministic unit tests.
      await wait(params.get("history") === "fast" ? 0 : toUnixMs - fromUnixMs > 86400000 ? 1600 : toUnixMs - fromUnixMs > 3600000 ? 900 : 300);
      checkAvailable();
      const chart = groupedChart(fromUnixMs, toUnixMs, buckets);
      const targets = ["cpu", "memory", "disk", "gpu"].map((resource) => ({ kind: "resource", resource }));
      targets.push(...chart.events.map((e) => ({ kind: "event", event_id: e.id })));
      return { ...chart, source_id: publishSource(targets, toUnixMs), selection_unavailable: null };
    },
    history_overview: async ({ dayStartUnixMs }) => {
      await wait(params.get("history") === "fast" ? 0 : 180);
      checkAvailable();
      samplerStatus();
      const now = Date.now();
      const rows = recording?.ended_at_unix_ms == null && recording ? [recording, ...records] : records;
      return { periods: [["hour", now - 3600000], ["day", dayStartUnixMs], ["week", now - 604800000]].map(([id, from]) =>
        ({ id, from_unix_ms: from, to_unix_ms: now, event_count: historySeries("cpu_busy_basis_points", from, now).events.length })),
        latest_event_ended_at_unix_ms: historyCleared ? null : fixtureTime - 100000, latest_sample_unix_ms: now - 2000,
        recordings: rows.slice(0, 20), more_recordings: rows.length > 20 };
    },
    history_recordings: async ({ offset }) => {
      checkAvailable();
      const rows = recording?.ended_at_unix_ms == null && recording ? [recording, ...records] : records;
      return { recordings: rows.slice(offset, offset + 20), more: rows.length > offset + 20 };
    },
    recording_detail: async ({ recordingId }) => {
      await wait(params.get("history") === "fast" ? 0 : 450);
      checkAvailable();
      const r = records.find((r) => r.id === recordingId);
      if (!r) return null;
      const chart = groupedChart(r.started_at_unix_ms, r.ended_at_unix_ms, 120);
      const peaks = (metrics) => ({ status: "ok", from_unix_ms: r.started_at_unix_ms, to_unix_ms: r.ended_at_unix_ms,
        process_names: { "proc-fixture": "browser.exe" }, limitations: [],
        observations: metrics.map((metric) => ({ metric, entity_ref: "proc-fixture", value: metric.includes("bytes") ? 3400000 : 8700,
          status: "ok", origin: "synthetic_fixture", measured_at_unix_ms: r.ended_at_unix_ms - 12000 })) });
      return { recording: r, chart, cpu_peaks: peaks(["process_cpu_basis_points"]),
        resource_peaks: peaks(["process_gpu_basis_points", "process_io_read_bytes_per_second", "process_io_write_bytes_per_second"]),
        battery: batteryRecording(r), minute_summaries: Date.now() - r.started_at_unix_ms > 86400000,
        source_id: publishSource([{ kind: "recording", recording_id: r.id }], r.ended_at_unix_ms), selection_unavailable: null };
    },
    history_collection: () => { checkAvailable(); return collection(); },
    history_collection_set_budget: ({ bytes }) => {
      checkAvailable();
      if (!Number.isSafeInteger(bytes) || bytes < MiB) throw new Error("Invalid budget");
      budget = bytes;
      return collection();
    },
    metric_history: async ({ metric, fromUnixMs, toUnixMs }) => {
      await wait(150);
      if (params.get("history") === "unavailable") fail("history_unavailable", "Local history is unavailable.");
      const series = historySeries(metric, fromUnixMs, toUnixMs);
      // Reading history publishes its events (and a finished recording) as selection sources.
      const targets = series.events.map((e) => ({ kind: "event", event_id: e.id }));
      if (recording && recording.ended_at_unix_ms != null) targets.push({ kind: "recording", recording_id: recording.id });
      if (targets.length) publishSource(targets, toUnixMs);
      return series;
    },
    record_activity_start: ({ durationSeconds }) => {
      if (params.get("history") === "unavailable") fail("history_unavailable", "Local history is unavailable.");
      if (recording && recording.ended_at_unix_ms == null) throw new Error("A recording is already running.");
      // ?recording=short makes the test recordings last 3 seconds.
      const seconds = params.get("recording") === "short" ? 3 : durationSeconds;
      recording = { id: `recording-${Date.now()}`, started_at_unix_ms: Date.now(), ended_at_unix_ms: null, duration_seconds: seconds, status: "recording", samples: 0, markers: [{ at_unix_ms: Date.now(), kind: "start" }] };
      return { ...recording };
    },
    battery_recording: async ({ recordingId }) => {
      await wait(150);
      if (!recording || recording.id !== recordingId) return null;
      return batteryRecording(recording);
    },
    record_activity_cancel: ({ recordingId }) => {
      if (!recording || recording.id !== recordingId || recording.ended_at_unix_ms != null) throw new Error("This recording is over.");
      recording.ended_at_unix_ms = Date.now();
      recording.status = "cancelled";
      records = [recording, ...records.filter((r) => r.id !== recording.id)];
    },
    history_clear: () => {
      historyCleared = true;
      return 7800;
    },
    startup_get: () => startupEnabled,
    startup_set: ({ enabled }) => {
      startupEnabled = Boolean(enabled);
      return startupEnabled;
    }
  };
  return { commands };
}
