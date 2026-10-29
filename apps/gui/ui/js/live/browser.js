// Closed measured types may be shared. Window/tab text stays in this ephemeral view only.
import { h } from "../dom.js";
import { t } from "../i18n.js";
import { bytes, basisPoints } from "../format.js";
import { call } from "../api.js";
import { button, dialog } from "../ui.js";
import { failureText } from "../failure.js";

export const BROWSER_TYPES = ["browser_main", "renderer", "extension", "gpu", "network", "storage", "audio", "media_decoding", "utility", "crash_handler", "unknown"];
const localReads = new Map();
const sorts = new Map();
const localKey = (app) => app.local_key || app.entity_ref;
const typeLabel = (type) => t(`browser.type.${BROWSER_TYPES.includes(type) ? type : "unknown"}`);
const measured = (measure, format) => measure?.value == null ? t("metric.notMeasured") : format(measure.value);

export function sortedBrowserTypes(rows, sort = "memory") {
  return [...rows].sort((a, b) => {
    if (sort === "type") return typeLabel(a.process_type).localeCompare(typeLabel(b.process_type));
    const value = (row) => (sort === "count" ? row.process_count : sort === "cpu" ? row.cpu?.value : row.working_set?.value) ?? -1;
    return value(b) - value(a) || a.process_type.localeCompare(b.process_type);
  });
}

export function browserSummary(app) {
  if (!app.browser_types?.length) return null;
  return sortedBrowserTypes(app.browser_types).map((row) =>
    `${typeLabel(row.process_type)}: ${row.process_count} · ${measured(row.working_set, bytes)}`
  ).join(" / ");
}

export function browserBreakdown(app, redraw) {
  const rows = app.browser_types || [];
  if (!rows.length) return null;
  const sort = sorts.get(localKey(app)) || "memory";
  return h("section", { class: "browser-breakdown" },
    h("h3", { class: "caption" }, t("browser.breakdown")),
    h("p", { class: "note" }, t("browser.coverage", {
      classified: rows.reduce((sum, row) => sum + row.classified_count, 0),
      total: rows.reduce((sum, row) => sum + row.process_count, 0),
    })),
    h("div", { class: "mini-tabs browser-sorts", role: "group", "aria-label": t("browser.sort") },
      ["memory", "cpu", "count", "type"].map((key) => h("button", {
        type: "button", "aria-pressed": String(sort === key), "data-key": `browser-sort:${key}`,
        onclick: () => { sorts.set(localKey(app), key); redraw(); },
      }, t(`browser.sort.${key}`)))),
    h("table", { class: "browser-table" },
      h("caption", { class: "sr-only" }, t("browser.breakdown")),
      h("thead", null, h("tr", null,
        ["type", "count", "memory", "cpu"].map((key) => h("th", { scope: "col" }, t(`browser.sort.${key}`))))),
      h("tbody", null, sortedBrowserTypes(rows, sort).map((row) => h("tr", null,
        h("th", { scope: "row" }, typeLabel(row.process_type)),
        h("td", null, String(row.process_count)),
        h("td", null, measured(row.working_set, bytes)),
        h("td", null, measured(row.cpu, basisPoints)),
      )))),
    h("p", { class: "note" }, t("browser.overlap")),
  );
}

function snapshot(ref) {
  if (!localReads.has(ref)) localReads.set(ref, { busy: false, data: null, error: null, result: null });
  return localReads.get(ref);
}

async function readWindows(app, tabs, redraw) {
  const state = snapshot(localKey(app));
  state.busy = true;
  state.error = null;
  state.result = null;
  redraw();
  try {
    const data = await call("local_browser_windows", { applicationRef: localKey(app), includeTabs: tabs });
    if (localReads.get(localKey(app)) === state) state.data = data;
  } catch (error) {
    state.error = failureText(error, "failure.live_unavailable");
  } finally {
    state.busy = false;
    redraw();
  }
}

