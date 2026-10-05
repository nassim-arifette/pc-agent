// Application state and the backend's events. Views subscribe to topics ("info", "inv",
// "conversation", "history", "scan", "extension", "live", "panel", "draft", "privacy") and redraw
// the parts they own.
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
  /** The stored sharing choice: complete, anonymized or local; cloud modes need confirmation. */
  privacy: { mode: "complete", confirmed: false },
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

export async function refreshPrivacy() {
  try {
    state.privacy = await call("privacy_get");
  } catch {
    state.privacy = { mode: "local", confirmed: false };
  }
  notify("privacy");
  return state.privacy;
}

/** Stores a sharing choice. Changing the mode invalidates any reviewed preview on the Rust side. */
export async function setPrivacy(mode, confirmed) {
  const previous = state.privacy.mode;
  state.privacy = await call("privacy_set", { settings: { mode, confirmed } });
  if (state.privacy.mode !== previous) state.skipConsent = false;
  notify("privacy");
  return state.privacy;
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
    if (document.hidden) return;
    liveTick();
    activityTick();
  });
  liveTick();
}

export function setLivePaused(paused) {
  state.live.paused = paused;
  notify("live");
  if (!paused) {
    liveTick();
    activityTick();
  }
}

// Application activity takes a two-second measurement window: it runs only while a view that
// shows it is visible, every ten seconds, never in the background.
const ACTIVITY_MS = 10000;
const activityOwners = new Set();
let activityTimer = null;
let activityBusy = false;

async function activityTick() {
  window.clearTimeout(activityTimer);
  const wanted = [...activityOwners].some((visible) => visible());
  if (!wanted || document.hidden || state.live.paused) return;
  if (!activityBusy) {
    activityBusy = true;
    try {
      state.live.activity = await call("process_activity");
    } catch {
      // The resource rows still show; the list keeps its last value.
    }
    activityBusy = false;
    notify("live");
  }
  activityTimer = window.setTimeout(activityTick, ACTIVITY_MS);
}

/** Registers a view that shows applications while `visible()` is true; returns its removal. */
export function watchActivity(visible) {
  activityOwners.add(visible);
  activityTick();
  return () => activityOwners.delete(visible);
}

