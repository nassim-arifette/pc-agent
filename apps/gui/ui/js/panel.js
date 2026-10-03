// The context panel on the right: closed by default, opened from a citation, the PC status or a
// storage link. Three tabs: the sources of the conversation's answers, live measurements, and
// the last folder analysis.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { state, subscribe, openPanel, closePanel, setLivePaused, notify } from "./store.js";
import { call } from "./api.js";
import { liveView, driveName } from "./live.js";
import { numbering, citationTitle } from "./citations.js";
import { bytes, clock, date, labelled, number, price, usd } from "./format.js";
import { navigate } from "./router.js";

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
    h("ul", { class: "source-facts" }, citation.facts.map((fact) => h("li", null, fact))),
  );
}

/** What could not be measured, and what the run consumed: kept out of the answer, one click away. */
function runDetails(report) {
  const limits = [...new Set([...(report.limitations || []), ...(report.selected?.limitations || [])])];
  const missing = (report.coverage?.domains || []).filter((d) => d.status !== "complete");
  const u = report.usage;
  const blocks = [];
  if (limits.length || missing.length) {
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
            missing.map((d) => h("li", null, labelled(`${t(`domain.${d.domain}`)}${d.target ? ` (${d.target})` : ""}`, t(`coverage.${d.status}`)))),
            limits.map((x) => h("li", null, x)),
          ),
        ),
      ),
    );
  }
  if (u) {
    const facts = [
      labelled(t("usage.requests"), number(u.requests)),
      labelled(t("usage.tools"), number(u.tool_calls)),
      u.prompt_tokens != null ? labelled(t("usage.tokensIn"), number(u.prompt_tokens)) : null,
      u.completion_tokens != null ? labelled(t("usage.tokensOut"), number(u.completion_tokens)) : null,
      u.cost ? labelled(t("usage.cost"), usd(u.cost.usd)) : t("usage.noCost"),
      u.cost
        ? t("usage.price", { input: price(u.cost.price.input_per_million), output: price(u.cost.price.output_per_million), date: date(u.cost.price.read_at_unix_ms, true) })
        : null,
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
        answer.citations.map((c) => sourceCard(c, numbers.get(c.reference), focus && focus.turn === index && focus.reference === c.reference)),
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
            h("span", { class: "proc-value" }, d.size_text),
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

// Application activity takes a two-second measurement window: it runs only while the Live PC tab
// is visible, every ten seconds, never in the background.
const ACTIVITY_MS = 10000;
let activityTimer = null;
let activityBusy = false;
const appsView = { sort: "memory", expanded: null };

async function activityTick() {
  window.clearTimeout(activityTimer);
  const visible = state.panel.open && state.panel.tab === "live" && !document.hidden && !state.live.paused;
  if (!visible) return;
  if (!activityBusy) {
    activityBusy = true;
    try {
      state.live.activity = await call("process_activity");
    } catch {
      // The dashboard rows above still show; the list keeps its last value.
    }
    activityBusy = false;
    notify("live");
  }
  activityTimer = window.setTimeout(activityTick, ACTIVITY_MS);
}

export function mountPanel(host) {
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
        h("button", { type: "button", class: "icon-btn", title: t("panel.close"), "aria-label": t("panel.close"), onclick: closePanel }, icon("x")),
      ),
      body,
    ),
  );

  const renderTabs = () => {
    const sources = answeredTurns().reduce((n, { turn }) => n + turn.report.answer.citations.length, 0);
    fill(
      tabs,
      TABS.map((tab) =>
        h(
          "button",
          {
            type: "button",
            role: "tab",
            "aria-selected": String(state.panel.tab === tab.id),
            onclick: () => openPanel(tab.id),
          },
          t(tab.label),
          tab.id === "sources" && sources ? h("span", { class: "count" }, String(sources)) : null,
        ),
      ),
    );
  };

  const render = () => {
    host.classList.toggle("open", state.panel.open);
    host.setAttribute("aria-hidden", String(!state.panel.open));
    host.inert = !state.panel.open;
    if (!state.panel.open) return;
    renderTabs();
    const tab = state.panel.tab;
    if (tab === "sources") fill(body, sourcesView());
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
  subscribe((topic) => {
    if (topic === "panel") activityTick();
    if (topic === "panel" || topic === "conversation") render();
    else if (topic === "live" && state.panel.open && (state.panel.tab === "live" || state.panel.tab === "storage")) render();
    else if (topic === "scan" && state.panel.open && state.panel.tab === "storage") render();
  });
}

/** Opens the sources on one citation of one turn. */
export function showSource(turn, reference) {
  openPanel("sources", { turn, reference });
}

