// The conversation: a welcome screen with the composer, then the thread of questions and written
// answers. Citations, the PC status and storage links open the context panel instead of
// crowding the answer.
import { h, fill, announce } from "../dom.js";
import { icon } from "../icons.js";
import { t, classificationLabel } from "../i18n.js";
import { markdown, labelCitations } from "../markdown.js";
import { mark } from "../brand.js";
import { composer, ask, askLocally, askWithAi, cloudReady, effectiveMode } from "../ask.js";
import { state, subscribe, notify, openPanel, runPlan, declinePlan } from "../store.js";
import { showSource } from "../panel.js";
import { health } from "../live.js";
import { numbering, citationTitle, toolLabel } from "../citations.js";
import { button, copyText, dialog, toast } from "../ui.js";
import { call } from "../api.js";
import { bytes, date, duration, labelled, usd } from "../format.js";
import { modelName } from "../models.js";
import { navigate } from "../router.js";
import { sourcesLabel, webDescriber, webOrigin, webStepQuery } from "../web.js";

// ---------- Small pieces ----------

/** A question saved without an answer, with a way to ask it again in the same conversation. */
function interruptedView(question) {
  return h(
    "section",
    { class: "turn" },
    bubble(question),
    h(
      "div",
      { class: "assistant" },
      h("div", { class: "callout warn" }, icon("alert"), h("div", null, h("strong", null, t("chat.interrupted.title")), h("span", null, t("chat.interrupted.text")))),
      h("div", { class: "row" }, button(t("chat.interrupted.retry"), { iconName: "refresh", onClick: () => ask(question, { followUp: true }) })),
    ),
  );
}

function bubble(text) {
  return h("div", { class: "bubble-row" }, h("div", { class: "user-bubble" }, text));
}

/** Selection metadata comes from the local action, never from the generated question or finding. */
function requestView(question, selection) {
  if (!selection || !["explain", "investigate"].includes(selection.intent)) return bubble(question);
  const resource = selection.reference?.target?.kind === "resource" ? selection.reference.target.resource : null;
  const resources = { memory: "metric.memory", cpu: "metric.cpu", battery: "battery.label" };
  const resourceLabel = Object.hasOwn(resources, resource) ? resources[resource] : "select.label.fallback";
  const label = typeof selection.label === "string" && selection.label.trim()
    ? selection.label.trim()
    : t(resourceLabel);
  return h(
    "div",
    { class: "selection-request", dataset: { intent: selection.intent } },
    icon(selection.intent === "explain" ? "bulb" : "search"),
    h("span", { class: "selection-request-label" }, t(`select.label.${selection.intent}`, { what: label })),
  );
}

function meta(report, inv) {
  const parts = [];
  if (["local_rules", "local_evidence"].includes(report?.mode)) parts.push(t("chat.local"));
  else {
    const model = report?.usage?.cost?.price?.model_id || state.info?.cloud?.model_id;
    parts.push(model ? modelName(model) : t("mode.cloud"));
  }
  if (inv?.finishedAt && inv.startedAt) parts.push(duration(inv.finishedAt - inv.startedAt));
  if (report?.usage?.cost) parts.push(usd(report.usage.cost.usd));
  return parts.join(" · ");
}

function head(text) {
  return h("div", { class: "assistant-head" }, mark(20), h("span", null, text));
}

/**
 * What a step did, in plain words. A finished model request is described by what followed it in
 * the recorded steps: the checks it asked for, the answer it wrote, or a correction requested
 * from it. Nothing is claimed about what the model "thought".
 */
