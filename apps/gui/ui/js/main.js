// Entry point: preferences, the Rust bridge, the window layout and the router. The layout is a
// rail of views, the conversation list (on the chat only), the current view and a context panel
// that stays closed until a citation, the PC status or a storage link opens it.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { connect } from "./api.js";
import { loadPrefs } from "./prefs.js";
import {
  state,
  subscribe,
  refreshInfo,
  refreshPrivacy,
  wireEvents,
  answerExtension,
  refreshConversations,
  openConversation,
  newConversation,
  deleteConversation,
  startLive,
  openPanel,
  closePanel,
  togglePanel,
} from "./store.js";
import { mountPanel } from "./panel.js";
import { health } from "./live.js";
import { dialog, button, dialogOpen } from "./ui.js";
import { usd } from "./format.js";
import { modelName } from "./models.js";
import { namesShared } from "./ask.js";
import { currentRoute, navigate, ROUTES } from "./router.js";
import { mark } from "./brand.js";
import * as chat from "./views/chat.js";
import * as history from "./views/history.js";
import * as live from "./views/live.js";
import * as storage from "./views/storage.js";
import * as settings from "./views/settings.js";

const VIEWS = { chat, history, live, storage, settings };
const narrow = window.matchMedia("(max-width: 900px)");

let app = null;
let railHost = null;
let main = null;
let sidebarHost = null;
let topbar = null;
let panelHost = null;
let unmount = null;
let mountedRoute = null;
let sidebarOpen = !narrow.matches;
let filter = "";

// ---------- Sidebar ----------

function confirmDelete(conversation) {
  dialog(
    (close) => [
      h("h2", { class: "dialog-title" }, t("history.delete.title")),
      h("p", null, t("history.delete.body", { title: conversation.title })),
      h(
        "div",
        { class: "dialog-actions" },
        button(t("common.cancel"), { onClick: () => close(false) }),
        button(t("history.delete"), { variant: "danger", onClick: () => close(true) }),
      ),
    ],
    { label: t("history.delete.title"), dismissValue: false },
  ).then((yes) => {
    if (yes) deleteConversation(conversation.id);
  });
}

/** Conversations grouped by when they were last updated. */
function groups(items) {
  const startOfDay = new Date();
  startOfDay.setHours(0, 0, 0, 0);
  const today = startOfDay.getTime();
  const day = 24 * 3600 * 1000;
  const buckets = [
    { label: "history.today", from: today },
    { label: "history.yesterday", from: today - day },
    { label: "history.week", from: today - 7 * day },
    { label: "history.older", from: -Infinity },
  ].map((b) => ({ ...b, items: [] }));
  for (const c of items) buckets.find((b) => c.updated_at_unix_ms >= b.from).items.push(c);
  return buckets.filter((b) => b.items.length);
}

function historyView() {
  const needle = filter.trim().toLowerCase();
  const items = (state.conversations || []).filter((c) => !needle || c.title.toLowerCase().includes(needle));
  if (!items.length) return h("p", { class: "history-empty" }, t(needle ? "sidebar.noMatch" : "history.empty"));
  return groups(items).map((group) =>
    h(
      "div",
      { class: "history-group" },
      h("div", { class: "history-label" }, t(group.label)),
      group.items.map((c) =>
        h(
          "div",
          { class: "history-row" },
          h(
            "button",
            {
              type: "button",
              class: "history-item",
              "aria-current": state.conversation?.id === c.id && currentRoute() === "chat" ? "true" : null,
              title: c.last_turn_status === "failed" ? `${c.title} — ${t("history.lastFailed")}` : c.title,
              onclick: async () => {
                if (await openConversation(c.id)) navigate("chat");
                if (narrow.matches) setSidebar(false);
              },
            },
            c.last_turn_status === "failed" ? h("span", { class: "dot warn", "aria-label": t("history.lastFailed") }) : null,
            c.title,
          ),
          h("button", { type: "button", class: "history-delete", title: t("history.delete"), "aria-label": t("history.delete"), onclick: () => confirmDelete(c) }, icon("x")),
        ),
      ),
    ),
  );
}

