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

const GIB = 1024 ** 3;

/**
 * The rule Live PC proposes for a selected item, or the closed reason it cannot be watched. The
 * values are starting points the form shows; nothing persists before its confirmation.
 */
export function liveWatch(item) {
  if (!item) return { reason: "watch.reason.none" };
  if (item.kind === "resource") {
    const card = item.card;
    if (card.resource === "memory" || card.resource === "cpu") {
      return { rule: { kind: "resource_above", resource: card.resource, threshold_basis_points: 9000, sustained_seconds: 60 } };
    }
    if (card.resource === "disk") {
      const volume = card.volume;
      if (!volume || volume.kind !== "fixed" || !/^[a-z]:\\$/i.test(volume.root) || !(volume.total_bytes >= GIB)) return { reason: "watch.reason.volume" };
      const tenth = Math.floor(volume.total_bytes / 10 / GIB) * GIB;
      return { rule: { kind: "volume_free_below", root: volume.root.toUpperCase(), threshold_bytes: Math.max(GIB, Math.min(10 * GIB, tenth)) } };
    }
    if (card.resource === "gpu") {
      const gpu = card.gpu;
      if (!Number.isSafeInteger(gpu?.nvml_index) || gpu.temperature?.value == null) return { reason: "watch.reason.gpu" };
      return { rule: { kind: "gpu_temperature_above", nvml_index: gpu.nvml_index, threshold_celsius: 85, sustained_seconds: 60 } };
    }
    return { reason: "watch.reason.battery" };
  }
  const name = item.kind === "process" ? item.member?.name : item.app?.name;
  const named = (item.kind === "process" ? item.member?.named : item.app?.named) !== false;
  if (!named || typeof name !== "string" || !name.trim() || /[\\/:\p{Cc}]/u.test(name)) return { reason: "watch.reason.unnamed" };
  const current = (item.kind === "process" ? item.member?.working_set?.value : item.app?.working_set?.value) ?? 0;
  const threshold = Math.min(2 ** 40, Math.max(64 * MIB, Math.ceil((current * 1.5) / (64 * MIB)) * 64 * MIB));
  return { rule: { kind: "app_memory_above", executable: name.trim(), threshold_bytes: threshold, sustained_seconds: 60 } };
}
