// The context panel on the right: closed by default, opened from a citation, the PC status or a
// storage link. The selected summary, conversation sources, live measurements and last folder
// analysis keep their own context.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t, language } from "./i18n.js";
import { citationFacts, limitationLines, targetLabel } from "./phrases.js";
import { state, subscribe, openPanel, closePanel, setLivePaused, watchActivity, pokeActivity } from "./store.js";
import { liveView, driveName } from "./live.js";
import { numbering, citationTitle, isWeb } from "./citations.js";
import { webSourceCard } from "./web.js";
import { bytes, clock, date, labelled, number, price, usd } from "./format.js";
import { modelShareText, turnModels } from "./usage.js";
import { navigate } from "./router.js";
import { mountSelectionInspector } from "./selection-inspector.js";
import { keepFocus } from "./actionbar.js";

const TABS = [
  { id: "sources", label: "panel.sources" },
  { id: "live", label: "panel.live" },
  { id: "storage", label: "panel.storage" },
];

/** Turns of the open conversation that carry a written answer with sources, newest first. */
function answeredTurns() {
  return (state.conversation?.turns || [])
    .map((turn, index) => ({ turn, index }))
    .filter(({ turn }) => turn.report?.answer?.citations?.length)
    .reverse();
}

function sourceCard(citation, n, focused) {
  return h(
    "article",
    { class: ["source-card", focused ? "focused" : ""], dataset: { reference: citation.reference } },
    h(
      "div",
      { class: "source-head" },
      h("span", { class: "num" }, String(n)),
      h("strong", null, citationTitle(citation)),
      h("span", { class: "spacer" }),
      citation.measured_at_unix_ms ? h("span", { class: "source-time" }, t("sources.measuredAt", { time: clock(citation.measured_at_unix_ms) })) : null,
    ),
    citation.earlier ? h("p", { class: "note" }, t("sources.earlier")) : null,
    h("ul", { class: "source-facts" }, citationFacts(citation, state.conversation?.turns).map((fact) => h("li", null, fact))),
  );
}

/**
 * What could not be measured, and what the run consumed: kept out of the answer, one click away.
 * Limitations the window cannot word in another language stay one more click away, in English.
 */
function runDetails(report) {
  const lines = limitationLines(report);
  const english = language() === "en" ? [] : lines.filter((line) => !line.worded);
  const limits = english.length ? lines.filter((line) => line.worded) : lines;
  const missing = (report.coverage?.domains || []).filter((d) => d.status !== "complete");
  const u = report.usage;
  const blocks = [];
  if (lines.length || missing.length) {
    blocks.push(
      h(
        "details",
        { class: "more" },
        h("summary", null, icon("chevron"), t("answer.limits")),
        h(
          "div",
          null,
          h(
            "ul",
            { class: "bullets note" },
            missing.map((d) => h("li", null, labelled(`${t(`domain.${d.domain}`)}${d.target ? ` (${targetLabel(d.target)})` : ""}`, t(`coverage.${d.status}`)))),
            limits.map((x) => h("li", null, x.text)),
          ),
          english.length
            ? h(
                "details",
                { class: "more" },
                h("summary", null, icon("chevron"), t("answer.limits.english", { count: number(english.length) })),
                h("ul", { class: "bullets note", lang: "en" }, english.map((x) => h("li", null, x.text))),
              )
            : null,
        ),
      ),
    );
  }
  if (u) {
    const priced = u.cost || u.cost_lower_bound;
    const priceText = (p) => t("usage.price", { input: price(p.input_per_million), output: price(p.output_per_million), date: date(p.read_at_unix_ms, true) });
    // After an escalation each model keeps its own requests, tokens, cost and price; the total
    // exists only when every request is priced, and is never one model's price on all tokens.
    const shares = turnModels(u);
    const costs = shares.length > 1
      ? [
          ...shares.map((share) => [modelShareText(share), share.price ? [" ", h("span", { class: "usage-price" }, priceText(share.price))] : null]),
          u.total_cost
            ? labelled(t("usage.costTotal"), usd(u.total_cost.usd))
            : u.priced_cost_lower_bound
              ? labelled(t("usage.costAtLeast"), usd(u.priced_cost_lower_bound.usd))
              : labelled(t("usage.cost"), t("usage.noCost")),
        ]
      : [
          u.cost ? labelled(t("usage.cost"), usd(u.cost.usd)) : u.cost_lower_bound ? labelled(t("usage.costAtLeast"), usd(u.cost_lower_bound.usd)) : t("usage.noCost"),
          priced ? priceText(priced.price) : null,
        ];
    const facts = [
      labelled(t("usage.requests"), number(u.requests)),
      labelled(t("usage.tools"), number(u.tool_calls)),
      u.prompt_tokens != null ? labelled(t("usage.tokensIn"), number(u.prompt_tokens)) : null,
      u.completion_tokens != null ? labelled(t("usage.tokensOut"), number(u.completion_tokens)) : null,
      // A reply cut after its tool calls, or a failed request, reports no usage: a lower bound.
      u.prompt_tokens == null && u.unknown_requests > 0 && u.known_prompt_tokens + u.known_completion_tokens > 0
        ? labelled(t("usage.tokensAtLeast"), t("usage.tokensKnown", { in: number(u.known_prompt_tokens), out: number(u.known_completion_tokens), count: number(u.unknown_requests) }))
        : null,
      ...costs,
    ].filter(Boolean);
    blocks.push(h("details", { class: "more" }, h("summary", null, icon("chevron"), t("usage.title")), h("div", null, h("ul", { class: "bullets note" }, facts.map((x) => h("li", null, x))))));
  }
  return blocks;
}

