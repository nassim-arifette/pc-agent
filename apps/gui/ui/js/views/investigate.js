// Investigate: the question, each real step as it happens, and the answer with its evidence,
// support level and limits.
import { h, fill, announce } from "../dom.js";
import { icon } from "../icons.js";
import { t, classificationLabel } from "../i18n.js";
import { markdown } from "../markdown.js";
import { mascotWithBubble } from "../mascot.js";
import { composer, suggestions, ask, cloudReady } from "../ask.js";
import { getPrefs } from "../prefs.js";
import { state, subscribe, stopInvestigation } from "../store.js";
import { badge, button, copyText, supportTone } from "../ui.js";
import { date, duration, number, price, usd } from "../format.js";
import { navigate } from "../router.js";

const TOOL_ICONS = {
  get_system_snapshot: "activity",
  get_processes: "list",
  get_process_details: "eye",
  get_process_tree: "list",
  get_metric_history: "clock",
  get_hardware_inventory: "cpu",
  list_storage_scopes: "folder",
  get_storage_overview: "drive",
  start_storage_scan: "drive",
  get_largest_items: "drive",
  get_cleanup_candidates: "drive",
  get_user_preferences: "sliders",
  prepare_action_plan: "flag",
};

function stepLabel(step) {
  if (step.type === "snapshot") return t(step.state === "running" ? "step.snapshot.running" : "step.snapshot");
  if (step.type === "model") {
    if (step.state === "failed") return t("step.model.failed");
    if (step.final) return t("step.model.final");
    return t(step.state === "running" ? "step.model.running" : "step.model");
  }
  if (step.type === "local") return t("step.local");
  const key = `tool.${step.tool}`;
  const label = t(key);
  return label === key ? step.tool : label;
}

function stepDetail(step) {
  if (step.type === "snapshot" && step.count != null) return t("step.snapshot.detail", { count: step.count });
  if (step.type === "model") {
    const tokens = step.promptTokens != null ? t("step.model.detail", { in: number(step.promptTokens), out: number(step.completionTokens ?? 0) }) : "";
    return step.state === "failed" && step.outcome ? [tokens, step.outcome].filter(Boolean).join(" · ") : tokens;
  }
  if (step.type === "local") return t("step.local.detail");
  if (step.type === "tool") {
    const status = step.status && step.status !== "ok" ? ` · ${t(`step.status.${step.status === "partial" ? "partial" : "error"}`)}` : "";
    return `${step.tool}${status}`;
  }
  return "";
}

function stepIcon(step) {
  const map = {
    running: ["running", "loader"],
    done: ["done", step.type === "model" ? "sparkle" : step.type === "tool" ? TOOL_ICONS[step.tool] || "check" : "check"],
    partial: ["", "alert"],
    failed: ["failed", "x"],
    cancelled: ["", "x"],
  };
  const [cls, name] = map[step.state] || ["", "check"];
  return h("span", { class: ["step-icon", cls, step.type === "model" && step.state === "done" ? "model" : ""] }, icon(name));
}

function timeline(inv) {
  if (!inv.steps.length) return h("p", { class: "muted" }, t("inv.steps.empty"));
  return h(
    "ol",
    { class: "timeline" },
    inv.steps.map((step) => {
      // Only a step drawn for the first time slides in; redraws stay still.
      const fresh = !step.shown;
      step.shown = true;
      return h(
        "li",
        { class: fresh ? "enter" : null },
        stepIcon(step),
        h("div", { class: "step-text" }, h("strong", null, stepLabel(step)), h("span", { class: step.type === "tool" ? "mono" : "" }, stepDetail(step))),
        h("span", { class: "step-time" }, step.duration != null ? duration(step.duration) : ""),
      );
    }),
  );
}

function mascotState(inv) {
  if (!inv) return "idle";
  if (inv.status === "failed") return "error";
  if (inv.status === "running" || inv.status === "stopping") {
    const running = [...inv.steps].reverse().find((s) => s.state === "running");
    return running?.type === "model" ? "thinking" : "working";
  }
  const report = inv.report;
  if (!report) return "idle";
  if (report.error?.code === "cancelled") return "idle";
  if (report.answer) {
    const category = report.answer.category;
    if (category === "no_problem_found") return "happy";
    if (category === "insufficient_evidence" || report.answer.confidence === "low") return "unsure";
    return "concerned";
  }
  const cls = report.selected.classification;
  if (cls === "no_contention_observed") return "happy";
  if (cls === "insufficient_evidence" || report.error) return "unsure";
  return "concerned";
}

