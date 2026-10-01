// Application state and the backend's events. Views subscribe to topics ("info", "inv",
// "conversation", "history", "scan", "extension", "live", "panel") and redraw the parts they own.
import { call, on } from "./api.js";

const listeners = new Set();

export const state = {
  info: null,
  /** The current investigation, or null. */
  inv: null,
  /** The open conversation: { id, title, turns: [{ question, report, at }] }, or null. */
  conversation: null,
  /** Saved conversations, most recently updated first. */
  conversations: [],
  scan: { status: "idle", root: null, scopeId: null, progress: null, view: null, error: null },
  /** "Don't ask again until I close PC Agent" was ticked in the consent dialog. */
  skipConsent: false,
  /** Live measurements shared by the status pill and the panel, with a short history for sparklines. */
  live: { dash: null, error: null, paused: false, history: [] },
  /** The context panel: closed by default, opened on a tab, optionally pointing at one source. */
  panel: { open: false, tab: "sources", focus: null },
};

export function subscribe(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function notify(topic) {
  for (const listener of [...listeners]) listener(topic);
}

export async function refreshInfo() {
  state.info = await call("app_info");
  notify("info");
  return state.info;
}

// ---------- Context panel ----------

/** Opens the panel on a tab; `focus` = { turn, reference } points at one cited source. */
export function openPanel(tab, focus = null) {
  state.panel = { open: true, tab, focus };
  notify("panel");
}

export function closePanel() {
  state.panel = { ...state.panel, open: false, focus: null };
  notify("panel");
}

export function togglePanel() {
  if (state.panel.open) closePanel();
  else openPanel(state.panel.tab);
}

// ---------- Live measurements ----------

const LIVE_MS = 5000;
const HISTORY = 24;
let liveTimer = null;

async function liveTick() {
  window.clearTimeout(liveTimer);
  const live = state.live;
  if (!live.paused && !document.hidden) {
    try {
      live.dash = await call("dashboard");
      live.error = null;
      live.history.push(live.dash);
      if (live.history.length > HISTORY) live.history.shift();
    } catch (error) {
      live.error = error.message;
    }
    notify("live");
  }
  liveTimer = window.setTimeout(liveTick, LIVE_MS);
}

/** Measures every 5 s while the window is visible; the first sample comes right away. */
export function startLive() {
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) liveTick();
  });
  liveTick();
}

export function setLivePaused(paused) {
  state.live.paused = paused;
  notify("live");
  if (!paused) liveTick();
}

// ---------- Conversations ----------

function running() {
  return Boolean(state.inv && (state.inv.status === "running" || state.inv.status === "stopping"));
}

export async function refreshConversations() {
  try {
    state.conversations = await call("conversations_list");
  } catch {
    state.conversations = [];
  }
  notify("history");
}

/** Opens a saved conversation; returns false when it no longer exists. */
export async function openConversation(id) {
  const saved = await call("conversation_get", { id });
  if (!saved) {
    await refreshConversations();
    return false;
  }
  state.conversation = {
    id: saved.id,
    title: saved.title,
    turns: saved.turns.map((turn) => ({ question: turn.question, report: turn.report, at: turn.asked_at_unix_ms })),
  };
  // A running turn keeps going; it shows only in its own conversation.
  if (!running()) state.inv = null;
  notify("conversation");
  notify("inv");
  return true;
}

/** Leaves the open conversation; the next question starts a new one. */
export function newConversation() {
  state.conversation = null;
  if (!running()) state.inv = null;
  notify("conversation");
  notify("inv");
}

export async function deleteConversation(id) {
  await call("conversation_delete", { id });
  if (state.conversation?.id === id) newConversation();
  await refreshConversations();
}

export async function clearConversations() {
  const removed = await call("conversations_clear");
  newConversation();
  await refreshConversations();
  return removed;
}

// ---------- Investigations ----------

/** Events can arrive before start_investigation returns: the first one names the id. */
function current(id) {
  const inv = state.inv;
  if (!inv) return null;
  if (inv.id == null) inv.id = id;
  return inv.id === id ? inv : null;
}

/** Asks in the open conversation when `followUp` is true, otherwise starts a new one. */
export async function startInvestigation(question, { cloud, useStorage, names = true, followUp = false, previewId = null }) {
  const conversationId = followUp ? (state.conversation?.id ?? null) : null;
  if (!followUp || !state.conversation) state.conversation = { id: null, title: question, turns: [] };
  state.inv = {
    id: null,
    question,
    cloud,
    conversationId,
    status: "running",
    steps: [],
    report: null,
    failure: null,
    extension: null,
    startedAt: Date.now(),
  };
  notify("conversation");
  notify("inv");
  try {
    const started = await call("start_investigation", { request: { question, cloud, useStorage, names: Boolean(names), conversationId, previewId } });
    if (state.inv && state.inv.id == null) state.inv.id = started.id;
    if (state.inv && state.inv.id === started.id) state.inv.conversationId = started.conversation_id;
    if (state.conversation && state.conversation.id == null) state.conversation.id = started.conversation_id;
    refreshConversations();
  } catch (error) {
    state.inv.status = "failed";
    state.inv.failure = { code: "start", message: error.message };
    notify("inv");
  }
}

export async function stopInvestigation() {
  const inv = state.inv;
  if (!inv || inv.status !== "running") return;
  inv.status = "stopping";
  notify("inv");
  if (inv.id != null) await call("stop_investigation", { id: inv.id });
}

export async function answerExtension(accept) {
  const inv = state.inv;
  if (!inv || inv.id == null) return;
  inv.extension = null;
  notify("inv");
  try {
    await call("answer_extension", { id: inv.id, accept });
  } catch {
    // The investigation ended meanwhile; its final event updates the view.
  }
}