function sourcesView() {
  const turns = answeredTurns();
  if (!turns.length) return h("p", { class: "panel-empty" }, t("sources.empty"));
  const focus = state.panel.focus;
  return h(
    "div",
    { class: "sources" },
    turns.map(({ turn, index }) => {
      const answer = turn.report.answer;
      const numbers = numbering(answer);
      const unverified = answer.unverified.length;
      return h(
        "section",
        { class: "source-group", dataset: { turn: String(index) } },
        h("h3", { class: "caption" }, turn.question),
        h(
          "div",
          { class: ["verified", unverified ? "warn" : ""] },
          icon(unverified ? "alert" : "check"),
          h("span", null, unverified ? t("sources.unverified", { count: unverified }) : t("sources.verified")),
        ),
        answer.citations.map((c) => {
          const focused = focus && focus.turn === index && focus.reference === c.reference;
          // A web page opens through the Rust side, which finds its address in the saved turn.
          if (isWeb(c)) return webSourceCard(c, numbers.get(c.reference), focused, { conversationId: state.conversation?.id, turnId: turn.id, reference: c.reference });
          return sourceCard(c, numbers.get(c.reference), focused);
        }),
        runDetails(turn.report),
      );
    }),
  );
}

function storageView() {
  const scan = state.scan;
  const volumes = state.live.dash?.volumes || [];
  const drives = volumes.length
    ? h(
        "div",
        { class: "drives" },
        volumes.map((v) =>
          h(
            "div",
            { class: "drive" },
            h(
              "div",
              { class: "live-line" },
              h("span", { class: "live-label" }, t("live.drive", { drive: driveName(v.root) })),
              h("span", { class: ["live-value", v.free_ratio < 0.05 ? "warn" : ""] }, t("storagePanel.free", { free: bytes(v.free_bytes) })),
              h("span", { class: "live-detail" }, t("storagePanel.of", { total: bytes(v.total_bytes) })),
            ),
            h("div", { class: ["meter", v.free_ratio < 0.05 ? "warn" : ""] }, h("span", { style: { width: `${(1 - v.free_ratio) * 100}%` } })),
          ),
        ),
      )
    : null;
  let body;
  if (scan.status === "scanning" || scan.status === "starting") {
    body = h(
      "div",
      { class: "panel-scan" },
      h("p", null, t("storagePanel.scanning", { folder: scan.root || "" })),
      h("div", { class: "indeterminate" }),
      scan.progress ? h("p", { class: "note" }, `${number(scan.progress.entries)} · ${bytes(scan.progress.bytes)}`) : null,
    );
  } else if (scan.status === "done" && scan.view) {
    const dirs = scan.view.dirs.items.slice(0, 6);
    const max = Math.max(1, ...dirs.map((d) => d.size));
    body = h(
      "div",
      null,
      h("h3", { class: "caption" }, t("storagePanel.largest", { folder: scan.view.root })),
      h(
        "div",
        { class: "live-procs" },
        dirs.map((d) => {
          return h(
            "div",
            { class: "proc" },
            h("span", { class: "proc-name", title: d.path || d.name }, d.name),
            h("span", { class: "proc-value" }, bytes(d.size)),
            h("div", { class: "meter" }, h("span", { style: { width: `${(d.size / max) * 100}%` } })),
          );
        }),
      ),
    );
  } else {
    body = h("p", { class: "panel-empty" }, t("storagePanel.none"));
  }
  return h(
    "div",
    { class: "panel-storage" },
    drives,
    body,
    h("button", { type: "button", class: "btn", onclick: () => navigate("storage") }, icon("folder"), t(scan.status === "done" ? "storagePanel.open" : "storagePanel.scan")),
  );
}