function modelStatus() {
  const cloud = state.info?.cloud;
  let tone = "neutral";
  let title = t("status.local.title");
  let text = t("status.local.text");
  if (cloud?.ready && state.privacy.mode === "local") {
    tone = "neutral";
    title = t("privacy.local");
    text = t("status.localChosen");
  } else if (cloud?.ready) {
    tone = "good";
    title = modelName(cloud.model_id);
    text = t(namesShared() ? "status.names.shared" : "status.names.hidden");
  } else if (cloud && (cloud.key_configured || cloud.model_id)) {
    tone = "warn";
    title = t("status.problem.title");
    text = t("status.problem.text");
  }
  return h(
    "button",
    { type: "button", class: "model-status", onclick: () => navigate("settings") },
    h("span", { class: ["dot", tone] }),
    h("span", { class: "model-text" }, h("strong", null, title), h("span", null, text)),
  );
}

let historyHost = null;
let footHost = null;

function renderSidebar() {
  const search = h("input", {
    type: "search",
    class: "search-input",
    placeholder: t("sidebar.search"),
    "aria-label": t("sidebar.searchLabel"),
    value: filter,
    oninput: (e) => {
      filter = e.target.value;
      fill(historyHost, historyView());
    },
  });
  historyHost = h("nav", { class: "history", "aria-label": t("history.title") });
  footHost = h("div", { class: "sidebar-foot" });
  fill(
    sidebarHost,
    h(
      "div",
      { class: "sidebar-inner" },
      h(
        "div",
        { class: "sidebar-top" },
        h(
          "button",
          {
            type: "button",
            class: "new-chat",
            title: `${t("sidebar.new")} (Ctrl+N)`,
            onclick: () => {
              newConversation();
              navigate("chat");
              if (narrow.matches) setSidebar(false);
            },
          },
          icon("plus"),
          h("span", null, t("sidebar.new")),
        ),
        h("button", { type: "button", class: "icon-btn", title: t("sidebar.hide"), "aria-label": t("sidebar.hide"), onclick: () => setSidebar(false) }, icon("sidebar")),
      ),
      h("label", { class: "search" }, icon("search"), search),
      historyHost,
      footHost,
    ),
  );
  fill(historyHost, historyView());
  renderFoot();
}

function renderFoot() {
  if (footHost) fill(footHost, modelStatus());
  renderRail();
}

// ---------- Rail ----------

const RAIL = [
  { route: "chat", icon: "chat", label: "nav.chat" },
  { route: "history", icon: "history", label: "nav.history" },
  { route: "live", icon: "activity", label: "nav.live" },
  { route: "storage", icon: "drive", label: "nav.storage" },
];

function railButton({ route: name, icon: iconName, label }, extra = null) {
  const index = ROUTES.indexOf(name) + 1;
  return h(
    "button",
    {
      type: "button",
      class: "rail-item",
      "aria-current": currentRoute() === name ? "page" : null,
      title: `${t(label)} (Ctrl+${index})`,
      onclick: () => navigate(name),
    },
    h("span", { class: "rail-icon" }, icon(iconName), extra),
    h("span", { class: "rail-label" }, t(label)),
  );
}

function renderRail() {
  if (!railHost) return;
  const cloud = state.info?.cloud;
  // The settings entry carries the AI status, as the sidebar does on the chat.
  const tone = cloud?.ready && state.privacy.mode !== "local" ? "good" : cloud && (cloud.key_configured || cloud.model_id) && !cloud.ready ? "warn" : null;
  fill(
    railHost,
    h("span", { class: "rail-mark" }, mark(28)),
    RAIL.map((item) => railButton(item)),
    h("span", { class: "spacer" }),
    railButton({ route: "settings", icon: "sliders", label: "nav.settings" }, tone ? h("span", { class: ["rail-dot", "dot", tone] }) : null),
  );
}

function setSidebar(open) {
  sidebarOpen = open;
  app.classList.toggle("sidebar-closed", !open);
  sidebarHost.inert = !open || currentRoute() !== "chat";
  renderTopbar();
}

/** The conversation list belongs to the chat; other views use the full width. */
function applyRouteLayout() {
  const chatRoute = currentRoute() === "chat";
  app.classList.toggle("no-sidebar", !chatRoute);
  sidebarHost.inert = !sidebarOpen || !chatRoute;
}

// ---------- Top bar ----------

