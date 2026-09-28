// Entry point: preferences, the Rust bridge, the shell and the router.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { connect } from "./api.js";
import { loadPrefs } from "./prefs.js";
import {
  state,
  subscribe,
  refreshInfo,
  wireEvents,
  answerExtension,
  refreshConversations,
  openConversation,
  newConversation,
  deleteConversation,
} from "./store.js";
import { createMark } from "./mascot.js";
import { dialog, button, dialogOpen } from "./ui.js";
import { usd, date } from "./format.js";
import { modelName } from "./models.js";
import { currentRoute, navigate, ROUTES } from "./router.js";
import * as home from "./views/home.js";
import * as investigate from "./views/investigate.js";
import * as storage from "./views/storage.js";
import * as settings from "./views/settings.js";

const VIEWS = { home, investigate, storage, settings };
const NAV = [
  { route: "home", icon: "home", label: "nav.home" },
  { route: "investigate", icon: "sparkle", label: "nav.investigate" },
  { route: "storage", icon: "drive", label: "nav.storage" },
  { route: "settings", icon: "sliders", label: "nav.settings" },
];

let main = null;
let nav = null;
let statusHost = null;
let historyHost = null;
let unmount = null;
let mountedRoute = null;

function renderStatus() {
  if (!statusHost) return;
  const cloud = state.info?.cloud;
  let dot = "neutral";
  let title = t("status.local.title");
  let text = t("status.local.text");
  if (cloud?.ready) {
    dot = "good";
    title = t("status.cloud.title");
    text = modelName(cloud.model_id);
  } else if (cloud && (cloud.key_configured || cloud.model_id)) {
    dot = "warn";
    title = t("status.problem.title");
    text = t("status.problem.text");
  }
  fill(
    statusHost,
    h(
      "button",
      { type: "button", class: "status-card", title, onclick: () => navigate("settings") },
      h("span", { class: ["dot", dot] }),
      h("span", { class: "status-text" }, h("strong", null, title), h("span", null, text)),
    ),
  );
}

function renderNav() {
  const route = currentRoute();
  const running = state.inv && (state.inv.status === "running" || state.inv.status === "stopping");
  fill(
    nav,
    NAV.map((item) =>
      h(
        "button",
        {
          type: "button",
          class: "nav-item",
          "aria-current": item.route === route ? "page" : null,
          title: t(item.label),
          onclick: () => navigate(item.route),
        },
        icon(item.icon),
        h("span", { class: "nav-label" }, t(item.label)),
        item.route === "investigate" && running ? h("span", { class: "nav-badge", "aria-label": t("inv.running") }) : null,
      ),
    ),
  );
}

function confirmDelete(conversation) {
  dialog(
    (close) => [
      h("div", { class: "dialog-head" }, h("div", { class: "metric-icon warn" }, icon("x")), h("h2", { class: "dialog-title" }, t("history.delete.title"))),
      h("p", null, t("history.delete.body", { title: conversation.title })),
      h(
        "div",
        { class: "dialog-actions" },
        button(t("common.cancel"), { variant: "ghost", onClick: () => close(false) }),
        button(t("history.delete"), { variant: "danger", iconName: "x", onClick: () => close(true) }),
      ),
    ],
    { label: t("history.delete.title"), dismissValue: false },
  ).then((yes) => {
    if (yes) deleteConversation(conversation.id);
  });
}

/** Saved conversations in the sidebar, most recent first. */
function renderHistory() {
  if (!historyHost) return;
  const items = state.conversations || [];
  fill(
    historyHost,
    h(
      "div",
      { class: "history-head" },
      h("span", null, t("history.title")),
      h(
        "button",
        {
          type: "button",
          class: "history-new",
          title: t("history.new"),
          onclick: () => {
            newConversation();
            navigate("home");
          },
        },
        icon("sparkle"),
        h("span", null, t("history.new")),
      ),
    ),
    items.length
      ? h(
          "ul",
          { class: "history-list" },
          items.map((c) =>
            h(
              "li",
              { class: "history-row" },
              h(
                "button",
                {
                  type: "button",
                  class: "history-item",
                  "aria-current": state.conversation?.id === c.id ? "true" : null,
                  title: c.title,
                  onclick: async () => {
                    if (await openConversation(c.id)) navigate("investigate");
                  },
                },
                h("span", { class: "history-title" }, c.title),
                h("span", { class: "history-date" }, date(c.updated_at_unix_ms)),
              ),
              h("button", { type: "button", class: "history-delete", title: t("history.delete"), "aria-label": t("history.delete"), onclick: () => confirmDelete(c) }, icon("x")),
            ),
          ),
        )
      : h("p", { class: "hint history-empty" }, t("history.empty")),
  );
}