function mascotText(inv) {
  if (!inv) return t("say.idle");
  if (inv.status === "failed") return t("say.error");
  if (inv.status === "stopping") return t("inv.stopping");
  if (inv.status === "running") {
    const running = [...inv.steps].reverse().find((s) => s.state === "running");
    if (!running || running.type === "snapshot") return t("say.snapshot");
    if (running.type === "model") return t("say.model");
    return t("say.tool", { step: stepLabel(running) });
  }
  const state = mascotState(inv);
  if (inv.report?.error?.code === "cancelled") return t("say.stopped");
  return t({ happy: "say.happy", unsure: "say.unsure", concerned: "say.found" }[state] || "say.found");
}

function list(title, iconName, items, cls = "") {
  if (!items?.length) return null;
  return h("div", { class: "answer-block" }, h("h4", null, icon(iconName), title), h("ul", { class: ["bullets", cls] }, items.map((x) => h("li", null, x))));
}

function supportBadge(support) {
  const iconName = { supported: "check", plausible: "help", insufficient: "info" }[support];
  const el = badge(t(`support.${support}`), supportTone(support), iconName);
  el.title = t(`support.hint.${support}`);
  return el;
}

function toolLabel(name) {
  const key = `tool.${name}`;
  const label = t(key);
  return label === key ? name : label;
}

function confidenceBadge(confidence) {
  const tone = { high: "good", medium: "", low: "warn" }[confidence] ?? "";
  const el = badge(t(`confidence.${confidence}`), tone, { high: "check", medium: "help", low: "info" }[confidence]);
  el.title = t("confidence.hint");
  return el;
}

/** Opens the sources and points at the cited one. */
function showCitation(reference) {
  const target = document.getElementById(`cite-${reference}`);
  if (!target) return;
  const details = target.closest("details");
  if (details) details.open = true;
  const still = document.documentElement.classList.contains("reduce-motion");
  target.scrollIntoView({ behavior: still ? "auto" : "smooth", block: "center" });
  target.classList.remove("flash");
  void target.offsetWidth;
  target.classList.add("flash");
}

function answerText(inv) {
  const a = inv.report.answer;
  const lines = [`PC Agent — ${inv.question}`, "", a.text];
  if (a.unverified.length) lines.push("", `${t("answer.unverified")}:`, ...a.unverified.map((i) => `- "${i.excerpt}": ${t(`issue.${i.kind}`)}`));
  for (const c of a.citations) lines.push("", `[${c.reference}] ${c.kind === "observation" ? toolLabel(c.title) : c.title}`, ...c.facts.map((x) => `- ${x}`));
  if (inv.report.usage?.cost) lines.push("", `${t("usage.cost")}: ${usd(inv.report.usage.cost.usd)}`);
  return lines.join("\n");
}

/** The model's written answer: the text first, then what could not be verified, follow-ups and sources. */
function writtenAnswerCard(inv, error, limits) {
  const r = inv.report;
  const a = r.answer;
  const unverified = a.unverified.length
    ? h(
        "div",
        { class: "callout warn answer-block" },
        icon("alert"),
        h("div", null, h("strong", null, t("answer.unverified")), h("span", { class: "muted" }, t("answer.unverified.note")), h("ul", { class: "bullets" }, a.unverified.map((i) => h("li", null, h("q", null, i.excerpt), ` — ${t(`issue.${i.kind}`)}`)))),
      )
    : null;
  const followUps = a.follow_ups.length
    ? h("div", { class: "answer-block" }, h("h4", null, icon("sparkle"), t("answer.followups")), h("div", { class: "row wrap" }, a.follow_ups.map((q) => h("button", { class: "chip", type: "button", onClick: () => ask(q, { cloud: inv.cloud }) }, q))))
    : null;
  const sources = a.citations.length
    ? h(
        "details",
        { class: "more sources" },
        h("summary", null, icon("chevron"), t("answer.sources", { count: a.citations.length })),
        h(
          "div",
          { class: "alt-list" },
          a.citations.map((c) =>
            h(
              "div",
              { class: "alt source", id: `cite-${c.reference}` },
              h(
                "div",
                null,
                h("strong", null, h("span", { class: "cite static" }, c.reference), c.kind === "observation" ? toolLabel(c.title) : c.title),
                c.earlier ? h("span", { class: "faint" }, ` · ${t("answer.earlier")}`) : null,
                h("ul", { class: "bullets" }, c.facts.map((x) => h("li", null, x))),
              ),
            ),
          ),
        ),
      )
    : null;
  const actions = h(
    "div",
    { class: "row wrap", style: { "margin-top": "20px" } },
    button(t("answer.copy"), { iconName: "copy", size: "sm", onClick: () => copyText(answerText(inv)) }),
    button(t("answer.again"), { iconName: "refresh", size: "sm", onClick: () => ask(inv.question, { cloud: inv.cloud }) }),
  );
  return h(
    "article",
    { class: "card answer" },
    error,
    h(
      "div",
      { class: "answer-head" },
      h("div", null, h("div", { class: "answer-kicker" }, t(`answer.kicker.${r.mode}`)), h("h2", { class: "answer-title" }, t(`category.${a.category}`))),
      confidenceBadge(a.confidence),
    ),
    h("div", { class: "answer-text" }, markdown(a.text, showCitation)),
    unverified,
    followUps,
    sources,
    limits.length
      ? h("details", { class: "more" }, h("summary", null, icon("chevron"), t("answer.limits")), h("div", null, h("ul", { class: "bullets" }, limits.map((x) => h("li", null, x)))))
      : null,
    actions,
  );
}