function stepLabel(step, steps = [], index = -1) {
  if (step.type === "snapshot") return t("step.snapshot");
  if (step.type === "switch") return t("step.switch", { model: modelName(step.to) });
  if (step.type === "model") {
    if (step.state === "failed") {
      const key = `step.model.failed.${step.outcome}`;
      const label = t(key);
      return label === key ? t("step.model.failed") : label;
    }
    if (step.state === "running") return t(step.writing ? "step.model.writing" : "step.model.running");
    if (step.final) return t("step.model.final");
    let tools = 0;
    let next = null;
    for (let i = index + 1; i < steps.length; i++) {
      if (steps[i].type === "model") {
        next = steps[i];
        break;
      }
      if (steps[i].type === "tool") tools += 1;
      // A switch may follow checks, before the first answer has been written.
      if (steps[i].type === "switch") {
        return tools
          ? t(tools === 1 ? "step.model.asked.one" : "step.model.asked.many", { count: tools })
          : t("step.model.handed", { model: modelName(steps[i].to) });
      }
    }
    if (tools) return t(tools === 1 ? "step.model.asked.one" : "step.model.asked.many", { count: tools });
    // A usable reply without a tool call that was followed by another request: the controller
    // asked for a corrected answer (missing citation, unchecked figure, missing tool call).
    return next ? t("step.model.redo") : t("step.model");
  }
  if (step.type === "local") return t("step.local");
  return toolLabel(step.tool);
}

function stepRow(step, index = -1, steps = []) {
  const mark = {
    running: h("span", { class: "spinner", "aria-hidden": "true" }),
    done: h("span", { class: "step-ok" }, icon("check")),
    partial: h("span", { class: "step-warn" }, icon("alert")),
    failed: h("span", { class: "step-bad" }, icon("x")),
    cancelled: h("span", { class: "step-off" }, icon("x")),
  }[step.state] || h("span", { class: "step-off" });
  const detail = step.state === "running" ? t("chat.inProgress") : step.status && step.status !== "ok" ? statusText(step.status) : "";
  return h(
    "div",
    { class: ["step", step.state, step.query ? "web-step" : ""] },
    mark,
    h("span", { class: "step-label" }, stepLabel(step, steps, index)),
    h("span", { class: "step-detail" }, detail),
    h("span", { class: "step-time" }, step.duration != null ? duration(step.duration) : ""),
    webStepQuery(step.query),
  );
}

function statusText(status) {
  const key = `step.status.${status}`;
  const label = t(key);
  return label === key ? t("step.status.error") : label;
}

/**
 * The tool steps a saved turn recorded (`tool_events`), for an answer whose live run is no longer
 * on screen: what was checked, how it went, and the exact query of each web lookup.
 */
function recordedSteps(report) {
  return (report?.tool_events || []).map((event, i) => ({
    key: `recorded-${i}`,
    type: "tool",
    tool: event.tool,
    state: event.status === "ok" ? "done" : event.status === "partial" ? "partial" : "failed",
    status: event.status,
    duration: event.duration_ms,
    query: event.query ?? null,
  }));
}

function stepList(steps) {
  return h("div", { class: "steps" }, steps.map((step, index) => stepRow(step, index, steps)));
}

// ---------- Answers ----------