const appsView = { sort: "memory", expanded: null };
let disposePanel = null;

export function mountPanel(host) {
  disposePanel?.();
  let disposeSummary = null;
  let showingSummary = false;
  let wasOpen = false;
  let returnFocus = null;
  let returnKey = null;
  const close = () => {
    closePanel();
    const target = returnFocus?.isConnected ? returnFocus : returnKey ? document.querySelector(`[data-key="${CSS.escape(returnKey)}"]`) : null;
    target?.focus({ preventScroll: true });
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (state.panel.tab === "selection" && ["e", "E", "Enter"].includes(event.key)) {
      // The data view's shortcuts must not start another selection while using this inspector.
      event.stopPropagation();
    }
  };
  host.addEventListener("keydown", onKey);
  // Applications are measured only while the Live PC tab is open (see watchActivity).
  const stopWatching = watchActivity(() => state.panel.open && state.panel.tab === "live");
  const tabs = h("div", { class: "tabs-pill", role: "tablist", "aria-label": t("panel.label") });
  const body = h("div", { class: "panel-body", role: "tabpanel" });
  fill(
    host,
    h(
      "div",
      { class: "panel-inner" },
      h(
        "div",
        { class: "panel-top" },
        tabs,
        h("span", { class: "spacer" }),
        h("button", { type: "button", class: "icon-btn", title: t("panel.close"), "aria-label": t("panel.close"), onclick: close }, icon("x")),
      ),
      body,
    ),
  );

  const renderTabs = () => {
    const sources = answeredTurns().reduce((n, { turn }) => n + turn.report.answer.citations.length, 0);
    keepFocus(tabs, () => fill(
      tabs,
      (state.summary ? [{ id: "selection", label: "panel.selection" }, ...TABS] : TABS).map((tab) =>
        h(
          "button",
          {
            type: "button",
            role: "tab",
            "data-key": `panel:${tab.id}`,
            "aria-selected": String(state.panel.tab === tab.id),
            onclick: () => openPanel(tab.id),
          },
          t(tab.label),
          tab.id === "sources" && sources ? h("span", { class: "count" }, String(sources)) : null,
        ),
      ),
    ));
  };

  const render = () => {
    if (state.panel.open && state.panel.tab === "selection" && !state.summary) {
      close();
      return;
    }
    if (state.panel.open && !wasOpen) {
      returnFocus = document.activeElement;
      returnKey = returnFocus?.closest?.("[data-key]")?.dataset.key || null;
    }
    wasOpen = state.panel.open;
    host.classList.toggle("open", state.panel.open);
    host.setAttribute("aria-hidden", String(!state.panel.open));
    host.inert = !state.panel.open;
    if (!state.panel.open) {
      disposeSummary?.();
      disposeSummary = null;
      showingSummary = false;
      return;
    }
    renderTabs();
    const tab = state.panel.tab;
    if (tab === "selection" && state.summary) {
      if (!showingSummary) {
        showingSummary = true;
        fill(body);
        disposeSummary = mountSelectionInspector(body);
      }
      return;
    }
    disposeSummary?.();
    disposeSummary = null;
    showingSummary = false;
    if (tab === "sources" || tab === "selection") fill(body, sourcesView());
    else if (tab === "live") {
      fill(
        body,
        liveView(state.live, {
          onPause: () => setLivePaused(!state.live.paused),
          sort: appsView.sort,
          onSort: (sort) => {
            appsView.sort = sort;
            render();
          },
          expanded: appsView.expanded,
          onToggle: (ref) => {
            appsView.expanded = ref;
            render();
          },
        }),
      );
    }
    else fill(body, storageView());
    if (tab === "sources" && state.panel.focus) {
      const focused = body.querySelector(".source-card.focused");
      const still = document.documentElement.classList.contains("reduce-motion");
      focused?.scrollIntoView({ block: "nearest", behavior: still ? "auto" : "smooth" });
    }
  };

  render();
  const unsubscribe = subscribe((topic) => {
    if (topic === "panel") pokeActivity();
    if (topic === "panel" || topic === "conversation" || topic === "summary") render();
    else if (topic === "live" && state.panel.open && (state.panel.tab === "live" || state.panel.tab === "storage")) render();
    else if (topic === "scan" && state.panel.open && state.panel.tab === "storage") render();
  });
  disposePanel = () => {
    disposeSummary?.();
    unsubscribe();
    stopWatching();
    host.removeEventListener("keydown", onKey);
  };
}

/** Opens the sources on one citation of one turn. */
export function showSource(turn, reference) {
  openPanel("sources", { turn, reference });
}

