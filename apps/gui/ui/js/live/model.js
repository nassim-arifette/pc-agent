// Display decisions from measured data, independent of the DOM and the bridge.
import { t } from "../i18n.js";
import { lowBattery } from "./battery.js";

const GiB = 1024 ** 3;

/** A drive is "almost full" under 5 % or 5 GiB free: a hint for the status line, not a finding. */
export function almostFull(volume) {
  return volume.free_ratio < 0.05 || volume.free_bytes < 5 * GiB;
}

export function driveName(root) {
  return (root || "").replace(/[\\/]+$/, "");
}

/** The most important thing to know right now, in a few words. */
export function health(dash) {
  if (!dash) return { tone: "neutral", text: t("health.measuring") };
  const full = (dash.volumes || []).filter(almostFull).sort((a, b) => a.free_ratio - b.free_ratio)[0];
  const drive = full ? driveName(full.root) : undefined;
  // A battery Windows flags as critical while unplugged comes first; the almost-full drive stays
  // in the result so the chat home keeps its suggestion about it.
  if ((dash.batteries || []).some((b) => lowBattery(b, dash))) return { tone: "warn", text: t("health.battery"), drive };
  if (full) return { tone: "warn", text: t("health.diskFull", { drive }), drive };
  if (dash.memory.used_ratio != null && dash.memory.used_ratio >= 0.9) return { tone: "warn", text: t("health.memory") };
  if (dash.cpu.busy.value != null && dash.cpu.busy.value >= 9000) return { tone: "warn", text: t("health.cpu") };
  if (dash.gpus.some((g) => g.slowdown.value != null && g.slowdown.value !== 0)) return { tone: "warn", text: t("health.gpu") };
  return { tone: "good", text: t("health.ok") };
}

export function gpuName(name) {
  return name.replace(/^NVIDIA\s+/i, "").replace(/\s+GPU$/i, "");
}
