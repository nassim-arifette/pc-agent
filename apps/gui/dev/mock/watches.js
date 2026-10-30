// Synthetic C7 watches: rules, their live state as the evaluator reports it, alerts of the last
// seven days and the local weekly summary. ?watches=none starts without rules or alerts;
// ?watches=paused shows another window evaluating; ?notifications=disabled, Windows having
// turned PC Agent's notifications off.
import { fail, GiB } from "./common.js";

const ID = /^[A-Za-z0-9-]{1,64}$/;
const KINDS = ["volume_free_below", "resource_above", "gpu_temperature_above", "app_cpu_above", "app_memory_above"];
const target = (rule) => `${rule.kind}:${rule.root ?? rule.resource ?? rule.nvml_index ?? String(rule.executable).toLowerCase()}`;

export function createWatchFixture(params, publishSource) {
  const now = () => Date.now();
  const none = params.get("watches") === "none";
  // The same C: as Live PC's mock, so screenshots agree (0.3 GiB free unless ?disk=ok).
  const cFree = (params.get("disk") === "ok" ? 61 : 0.3) * GiB;
  const paused = params.get("watches") === "paused";
  const notifications = params.get("notifications") === "disabled" ? "disabled" : "enabled";
  let sequence = 0;
  let pending = params.get("activation") === "alert" ? "watch-alert-mock-1" : null;
  const rule = (r, enabled = true) => ({ id: `watch-mock-${++sequence}`, version: 1, rule: r, enabled, created_at_unix_ms: now() - 3 * 86_400_000, updated_at_unix_ms: now() - 3 * 86_400_000, last_notified_unix_ms: null });
  const rules = none ? [] : [
    rule({ kind: "resource_above", resource: "cpu", threshold_basis_points: 9000, sustained_seconds: 60 }),
    rule({ kind: "volume_free_below", root: "C:\\", threshold_bytes: 10 * GiB }),
    rule({ kind: "app_memory_above", executable: "chrome.exe", threshold_bytes: 4 * GiB, sustained_seconds: 300 }, false),
  ];
  if (rules[1]) rules[1].last_notified_unix_ms = now() - 2 * 3_600_000;
  const observation = (metric, value, unit, at) => ({ entity_ref: "system", metric, value, unit, source: "mock", status: "ok", origin: "synthetic_fixture", measured_at_unix_ms: at, window: null, derivation: null, inputs: [] });
  const alert = (id, r, startedAgo, endedAgo, value, unit, extra = {}) => ({
    id, watch_id: r.id, rule: r.rule, started_at_unix_ms: now() - startedAgo, last_checked_unix_ms: now() - (endedAgo ?? 0),
    ended_at_unix_ms: endedAgo == null ? null : now() - endedAgo, close_reason: endedAgo == null ? null : "returned",
    observation: observation(r.rule.resource ? "cpu_busy_basis_points" : "volume_free_bytes", value, unit, now() - startedAgo),
    extreme_observation: observation(r.rule.resource ? "cpu_busy_basis_points" : "volume_free_bytes", value, unit, now() - startedAgo + 30_000),
    lower_bound: false, notification: "sent", seen: true, ...extra,
  });
  let alerts = none ? [] : [
    alert("watch-alert-mock-1", rules[1], 2 * 3_600_000, null, cFree, "bytes", { seen: false }),
    alert("watch-alert-mock-2", rules[0], 26 * 3_600_000, 25.9 * 3_600_000, 9730, "basis_points"),
    alert("watch-alert-mock-3", rules[0], 50 * 3_600_000, 49.95 * 3_600_000, 9410, "basis_points", { notification: "rule_cooldown" }),
    alert("watch-alert-mock-4", rules[0], 4 * 86_400_000, null, 9120, "basis_points", { ended_at_unix_ms: now() - 4 * 86_400_000 + 600_000, close_reason: "interrupted" }),
  ];
  const find = (id) => {
    if (typeof id !== "string" || !ID.test(id)) fail("unknown", "Invalid watch or alert identifier.");
    return rules.find((r) => r.id === id) ?? fail("history_unavailable", "No such watch.");
  };
  const check = (r) => {
    if (!r || !KINDS.includes(r.kind)) fail("history_unavailable", "Invalid watch rule.");
    if (r.kind === "gpu_temperature_above") fail("history_unavailable", "No NVIDIA GPU temperature is read in the mock.");
  };
  const duplicate = (r, except) => rules.some((x) => x.id !== except && target(x.rule) === target(r));
  const unseen = () => alerts.filter((a) => !a.seen).length;
  const live = () => rules.map((watch) => {
    const open = alerts.find((a) => a.watch_id === watch.id && a.ended_at_unix_ms == null);
    const phase = !watch.enabled ? "disabled" : paused ? "paused" : open ? "alert" : "normal";
    const reading = watch.rule.kind === "volume_free_below" ? observation("volume_free_bytes", cFree, "bytes", now() - 40_000)
      : watch.rule.kind === "resource_above" ? observation("cpu_busy_basis_points", 2310, "basis_points", now() - 5_000) : null;
    return { watch, phase, reason: null, observation: watch.enabled ? reading : null, lower_bound: false, alert_id: open?.id ?? null };
  });
  return {
    commands: {
      watches_list: () => ({
        rules: structuredClone(rules),
        engine: { owns_lease: !paused, rules: live(), unseen_alerts: unseen(), error: null },
        notification_status: notifications,
        language: "en",
        unseen_alerts: unseen(),
      }),
      watch_create: ({ rule: r }) => {
        check(r);
        if (duplicate(r)) fail("history_unavailable", "A watch of this measure already exists for this target.");
        const created = rule(structuredClone(r));
        created.created_at_unix_ms = created.updated_at_unix_ms = now();
        rules.push(created);
        return structuredClone(created);
      },
      watch_update: ({ id, rule: r }) => {
        const watch = find(id);
        check(r);
        if (duplicate(r, id)) fail("history_unavailable", "A watch of this measure already exists for this target.");
        watch.rule = structuredClone(r);
        watch.updated_at_unix_ms = now();
        for (const a of alerts) if (a.watch_id === id && a.ended_at_unix_ms == null) Object.assign(a, { ended_at_unix_ms: now(), close_reason: "modified" });
        return null;
      },
      watch_set_enabled: ({ id, enabled }) => {
        if (typeof enabled !== "boolean") fail("unknown", "enabled must be a boolean");
        const watch = find(id);
        watch.enabled = enabled;
        if (!enabled) for (const a of alerts) if (a.watch_id === id && a.ended_at_unix_ms == null) Object.assign(a, { ended_at_unix_ms: now(), close_reason: "disabled" });
        return null;
      },
      watch_delete: ({ id }) => {
        find(id);
        rules.splice(rules.findIndex((r) => r.id === id), 1);
        alerts = alerts.filter((a) => a.watch_id !== id);
        return notifications === "disabled" ? "disabled" : "sent";
      },
      watch_alerts: ({ fromUnixMs, toUnixMs, limit, offset }) => {
        if (![fromUnixMs, toUnixMs, limit, offset].every(Number.isSafeInteger) || fromUnixMs > toUnixMs || toUnixMs - fromUnixMs > 7 * 86_400_000 || limit < 1 || limit > 200 || offset < 0) {
          fail("history_unavailable", "Invalid alert query.");
        }
        const matching = alerts.filter((a) => a.started_at_unix_ms >= fromUnixMs && a.started_at_unix_ms <= toUnixMs).sort((a, b) => b.started_at_unix_ms - a.started_at_unix_ms);
        const page = matching.slice(offset, offset + limit);
        return { alerts: structuredClone(page), total: matching.length, next_offset: offset + page.length < matching.length ? offset + page.length : null };
      },
      watch_alert_detail: ({ id }) => {
        if (typeof id !== "string" || !ID.test(id)) fail("unknown", "Invalid watch or alert identifier.");
        const found = alerts.find((a) => a.id === id);
        if (!found) return null;
        return { alert: structuredClone(found), source_id: publishSource([{ kind: "watch_alert", alert_id: id }], found.started_at_unix_ms) };
      },
      watch_alerts_seen: ({ upToUnixMs }) => {
        if (!Number.isSafeInteger(upToUnixMs)) fail("unknown", "upToUnixMs must be an integer");
        for (const a of alerts) if (a.started_at_unix_ms <= upToUnixMs) a.seen = true;
        return null;
      },
      watch_language_set: ({ language }) => {
        if (!["en", "fr"].includes(language)) fail("history_unavailable", "Invalid language.");
        return null;
      },
      watch_notification_test: () => (notifications === "disabled" ? "disabled" : "sent"),
      watch_weekly_summary: () => {
        const from = now() - 7 * 86_400_000;
        const week = alerts.filter((a) => a.started_at_unix_ms >= from);
        const by = {};
        for (const a of week) by[a.rule.kind] = (by[a.rule.kind] ?? 0) + 1;
        return {
          from_unix_ms: from, to_unix_ms: now(), total: week.length,
          open: week.filter((a) => a.ended_at_unix_ms == null).length,
          returned: week.filter((a) => a.close_reason === "returned").length,
          interrupted: week.filter((a) => a.close_reason === "interrupted").length,
          by_measure: by, local_only: true,
        };
      },
      watch_activation_take: () => {
        const id = pending;
        pending = null;
        return id;
      },
    },
  };
}
