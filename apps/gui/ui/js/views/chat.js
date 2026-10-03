// The conversation: a welcome screen with the composer, then the thread of questions and written
// answers. Citations, the PC status and storage links open the context panel instead of
// crowding the answer.
import { h, fill, announce } from "../dom.js";
import { icon } from "../icons.js";
import { t, classificationLabel } from "../i18n.js";
import { markdown } from "../markdown.js";
import { mark } from "../brand.js";
import { composer, ask, askLocally, askWithAi, cloudReady, effectiveMode } from "../ask.js";
import { state, subscribe, openPanel } from "../store.js";
import { showSource } from "../panel.js";
import { health } from "../live.js";
import { numbering, citationTitle, toolLabel } from "../citations.js";
import { copyText } from "../ui.js";
import { duration, labelled, usd } from "../format.js";
import { modelName } from "../models.js";
import { navigate } from "../router.js";

// ---------- Small pieces ----------

function bubble(text) {
  return h("div", { class: "bubble-row" }, h("div", { class: "user-bubble" }, text));
}

function meta(report, inv) {
  const parts = [];
  if (report?.mode === "local_rules") parts.push(t("chat.local"));
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

function stepLabel(step) {
  if (step.type === "snapshot") return t("step.snapshot");
  if (step.type === "model") {
    if (step.state === "failed") return t("step.model.failed");
    if (step.state === "running") return t(step.writing ? "step.model.writing" : "step.model.running");
    return step.final ? t("step.model.final") : t("step.model");
  }
  if (step.type === "local") return t("step.local");
  return toolLabel(step.tool);
}

function stepRow(step) {
  const mark = {
    running: h("span", { class: "spinner", "aria-hidden": "true" }),
    done: h("span", { class: "step-ok" }, icon("check")),
    partial: h("span", { class: "step-warn" }, icon("alert")),
    failed: h("span", { class: "step-bad" }, icon("x")),
    cancelled: h("span", { class: "step-off" }, icon("x")),
  }[step.state] || h("span", { class: "step-off" });
  const detail = step.state === "running" ? t("chat.inProgress") : step.status && step.status !== "ok" ? t(`step.status.${step.status === "partial" ? "partial" : "error"}`) : "";
  return h(
    "div",
    { class: ["step", step.state] },
    mark,
    h("span", { class: "step-label" }, stepLabel(step)),
    h("span", { class: "step-detail" }, detail),
    h("span", { class: "step-time" }, step.duration != null ? duration(step.duration) : ""),
  );
}

function stepList(steps) {
  return h("div", { class: "steps" }, steps.map(stepRow));
}

// ---------- Answers ----------

function answerText(question, report) {
  const a = report.answer;
  const numbers = numbering(a);
  const text = a.text.replace(/\[((?:[OF]\d{1,5})(?:\s*[,;]\s*[OF]\d{1,5})*)\]/g, (_, refs) => refs.split(/\s*[,;]\s*/).map((r) => `[${numbers.get(r) ?? r}]`).join(""));
  const lines = [`PC Agent — ${question}`, "", text];
  if (a.unverified.length) lines.push("", `${t("answer.unverified")}:`, ...a.unverified.map((i) => `- "${i.excerpt}": ${t(`issue.${i.kind}`)}`));
  for (const c of a.citations) lines.push("", `[${numbers.get(c.reference)}] ${citationTitle(c)}`, ...c.facts.map((x) => `- ${x}`));
  if (report.usage?.cost) lines.push("", `${t("usage.cost")}: ${usd(report.usage.cost.usd)}`);
  return lines.join("\n");
}

function findingText(question, report) {
  const f = report.selected;
  return [`PC Agent — ${question}`, classificationLabel(f), f.summary, "", ...f.facts.map((x) => `- ${x}`), "", `${t("answer.next")}: ${f.next_step}`].join("\n");
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
  return h("div", { class: "answer-text" }, markdown(a.text, (reference) => showSource(index, reference), (reference) => numbers.get(reference) ?? reference));
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

/** One answered turn. `inv` is the live run that produced it, when still known. */
function turnView(turn, index, { last, inv, cloud }) {
  const report = turn.report;
  const a = report?.answer;
  const cited = a ? a.citations.length : 0;
  // The run's steps fold out in place; the cited measurements open in the panel.
  const steps = inv?.steps?.length
    ? h("details", { class: "steps-fold" }, h("summary", { class: "summary" }, icon("chevron"), h("span", { class: "summary-main" }, t("chat.steps", { count: inv.steps.length }))), stepList(inv.steps))
    : null;
  const sources = cited
    ? h(
        "button",
        { type: "button", class: "summary", onclick: () => openPanel("sources", { turn: index, reference: null }) },
        icon("eye"),
        h("span", { class: "summary-main" }, t(cited === 1 ? "chat.cited.one" : "chat.cited", { count: cited })),
      )
    : null;
  const summary = steps || sources ? h("div", { class: "summaries" }, steps, sources) : null;
  const body = !report ? null : a ? writtenAnswer(report, index) : localFinding(turn.question, report);
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
    bubble(turn.question),
    h("div", { class: "assistant" }, head(meta(report, inv)), summary, report ? errorCallout(report) : null, body, followUps, footer),
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
    const key = `${state.conversation?.id}|${turns.length}|${inv?.report ? "r" : ""}|${state.inv?.status}`;
    if (key === turnsKey) return;
    turnsKey = key;
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

  /** The answer while the model writes it: plain text with numbered chips, replaced by the
   * checked answer when the run ends. */
  const renderDraft = () => {
    const inv = liveInv();
    if (!draftHost || !inv) return;
    if (!inv.draft) {
      fill(draftHost);
      return;
    }
    const order = new Map();
    const label = (reference) => {
      if (!order.has(reference)) order.set(reference, order.size + 1);
      return order.get(reference);
    };
    fill(
      draftHost,
      h("div", { class: "answer-text draft", "aria-live": "off" }, markdown(inv.draft, null, label)),
      h("p", { class: "draft-note" }, h("span", { class: "spinner" }), t("chat.draft")),
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
    const key = `${inv.id}|${inv.status}|${inv.steps.map((s) => `${s.state}${s.writing ? "w" : ""}`).join(",")}`;
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
        const text = t(`error.${code}`);
        const setup = ["missing_key", "missing_model", "invalid_model", "model_unavailable", "http"].includes(code);
        content = [
          h("div", { class: "callout bad" }, icon("alert"), h("div", null, h("strong", null, t("inv.failed.title")), h("span", null, text === `error.${code}` ? inv.failure.message : text))),
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
            inv.steps.length ? stepList(inv.steps) : h("div", { class: "step running" }, h("span", { class: "spinner" }), h("span", { class: "step-label" }, t("step.snapshot.running"))),
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
          bubble(inv.question),
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
