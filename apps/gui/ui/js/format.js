// Number, size and time formatting in the interface language. Sizes use binary units, like the
// core, so the same value reads the same everywhere.
import { language, t } from "./i18n.js";

const locale = () => (language() === "fr" ? "fr-FR" : "en-US");

export function number(value, digits = 0) {
  return new Intl.NumberFormat(locale(), { maximumFractionDigits: digits, minimumFractionDigits: digits }).format(value);
}

export function bytes(value) {
  if (value == null) return "";
  const french = language() === "fr";
  const units = french ? ["Kio", "Mio", "Gio", "Tio", "Pio"] : ["KiB", "MiB", "GiB", "TiB", "PiB"];
  if (value < 1024) return `${number(value)} ${french ? "o" : "B"}`;
  let v = value / 1024;
  let unit = 0;
  while (v >= 1024 && unit < units.length - 1) {
    v /= 1024;
    unit += 1;
  }
  return `${number(v, unit >= 2 ? 2 : 1)} ${units[unit]}`;
}

/** 0..1 → "37%" */
export function percent(ratio, digits = 0) {
  if (ratio == null || Number.isNaN(ratio)) return "";
  return new Intl.NumberFormat(locale(), { style: "percent", maximumFractionDigits: digits, minimumFractionDigits: digits }).format(ratio);
}

/** Basis points (1/100 of a percent) → "12.5%" */
export function basisPoints(value, digits = 1) {
  return value == null ? "" : percent(value / 10000, digits);
}

export function duration(ms) {
  if (ms == null) return "";
  if (ms < 1000) return `${number(ms)} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${number(seconds, seconds < 10 ? 1 : 0)} s`;
  const minutes = Math.floor(seconds / 60);
  const rest = Math.round(seconds % 60);
  return rest ? `${minutes} min ${rest} s` : `${minutes} min`;
}

export function ago(unixMs) {
  const s = Math.max(0, Math.round((Date.now() - unixMs) / 1000));
  if (s < 3) return t("time.justNow");
  if (s < 90) return t("time.secondsAgo", { n: s });
  return t("time.minutesAgo", { n: Math.round(s / 60) });
}

export function date(unixMs, withTime = false) {
  if (unixMs == null) return "";
  const options = withTime
    ? { dateStyle: "medium", timeStyle: "short" }
    : { dateStyle: "medium" };
  return new Intl.DateTimeFormat(locale(), options).format(new Date(unixMs));
}

/**
 * A wall-clock time with no known zone ("2026-10-03 21:14:05…", as Windows' energy history writes
 * it) as a date and minute in the interface language, never shifted by this PC's zone; null when
 * the text does not start with one.
 */
export function civilTime(text) {
  const parts = /^(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2})/.exec(String(text ?? ""));
  if (!parts) return null;
  const [year, month, day, hour, minute] = parts.slice(1).map(Number);
  const at = new Date(Date.UTC(year, month - 1, day, hour, minute));
  return new Intl.DateTimeFormat(locale(), { dateStyle: "medium", timeStyle: "short", timeZone: "UTC" }).format(at);
}

/** Price strings such as "0.30" from the core, localized. */
export function price(value) {
  if (value == null) return "?";
  const n = Number(value);
  return Number.isFinite(n) ? number(n, 2) : value;
}

/** Exact USD amounts from the core ("0.0044") shown with the locale's decimal mark. */
export function usd(value) {
  if (value == null) return "";
  return language() === "fr" ? `${value.replace(".", ",")} $` : `$${value}`;
}

/** "14:02" today, otherwise the date and time. */
export function clock(unixMs, { seconds = false } = {}) {
  if (unixMs == null) return "";
  const at = new Date(unixMs);
  if (at.toDateString() !== new Date().toDateString()) return date(unixMs, true);
  return new Intl.DateTimeFormat(locale(), { hour: "2-digit", minute: "2-digit", second: seconds ? "2-digit" : undefined }).format(at);
}

/** "Label: value" in English, "Label : value" in French. */
export function labelled(label, value) {
  return `${label}${language() === "fr" ? " : " : ": "}${value}`;
}
