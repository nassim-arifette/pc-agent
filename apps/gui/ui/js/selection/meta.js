// Local display metadata for actions started from a selection. It never enters an IPC request
// or model context. Keep it bounded and erase it with its conversation; old reports need none.
const KEY = "pcagent.selection-labels.v1";
const MAX = 500;
const RETENTION = 90 * 24 * 60 * 60 * 1000;
let entries = null;

function read() {
  if (entries) return entries;
  try {
    const stored = JSON.parse(localStorage.getItem(KEY) || "[]");
    entries = Array.isArray(stored) ? stored.filter((entry) =>
      entry && typeof entry.turnId === "string" && entry.turnId.length <= 128
      && typeof entry.conversationId === "string" && entry.conversationId.length <= 128
      && ["explain", "investigate"].includes(entry.intent)
      && typeof entry.label === "string" && entry.label.length <= 160
      && Number.isFinite(entry.at) && entry.at > Date.now() - RETENTION,
    ).slice(-MAX) : [];
  } catch { entries = []; }
  return entries;
}

function save() {
  entries = read().filter((entry) => entry.at > Date.now() - RETENTION).slice(-MAX);
  try {
    if (entries.length) localStorage.setItem(KEY, JSON.stringify(entries));
    else localStorage.removeItem(KEY);
  } catch { /* Display labels still work in memory when browser storage is unavailable. */ }
}

export function rememberSelection(turnId, conversationId, selection, first = false) {
  if (!turnId || !conversationId || !selection) return;
  const previous = read().find((entry) => entry.turnId === turnId);
  entries = read().filter((entry) => entry.turnId !== turnId);
  entries.push({ turnId, conversationId, intent: selection.intent, label: selection.label.slice(0, 160), first: first || previous?.first === true, at: Date.now() });
  save();
}

export function selectionForTurn(turnId) {
  const entry = read().find((item) => item.turnId === turnId && item.at > Date.now() - RETENTION);
  return entry ? { intent: entry.intent, label: entry.label } : null;
}

export function selectionForConversation(conversationId) {
  const entry = read().find((item) => item.conversationId === conversationId && item.first === true && item.at > Date.now() - RETENTION);
  return entry ? { intent: entry.intent, label: entry.label } : null;
}

export function forgetSelections(conversationId) {
  entries = conversationId == null ? [] : read().filter((item) => item.conversationId !== conversationId);
  save();
}
