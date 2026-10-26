// The live battery read (view::BatteryView): words and numbers for the Live PC card, the panel
// row, the inspector and the PC status pill. Values arrive raw from the host; an unknown value
// stays null and is shown as not reported, never as zero. Each system battery is shown on its
// own: batteries are never summed. Data and text only, no DOM, shared by the panel and views.
import { t } from "../i18n.js";
import { ago, duration, number, percent } from "../format.js";

/**
 * A battery read older than this, against the dashboard it came with, shows its age and no longer
 * raises the status pill: three times the 10-second battery read interval. The host already stops
 * sending older reads; this keeps the window from presenting one as current on its own.
 */
export const STALE_MS = 30000;

/** Milliwatts to "8.4 W", without a sign. */
export function watts(mw) {
  return `${number(Math.abs(mw) / 1000, 1)} W`;
}

/** Milliwatts to "+23.5 W" while charging, "−8.4 W" while discharging (U+2212), or "0 W". */
export function signedWatts(mw) {
  if (!mw) return `${number(0)} W`;
  return `${mw > 0 ? "+" : "−"}${watts(mw)}`;
}

/** Milliwatt-hours to "41.2 Wh". */
export function wattHours(mwh) {
  return `${number(mwh / 1000, 1)} Wh`;
}

/**
 * An estimated time left: whole minutes under an hour ("< 1 min" below 60 s), otherwise
 * "3 h 10 min". An estimate never gets seconds: that would be false precision.
 */
export function hoursMinutes(seconds) {
  if (seconds < 60) return "< 1 min";
  const total = Math.round(seconds / 60);
  if (total < 60) return `${total} min`;
  const hours = Math.floor(total / 60);
  const minutes = total % 60;
  return minutes ? `${hours} h ${minutes} min` : `${hours} h`;
}

export function batteryLabel(count, index) {
  return count > 1 ? t("battery.labelN", { n: index + 1 }) : t("battery.label");
}

/** True when this read is older than the dashboard by more than STALE_MS. */
export function staleBattery(b, dash) {
  const at = b?.measured_at_unix_ms;
  const now = dash?.collected_at_unix_ms;
  return Number.isFinite(at) && Number.isFinite(now) && now - at > STALE_MS;
}

/**
 * Windows' own critical flag while not on AC power: no percentage threshold is invented here.
 * A stale read never raises the alert.
 */
export function lowBattery(b, dash) {
  return b?.critical === true && b.on_ac !== true && !staleBattery(b, dash);
}

/** The state alone ("Discharging"), for the inspector; null when Windows did not report it. */
export function stateOnly(b) {
  if (b.charge_state === "charging") return t("battery.charging.only");
  if (b.charge_state === "discharging") return t("battery.discharging.only");
  if (b.charge_state === "idle") return t(b.on_ac === true ? "battery.idle" : "battery.notCharging");
  return null;
}

/** The state with its power when known ("Discharging at 8.4 W"); null when not reported. */
export function stateText(b) {
  const power = b.power_rate_mw;
  if (b.charge_state === "charging" && power > 0) return t("battery.charging", { power: watts(power) });
  if (b.charge_state === "discharging" && power < 0) return t("battery.discharging", { power: watts(power) });
  return stateOnly(b);
}

/** Time left, only while discharging: Windows' estimate first, otherwise the calculated one. */
export function leftText(b) {
  if (b.charge_state !== "discharging") return null;
  if (b.estimated_runtime_windows_seconds != null) return t("battery.left", { time: hoursMinutes(b.estimated_runtime_windows_seconds) });
  if (b.estimated_runtime_at_current_rate_seconds != null) return t("battery.leftAtRate", { time: hoursMinutes(b.estimated_runtime_at_current_rate_seconds) });
  return null;
}

/** The reads of one battery in the window's recent dashboards: one point per distinct read. */
function readings(history, ref) {
  const seen = new Set();
  const points = [];
  for (const d of history || []) {
    const b = (d?.batteries || []).find((x) => x?.battery_ref === ref);
    if (!b || !Number.isFinite(b.measured_at_unix_ms) || seen.has(b.measured_at_unix_ms)) continue;
    seen.add(b.measured_at_unix_ms);
    points.push({ at: b.measured_at_unix_ms, charge: b.charge_percent ?? null, power: b.power_rate_mw ?? null });
  }
  return points.sort((a, b) => a.at - b.at);
}

/**
 * One item per system battery of the dashboard, for the Live PC cards and the panel rows:
 * { key, resource, label, value, detail, warn, stale, history, trend, measuredAt, reference,
 * sent, subject, battery }. `value` is null when the charge is unknown: the caller shows it as
 * not reported. With several batteries the one `battery` target carries all of them, so `sent`
 * and `subject` say so in the consent dialog and the question. When Windows lists a battery but
 * gave no usable reading, a single item with `selectable: false` says so; nothing on a desktop.
 */
