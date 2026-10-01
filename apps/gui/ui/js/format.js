// Number, size and time formatting in the interface language. Sizes use binary units, like the
// core, so the same value reads the same everywhere.
import { language, t } from "./i18n.js";

const locale = () => (language() === "fr" ? "fr-FR" : "en-US");

export function number(value, digits = 0) {
  return new Intl.NumberFormat(locale(), { maximumFractionDigits: digits, minimumFractionDigits: digits }).format(value);
}

export function bytes(value) {
  if (value == null) return "";
  const units = ["KiB", "MiB", "GiB", "TiB", "PiB"];
  if (value < 1024) return `${number(value)} B`;
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
  return `${minutes} min ${rest} s`;
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
export function clock(unixMs) {
  if (unixMs == null) return "";
  const at = new Date(unixMs);
  if (at.toDateString() !== new Date().toDateString()) return date(unixMs, true);
  return new Intl.DateTimeFormat(locale(), { hour: "2-digit", minute: "2-digit" }).format(at);
}

/** "Label: value" in English, "Label : value" in French. */
export function labelled(label, value) {
  return `${label}${language() === "fr" ? " : " : ": "}${value}`;
}
