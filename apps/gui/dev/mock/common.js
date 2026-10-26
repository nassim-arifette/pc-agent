// Shared synthetic measurements and Rust-shaped command failures.
export const wait = (ms) => new Promise((resolve) => window.setTimeout(resolve, ms));

export const GiB = 1024 ** 3;

export const MiB = 1024 ** 2;

export function fail(code, message, reason = null) {
  throw reason ? { code, reason, message } : { code, message };
}

export function fmt(bytes) {
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let v = bytes / 1024;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u += 1;
  }
  return `${v.toFixed(u >= 2 ? 2 : 1)} ${units[u]}`;
}

export const m = (value, text) => ({ value, text, status: "ok", note: null });

export const missing = (status, note) => ({ value: null, text: null, status, note });

export const absent = () => ({ value: null, text: null, status: null, note: null });

export const jitter = (base, spread) => base + (Math.random() - 0.5) * spread;

export function volume(root, totalGiB, freeGiB, filesystem = "NTFS") {
  return { root, kind: "fixed", filesystem, total_bytes: totalGiB * GiB, free_bytes: freeGiB * GiB, free_ratio: freeGiB / totalGiB };
}
