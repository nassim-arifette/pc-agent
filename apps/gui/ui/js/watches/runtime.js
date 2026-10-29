// What the whole window knows about watches: the unseen-alert count for the rail, a short toast
// when an alert opens in this window, and the Surveillances view when a notification is clicked.
// Nothing here creates a rule or starts a model turn.
import { call, on } from "../api.js";
import { t } from "../i18n.js";
import { toast } from "../ui.js";
import { navigate } from "../router.js";

let unseen = 0;
const changed = new Set();

export function unseenAlerts() {
  return unseen;
}

/** Called with the new count whenever it changes. Returns the unsubscribe function. */
export function onUnseenChange(run) {
  changed.add(run);
  return () => changed.delete(run);
}

function setUnseen(value) {
  const next = Number.isSafeInteger(value) && value >= 0 ? value : 0;
  if (next === unseen) return;
  unseen = next;
  for (const run of changed) run(unseen);
}

export async function refreshWatches() {
  try {
    const list = await call("watches_list");
    setUnseen(list?.unseen_alerts ?? 0);
    return list;
  } catch {
    return null;
  }
}

export async function startWatchRuntime() {
  await Promise.all([
    on("watch:changed", (count) => setUnseen(count)),
    on("watch:alert", (payload) => {
      setUnseen(payload?.unseen_alerts ?? unseen + 1);
      toast(t("watches.toast.alert"), "bad");
      window.dispatchEvent(new CustomEvent("watches:changed"));
    }),
    on("watch:open", () => navigate("watches")),
  ].map((p) => Promise.resolve(p).catch(() => null)));
  window.addEventListener("watches:changed", () => { refreshWatches(); });
  // A notification clicked while the window was starting is kept on the Rust side until read.
  const pending = await call("watch_activation_take").catch(() => null);
  if (pending) navigate("watches");
  await refreshWatches();
}