export function batteryItems(dash, history) {
  const batteries = Array.isArray(dash?.batteries) ? dash.batteries.filter((b) => b && typeof b.battery_ref === "string") : [];
  if (!batteries.length) {
    if (dash?.battery_unavailable !== true) return [];
    return [{ key: "battery:unavailable", resource: "battery", label: t("battery.label"), value: null, detail: t("battery.unavailable"), warn: false, history: [], selectable: false }];
  }
  const several = batteries.length > 1;
  const reference = dash.battery_source_id ? { source_id: dash.battery_source_id, target: { kind: "resource", resource: "battery" } } : null;
  return batteries.map((b, index) => {
    const points = readings(history, b.battery_ref);
    const stale = staleBattery(b, dash);
    return {
      key: `battery:${b.battery_ref}`,
      resource: "battery",
      label: batteryLabel(batteries.length, index),
      value: b.charge_percent != null ? percent(b.charge_percent / 100) : null,
      detail: [stateText(b), leftText(b), stale && Number.isFinite(b.measured_at_unix_ms) ? t("inspect.measured", { ago: ago(b.measured_at_unix_ms) }) : null].filter(Boolean).join(" · "),
      warn: lowBattery(b, dash),
      stale,
      history: points.map((p) => (p.charge != null ? p.charge / 100 : null)),
      trend: points.filter((p) => p.power != null),
      measuredAt: Number.isFinite(b.measured_at_unix_ms) ? b.measured_at_unix_ms : null,
      reference,
      sent: several ? t("battery.sentAll") : undefined,
      subject: several ? t("select.what.batteries") : undefined,
      battery: b,
    };
  });
}

/**
 * The inspector's facts for one battery: { rows: [label, text or null], capacity, notes }. A null
 * text is a value Windows did not report. A battery in relative units shows its charge, state,
 * time left per Windows and cycles only: it has no energy, power or wear.
 */
export function batteryFacts(b) {
  const relative = b.capacity_units === "relative";
  const wh = (mwh) => (mwh != null ? wattHours(mwh) : null);
  const rows = [];
  const charge = b.charge_percent != null ? percent(b.charge_percent / 100) : null;
  rows.push([t("history.battery.charge"), charge != null && b.critical === true ? `${charge} · ${t("battery.critical")}` : charge]);
  rows.push([t("inspect.battery.state"), stateOnly(b)]);
  if (!relative) rows.push([t("inspect.battery.power"), b.power_rate_mw != null ? signedWatts(b.power_rate_mw) : null]);
  // Both estimates exist only while discharging; they keep their own labels and never merge.
  if (b.charge_state === "discharging") {
    const windows = b.estimated_runtime_windows_seconds;
    rows.push([t("history.battery.windows"), windows != null ? t("history.battery.windowsValue", { time: hoursMinutes(windows) }) : null]);
    if (!relative) {
      const atRate = b.estimated_runtime_at_current_rate_seconds;
      rows.push([t("history.battery.left"), atRate != null ? t("history.battery.leftValue", { time: hoursMinutes(atRate) }) : null]);
    }
  }
  if (!relative) {
    rows.push([t("inspect.battery.remaining"), wh(b.remaining_capacity_mwh)]);
    rows.push([t("inspect.battery.full"), wh(b.full_charge_capacity_mwh)]);
    rows.push([t("inspect.battery.design"), wh(b.design_capacity_mwh)]);
    rows.push([t("inspect.battery.retention"), b.capacity_retention_percent != null ? percent(b.capacity_retention_percent / 100, 1) : null]);
    rows.push([t("inspect.battery.wear"), b.estimated_wear_percent != null ? percent(b.estimated_wear_percent / 100, 1) : null]);
  }
  rows.push([t("inspect.battery.cycles"), b.cycle_count != null ? number(b.cycle_count) : null]);
  const full = b.full_charge_capacity_mwh;
  const design = b.design_capacity_mwh;
  const capacity =
    !relative && full != null && design > 0 ? { text: t("inspect.battery.capacityBar", { full: wattHours(full), design: wattHours(design) }), ratio: Math.max(0, Math.min(1, full / design)) } : null;
  const notes = [];
  if (b.inconsistent) notes.push(t("inspect.battery.inconsistent"));
  notes.push(relative ? t("inspect.battery.relative") : t("inspect.battery.powerNote"));
  // "Calculated" is said where the at-rate estimate is shown; otherwise only when time left exists.
  notes.push(t(b.charge_state === "discharging" && !relative ? "inspect.battery.estimateNote" : "inspect.battery.estimateNote.short"));
  if (!relative && (b.capacity_retention_percent != null || b.estimated_wear_percent != null)) notes.push(t("inspect.battery.wearNote"));
  return { rows, capacity, notes };
}

/** "Power over 1 min 50 s": the span actually covered by the window's reads, never a fixed one. */
export function trendCaption(trend) {
  return t("inspect.battery.trend", { time: duration(trend[trend.length - 1].at - trend[0].at) });
}