function update(inv, key, patch) {
  const step = inv.steps.find((s) => s.key === key);
  if (step) Object.assign(step, patch);
}

function onStep({ id, at_unix_ms: at, step }) {
  const inv = current(id);
  if (!inv) return;
  switch (step.kind) {
    case "snapshot_started":
      inv.steps.push({ key: "snapshot", type: "snapshot", state: "running", at });
      break;
    case "snapshot_finished":
      update(inv, "snapshot", { state: "done", duration: step.duration_ms, count: step.measured });
      break;
    case "model_started":
      inv.steps.push({ key: `model-${step.request}`, type: "model", state: "running", at });
      // A new request may write a new answer; an earlier draft was not accepted.
      inv.draft = null;
      break;
    case "answer_draft": {
      inv.draft = step.text;
      const writing = inv.steps.find((s) => s.key === `model-${step.request}`);
      if (writing && !writing.writing) {
        writing.writing = true;
        notify("inv");
      }
      notify("draft");
      return;
    }
    case "model_finished":
      update(inv, `model-${step.request}`, {
        state: step.outcome === "ok" ? "done" : "failed",
        duration: step.duration_ms,
        promptTokens: step.prompt_tokens,
        completionTokens: step.completion_tokens,
        outcome: step.outcome,
      });
      break;
    case "tool_started":
      inv.steps.push({ key: `tool-${inv.steps.length}`, type: "tool", tool: step.tool, state: "running", at });
      break;
    case "tool_finished": {
      const running = [...inv.steps].reverse().find((s) => s.type === "tool" && s.tool === step.tool && s.state === "running");
      const patch = {
        state: step.status === "ok" ? "done" : step.status === "partial" ? "partial" : "failed",
        status: step.status,
        duration: step.duration_ms,
      };
      if (running) Object.assign(running, patch);
      else inv.steps.push({ key: `tool-${inv.steps.length}`, type: "tool", tool: step.tool, at, ...patch });
      break;
    }
    default:
      return;
  }
  notify("inv");
}

function onExtension(payload) {
  const inv = current(payload.id);
  if (!inv) return;
  inv.extension = payload;
  notify("inv");
  notify("extension");
}

function onDone({ id, report, conversation_id: conversationId }) {
  const inv = current(id);
  if (!inv) return;
  // A local analysis runs the rules engine on the first measurement; show it as its own step.
  for (const step of inv.steps) if (step.state === "running") step.state = "cancelled";
  if (report.mode === "local_rules") inv.steps.push({ key: "local", type: "local", state: "done" });
  // The last request of a completed investigation is the one that chose the conclusion.
  const lastModel = [...inv.steps].reverse().find((s) => s.type === "model");
  if (lastModel && !report.error && lastModel.state === "done") lastModel.final = true;
  inv.report = report;
  inv.extension = null;
  inv.status = "done";
  inv.finishedAt = Date.now();
  const conversation = state.conversation;
  if (conversation && (conversation.id == null || conversation.id === (conversationId ?? inv.conversationId))) {
    conversation.id = conversationId ?? conversation.id;
    conversation.turns.push({ question: inv.question, report, at: inv.finishedAt });
  }
  notify("conversation");
  notify("inv");
  refreshConversations();
}

function onFailed({ id, code, message }) {
  const inv = current(id);
  if (!inv) return;
  for (const step of inv.steps) if (step.state === "running") step.state = "cancelled";
  inv.status = "failed";
  inv.failure = { code, message };
  inv.extension = null;
  notify("inv");
}

// ---------- Storage scans ----------

function currentScan(scopeId) {
  const scan = state.scan;
  if (scan.status === "starting" && !scan.scopeId) scan.scopeId = scopeId;
  return scan.scopeId === scopeId ? scan : null;
}

export async function startScan(path) {
  state.scan = { status: "starting", root: path, scopeId: null, progress: null, view: null, error: null };
  notify("scan");
  try {
    const started = await call("start_scan", { path });
    if (state.scan.status === "starting" || state.scan.scopeId === started.scope_id) {
      state.scan.scopeId = started.scope_id;
      state.scan.root = started.root;
      if (state.scan.status === "starting") state.scan.status = "scanning";
    }
  } catch (error) {
    state.scan.status = "failed";
    state.scan.error = error.message;
  }
  notify("scan");
}

export async function cancelScan() {
  if (state.scan.status !== "scanning" && state.scan.status !== "starting") return;
  await call("cancel_scan");
}

export async function morePage(kind) {
  const view = state.scan.view;
  const page = view?.[kind];
  if (!page || page.next_offset == null) return;
  const next = await call("scan_page", { scopeId: view.scope_id, kind, offset: page.next_offset });
  page.items.push(...next.items);
  page.next_offset = next.next_offset;
  notify("scan");
}

export function wireEvents() {
  on("inv:step", onStep);
  on("inv:extension", onExtension);
  on("inv:done", onDone);
  on("inv:failed", onFailed);
  on("scan:progress", (p) => {
    const scan = currentScan(p.scope_id);
    if (!scan) return;
    if (scan.status === "starting") scan.status = "scanning";
    scan.progress = p;
    notify("scan");
  });
  on("scan:done", (view) => {
    const scan = currentScan(view.scope_id);
    if (!scan) return;
    scan.status = "done";
    scan.view = view;
    scan.root = view.root;
    notify("scan");
  });
  on("scan:failed", ({ scope_id: scopeId, message }) => {
    const scan = currentScan(scopeId);
    if (!scan) return;
    scan.status = "failed";
    scan.error = message;
    notify("scan");
  });
}
