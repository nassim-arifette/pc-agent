// Application state and the backend's events. Views subscribe to topics ("info", "inv",
// "conversation", "history", "scan", "extension", "live", "panel", "draft", "privacy") and redraw
// the parts they own.
import { call, commandError, on } from "./api.js";
import { t } from "./i18n.js";
import { startFailure } from "./failure.js";
import { rememberSelection, selectionForTurn, selectionForConversation, forgetSelections } from "./selection/meta.js";
import { startModelId } from "./chat/models.js";
import { recordedSwitch } from "./chat/usage.js";
import { IMAGE_MODEL_ID } from "./chat/image-draft.js";
import { markSpecialistRequest } from "./chat/specialist.js";

const listeners = new Set();

export const state = {
  info: null,
  /** The current investigation, or null. */
  inv: null,
  /** One selected-evidence summary, kept while its inspector is closed or another view is open. */
  summary: null,
  /** A send is waiting for saved settings or its exact preview; reserve it before any await. */
  preparing: false,
  privacyPending: 0,
  privacyRevision: 0,
  /** The open conversation: { id, title, turns: [{ question, report, at }] }, or null. */
  conversation: null,
  conversationRevision: 0,
  /** Saved conversations, most recently updated first. */
  conversations: [],
  scan: { status: "idle", root: null, scopeId: null, progress: null, view: null, error: null },
  recentScans: [],
  /** The scan the person detached from their questions with the composer's folder chip. */
  scanDetached: null,
  /**
   * The stored sharing choice: complete, anonymized or local, and the composer's toggles, on by
   * default (C5): `web_search` for web lookups through Tavily (C6), `file_names` for the names of
   * an attached scanned folder. No dialog waits for a confirmation.
   */
  privacy: { mode: "complete", confirmed: false, web_search: true, file_names: true },
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
    state.privacy = { mode: "local", confirmed: false, web_search: false, file_names: false };
  }
  notify("privacy");
  return state.privacy;
}

/**
 * Stores a sharing choice, keeping the toggles not named in `changes`. Changing the mode or a
 * toggle invalidates any preview on the Rust side. The whole settings object is always sent: Rust
 * turns a missing toggle on, so leaving one out could silently turn it back on.
 */
let privacyWrites = Promise.resolve();

export function setPrivacy(mode, changes = {}) {
  const patch = { ...changes, ...(mode == null ? {} : { mode }) };
  state.privacyPending += 1;
  state.privacyRevision += 1;
  notify("privacy");
  // Compose against the last saved choice, not the state before an earlier write completed.
  const write = privacyWrites.catch(() => {}).then(async () => {
    const current = state.privacy;
    const settings = { ...current, confirmed: Boolean(current.confirmed), web_search: current.web_search !== false, file_names: current.file_names !== false, ...patch };
    state.privacy = await call("privacy_set", { settings });
    return state.privacy;
  });
  privacyWrites = write;
  const finished = () => {
    state.privacyPending -= 1;
    notify("privacy");
  };
  write.then(finished, finished);
  return write;
}

/** A send must use the persisted choices; a failed last save refuses the send. */
export async function waitForPrivacy() {
  let pending;
  do {
    pending = privacyWrites;
    await pending;
  } while (pending !== privacyWrites);
}

export function beginTurnPreparation() {
  if (state.preparing || running()) return false;
  state.preparing = true;
  notify("inv");
  return true;
}

export function endTurnPreparation() {
  state.preparing = false;
  notify("inv");
}

/** Turns web lookups on or off, keeping the sharing choice as it is. */
export function setWebSearch(on) {
  return setPrivacy(null, { web_search: Boolean(on) });
}

/** Turns the names of an attached scanned folder on or off. */
export function setFileNames(on) {
  return setPrivacy(null, { file_names: Boolean(on) });
}

/** Web lookups can run: a usable cloud mode, the consent, and a saved Tavily key. */
export function webSearchActive() {
  const cloud = state.info?.cloud;
  return state.privacy.mode !== "local" && Boolean(cloud?.ready) && Boolean(state.privacy.web_search) && Boolean(cloud?.search_key_configured);
}

/** Saves the Tavily key (null or "" removes it); the page never reads a key back. */
export async function saveSearchKey(apiKey) {
  const cloud = await call("save_search_key", { apiKey: apiKey || null });
  state.info = { ...state.info, cloud };
  notify("info");
  return cloud;
}