/** Text placed in Markdown brackets stays text: a title cannot open or close a link. */
function mdText(text) {
  return String(text).replace(/([\\[\]*_`])/g, "\\$1");
}

/** One source in the copied list: its title, when it was measured or found, then its facts. */
function sourceLines(n, c) {
  const url = c.web?.url;
  // Parentheses and spaces would end the link early: they are percent-encoded.
  const target = url && /^https:\/\//i.test(url) ? url.replace(/[()\s]/g, (ch) => `%${ch.charCodeAt(0).toString(16).toUpperCase().padStart(2, "0")}`) : null;
  const title = target ? `[${mdText(citationTitle(c))}](${target})` : mdText(citationTitle(c));
  const when = c.web ? webOrigin(c).trim() : c.measured_at_unix_ms ? `· ${t("copy.measured", { at: date(c.measured_at_unix_ms, true) })}` : "";
  const earlier = c.earlier ? ` · ${t("copy.earlier")}` : "";
  return [`${n}. ${title}${when ? ` ${when}` : ""}${earlier}`, ...c.facts.map((fact) => `   - ${fact}`)];
}

/** The answer as Markdown: the question, the answer with its numbered citations, then its sources. */
function answerText(question, report) {
  const a = report.answer;
  const numbers = numbering(a);
  const lines = [labelled(`**${t("copy.question")}**`, question), "", labelCitations(a.text, (reference) => numbers.get(reference) ?? reference)];
  if (a.unverified.length) {
    lines.push("", `**${t("answer.unverified")}**`, ...a.unverified.map((i) => `- ${t("copy.quoted", { text: i.excerpt })} — ${t(`issue.${i.kind}`)}`));
  }
  if (a.citations.length) {
    lines.push("", "---", "", `**${t("copy.sources")}**`, "");
    a.citations.forEach((c) => lines.push(...sourceLines(numbers.get(c.reference), c)));
  }
  if (report.usage?.cost) lines.push("", `_${labelled(t("usage.cost"), usd(report.usage.cost.usd))}_`);
  return lines.join("\n");
}

function findingText(question, report) {
  const f = report.selected;
  const lines = [labelled(`**${t("copy.question")}**`, question), "", `**${classificationLabel(f)}**`, "", f.summary];
  if (f.facts.length) lines.push("", ...f.facts.map((x) => `- ${x}`));
  if (f.next_step) lines.push("", labelled(`**${t("answer.next")}**`, f.next_step));
  return lines.join("\n");
}

function errorCallout(report) {
  if (!report.error) return null;
  const code = report.error.code;
  const text = t(`error.${code}`);
  return h(
    "div",
    { class: ["callout", code === "cancelled" ? "info" : "warn"] },
    icon(code === "cancelled" ? "info" : "alert"),
    h("div", null, h("strong", null, text === `error.${code}` ? report.error.message : text), h("span", null, t("error.kept"))),
  );
}

/** The written answer: text with numbered citation chips that open the panel. */
function writtenAnswer(report, index) {
  const a = report.answer;
  const numbers = numbering(a);
  return h("div", { class: "answer-text" }, markdown(a.text, (reference) => showSource(index, reference), (reference) => numbers.get(reference) ?? reference, webDescriber(a)));
}

/** A local analysis: the rules' best finding, kept short. */
function localFinding(question, report) {
  const f = report.selected;
  return h(
    "div",
    { class: "answer-text" },
    h("h4", { class: "md-heading" }, classificationLabel(f)),
    h("p", null, f.summary),
    f.facts.length ? h("ul", { class: "md-list" }, f.facts.map((x) => h("li", null, x))) : null,
    f.next_step ? h("p", null, labelled(t("answer.next"), f.next_step)) : null,
    cloudReady() ? h("div", { class: "row" }, h("button", { type: "button", class: "btn", onclick: () => askWithAi(question) }, icon("sparkle"), t("chat.askAi"))) : null,
  );
}

function verification(report) {
  const a = report.answer;
  if (!a) return null;
  if (!a.unverified.length) {
    return h("span", { class: "verified-line" }, icon("check"), t("chat.verified"), a.confidence === "low" ? h("span", { class: "faint" }, ` · ${t("confidence.low")}`) : null);
  }
  return h(
    "details",
    { class: "unverified" },
    h("summary", null, icon("alert"), t("chat.unverified", { count: a.unverified.length })),
    h("p", { class: "note" }, t("answer.unverified.note")),
    h("ul", null, a.unverified.map((i) => h("li", null, h("q", null, i.excerpt), ` — ${t(`issue.${i.kind}`)}`))),
  );
}

function iconButton(name, label, onClick) {
  return h("button", { type: "button", class: "icon-btn", title: label, "aria-label": label, onclick: onClick }, icon(name));
}

// ---------- Action proposals ----------
// The model only proposes. Each card says exactly what would happen; "Review and confirm" lists
// every target before anything runs, and the result is measured afterwards.

const KIND_ICONS = { recycle_selected_files: "x", run_cleanup_adapter: "x", open_settings_page: "sliders", open_in_explorer: "folder", request_normal_close: "stop", disable_user_startup: "pause", restore_user_startup: "play" };

function planTitle(plan) {
  const key = `action.kind.${plan.kind}`;
  const label = t(key);
  return label === key ? plan.kind : label;
}

function planSummary(plan) {
  const files = plan.targets.reduce((n, x) => n + (x.files || 0), 0);
  const size = plan.targets.reduce((n, x) => n + (x.bytes || 0), 0);
  const parts = [];
  if (files) parts.push(t(files === 1 ? "action.files.one" : "action.files", { count: files }));
  if (size) parts.push(bytes(size));
  parts.push(t(`action.recovery.${plan.recovery}`));
  return parts.join(" · ");
}

function outcomeText(plan) {
  if (plan.decision === "running") return t("action.running");
  if (plan.decision === "declined") return t("action.declined");
  if (plan.decision === "refused") return plan.error ? `${t("action.refused")} ${plan.error}` : t("action.refused");
  const gained = plan.outcome?.verification?.volume?.gained_bytes;
  const base = t(`action.outcome.${plan.decision}`);
  return gained ? `${base} ${t("action.gained", { size: bytes(gained) })}` : base;
}

async function review(turn, plan) {
  const go = await dialog(
    (close) => [
      h("h2", { class: "dialog-title" }, planTitle(plan)),
      h("p", null, t("action.review.lead")),
      h(
        "ul",
        { class: "plan-targets" },
        plan.targets.map((x) =>
          h("li", null, h("span", { class: "mono" }, x.label), x.files || x.bytes ? h("span", { class: "faint" }, ` · ${[x.files ? t("action.files", { count: x.files }) : null, x.bytes ? bytes(x.bytes) : null].filter(Boolean).join(", ")}`) : null),
        ),
      ),
      h("p", { class: "note" }, labelled(t("action.recovery"), t(`action.recovery.${plan.recovery}`))),
      h("p", { class: "note" }, t("action.review.checks")),
      h(
        "div",
        { class: "dialog-actions" },
        button(t("common.cancel"), { onClick: () => close(false) }),
        button(t("action.run"), { variant: "primary", onClick: () => close(true) }),
      ),
    ],
    { label: planTitle(plan), dismissValue: false },
  );
  if (go) runPlan(turn, plan);
}

/**
 * Puts back a start-up entry that an approved action turned off. The Rust side reads that
 * action's local journal and prepares a fresh restoration plan (startup_restore_prepare), which
 * goes through the same review and click as any proposal; nothing runs before "Run now".
 */
async function restoreStartup(plan) {
  plan.restore = { preparing: true, plan: null, turnId: null };
  notify("conversation");
  try {
    const restoration = await call("startup_restore_prepare", { sourcePlanId: plan.plan_id, itemIndex: 0 });
    plan.restore = { preparing: false, plan: restoration.plan, turnId: restoration.turn_id };
  } catch (error) {
    plan.restore = null;
    notify("conversation");
    toast(error.message, "bad");
    return;
  }
  notify("conversation");
  // The restoration is its own local action group, not a conversation turn.
  await review({ id: plan.restore.turnId }, plan.restore.plan);
}

function restoreText(restore) {
  const decision = restore.plan?.decision;
  if (restore.preparing) return t("action.restore.preparing");
  if (decision === "running") return t("action.restore.running");
  if (decision === "completed") return t("action.restore.done");
  if (decision === "refused") return restore.plan.error ? `${t("action.restore.refused")} ${restore.plan.error}` : t("action.restore.refused");
  return decision ? `${t("action.restore.prefix")} ${t(`action.outcome.${decision}`)}` : null;
}

/** A Restore button for a start-up entry turned off here, once nothing else would be lost. */
function restoreControl(plan) {
  if (plan.kind !== "disable_user_startup" || plan.decision !== "completed") return null;
  const decision = plan.restore?.plan?.decision;
  if (plan.restore?.preparing || decision === "running" || decision === "completed") return null;
  // Preparing a restoration replaces the proposals still waiting under the latest answer.
  const latest = state.conversation?.turns?.at(-1);
  const waiting = (latest?.plans || []).some((other) => other !== plan && !other.decision);
  return h(
    "button",
    { type: "button", class: "btn btn-sm", disabled: waiting, title: t(waiting ? "action.restore.wait" : "action.restore.title"), onclick: () => restoreStartup(plan) },
    icon("refresh"),
    t("action.restore"),
  );
}

function actionCards(turn, current) {
  const plans = turn.plans || [];
  if (!plans.length) return null;
  return h(
    "div",
    { class: "action-cards" },
    h("span", { class: "caption" }, t("action.proposed")),
    h(
      "div",
      { class: "action-list" },
      plans.map((plan) => {
        const open = !plan.decision && current;
        return h(
          "div",
          { class: ["action-card", plan.decision ? `is-${plan.decision}` : ""] },
          h("span", { class: "action-icon" }, icon(plan.decision === "completed" ? "check" : KIND_ICONS[plan.kind] || "flag")),
          h(
            "div",
            { class: "action-text" },
            h("strong", null, planTitle(plan)),
            h("span", null, plan.decision ? outcomeText(plan) : planSummary(plan)),
            plan.restore && restoreText(plan.restore) ? h("span", null, restoreText(plan.restore)) : null,
          ),
          restoreControl(plan),
          open
            ? h(
                "div",
                { class: "row" },
                h("button", { type: "button", class: "link-btn", onclick: () => declinePlan(turn, plan) }, t("action.dismiss")),
                h("button", { type: "button", class: "btn btn-sm", onclick: () => review(turn, plan) }, t("action.review")),
              )
            : null,
        );
      }),
    ),
    h("span", { class: "note" }, icon("shield"), t("action.note")),
  );
}

function sentMessage(sent) {
  return h("details", { class: "more sent-message" }, h("summary", null, icon("chevron"), t("chat.sent")), h("pre", { class: "code" }, JSON.stringify(sent, null, 2)));
}

function stepsFold(shown, sent) {
  if (!shown.length) return sent ? sentMessage(sent) : null;
  return h(
    "details",
    { class: "steps-fold" },
    h("summary", { class: "summary" }, icon("chevron"), h("span", { class: "summary-main" }, t("chat.steps", { count: shown.length }))),
    stepList(shown),
    sent ? sentMessage(sent) : null,
  );
}

/** One answered turn. `inv` is the live run that produced it, when still known. */
function turnView(turn, index, { last, inv, cloud }) {
  const report = turn.report;
  const a = report?.answer;
  const cited = a ? a.citations.length : 0;
  // The run's steps fold out in place; the cited measurements open in the panel. A saved turn
  // whose run is no longer on screen shows the tool steps it recorded.
  const shown = inv?.steps?.length ? inv.steps : recordedSteps(report);
  // What left for Nebius, exactly, kept from the preview taken before the send (this session).
  const sent = turn.sent ?? inv?.sent ?? null;
  const steps = stepsFold(shown, sent);
  const sources = cited
    ? h(
        "button",
        { type: "button", class: "summary", onclick: () => openPanel("sources", { turn: index, reference: null }) },
        icon("eye"),
        h("span", { class: "summary-main" }, sourcesLabel(a)),
      )
    : null;
  const summary = steps || sources ? h("div", { class: "summaries" }, steps, sources) : null;
  const body = !report ? null : a ? writtenAnswer(report, index) : localFinding(turn.question, report);
  // The text written before a Stop stays readable, plainly unchecked (this session only).
  const partial = !a && turn.partial ? partialAnswer(turn.partial) : null;
  const followUps =
    last && a?.follow_ups?.length && !(state.inv && (state.inv.status === "running" || state.inv.status === "stopping"))
      ? h("div", { class: "follow-ups" }, a.follow_ups.map((q) => h("button", { type: "button", class: "chip", onclick: () => ask(q, { followUp: true }) }, q)))
      : null;
  const footer = report
    ? h(
        "div",
        { class: "answer-foot" },
        iconButton("copy", t("answer.copy"), () => copyText(a ? answerText(turn.question, report) : findingText(turn.question, report))),
        iconButton("refresh", t("answer.again"), () => ask(turn.question, { followUp: true })),
        verification(report),
      )
    : null;
  return h(
    "section",
    { class: "turn" },
    requestView(turn.question, turn.selection ?? inv?.selection),
    h("div", { class: "assistant" }, head(meta(report, inv)), summary, report ? errorCallout(report) : null, partial, body, actionCards(turn, last), followUps, footer),
  );
}

/** Chips of a text still being written (or stopped): numbered in the order they appear. */
function draftLabels() {
  const order = new Map();
  return (reference) => {
    if (!order.has(reference)) order.set(reference, order.size + 1);
    return order.get(reference);
  };
}

/** What the model had written when the turn was stopped: kept, never checked. */
function partialAnswer(text) {
  return h(
    "div",
    { class: "draft-host" },
    h("div", { class: "answer-text draft stopped" }, markdown(text, null, draftLabels())),
    h("p", { class: "draft-note" }, icon("alert"), t("chat.partial")),
  );
}

// ---------- The view ----------

const SUGGESTIONS = [
  { key: "suggest.slow", icon: "activity" },
  { key: "suggest.ram", icon: "memory" },
  { key: "suggest.space", icon: "drive" },
  { key: "suggest.heat", icon: "thermo" },
];

function wantsCloud() {
  return effectiveMode() !== "local";
}

export function mount(page) {
  page.classList.add("chat-page");
  const scroller = h("div", { class: "chat-scroll" });
  const column = h("div", { class: "chat-column" });
  const bottom = h("div", { class: "chat-bottom" });
  const footnote = h("p", { class: "footnote" });
  scroller.append(column);
  fill(page, scroller, bottom);

  let mode = null; // "home" or "thread"
  let box = null;
  let turnsKey = "";
  const turnsHost = h("div", { class: "turns" });
  const liveHost = h("div", { class: "turns" });
  let liveKey = "";
  let elapsedTimer = null;
  let lastAnnounced = "";
  let enteredId = null; // the run whose turn already slid in

  const nearBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 120;
  const toBottom = () => {
    scroller.scrollTop = scroller.scrollHeight;
  };

  const renderFootnote = () => {
    footnote.textContent = t(wantsCloud() ? "chat.footnote" : "foot.privacy");
  };

  let suggestionsHost = null;
  let suggestionsDrive = undefined;

  /** Suggested questions; when a drive is almost full, asking about it comes second. */
  const renderSuggestions = () => {
    const drive = health(state.live.dash).drive || null;
    if (!suggestionsHost || drive === suggestionsDrive) return;
    suggestionsDrive = drive;
    const items = SUGGESTIONS.map((s) => ({ text: t(s.key), icon: s.icon, warn: false }));
    if (drive) items.splice(2, 1, { text: t("suggest.spaceDrive", { drive }), icon: "drive", warn: true });
    if (drive) items.splice(1, 0, items.splice(2, 1)[0]);
    fill(
      suggestionsHost,
      items.map((s) =>
        h(
          "button",
          { type: "button", class: "suggestion", onclick: () => ask(s.text) },
          h("span", { class: ["suggestion-icon", s.warn ? "warn" : ""] }, icon(s.icon)),
          s.text,
        ),
      ),
    );
  };

  const renderHome = () => {
    mode = "home";
    turnsKey = "";
    liveKey = "";
    box = composer({ autofocus: true, big: true });
    suggestionsHost = h("div", { class: "suggestions" });
    suggestionsDrive = undefined;
    fill(
      column,
      h("div", { class: "welcome" }, mark(36), h("h1", { class: "welcome-title" }, t("chat.title")), h("p", { class: "welcome-sub" }, t("chat.sub")), box.el, suggestionsHost),
    );
    renderSuggestions();
    fill(bottom, footnote);
    renderFootnote();
  };

  const ensureThread = () => {
    if (mode === "thread") return;
    mode = "thread";
    suggestionsHost = null;
    turnsKey = "";
    liveKey = "";
    fill(column, h("div", { class: "thread" }, turnsHost, liveHost));
    box = composer({ followUp: true });
    fill(bottom, box.el, footnote);
    renderFootnote();
  };

  /** The run shown live: only in its own conversation, and only until its turn is saved. */
  const liveInv = () => {
    const inv = state.inv;
    const conversation = state.conversation;
    if (!inv) return null;
    if (conversation?.id != null && inv.conversationId != null && inv.conversationId !== conversation.id) return null;
    if (inv.report && conversation?.turns.some((turn) => turn.report === inv.report)) return null;
    return inv;
  };

  const renderTurns = () => {
    const turns = state.conversation?.turns || [];
    const inv = state.inv;
    // Action proposals load after the turn and change when decided: they are part of the key.
    const plans = turns.map((turn) => (turn.plans || []).map((p) => `${p.decision || "-"}${p.restore ? `:${p.restore.preparing ? "p" : p.restore.plan?.decision || "r"}` : ""}`).join("")).join("|");
    const selections = turns.map((turn) => turn.selection ?? (inv && inv.report === turn.report ? inv.selection : null));
    const key = `${state.conversation?.id}|${turns.length}|${inv?.report ? "r" : ""}|${state.inv?.status}|${plans}|${JSON.stringify(selections)}`;
    if (key === turnsKey) return;
    turnsKey = key;
    // A saved conversation whose only question never got an answer (the window closed or
    // reloaded while it ran): show the question and offer to ask again, never a blank page.
    if (!turns.length && state.conversation?.id != null && !liveInv()) {
      fill(turnsHost, interruptedView(state.conversation.title));
      return;
    }
    fill(
      turnsHost,
      turns.map((turn, index) => {
        const own = inv && inv.report === turn.report ? inv : null;
        const cloud = own ? own.cloud : turn.report?.mode === "nebius";
        return turnView(turn, index, { last: index === turns.length - 1, inv: own, cloud });
      }),
    );
  };

  let draftHost = null;

  /**
   * The answer while the model writes it: plain text with numbered chips, replaced by the checked
   * answer when the run ends. While a new request checks or rewrites it (a correction, a cut-off
   * reply asked again, or a stronger model taking over), the last text stays on screen, dimmed,
   * until the new one catches up: it never vanishes.
   */
  const renderDraft = () => {
    const inv = liveInv();
    if (!draftHost || !inv) return;
    if (!inv.draft) {
      fill(draftHost);
      return;
    }
    const note = inv.checking ? (inv.switchedTo ? t("chat.draft.switched", { model: modelName(inv.switchedTo) }) : t("chat.draft.checking")) : t("chat.draft");
    fill(
      draftHost,
      h("div", { class: ["answer-text draft", inv.checking ? "checking" : ""], "aria-live": "off" }, markdown(inv.draft, null, draftLabels())),
      h("p", { class: "draft-note" }, h("span", { class: "spinner" }), note),
    );
  };

  const renderLive = () => {
    const inv = liveInv();
    if (!inv) {
      window.clearInterval(elapsedTimer);
      liveKey = "";
      draftHost = null;
      fill(liveHost);
      return;
    }
    const running = inv.status === "running" || inv.status === "stopping";
    const key = `${inv.id}|${inv.status}|${inv.steps.map((s) => `${s.state}${s.writing ? "w" : ""}`).join(",")}|${JSON.stringify(inv.selection ?? null)}`;
    if (key !== liveKey) {
      liveKey = key;
      window.clearInterval(elapsedTimer);
      draftHost = h("div", { class: "draft-host" });
      const elapsed = h("span", { class: "faint tabular" });
      const tickElapsed = () => {
        elapsed.textContent = duration(Date.now() - inv.startedAt);
      };
      tickElapsed();
      let content;
      if (inv.status === "failed") {
        const code = inv.failure?.code;
        if (code === "start" && !inv.selection && !box?.text().trim()) box?.setText(inv.question);
        const text = t(`error.${code}`);
        const setup = ["missing_key", "missing_model", "invalid_model", "model_unavailable", "http"].includes(code);
        content = [
          h("div", { class: "callout bad" }, icon("alert"), h("div", null, h("strong", null, t(code === "cancelled" ? "inv.stopped.title" : "inv.failed.title")), h("span", null, text === `error.${code}` ? inv.failure.message : text))),
          inv.partial ? partialAnswer(inv.partial) : null,
          // A failed request still has inspectable steps and the message used for that request.
          // A rejected start or a cancellation before any model request sent nothing.
          stepsFold(inv.steps, inv.cloud && inv.steps.some((step) => step.type === "model") ? inv.sent : null),
          h(
            "div",
            { class: "row" },
            setup ? h("button", { type: "button", class: "btn", onclick: () => navigate("settings") }, icon("sliders"), t("error.openSettings")) : null,
            h("button", { type: "button", class: "btn", onclick: () => askLocally(inv.question) }, icon("pc"), t("error.useLocal")),
          ),
        ];
      } else {
        content = [
          h(
            "div",
            { class: "progress" },
            h("div", { class: "progress-head" }, h("strong", null, t(inv.status === "stopping" ? "inv.stopping" : "chat.progress")), h("span", { class: "faint" }, "·"), elapsed),
            inv.steps.length
              ? stepList(inv.steps)
              : h("div", { class: "step running" }, h("span", { class: "spinner" }), h("span", { class: "step-label" }, t(inv.resumed ? "chat.resumed" : "step.snapshot.running"))),
          ),
          draftHost,
        ];
      }
      const enter = enteredId !== inv.id;
      enteredId = inv.id;
      fill(
        liveHost,
        h(
          "section",
          { class: ["turn", enter ? "enter" : ""] },
          requestView(inv.question, inv.selection),
          h("div", { class: "assistant" }, head(running ? `${inv.cloud ? meta(null, null) : t("chat.local")} · ${t("chat.running")}` : meta(null, inv)), ...content),
        ),
      );
      renderDraft();
      if (running) elapsedTimer = window.setInterval(tickElapsed, 1000);
      const running_ = [...inv.steps].reverse().find((s) => s.state === "running");
      const say = running ? (running_ ? stepLabel(running_) : t("step.snapshot.running")) : "";
      if (say && say !== lastAnnounced) {
        lastAnnounced = say;
        announce(say);
      }
    }
  };

  const render = () => {
    const conversation = state.conversation;
    if (!conversation && !liveInv()) {
      if (mode !== "home") renderHome();
      box?.refresh();
      return;
    }
    const stick = mode !== "thread" || nearBottom();
    ensureThread();
    renderTurns();
    renderLive();
    box?.refresh();
    if (stick) toBottom();
  };

  render();
  window.addEventListener("pcagent:mode", renderFootnote);
  const unsubscribe = subscribe((topic) => {
    if (topic === "inv" || topic === "conversation") render();
    else if (topic === "draft") {
      const stick = nearBottom();
      renderDraft();
      if (stick) toBottom();
    }
    else if (topic === "info" || topic === "privacy") {
      box?.refresh();
      renderFootnote();
    } else if (topic === "live" && mode === "home") renderSuggestions();
  });
  return () => {
    window.clearInterval(elapsedTimer);
    window.removeEventListener("pcagent:mode", renderFootnote);
    unsubscribe();
  };
}
