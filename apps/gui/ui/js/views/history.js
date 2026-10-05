// History: what the local sampler measured over time, gaps included, the anomalies it detected,
// a recording taken during a problem, and what the collection itself costs. Nothing here is sent
// anywhere; Explain and Investigate go through the selected-evidence preview (select.js).
import { h, s, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { ask } from "../ask.js";
import { INTENTS, findReference, runSelection } from "../select.js";
import { subscribe } from "../store.js";
import { button, segmented, toast, toggle, dialog } from "../ui.js";
import { bytes, clock, date, duration, number, percent } from "../format.js";

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
 * Explains or investigates a stored event or recording through its host-issued source. Without
 * one (history not read yet in this session, or expired), the typed question still works.
 */
async function selectOrAsk(match, { label, subject }, intent, fallback) {
  const reference = await findReference(match);
  if (reference) return runSelection({ reference, label, subject }, intent);
  return ask(fallback);
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

/** Merges the series of one row point by point; a point missing in any input stays unknown. */
function rowPoints(row, series) {
  const first = series[0]?.points || [];
  return first.map((p, i) => {
    const values = series.map((s) => s.points[i]?.value ?? null);
    const known = values.every((v) => v != null);
    const v = known ? row.value(values) : null;
    return { at: p.unix_ms, value: v == null || Number.isNaN(v) ? null : Math.max(0, Math.min(1, v)) };
  });
}

/** A line chart that breaks at unknown values instead of drawing through them. */
function chart(points, from, to, events, cls) {
  const W = 1000;
  const H = 54;
  const x = (at) => ((at - from) / Math.max(1, to - from)) * W;
  const y = (v) => H - 3 - v * (H - 6);
  const segments = [];
  let current = [];
  for (const p of points) {
    if (p.value == null) {
      if (current.length) segments.push(current);
      current = [];
    } else current.push(`${x(p.at).toFixed(1)},${y(p.value).toFixed(1)}`);
  }
  if (current.length) segments.push(current);
  return s(
    "svg",
    { class: ["history-chart", cls], viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "none", "aria-hidden": "true" },
    events.map((e) => s("rect", { class: "history-mark", x: x(e.started_at_unix_ms).toFixed(1), y: 0, width: Math.max(2, x(e.ended_at_unix_ms) - x(e.started_at_unix_ms)).toFixed(1), height: H })),
    segments.map((seg) => (seg.length === 1 ? s("circle", { cx: seg[0].split(",")[0], cy: seg[0].split(",")[1], r: 2 }) : s("polyline", { points: seg.join(" ") }))),
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
  let status = null;
  let startup = null;
  let selected = null;
  let poll = null;
  // The sampler reports the active recording; once it ends the view keeps showing it so the
  // person can ask about it.
  let lastRecording = null;
  let cancelledByUs = false;
  const head = h("div", { class: "page-head" });
  const recordHost = h("div");
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
    if (!seconds) return;
    cancelledByUs = false;
    try {
      await call("record_activity_start", { durationSeconds: seconds });
      refreshStatus();
    } catch (error) {
      toast(error.message, "bad");
    }
  };

  const renderRecording = () => {
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
    fill(
      recordHost,
      h(
        "section",
        { class: ["callout", running ? "recording" : "info"], style: { "margin-bottom": "16px" } },
        running ? h("span", { class: "rec-dot", "aria-hidden": "true" }) : icon("check"),
        h(
          "div",
          null,
          h("strong", null, running ? t("history.recording", { left: duration(left) }) : t(`history.recording.${r.status}`, { from, to: to ?? "" })),
          h("span", null, t(running ? "history.recording.samples" : "history.recording.samplesDone", { count: r.samples })),
          running ? h("div", { class: "rec-bar" }, h("span", { style: { width: `${(elapsed / (r.duration_seconds * 1000)) * 100}%` } })) : null,
        ),
        h("span", { class: "spacer" }),
        running
          ? button(t("history.recording.stop"), {
              iconName: "stop",
              onClick: async () => {
                cancelledByUs = true;
                try {
                  await call("record_activity_cancel", { recordingId: r.id });
                } catch (error) {
                  toast(error.message, "bad");
                }
                refreshStatus();
              },
            })
          : h(
              "div",
              { class: "row" },
              INTENTS.map((intent) =>
                button(t(`inspect.${intent}`), {
                  variant: intent === "explain" ? "primary" : "",
                  iconName: intent === "explain" ? "sparkle" : "search",
                  onClick: () =>
                    selectOrAsk(
                      (target) => target.kind === "recording" && target.recording_id === r.id,
                      { label: t("history.recording.label", { from, to: to ?? from }), subject: t("history.recording.subject", { from, to: to ?? from }) },
                      intent,
                      t("history.recording.question", { from, to: to ?? from }),
                    ),
                }),
              ),
            ),
      ),
    );
  };

  // ---------- Charts and events ----------

  const load = async () => {
    const span = PERIODS.find((p) => p.id === period).ms;
    const to = Date.now();
    const from = to - span;
    // The skeleton only on the first load; later reloads keep the charts until new data arrives.
    if (!chartsHost.querySelector(".hist-row")) fill(chartsHost, h("div", { class: "skeleton", style: { height: "260px" } }));
    const metrics = [...new Set(ROWS.flatMap((r) => r.metrics))];
    let series;
    try {
      series = Object.fromEntries(
        await Promise.all(metrics.map(async (metric) => [metric, await call("metric_history", { metric, fromUnixMs: from, toUnixMs: to })])),
      );
    } catch (error) {
      if (disposed) return;
      fill(chartsHost, h("div", { class: "callout warn" }, icon("alert"), h("div", null, h("strong", null, t("history.unavailable")), h("span", null, error.message))));
      fill(eventsHost);
      return;
    }
    if (disposed) return;
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
          known.length ? chart(points, from, to, rowEvents, row.color) : h("div", { class: "history-empty-row" }, t("history.noData")),
        );
      }),
      axis(from, to),
    );
    renderEvents(events);
  };

  const renderEvents = (events) => {
    fill(
      eventsHost,
      h("div", { class: "card-head" }, h("h2", { class: "card-title" }, t("history.events")), h("span", { class: "hint" }, t("history.events.note"))),
      events.length
        ? h(
            "div",
            { class: "event-list" },
            events.map((e) => {
              const isSelected = selected === e.id;
              const when = clock(e.started_at_unix_ms);
              return h(
                "div",
                { class: ["event-row", isSelected ? "selected" : ""] },
                h(
                  "button",
                  {
                    type: "button",
                    class: "event-main",
                    "aria-expanded": String(isSelected),
                    onclick: () => {
                      selected = isSelected ? null : e.id;
                      renderEvents(events);
                    },
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
                isSelected
                  ? h(
                      "div",
                      { class: "event-actions" },
                      h("p", { class: "note" }, t("history.event.note")),
                      h(
                        "div",
                        { class: "row" },
                        INTENTS.map((intent) =>
                          button(t(`inspect.${intent}`), {
                            variant: intent === "explain" ? "primary" : "",
                            iconName: intent === "explain" ? "sparkle" : "search",
                            onClick: () =>
                              selectOrAsk(
                                (target) => target.kind === "event" && target.event_id === e.id,
                                { label: `${eventLabel(e)} · ${when}`, subject: t("history.event.subject", { what: eventLabel(e), when }) },
                                intent,
                                t("history.event.question", { what: eventLabel(e).toLowerCase(), when }),
                              ),
                          }),
                        ),
                      ),
                    )
                  : null,
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
                toast(error.message, "bad");
              }
              renderCost();
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
            if (!yes) return;
            try {
              await call("history_clear");
              toast(t("history.cleared"));
              load();
            } catch (error) {
              toast(error.message, "bad");
            }
          },
        }),
      ),
    );
  };

  const refreshStatus = async () => {
    window.clearTimeout(poll);
    try {
      status = await call("history_status");
    } catch {
      status = null;
    }
    if (disposed) return;
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
  return () => {
    disposed = true;
    window.clearTimeout(poll);
    unsubscribe();
  };
}