function reportText(inv) {
  if (inv.report.answer) return answerText(inv);
  const r = inv.report;
  const f = r.selected;
  const lines = [
    `PC Agent — ${inv.question}`,
    `${classificationLabel(f)} (${t(`support.${f.support}`)})`,
    f.summary,
    "",
    ...f.facts.map((x) => `- ${x}`),
    f.hypothesis ? `\n${t("answer.hypothesis")}: ${f.hypothesis}` : "",
    `\n${t("answer.next")}: ${f.next_step}`,
  ];
  if (r.usage?.cost) lines.push(`\n${t("usage.cost")}: ${usd(r.usage.cost.usd)}`);
  return lines.filter((x) => x !== "").join("\n");
}

function answerCard(inv) {
  const r = inv.report;
  const f = r.selected;
  const limits = [...new Set([...(f.limitations || []), ...(r.limitations || [])])];
  const error = r.error
    ? h(
        "div",
        { class: ["callout", r.error.code === "cancelled" ? "info" : "warn"], style: { "margin-bottom": "16px" } },
        icon(r.error.code === "cancelled" ? "info" : "alert"),
        h("div", null, h("strong", null, t(`error.${r.error.code}`) === `error.${r.error.code}` ? r.error.message : t(`error.${r.error.code}`)), h("span", { class: "muted" }, t("error.kept"))),
      )
    : null;
  if (r.answer) return writtenAnswerCard(inv, error, r.limitations || []);
  const actions = h(
    "div",
    { class: "row wrap", style: { "margin-top": "20px" } },
    button(t("answer.copy"), { iconName: "copy", size: "sm", onClick: () => copyText(reportText(inv)) }),
    button(t("answer.again"), { iconName: "refresh", size: "sm", onClick: () => ask(inv.question, { cloud: inv.cloud }) }),
    !inv.cloud && cloudReady() ? button(t("answer.withAi"), { iconName: "sparkle", size: "sm", variant: "primary", onClick: () => ask(inv.question, { cloud: true }) }) : null,
  );
  return h(
    "article",
    { class: "card answer" },
    error,
    h(
      "div",
      { class: "answer-head" },
      h("div", null, h("div", { class: "answer-kicker" }, t(`answer.kicker.${r.mode}`)), h("h2", { class: "answer-title" }, classificationLabel(f))),
      supportBadge(f.support),
    ),
    h("p", { class: "answer-summary" }, f.summary),
    list(t("answer.facts"), "activity", f.facts, "facts"),
    f.hypothesis
      ? h("div", { class: "answer-block" }, h("h4", null, icon("bulb"), t("answer.hypothesis")), h("p", null, f.hypothesis), h("p", { class: "note" }, t("answer.hypothesis.note")))
      : null,
    list(t("answer.counter"), "scale", f.counter_evidence),
    list(t("answer.missing"), "puzzle", f.missing),
    f.next_step ? h("div", { class: "answer-block" }, h("div", { class: "callout" }, icon("flag"), h("div", null, h("strong", null, t("answer.next")), h("span", null, f.next_step)))) : null,
    r.alternatives.length
      ? h(
          "div",
          { class: "answer-block" },
          h("h4", null, icon("list"), t("answer.alternatives")),
          h("div", { class: "alt-list" }, r.alternatives.map((a) => h("div", { class: "alt" }, h("div", null, h("strong", null, classificationLabel(a)), h("p", null, a.summary)), supportBadge(a.support)))),
        )
      : null,
    r.observations.length
      ? h(
          "div",
          { class: "answer-block" },
          h("h4", null, icon("eye"), t("answer.evidence")),
          h(
            "div",
            { class: "alt-list" },
            r.observations.map((o) =>
              h("div", { class: "alt" }, h("div", null, h("strong", null, t(`tool.${o.tool}`) === `tool.${o.tool}` ? o.tool : t(`tool.${o.tool}`)), h("ul", { class: "bullets" }, o.facts.map((x) => h("li", null, x))))),
            ),
          ),
        )
      : null,
    limits.length
      ? h("details", { class: "more" }, h("summary", null, icon("chevron"), t("answer.limits")), h("div", null, h("ul", { class: "bullets" }, limits.map((x) => h("li", null, x)))))
      : null,
    actions,
  );
}