/** Measures applications now if a view needs them, e.g. when it becomes visible. */
export function pokeActivity() {
  activityTick();
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

// ---------- Turns ----------
// A question and its answer form a turn with a stable ID, returned by send_message and carried by
// every turn:* event. The Rust side still emits the legacy inv:* events for older clients; this
// window listens to turn:* only, so nothing is drawn twice.

/** Events can arrive before send_message returns: the first one names the turn. */
function current(turnId) {
  const inv = state.inv;
  if (!inv) return null;
  if (inv.turnId == null) inv.turnId = turnId;
  return inv.turnId === turnId ? inv : null;
}

/**
 * Sends a question in the open conversation when `followUp` is true, otherwise in a new one.
 * The sharing mode is the stored privacy choice; a cloud mode needs the reviewed preview ticket.
 */
export async function sendMessage(question, { followUp = false, previewId = null, scopeIds = [] } = {}) {
  const conversationId = followUp ? (state.conversation?.id ?? null) : null;
  return startTurn(question, conversationId, followUp, () => call("send_message", { conversationId, text: question, scopeIds, previewId }));
}

/**
 * Starts an Explain or Investigate turn on a selected measurement, in a new conversation, with the
 * question and reference of the reviewed preview ticket; any difference makes Rust refuse it.
 */
export async function sendSelection(question, { reference, intent, previewId }) {
  return startTurn(question, null, false, () => call("selection_send", { reference, intent, question, conversationId: null, previewId }));
}

async function startTurn(question, conversationId, followUp, start) {
  if (!followUp || !state.conversation) state.conversation = { id: null, title: question, turns: [] };
  state.inv = {
    turnId: null,
    question,
    // Same rule as effectiveMode() and send_message: a cloud mode without a usable key runs locally.
    cloud: state.privacy.mode !== "local" && Boolean(state.info?.cloud?.ready),
    conversationId,
    status: "running",
    steps: [],
    draft: null,
    report: null,
    failure: null,
    extension: null,
    startedAt: Date.now(),
  };
  notify("conversation");
  notify("inv");
  try {
    const started = await start();
    if (state.inv && state.inv.turnId == null) state.inv.turnId = started.turn_id;
    if (state.inv && state.inv.turnId === started.turn_id) state.inv.conversationId = started.conversation_id;
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
  if (inv.turnId == null) return;
  try {
    await call("stop_turn", { turnId: inv.turnId });
  } catch {
    // The turn ended meanwhile; its final event updates the view.
  }
}

export async function answerExtension(accept) {
  const inv = state.inv;
  if (!inv || inv.turnId == null) return;
  inv.extension = null;
  notify("inv");
  try {
    await call("answer_extension", { turnId: inv.turnId, accept });
  } catch {
    // The turn ended meanwhile; its final event updates the view.
  }
}

function update(inv, key, patch) {
  const step = inv.steps.find((s) => s.key === key);
  if (step) Object.assign(step, patch);
}

function onStep({ turn_id: turnId, at_unix_ms: at, step }) {
  const inv = current(turnId);
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
      // turn:delta carries the text; this step only marks which request is writing.
      const writing = inv.steps.find((s) => s.key === `model-${step.request}`);
      if (!writing || writing.writing) return;
      writing.writing = true;
      break;
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

/** The whole draft so far when `replace` is true; never verified, replaced by the finished turn. */
function onDelta({ turn_id: turnId, text, replace }) {
  const inv = current(turnId);
  if (!inv || inv.status === "done") return;
  inv.draft = replace === false && inv.draft ? inv.draft + text : text;
  notify("draft");
}

function onExtension(payload) {
  const inv = current(payload.turn_id);
  if (!inv) return;
  inv.extension = payload;
  notify("inv");
  notify("extension");
}

/** Errors that leave no usable analysis: the window offers Settings or a local analysis. */
const SETUP_ERRORS = ["missing_key", "missing_model", "invalid_model", "model_unavailable", "http", "network", "start"];

function finish(inv, turn, conversationId, failure) {
  const report = turn?.report ?? null;
  for (const step of inv.steps) if (step.state === "running") step.state = "cancelled";
  if (report?.mode === "local_rules") inv.steps.push({ key: "local", type: "local", state: "done" });
  // The last request of a completed turn is the one that wrote the answer.
  const lastModel = [...inv.steps].reverse().find((s) => s.type === "model");
  if (lastModel && report && !report.error && lastModel.state === "done") lastModel.final = true;
  inv.draft = null;
  inv.extension = null;
  inv.finishedAt = Date.now();
  if (failure && (!report || SETUP_ERRORS.includes(failure.code))) {
    inv.status = "failed";
    inv.failure = failure;
  } else {
    inv.status = "done";
    inv.report = report;
    const conversation = state.conversation;
    if (conversation && (conversation.id == null || conversation.id === (conversationId ?? inv.conversationId))) {
      conversation.id = conversationId ?? conversation.id;
      const saved = { id: turn.id, question: inv.question, report, at: inv.finishedAt, plans: [] };
      conversation.turns.push(saved);
      if (report.mode === "nebius" && turn.id) loadPlans(saved);
    }
  }
  notify("conversation");
  notify("inv");
  refreshConversations();
}

/** Proposals of a finished turn; only the last turn's can still run. */
async function loadPlans(turn) {
  try {
    turn.plans = await call("action_plans", { turnId: turn.id });
  } catch {
    turn.plans = [];
  }
  if (turn.plans.length) notify("conversation");
}

/** Runs one reviewed proposal after the person's click; resolves with its measured outcome. */
export async function runPlan(turn, plan) {
  plan.decision = "running";
  notify("conversation");
  try {
    plan.outcome = await call("action_run", { turnId: turn.id, planId: plan.plan_id });
    plan.decision = plan.outcome.overall;
  } catch (error) {
    plan.decision = "refused";
    plan.error = error.message;
  }
  notify("conversation");
  return plan;
}

export async function declinePlan(turn, plan) {
  try {
    await call("action_decline", { turnId: turn.id, planId: plan.plan_id });
  } catch {
    // Already decided or no longer current: the card still shows it as set aside.
  }
  plan.decision = "declined";
  notify("conversation");
}

function onDone({ turn_id: turnId, conversation_id: conversationId, turn }) {
  const inv = current(turnId);
  if (inv) finish(inv, turn, conversationId, null);
}

function onFailed({ turn_id: turnId, conversation_id: conversationId, code, message, turn }) {
  const inv = current(turnId);
  if (inv) finish(inv, turn, conversationId, { code, message });
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
  on("turn:step", onStep);
  on("turn:delta", onDelta);
  on("turn:extension", onExtension);
  on("turn:done", onDone);
  on("turn:failed", onFailed);
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