/** Asks the Rust side to open a cited web page; it resolves the URL from the stored turn. */
export function openWebSource(conversationId, turnId, reference) {
  return call("open_web_source", { conversationId, turnId, reference });
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
// A processor rate needs two samples: a first read without one is followed up quickly.
const LIVE_RETRY_MS = 1200;
const HISTORY = 24;
let liveTimer = null;

async function liveTick() {
  window.clearTimeout(liveTimer);
  const live = state.live;
  let next = LIVE_MS;
  if (!live.paused && !document.hidden) {
    try {
      live.dash = await call("dashboard");
      live.error = null;
      live.history.push(live.dash);
      if (live.history.length > HISTORY) live.history.shift();
      const busy = live.dash?.cpu?.busy;
      if (busy && busy.value == null && busy.status === "partial") next = LIVE_RETRY_MS;
    } catch (error) {
      live.error = error;
    }
    notify("live");
  }
  liveTimer = window.setTimeout(liveTick, next);
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
    state.conversations = (await call("conversations_list")).map((conversation) => {
      const selection = selectionForConversation(conversation.id);
      return selection ? { ...conversation, title: t(`select.label.${selection.intent}`, { what: selection.label }) } : conversation;
    });
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
  const selection = selectionForConversation(saved.id);
  state.conversationRevision += 1;
  state.conversation = {
    id: saved.id,
    title: selection ? t(`select.label.${selection.intent}`, { what: selection.label }) : saved.title,
    // The turn ID lets the Rust side find a cited web page again (open_web_source).
    turns: saved.turns.map((turn) => ({ id: turn.id, question: turn.question, report: turn.report, at: turn.asked_at_unix_ms, image: turn.image ?? null, selection: selectionForTurn(turn.id) })),
  };
  // A running turn keeps going; it shows only in its own conversation.
  if (!running()) state.inv = null;
  notify("conversation");
  notify("inv");
  return true;
}

/**
 * Reattaches the turn the engine is still running after the window reloaded: its question, Stop
 * and an extension request waiting for an answer. Its earlier steps are not known again. Without
 * this, the turn ran unseen, could wait forever for an answer and blocked every new question.
 */
export async function resumeTurn() {
  if (running()) return;
  let turn = null;
  try {
    turn = await call("current_turn");
  } catch {
    return;
  }
  if (!turn) return;
  state.inv = {
    turnId: turn.turn_id,
    question: turn.question,
    cloud: state.privacy?.mode !== "local",
    conversationId: turn.conversation_id ?? null,
    status: turn.stopping ? "stopping" : "running",
    steps: [],
    draft: null,
    report: null,
    failure: null,
    extension: turn.extension ?? null,
    startedAt: turn.started_at_unix_ms || Date.now(),
    resumed: true,
    // Unknown after a reload: with Auto, Ultra may already have taken over (see chat.js).
    model: null,
    change: null,
    selection: selectionForTurn(turn.turn_id),
  };
  if (turn.conversation_id) await openConversation(turn.conversation_id);
  if (state.inv?.turnId === turn.turn_id) state.inv.owner = state.conversation?.id === turn.conversation_id ? state.conversation : null;
  notify("inv");
  if (turn.extension) notify("extension");
  return true;
}

/** Leaves the open conversation; the next question starts a new one. */
export function newConversation() {
  state.conversationRevision += 1;
  state.conversation = null;
  if (!running()) state.inv = null;
  notify("conversation");
  notify("inv");
}

export async function deleteConversation(id) {
  await call("conversation_delete", { id });
  forgetSelections(id);
  if (state.inv?.conversationId === id) state.inv.selection = null;
  if (state.summary?.inv?.conversationId === id || state.summary?.conversationId === id) { state.summary = null; notify("summary"); }
  if (state.conversation?.id === id) newConversation();
  await refreshConversations();
}

export async function clearConversations() {
  const removed = await call("conversations_clear");
  forgetSelections();
  if (state.inv) state.inv.selection = null;
  state.summary = null;
  notify("summary");
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
export async function sendMessage(question, { followUp = false, previewId = null, scopeIds = [], sent = null, imageId = null } = {}) {
  const conversationId = followUp ? (state.conversation?.id ?? null) : null;
  return startTurn(question, conversationId, followUp, sent, () => call("send_message", { conversationId, text: question, scopeIds, previewId, imageId }), null, null, imageId ? IMAGE_MODEL_ID : null);
}

/**
 * Starts an Explain or Investigate turn on a selected measurement, in the reviewed conversation, with the
 * question and reference of the reviewed preview ticket; any difference makes Rust refuse it.
 */
export async function sendSelection(question, { reference, intent, previewId, conversationId = null, label, sent = null, summary = null }) {
  if (conversationId !== (state.conversation?.id ?? null)) return false;
  const selection = { reference, intent, label };
  return startTurn(question, conversationId, Boolean(conversationId), sent, () => call("selection_send", { reference, intent, question, conversationId, previewId }), selection, summary);
}

/** `sent` is the exact first message of a cloud turn, from its preview: shown in its steps. */
async function startTurn(question, conversationId, followUp, sent, start, selection = null, summary = null, modelId = null) {
  if (running()) return false;
  const createdConversation = !followUp || !state.conversation;
  if (createdConversation) state.conversation = { id: null, title: selection ? t(`select.label.${selection.intent}`, { what: selection.label }) : question, turns: [] };
  const conversation = state.conversation;
  state.inv = {
    turnId: null,
    question,
    selection,
    createdConversation,
    // Same rule as effectiveMode() and send_message: a cloud mode without a usable key runs locally.
    cloud: state.privacy.mode !== "local" && Boolean(state.info?.cloud?.ready),
    conversationId,
    owner: conversation,
    status: "running",
    steps: [],
    draft: null,
    // While a new request checks or rewrites the draft: its text so far, and which model took over.
    nextDraft: null,
    checking: false,
    switchedTo: null,
    // The model the turn starts with under the routing policy, and the change once one happens.
    model: state.privacy.mode !== "local" && state.info?.cloud?.ready ? modelId ?? startModelId(state.info, state.routing) : null,
    change: null,
    partial: null,
    sent,
    report: null,
    failure: null,
    extension: null,
    startedAt: Date.now(),
  };
  const inv = state.inv;
  if (summary) { summary.inv = inv; summary.status = "running"; notify("summary"); }
  notify("conversation");
  notify("inv");
  try {
    const started = await start();
    if (inv.turnId == null) inv.turnId = started.turn_id;
    if (inv.turnId === started.turn_id) inv.conversationId = started.conversation_id;
    if (conversation.id == null) conversation.id = started.conversation_id;
    rememberSelection(inv.turnId, inv.conversationId, inv.selection, createdConversation);
    if (inv.status === "stopping") {
      try { await call("stop_turn", { turnId: inv.turnId }); }
      catch { /* The turn may have ended before its start acknowledgement arrived. */ }
    }
    notify("inv");
    refreshConversations();
    return true;
  } catch (error) {
    inv.status = "failed";
    inv.failure = startFailure(error);
    notify("inv");
    return false;
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
      inv.steps.push({ key: `model-${step.request}`, type: "model", request: step.request, state: "running", at });
      // A new request may write a new answer: the earlier draft was not accepted as it is. It
      // stays on screen, marked as being checked, until the new text catches up with it.
      if (inv.draft) inv.checking = true;
      inv.nextDraft = null;
      break;
    case "model_changed":
      // A stronger model takes the question over with the measurements already taken (chantier 14).
      inv.steps.push({ key: `switch-${inv.steps.length}`, type: "switch", state: "done", to: step.change?.to_model_id, at });
      // The run's header names the model in use: "Super → Ultra" from now on.
      inv.change = step.change ?? null;
      inv.switchedTo = inv.draft ? (step.change?.to_model_id ?? null) : null;
      if (inv.draft) inv.checking = true;
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
      // The new reply is complete: it replaces the kept draft even if it is shorter.
      if (inv.checking && inv.nextDraft) showNextDraft(inv);
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
        // A web lookup's exact query, as sent to Tavily (C6).
        query: step.query ?? null,
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

function showNextDraft(inv) {
  inv.draft = inv.nextDraft;
  inv.nextDraft = null;
  inv.checking = false;
  inv.switchedTo = null;
}

/**
 * The whole draft so far when `replace` is true; never verified, replaced by the finished turn.
 * While an earlier draft is kept on screen, the new text is collected aside and shown once it is
 * at least as long, so a correction never blanks the answer.
 */
function onDelta({ turn_id: turnId, text, replace }) {
  const inv = current(turnId);
  if (!inv || inv.status === "done") return;
  if (inv.checking) {
    inv.nextDraft = replace === false && inv.nextDraft ? inv.nextDraft + text : text;
    if (inv.nextDraft.length < inv.draft.length) return;
    showNextDraft(inv);
  } else {
    inv.draft = replace === false && inv.draft ? inv.draft + text : text;
  }
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
  inv.conversationId = conversationId ?? inv.conversationId;
  // An event can finish before the start acknowledgement. Bind the original owner even on
  // failure, so deletion and a later acknowledgement cannot attach this run to another draft.
  if (inv.owner && inv.owner.id == null) inv.owner.id = inv.conversationId;
  // Reopening the same thread replaces its view object while the turn is running.
  const conversation = state.conversation?.id === inv.conversationId ? state.conversation : inv.owner;
  rememberSelection(inv.turnId, inv.conversationId, inv.selection, inv.createdConversation);
  const report = turn?.report ?? null;
  for (const step of inv.steps) if (step.state === "running") step.state = "cancelled";
  if (["local_rules", "local_evidence"].includes(report?.mode)) inv.steps.push({ key: "local", type: "local", state: "done" });
  // A window reloaded after the model change never saw its step: put it back from the report,
  // before the first request the new model answered, or last when that request is not shown.
  const switched = recordedSwitch(report);
  if (switched && inv.steps.length && !inv.steps.some((s) => s.type === "switch")) {
    const next = inv.steps.findIndex((s) => s.type === "model" && s.request > switched.after);
    inv.steps.splice(next < 0 ? inv.steps.length : next, 0, switched);
  }
  // The last request of a completed turn is the one that wrote the answer.
  const lastModel = [...inv.steps].reverse().find((s) => s.type === "model");
  if (lastModel && report && !report.error && lastModel.state === "done") lastModel.final = true;
  // Without a written answer (stopped, or failed before submitting), the text already on screen
  // is kept as an unchecked partial answer instead of disappearing.
  const partial = report?.answer ? null : inv.nextDraft || inv.draft || null;
  inv.partial = partial;
  inv.draft = null;
  inv.nextDraft = null;
  inv.checking = false;
  inv.extension = null;
  inv.finishedAt = Date.now();
  if (failure && (!report || SETUP_ERRORS.includes(failure.code))) {
    inv.status = "failed";
    inv.failure = failure;
  } else {
    inv.status = "done";
    inv.report = report;
    if (conversation && (conversation.id == null || conversation.id === (conversationId ?? inv.conversationId))) {
      conversation.id = conversationId ?? conversation.id;
      const saved = { id: turn.id, question: inv.question, report, at: inv.finishedAt, plans: [], partial, sent: inv.sent ?? null, image: turn.image ?? null, selection: inv.selection ?? null };
      // A reopened thread may already contain the persisted turn before its event reaches us.
      const existing = turn.id ? conversation.turns.find((item) => item.id === turn.id) : null;
      if (existing) Object.assign(existing, saved, { plans: existing.plans ?? [] });
      else conversation.turns.push(saved);
      if (report.mode === "nebius" && turn.id) loadPlans(existing ?? saved);
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
    plan.error = error;
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
    state.scan.error = error;
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
  const offset = page.next_offset;
  const next = await call("scan_page", { scopeId: view.scope_id, kind, offset });
  // Each page is its own selection source: its items keep that source and where to read it again.
  page.items.push(...next.items.map((item) => ({ ...item, page_offset: offset, selection_source_id: next.selection_source_id ?? null })));
  page.next_offset = next.next_offset;
  notify("scan");
}

export function wireEvents() {
  on("specialist:step", ({ turn_id: turnId, event }) => {
    const inv = current(turnId);
    if (!inv || !event) return;
    markSpecialistRequest(inv.steps, event);
    const pending = [...inv.steps].reverse().find((s) => s.type === "specialist" && s.state === "running");
    if (pending) pending.state = "done";
    if (event.kind === "started") {
      inv.steps.push({ key: `specialist-${inv.steps.length}`, type: "specialist", state: "running" });
    } else {
      const step = { key: `specialist-${inv.steps.length}`, type: "specialist", state: "done", event };
      if (pending) Object.assign(pending, step);
      else inv.steps.push(step);
    }
    notify("inv");
  });
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
    state.recentScans.unshift({ root: view.root, at: view.scanned_at_unix_ms,
      size: view.scanned?.allocated_bytes, status: view.status });
    notify("scan");
  });
  on("scan:failed", ({ scope_id: scopeId, code, message }) => {
    const scan = currentScan(scopeId);
    if (!scan) return;
    scan.status = "failed";
    scan.error = commandError({ code, message });
    notify("scan");
  });
}