function coverageCard(report) {
  const tone = (s) => ({ complete: "good", partial: "warn", unavailable: "bad", not_collected: "" })[s] ?? "";
  const execTone = { completed: "good", budget_exhausted: "warn", failed: "bad" }[report.execution] ?? "";
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h3", { class: "card-title" }, icon("shield"), t("inv.coverage"))),
    h(
      "dl",
      { class: "kv" },
      h("dt", null, t("coverage.execution")),
      h("dd", null, badge(t(`exec.${report.execution}`), execTone)),
      h("dt", null, t("coverage.sensors")),
      h("dd", null, badge(t(`coverage.${report.coverage.status}`), tone(report.coverage.status))),
      h("dt", null, t(report.answer ? "coverage.confidence" : "coverage.support")),
      h("dd", null, report.answer ? confidenceBadge(report.answer.confidence) : supportBadge(report.support)),
    ),
    h(
      "details",
      { class: "more" },
      h("summary", null, icon("chevron"), t("storage.why")),
      h(
        "div",
        { class: "kv" },
        report.coverage.domains.flatMap((d) => [
          h("span", { class: "faint", title: d.missing.join(" · ") }, `${t(`domain.${d.domain}`)}${d.target ? ` (${d.target})` : ""}`),
          h("span", { style: { "text-align": "right" } }, badge(t(`coverage.${d.status}`), tone(d.status))),
        ]),
      ),
    ),
  );
}

function usageCard(report) {
  const u = report.usage;
  if (!u) return null;
  const cost = u.cost;
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h3", { class: "card-title" }, icon("bolt"), t("inv.usage"))),
    h(
      "dl",
      { class: "kv" },
      h("dt", null, t("usage.requests")),
      h("dd", null, number(u.requests)),
      h("dt", null, t("usage.tools")),
      h("dd", null, number(u.tool_calls)),
      h("dt", null, t("usage.tokensIn")),
      h("dd", null, u.prompt_tokens != null ? number(u.prompt_tokens) : t("common.unknown")),
      h("dt", null, t("usage.tokensOut")),
      h("dd", null, u.completion_tokens != null ? number(u.completion_tokens) : t("common.unknown"), u.reasoning_tokens ? h("span", { class: "faint" }, ` (${t("usage.reasoning")} ${number(u.reasoning_tokens)})`) : null),
      h("dt", null, t("usage.cost")),
      h("dd", { style: { "font-size": "15px" } }, cost ? usd(cost.usd) : h("span", { class: "unknown", title: t("usage.noCost.why") }, t("usage.noCost"))),
    ),
    cost
      ? h(
          "p",
          { class: "note", style: { "margin-top": "10px" } },
          t("usage.price", {
            input: price(cost.price.input_per_million),
            output: price(cost.price.output_per_million),
            date: date(cost.price.read_at_unix_ms, true),
          }),
        )
      : null,
  );
}

function failedCard(inv) {
  const code = inv.failure?.code;
  const text = t(`error.${code}`);
  const setup = ["missing_key", "missing_model", "invalid_model", "model_unavailable", "http"].includes(code);
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "callout bad" }, icon("alert"), h("div", null, h("strong", null, t("inv.failed.title")), h("span", null, text === `error.${code}` ? inv.failure.message : text))),
    h(
      "div",
      { class: "row wrap", style: { "margin-top": "14px" } },
      setup ? button(t("error.openSettings"), { iconName: "sliders", onClick: () => navigate("settings") }) : null,
      button(t("error.useLocal"), { iconName: "pc", variant: "primary", onClick: () => ask(inv.question, { cloud: false }) }),
    ),
  );
}