function renderTopbar() {
  if (!topbar) return;
  const route = currentRoute();
  const status = health(state.live.dash);
  const liveOpen = state.panel.open && state.panel.tab === "live";
  const title = route === "chat" ? state.conversation?.title || "" : "";
  fill(
    topbar,
    sidebarOpen || route !== "chat" ? null : h("button", { type: "button", class: "icon-btn", title: t("sidebar.show"), "aria-label": t("sidebar.show"), onclick: () => setSidebar(true) }, icon("sidebar")),
    h("div", { class: "topbar-title", title }, title),
    h("span", { class: "spacer" }),
    h(
      "button",
      {
        type: "button",
        class: ["status-pill", liveOpen ? "pressed" : ""],
        "aria-pressed": String(liveOpen),
        title: t("health.open"),
        onclick: () => (liveOpen ? closePanel() : openPanel("live")),
      },
      h("span", { class: ["dot", status.tone] }),
      h("span", null, t("health.label")),
      h("span", { class: "faint" }, "·"),
      h("span", { class: ["status-text", status.tone] }, status.text),
    ),
    h(
      "button",
      { type: "button", class: ["icon-btn", state.panel.open ? "pressed" : ""], title: `${t("panel.toggle")} (Ctrl+J)`, "aria-label": t("panel.toggle"), "aria-pressed": String(state.panel.open), onclick: togglePanel },
      icon("panel"),
    ),
  );
}

// ---------- Layout and routes ----------

function renderShell() {
  app = document.getElementById("app");
  railHost = h("nav", { class: "rail", "aria-label": t("rail.label") });
  sidebarHost = h("aside", { class: "sidebar", "aria-label": t("history.title") });
  topbar = h("header", { class: "topbar" });
  main = h("main", { class: "main", id: "main", tabindex: "-1" });
  panelHost = h("aside", { class: "panel", "aria-label": t("panel.label") });
  fill(app, railHost, sidebarHost, h("div", { class: "workspace" }, topbar, main), panelHost);
  app.classList.toggle("sidebar-closed", !sidebarOpen);
  applyRouteLayout();
  renderSidebar();
  renderTopbar();
  mountPanel(panelHost);
}

function route(force = false) {
  const name = currentRoute();
  if (!force && name === mountedRoute) return;
  unmount?.();
  mountedRoute = name;
  const page = h("div", { class: ["page", `page-${name}`] });
  fill(main, page);
  main.scrollTop = 0;
  unmount = VIEWS[name].mount(page) || null;
  applyRouteLayout();
  renderFoot();
  renderTopbar();
  if (historyHost) fill(historyHost, historyView());
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
      h("h2", { class: "dialog-title" }, t("extension.title")),
      h("p", null, t(`extension.reason.${ext.request.reason}`)),
      h(
        "p",
        { class: "note" },
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
        button(t("extension.continue"), { variant: "primary", onClick: () => close(true) }),
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
    if (topic === "info" || topic === "privacy") renderFoot();
    if (topic === "history" || topic === "conversation") {
      if (historyHost) fill(historyHost, historyView());
      renderTopbar();
    }
    if (topic === "live" || topic === "panel") renderTopbar();
    if (topic === "extension") extensionDialog();
  });
  window.addEventListener("hashchange", () => route());
  window.addEventListener("pcagent:redraw", redraw);
  window.addEventListener("pcagent:mode", renderFoot);
  narrow.addEventListener("change", () => setSidebar(!narrow.matches));
  document.addEventListener("keydown", (e) => {
    if (!e.ctrlKey || e.altKey || e.metaKey || dialogOpen()) return;
    const key = e.key.toLowerCase();
    const index = Number(e.key) - 1;
    if (index >= 0 && index < ROUTES.length) {
      e.preventDefault();
      navigate(ROUTES[index]);
    } else if (key === "k") {
      e.preventDefault();
      navigate("chat");
      window.setTimeout(() => document.querySelector(".ask textarea")?.focus(), 60);
    } else if (key === "n") {
      e.preventDefault();
      newConversation();
      navigate("chat");
    } else if (key === "j") {
      e.preventDefault();
      togglePanel();
    }
  });
  route(true);
  startLive();
  refreshConversations();
  refreshPrivacy();
  try {
    await refreshInfo();
  } catch {
    // The status keeps its local default.
  }
}

boot();
