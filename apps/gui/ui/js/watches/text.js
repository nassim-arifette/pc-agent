// Watch rules and alert values as the person reads them. Values come from the stored C4
// observations; a threshold is a setting, never presented as a measurement.
import { t } from "../i18n.js";
import { basisPoints, bytes } from "../format.js";

export function measured(value, unit) {
  if (value == null) return t("value.unknown");
  if (unit === "bytes") return bytes(value);
  if (unit === "degrees_celsius") return `${value} °C`;
  if (String(unit).startsWith("basis_points")) return basisPoints(value);
  return `${value} ${unit}`;
}

export function describe(rule) {
  const wait = rule.sustained_seconds ? ` · ${t(`watch.duration.${rule.sustained_seconds}`)}` : "";
  switch (rule.kind) {
    case "resource_above":
      return t("watch.describe.resource", { resource: t(`watch.resource.${rule.resource}`), threshold: basisPoints(rule.threshold_basis_points, 0) }) + wait;
    case "volume_free_below":
      return t("watch.describe.volume", { root: rule.root.replace(/\\$/, ""), threshold: bytes(rule.threshold_bytes) });
    case "gpu_temperature_above":
      return t("watch.describe.gpu", { index: rule.nvml_index, threshold: `${rule.threshold_celsius} °C` }) + wait;
    case "app_cpu_above":
      return t("watch.describe.appCpu", { name: rule.executable, threshold: basisPoints(rule.threshold_basis_points, 0) }) + wait;
    case "app_memory_above":
      return t("watch.describe.appMemory", { name: rule.executable, threshold: bytes(rule.threshold_bytes) }) + wait;
    default:
      return t("watch.invalid");
  }
}

/** A short subject for an alert's Explain question: what and when, with no name or ID. */
export function subject(alert, when) {
  return t("watch.alert.subject", { what: t(`watch.kind.${alert.rule.kind}`), when });
}