async function openManager(app, window, redraw) {
  const state = snapshot(localKey(app));
  state.busy = true;
  state.error = null;
  state.result = null;
  redraw();
  let prepared;
  try {
    prepared = await call("browser_manager_prepare", { applicationRef: localKey(app), processRef: window.process_ref });
    if (localReads.get(localKey(app)) !== state) {
      await call("action_decline", { turnId: prepared.turn_id, planId: prepared.plan.plan_id });
      return;
    }
    const approved = await dialog((close) => [
      h("h2", { class: "dialog-title" }, t("action.kind.open_browser_process_manager")),
      h("p", null, t(`browser.destination.${window.family}`)),
      h("p", null, t("browser.review")),
      h("ul", { class: "plan-targets" }, prepared.plan.targets.map((target) =>
        h("li", null, `${target.label} · PID ${target.pid}`))),
      h("p", { class: "note" }, t("action.review.checks")),
      h("div", { class: "dialog-actions" },
        button(t("common.cancel"), { onClick: () => close(false) }),
        button(t("action.run"), { variant: "primary", onClick: () => close(true) })),
    ], { label: t("action.kind.open_browser_process_manager"), dismissValue: false });
    if (!approved) {
      await call("action_decline", { turnId: prepared.turn_id, planId: prepared.plan.plan_id });
      return;
    }
    const result = await call("action_run", { turnId: prepared.turn_id, planId: prepared.plan.plan_id });
    state.result = result.overall === "completed" ? t("browser.opened") : t(`action.outcome.${result.overall}`);
  } catch (error) {
    state.error = failureText(error, "failure.action_refused");
  } finally {
    state.busy = false;
    redraw();
  }
}

export function browserLocalInspection(app, redraw) {
  if (!app.browser_types?.length) return null;
  const state = snapshot(localKey(app));
  const controls = h("div", { class: "browser-controls" },
    h("button", { type: "button", class: "btn btn-sm", disabled: state.busy, "data-key": "browser-read",
      onclick: () => readWindows(app, false, redraw) }, t("browser.windows")),
    h("button", { type: "button", class: "btn btn-sm", disabled: state.busy, "data-key": "browser-tabs",
      onclick: () => readWindows(app, true, redraw) }, t("browser.tabs")));
  return h("section", { class: "browser-local" },
    h("h3", { class: "caption" }, t("browser.local")),
    h("p", { class: "note" }, t("browser.privacy")),
    controls,
    state.busy ? h("p", { role: "status", class: "note" }, t("browser.reading")) : null,
    state.error ? h("p", { role: "alert", class: "note" }, state.error) : null,
    state.result ? h("p", { role: "status", class: "note" }, state.result) : null,
    state.data ? h("p", { class: "note" }, t("browser.snapshot")) : null,
    state.data && state.data.status !== "ok" ? h("p", { class: "note" }, t("browser.unavailable")) : null,
    (state.data?.windows || []).map((window, index) => h("div", { class: "browser-window" },
      h("p", { class: "browser-caption" }, window.title || t("browser.untitled")),
      window.tabs.length ? h("ul", { class: "browser-tabs" }, window.tabs.map((tab) =>
        h("li", null, tab.selected ? h("strong", null, tab.title, ` · ${t("browser.selected")}`) : tab.title))) : null,
      window.tabs_status === "ok" ? null : h("p", { class: "note" }, t(window.tabs_status === "unsupported" ? "browser.tabs.notRead" : "browser.tabs.partial")),
      ["firefox", "chrome", "edge"].includes(window.family) ? h("button", {
        type: "button", class: "btn btn-sm", disabled: state.busy, "data-key": `browser-manager:${index}`,
        onclick: () => openManager(app, window, redraw),
      }, t("action.kind.open_browser_process_manager")) : h("p", { class: "note" }, t("browser.embedded")),
    )),
  );
}

// No localStorage, global app store, selection, telemetry or conversation receives these strings.
export function clearLocalBrowsers() {
  localReads.clear();
  sorts.clear();
}
