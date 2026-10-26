import { wait, fail, GiB, MiB } from "./common.js";

export function createHistoryFixture({ params, batteryMode: BATTERY_MODE, publishSource }) {
  const historyStart = Date.now() - 7 * 24 * 3600 * 1000;

  const historyEvents = [
    { id: "ev-1", kind: "cpu_busy", hoursAgo: 30, minutes: 3, peak: 8700, metric: "cpu_busy_basis_points", unit: "basis_points_of_total_cpu", contributor: "proc-11aa22bb", name: "firefox.exe" },
    { id: "ev-2", kind: "disk_latency", hoursAgo: 52, minutes: 12, peak: 64000, metric: "disk_latency_microseconds", unit: "microseconds", contributor: "proc-33cc44dd", name: "TiWorker.exe" },
    { id: "ev-3", kind: "memory_pressure", hoursAgo: 3, minutes: 6, peak: 9700, metric: "physical_used_basis_points", unit: "basis_points", contributor: "proc-11aa22bb", name: "firefox.exe" },
  ];

  let historyCleared = false;

  let recording = null;

  let startupEnabled = false;

  function historyValue(metric, at) {
    const hours = (Date.now() - at) / 3600000;
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

  function historySeries(metric, from, to) {
    // At most 120 points, like the agent's projection; one night without measurements.
    const count = 120;
    const step = Math.max(5000, Math.floor((to - from) / count));
    const gapFrom = Date.now() - 20 * 3600 * 1000;
    const gapTo = gapFrom + 7 * 3600 * 1000;
    const points = [];
    for (let at = from; at <= to; at += step) {
      const inGap = (at >= gapFrom && at < gapTo) || at < historyStart || historyCleared;
      points.push({ unix_ms: at, value: inGap ? null : historyValue(metric, at), min: null, max: null, status: inGap ? "unsupported" : "ok", measured_samples: inGap ? 0 : 1, expected_samples: 1, resolution_ms: step, observed_duration_ms: inGap ? 0 : step, origin: "synthetic_fixture" });
    }
    const gaps = historyCleared ? [{ from_unix_ms: from, to_unix_ms: to, reason: "not_sampled" }] : gapTo > from && gapFrom < to ? [{ from_unix_ms: Math.max(from, gapFrom), to_unix_ms: Math.min(to, gapTo), reason: "not_sampled" }] : [];
    const events = historyCleared
      ? []
      : historyEvents
          .map((e) => {
            const end = Date.now() - e.hoursAgo * 3600000;
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
      }
    }
    const active = recording && recording.ended_at_unix_ms == null ? recording : null;
    return { running: true, samples: historyCleared ? 12 : 98213, last_sample_unix_ms: Date.now() - 2000, recording: active, error: null, thread_cpu_basis_points: 160, total_cpu_basis_points: 20, logical_processors: 8, resource_window_ms: 60000, host_working_set_bytes: 96 * MiB, cpu_target_basis_points: 50, memory_target_bytes: 50 * MiB };
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
      recording = { id: `rec-${Date.now()}`, started_at_unix_ms: Date.now(), ended_at_unix_ms: null, duration_seconds: seconds, status: "recording", samples: 0, markers: [{ at_unix_ms: Date.now(), kind: "start" }] };
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
    },
    history_clear: () => {
      historyCleared = true;
      return 98213;
    },
    startup_get: () => startupEnabled,
    startup_set: ({ enabled }) => {
      startupEnabled = Boolean(enabled);
      return startupEnabled;
    }
  };
  return { commands };
}
