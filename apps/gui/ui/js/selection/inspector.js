// A summary stays beside the selected measurements. Its run is owned by store.js, so changing
// views or opening a citation never starts another request or replaces its frozen evidence.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { citationFacts, findingFacts, findingSummary } from "../phrases.js";
import { state, subscribe, stopInvestigation } from "../store.js";
import { continueSummary, retrySummary } from "./actions.js";
import { markdown } from "../markdown.js";
import { numbering, citationTitle, isWeb } from "../chat/citations.js";
import { webSourceCard, webDescriber } from "../chat/web.js";
import { date, duration } from "../format.js";
import { costText, modelsLabel, routingNote, usedModelIds } from "../chat/usage.js";
import { keepFocus } from "../actionbar.js";
import { failureDetail, turnFailureText } from "../failure.js";

function sourceCard(citation, number, focused, inv) {
  if (isWeb(citation)) {
    return webSourceCard(citation, number, focused, {
      conversationId: inv?.conversationId,
      turnId: inv?.turnId,
      reference: citation.reference,
    });
  }
  return h(
    "article",
    { class: ["source-card", focused && "focused"], "data-reference": citation.reference },
    h("div", { class: "source-head" }, h("span", { class: "num" }, String(number)), h("strong", null, citationTitle(citation))),
    citation.measured_at_unix_ms ? h("p", { class: "source-time" }, date(citation.measured_at_unix_ms, true)) : null,
    citation.earlier ? h("p", { class: "note" }, t("sources.earlier")) : null,
    h("ul", { class: "source-facts" }, citationFacts(citation, state.conversation?.turns).map((fact) => h("li", null, fact))),
  );
}

