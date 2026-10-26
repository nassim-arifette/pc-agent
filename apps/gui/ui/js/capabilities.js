// What this PC and installation offer (`app_info.capabilities`, the registry of
// crates/agent/src/capabilities.rs): each name maps to true, false, or null while its probe has not
// answered yet (the window's first measurements). Like the model's tool catalog, an unknown
// capability counts as present, so the page never hides something that may work. Views offer only
// what works: the administrator cards of the Storage view and the chat need the elevated helper,
// the battery check a system battery, the quick drive analysis a fixed NTFS drive.
import { call } from "./api.js";
import { state, subscribe, notify } from "./store.js";

/** At most this many extra reads while a probe stays unanswered (one per live measurement). */
const MAX_READS = 12;

/** Whether this PC offers `name` (`elevated_helper`, `battery`, `ntfs_volumes`…). */
export function capable(name) {
  return state.info?.capabilities?.[name] !== false;
}

function pending() {
  const capabilities = state.info?.capabilities;
  return Boolean(capabilities) && Object.values(capabilities).includes(null);
}

/**
 * Reads the capabilities again after live measurements while a probe has not answered, and
 * notifies "info" when they changed. Only `capabilities` is replaced: the rest of app_info belongs
 * to the settings flows. Returns the unsubscribe.
 */
export function watchCapabilities() {
  let reads = 0;
  let reading = false;
  return subscribe(async (topic) => {
    if (topic !== "live" || reading || reads >= MAX_READS || !pending()) return;
    reading = true;
    reads += 1;
    try {
      const { capabilities } = await call("app_info");
      if (state.info && JSON.stringify(capabilities) !== JSON.stringify(state.info.capabilities)) {
        state.info = { ...state.info, capabilities };
        notify("info");
      }
    } catch {
      // The next measurement tries again.
    } finally {
      reading = false;
    }
  });
}
