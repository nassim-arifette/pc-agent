// Node-only test support; never loaded by the application.
import "node:assert/strict";
// Shared synthetic measurements and Rust-shaped command failures.
export const GiB = 1024 ** 3;

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
