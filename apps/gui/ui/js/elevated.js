// The elevated helper runs one prepared or running task at a time (elevation_status). The cards
// that start one (quick drive analysis, deeper checks) publish whether they hold that slot, so the
// other card can disable its buttons with a hint instead of failing on elevation_prepare.
// Both cards also turn the helper's errors into readable sentences here.
import { h } from "./dom.js";
import { t } from "./i18n.js";

const holders = new Map();
const listeners = new Set();

/** Records whether `owner` currently holds the slot (prepared, running, stopping or cleaning up). */
export function holdSlot(owner, held) {
  if (Boolean(holders.get(owner)) === Boolean(held)) return;
  holders.set(owner, Boolean(held));
  for (const listener of [...listeners]) listener();
}

/** Whether another card holds the slot. */
export function slotHeldElsewhere(owner) {
  return [...holders].some(([name, held]) => held && name !== owner);
}

/** Calls `listener` when any card takes or releases the slot; returns the unsubscribe. */
export function onSlotChange(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

// Sentences the Rust slot sends as they are, mapped to translated ones.
const SENTENCES = {
  "An elevated inspection is already prepared or running.": "busy",
  "PC Agent is shutting down.": "closing",
  "This elevated inspection is no longer awaiting approval.": "unavailable",
  "This elevated inspection is no longer available.": "unavailable",
  "Elevation state unavailable.": "state",
};

/**
 * A readable sentence for an elevated task's error, and the raw code for a details line.
 * The helper's errors arrive as codes (`InsecureJob`, `Io(NotFound)`, `LaunchFailed(2)`), which
 * map to `elevation.error.<Code>`; unknown codes get a generic sentence and keep the code.
 */
export function elevationError(message) {
  const raw = String(message || "").trim();
  if (SENTENCES[raw]) return { text: t(`elevation.error.${SENTENCES[raw]}`), code: null };
  const match = /^([A-Z][A-Za-z]+)(?:\(([^()]*)\))?$/.exec(raw);
  if (!match) return { text: raw || t("elevation.error.unknown"), code: null };
  const key = `elevation.error.${match[1]}`;
  const text = t(key, { detail: match[2] ?? "" });
  return { text: text === key ? t("elevation.error.unknown") : text, code: raw };
}

/** The lines of a failure callout: the sentence, then "Details: <code>" when there is one. */
export function errorLines(message) {
  const { text, code } = elevationError(message);
  return [h("span", null, text), code ? h("span", { class: "elevated-code" }, t("elevation.error.code", { code })) : null];
}