export function mount(page) {
  const iris = mascotWithBubble({ size: 84, side: true, label: t("mascot.label") });
  const head = h("div", { class: "page-head" });
  const liveHint = h("span", { class: "hint" });
  // Built once per run: re-inserting Iris on every step would restart her animations.
  const live = h("div", { class: "card live-line" }, iris.el, h("div", { class: "spacer" }), liveHint);
  const answerHost = h("div", { class: "stack" });
  const side = h("div", { class: "stack" });
  const stepsCard = h("section", { class: "card card-pad" });
  const followUp = h("div", { class: "section" });
  let shown = null; // what answerHost shows: "pending", a report object, or "failed"
  let headState = "";
  let lastAnnounced = "";

  const renderEmpty = () => {
    shown = null;
    headState = "";
    iris.set("idle");
    iris.say(t("say.idle"));
    const box = composer({ autofocus: true });
    fill(
      page,
      h("div", { class: "page-head" }, h("div", null, h("h1", { class: "page-title" }, t("inv.title")), h("p", { class: "page-sub" }, t("inv.sub")))),
      h("section", { class: "card card-pad empty" }, iris.el, h("h3", null, t("inv.empty.title")), h("p", null, t("inv.empty.text"))),
      h("div", { class: "section" }, box.el, suggestions((q) => ask(q, { cloud: getPrefs().mode === "cloud" && cloudReady() }))),
    );
  };

  const renderHead = (inv, running) => {
    const next = `${inv.id}|${inv.status}|${running}`;
    if (next === headState) return;
    headState = next;
    fill(
      head,
      h(
        "div",
        { style: { "min-width": "0" } },
        h("div", { class: "row", style: { "margin-bottom": "6px" } }, badge(t(inv.cloud ? "inv.mode.cloud" : "inv.mode.local"), inv.cloud ? "accent" : "", inv.cloud ? "sparkle" : "pc")),
        h("h1", { class: "question-title" }, inv.question),
      ),
      running
        ? button(inv.status === "stopping" ? t("inv.stopping") : t("inv.stop"), {
            variant: "danger",
            iconName: "stop",
            disabled: inv.status === "stopping",
            onClick: () => stopInvestigation(),
          })
        : button(t("inv.new"), {
            iconName: "back",
            onClick: () => {
              state.inv = null;
              renderEmpty();
            },
          }),
    );
  };

  const renderRun = (inv) => {
    const running = inv.status === "running" || inv.status === "stopping";
    renderHead(inv, running);
    iris.set(mascotState(inv));
    iris.say(mascotText(inv));
    liveHint.textContent = running ? t("inv.running") : "";
    fill(stepsCard, h("div", { class: "card-head" }, h("h3", { class: "card-title" }, icon("list"), t("inv.steps"))), timeline(inv));
    if (inv.status === "failed") {
      if (shown !== "failed") {
        shown = "failed";
        fill(answerHost, failedCard(inv));
        fill(side, stepsCard);
      }
    } else if (inv.report) {
      if (shown !== inv.report) {
        shown = inv.report;
        fill(answerHost, answerCard(inv));
        fill(side, stepsCard, coverageCard(inv.report), usageCard(inv.report));
        fill(followUp, composer().el);
      }
    } else if (shown !== "pending") {
      shown = "pending";
      fill(
        answerHost,
        h(
          "div",
          { class: "card answer" },
          h("div", { class: "skeleton", style: { height: "18px", width: "30%" } }),
          h("div", { class: "skeleton", style: { height: "26px", width: "60%", "margin-top": "12px" } }),
          h("div", { class: "skeleton", style: { height: "70px", "margin-top": "16px" } }),
        ),
      );
      fill(side, stepsCard);
      fill(followUp);
    }
    const text = mascotText(inv);
    if (text !== lastAnnounced) {
      lastAnnounced = text;
      announce(text);
    }
  };

  const render = () => {
    const inv = state.inv;
    if (!inv) {
      renderEmpty();
      return;
    }
    if (!page.contains(head)) {
      shown = null;
      headState = "";
      if (live.firstChild !== iris.el) live.prepend(iris.el);
      fill(page, head, h("div", { class: "split" }, h("div", { class: "stack" }, live, answerHost), side), followUp);
    }
    renderRun(inv);
  };

  render();
  return subscribe((topic) => {
    if (topic === "inv" || topic === "info") render();
  });
}