/** Mount once while the summary tab is open; event updates preserve focus and disclosures. */
export function mountSelectionInspector(host) {
  let current = null;
  let focusedReference = null;
  const opened = new Map();
  let disposed = false;

  const render = () => {
    if (disposed) return;
    const summary = state.summary;
    if (!summary) {
      fill(host);
      return;
    }
    if (current !== summary) {
      current = summary;
      focusedReference = null;
      opened.clear();
    } else {
      for (const details of host.querySelectorAll("details[data-fold]")) opened.set(details.dataset.fold, details.open);
    }
    const inv = summary.inv;
    const status = inv?.status || summary.status;
    const report = inv?.report;
    const answer = report?.answer;
    const error = summary.error || inv?.failure || report?.error;
    const preparing = status === "preparing";
    const running = status === "running" || status === "stopping";
    const stopped = error?.code === "cancelled";
    const busy = state.preparing || ["running", "stopping"].includes(state.inv?.status);
    const numbers = numbering(answer);
    const partial = answer ? null : inv?.partial || inv?.draft;
    // A rejected start only prepared a message; do not describe it as already sent.
    const sent = inv?.sent && (inv.steps?.some((step) => step.type === "model") || report?.usage?.requests > 0) ? inv.sent : null;
    const draftNumbers = new Map();
    const draftLabel = (reference) => {
      if (!draftNumbers.has(reference)) draftNumbers.set(reference, draftNumbers.size + 1);
      return draftNumbers.get(reference);
    };
    const showCitation = (reference) => {
      focusedReference = reference;
      render();
      const sources = host.querySelector(".summary-sources");
      if (sources) sources.open = true;
      const source = host.querySelector(`[data-reference="${CSS.escape(reference)}"]`);
      source?.scrollIntoView({ block: "nearest" });
      source?.focus({ preventScroll: true });
    };
    // The models that worked on it ("Super → Ultra" after an escalation) and the total or a lower bound.
    const models = usedModelIds(report?.usage);
    const meta = [
      ["local_rules", "local_evidence"].includes(report?.mode) ? t("chat.local") : models.length ? modelsLabel(models) : null,
      inv?.finishedAt && inv?.startedAt ? duration(inv.finishedAt - inv.startedAt) : null,
      costText(report?.usage),
    ].filter(Boolean);
    const statusText = preparing ? t("summary.preparing") : status === "stopping" ? t("inv.stopping") : inv?.checking ? t("chat.draft.checking") : t("summary.running");
    const statusNode = preparing || running
      ? h("p", { class: "summary-progress", role: "status" }, h("span", { class: "spinner", "aria-hidden": "true" }), statusText)
      : error || status === "failed"
        ? h("div", { class: ["callout", stopped ? "info" : "warn"], role: "status" }, icon(stopped ? "stop" : "alert"), h("div", null, h("strong", null, t(stopped ? "inv.stopped.title" : "summary.failed")), h("span", { title: failureDetail(error) }, error ? turnFailureText(error) : "")))
        : null;
    const body = answer
      ? h("div", { class: "answer-text summary-answer" }, markdown(answer.text, showCitation, (reference) => numbers.get(reference) ?? reference, webDescriber(answer)))
      : partial
        ? h("div", null,
            h("div", { class: ["answer-text summary-answer draft", inv?.checking && "checking"] }, markdown(partial, null, draftLabel)),
            h("p", { class: "note" }, t(running ? "chat.draft" : "chat.partial")))
        : report?.selected
          ? h("div", { class: "answer-text summary-answer" }, h("p", null, findingSummary(report.selected)), h("ul", null, findingFacts(report.selected).map((fact) => h("li", null, fact))))
          : !preparing && !running && !error ? h("p", { class: "note" }, t("summary.noAnswer")) : null;

    keepFocus(host, () => {
      fill(host, h("section", { class: "selection-summary", "aria-label": t("panel.selection") },
        h("div", { class: "summary-heading" }, h("h2", { class: "insp-title", tabindex: "-1", "data-key": "summary:heading" }, summary.selection?.label || t("panel.selection")), h("p", { class: "note" }, t("summary.frozen"))),
        statusNode,
        body,
        answer ? (answer.unverified?.length
          ? h("details", { class: "unverified", "data-fold": "verification" }, h("summary", { "data-key": "summary:verification" }, icon("alert"), t("chat.unverified", { count: answer.unverified.length })), h("ul", null, answer.unverified.map((issue) => h("li", null, h("q", null, issue.excerpt), ` — ${t(`issue.${issue.kind}`)}`))))
          : h("p", { class: "verified-line" }, icon("check"), t("chat.verified"))) : null,
        answer?.citations?.length ? h("details", { class: "summary-sources", "data-fold": "sources" },
          h("summary", { "data-key": "summary:sources" }, icon("chevron"), t("summary.sources"), ` · ${answer.citations.length}`),
          h("div", { class: "summary-source-list" }, answer.citations.map((citation) => sourceCard(citation, numbers.get(citation.reference), focusedReference === citation.reference, inv)))) : null,
        meta.length ? h("p", { class: "summary-meta" }, meta.join(" · ")) : null,
        routingNote(report),
        sent ? h("details", { class: "more sent-message", "data-fold": "sent" }, h("summary", { "data-key": "summary:sent" }, icon("chevron"), t("chat.sent")), h("pre", { class: "code" }, JSON.stringify(sent, null, 2))) : null,
        state.privacy?.mode !== "local" && state.info?.cloud?.ready ? h("p", { class: "note" }, t("select.cost.note")) : null,
        h("div", { class: "summary-actions" },
          running ? h("button", { type: "button", class: "btn btn-sm", "data-key": "summary:stop", disabled: status === "stopping", onclick: () => stopInvestigation() }, icon("stop"), t("inv.stop")) : null,
          !preparing && !running && (error || status === "failed") ? h("button", { type: "button", class: "btn btn-sm", "data-key": "summary:retry", disabled: busy, onclick: () => retrySummary() }, icon("refresh"), t("summary.retry")) : null,
          h("button", { type: "button", class: "btn btn-sm", "data-key": "summary:continue", disabled: status !== "done" || busy || !inv?.conversationId, onclick: () => continueSummary() }, icon("search"), t("summary.continue")),
        ),
      ));
      for (const details of host.querySelectorAll("details[data-fold]")) details.open = Boolean(opened.get(details.dataset.fold));
      for (const cite of host.querySelectorAll("button.cite[data-ref]")) cite.dataset.key = `summary:cite:${cite.dataset.ref}`;
      for (const source of host.querySelectorAll("[data-reference]")) {
        source.tabIndex = -1;
        source.dataset.key = `summary:source:${source.dataset.reference}`;
      }
    });
  };

  render();
  requestAnimationFrame(() => { if (!disposed && host.isConnected) host.querySelector('[data-key="summary:heading"]')?.focus({ preventScroll: true }); });
  const unsubscribe = subscribe((topic) => {
    if (["summary", "inv", "draft", "conversation", "privacy", "info"].includes(topic)) render();
  });
  return () => { disposed = true; unsubscribe(); };
}