function renderShell() {
  const app = document.getElementById("app");
  nav = h("nav", { class: "nav", "aria-label": t("nav.label") });
  statusHost = h("div");
  historyHost = h("div", { class: "history" });
  main = h("main", { class: "main", id: "main", tabindex: "-1" });
  fill(
    app,
    h(
      "aside",
      { class: "sidebar" },
      h("div", { class: "brand" }, createMark(), h("div", { class: "brand-text" }, h("div", { class: "brand-name" }, "PC Agent"), h("div", { class: "brand-sub" }, t("app.tagline")))),
      nav,
      historyHost,
      h("div", { class: "sidebar-foot" }, statusHost, h("p", { class: "foot-note hint" }, t("foot.privacy"))),
    ),
    main,
  );
  renderNav();
  renderStatus();
  renderHistory();
}

function route(force = false) {
  const name = currentRoute();
  if (!force && name === mountedRoute) return;
  unmount?.();
  mountedRoute = name;
  const page = h("div", { class: "page" });
  fill(main, page);
  main.scrollTop = 0;
  unmount = VIEWS[name].mount(page) || null;
  renderNav();
}

/** Redraws everything, e.g. after the language changes. */
function redraw() {
  unmount?.();
  unmount = null;
  mountedRoute = null;
  renderShell();
  route(true);
}

function extensionDialog() {
  const inv = state.inv;
  const ext = inv?.extension;
  if (!ext || dialogOpen()) return;
  const cost = ext.usage?.cost ? usd(ext.usage.cost.usd) : t("extension.cost.unknown");
  dialog(
    (close) => [
      h("div", { class: "dialog-head" }, h("div", { class: "metric-icon warn" }, icon("clock")), h("h2", { class: "dialog-title" }, t("extension.title"))),
      h("p", null, t(`extension.reason.${ext.request.reason}`)),
      h(
        "p",
        { class: "note", style: { "margin-top": "8px" } },
        t("extension.body", {
          requests: ext.usage.requests,
          cost,
          rounds: ext.request.additional_rounds,
          tools: ext.request.additional_tool_calls,
          seconds: ext.request.additional_seconds,
        }),
      ),
      h(
        "div",
        { class: "dialog-actions" },
        button(t("extension.stop"), { onClick: () => close(false) }),
        button(t("extension.continue"), { variant: "primary", iconName: "play", onClick: () => close(true) }),
      ),
    ],
    { label: t("extension.title"), dismissValue: false },
  ).then((accept) => answerExtension(accept));
}

async function boot() {
  loadPrefs();
  await connect();
  renderShell();
  wireEvents();
  subscribe((topic) => {
    if (topic === "info") renderStatus();
    if (topic === "inv") renderNav();
    if (topic === "history" || topic === "conversation") renderHistory();
    if (topic === "extension") extensionDialog();
  });
  window.addEventListener("hashchange", () => route());
  window.addEventListener("pcagent:redraw", redraw);
  document.addEventListener("keydown", (e) => {
    // Ctrl+1..4 switch screens; Ctrl+K jumps to the question box.
    if (!e.ctrlKey || e.altKey || e.metaKey) return;
    const index = Number(e.key) - 1;
    if (index >= 0 && index < ROUTES.length) {
      e.preventDefault();
      navigate(ROUTES[index]);
    } else if (e.key.toLowerCase() === "k") {
      e.preventDefault();
      if (currentRoute() !== "home" && currentRoute() !== "investigate") navigate("home");
      window.setTimeout(() => document.querySelector(".ask textarea")?.focus(), 60);
    }
  });
  route(true);
  refreshConversations();
  try {
    await refreshInfo();
  } catch {
    // The status card keeps its local default.
  }
}

boot();
