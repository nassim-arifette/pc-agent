export const KINDS = ["resource_above", "volume_free_below", "gpu_temperature_above", "app_cpu_above", "app_memory_above"];
export const DURATIONS = [0, 60, 300, 900];
export const MIB = 1024 ** 2;

const integer = (value, min, max = Number.MAX_SAFE_INTEGER) => Number.isSafeInteger(value) && value >= min && value <= max;
const executable = (value) => typeof value === "string" && value.trim().length > 0 && [...value].length <= 260 && !/[\\/:\p{Cc}]/u.test(value);

/** UI validation mirrors C7; native validation additionally proves the target is observable. */
export function validRule(rule) {
  if (!rule || typeof rule !== "object" || Array.isArray(rule) || !KINDS.includes(rule.kind)) return false;
  const fields = {
    volume_free_below: ["root", "threshold_bytes"],
    resource_above: ["resource", "threshold_basis_points", "sustained_seconds"],
    gpu_temperature_above: ["nvml_index", "threshold_celsius", "sustained_seconds"],
    app_cpu_above: ["executable", "threshold_basis_points", "sustained_seconds"],
    app_memory_above: ["executable", "threshold_bytes", "sustained_seconds"],
  }[rule.kind];
  if (Object.keys(rule).length !== fields.length + 1 || fields.some((key) => !Object.hasOwn(rule, key))) return false;
  if (rule.kind !== "volume_free_below" && !DURATIONS.includes(rule.sustained_seconds)) return false;
  switch (rule.kind) {
    case "volume_free_below": return typeof rule.root === "string" && /^[a-z]:\\$/i.test(rule.root) && integer(rule.threshold_bytes, 1024 ** 3);
    case "resource_above": return ["cpu", "memory", "disk_busy"].includes(rule.resource) && integer(rule.threshold_basis_points, 1000, 10000);
    case "gpu_temperature_above": return integer(rule.nvml_index, 0, 2 ** 32 - 1) && integer(rule.threshold_celsius, 50, 105);
    case "app_cpu_above": return executable(rule.executable) && integer(rule.threshold_basis_points, 100, 10000);
    case "app_memory_above": return executable(rule.executable) && integer(rule.threshold_bytes, 64 * MIB, 2 ** 40);
    default: return false;
  }
}

export function defaultRule(kind = "resource_above") {
  switch (kind) {
    case "volume_free_below": return { kind, root: "C:\\", threshold_bytes: 10 * 1024 ** 3 };
    case "gpu_temperature_above": return { kind, nvml_index: 0, threshold_celsius: 80, sustained_seconds: 60 };
    case "app_cpu_above": return { kind, executable: "", threshold_basis_points: 5000, sustained_seconds: 60 };
    case "app_memory_above": return { kind, executable: "", threshold_bytes: 1024 ** 3, sustained_seconds: 60 };
    default: return { kind: "resource_above", resource: "cpu", threshold_basis_points: 8000, sustained_seconds: 60 };
  }
}

/** Only an exact C7 DTO leaves the form. Never send a selection reference or wrapper. */
export function formRule(kind, { target, threshold, duration }) {
  const number = threshold.trim() === "" ? Number.NaN : Number(threshold);
  const sustained_seconds = Number(duration);
  switch (kind) {
    case "volume_free_below": return { kind, root: target.trim().toUpperCase(), threshold_bytes: Math.round(number * MIB) };
    case "resource_above": return { kind, resource: target, threshold_basis_points: Math.round(number * 100), sustained_seconds };
    case "gpu_temperature_above": return { kind, nvml_index: target.trim() === "" ? Number.NaN : Number(target), threshold_celsius: number, sustained_seconds };
    case "app_cpu_above": return { kind, executable: target.trim(), threshold_basis_points: Math.round(number * 100), sustained_seconds };
    case "app_memory_above": return { kind, executable: target.trim(), threshold_bytes: Math.round(number * MIB), sustained_seconds };
    default: return null;
  }
}
