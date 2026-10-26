// Quick drive analysis: the closed `mft_inventory` task of the separate elevated helper
// (pcagent-elevated: no network, no AI). The page shows the exact task and budget and asks for a
// local click; only then does Windows ask for administrator consent. The helper reads the file
// system's index (names and sizes, never contents) and changes nothing; the result stays on this
// PC. Only fixed NTFS drives are offered. One elevated task can be prepared or run at a time
// (elevation_status).
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { call } from "./api.js";
import { state } from "./store.js";
import { badge, bar, button, toast } from "./ui.js";
import { bytes, clock, duration, number, percent } from "./format.js";
import { driveName } from "./live.js";
import { holdSlot, onSlotChange, slotHeldElsewhere } from "./elevated.js";
import { failureDetail, failureText } from "./failure.js";
import { capable } from "./capabilities.js";
import { subscribe } from "./store.js";

/** Collection budget asked for: the helper's maximum; most drives finish well before. */
export const BUDGET_MS = 120000;

// Kept across visits to the Storage view: a running analysis outlives the page that started it.
const quick = { status: "idle", volume: null, request: null, result: null, error: null, startedAt: null, finishedAt: null, tab: "dirs" };
const listeners = new Set();
let ticker = null;
let poller = null;

function changed() {
  for (const listener of [...listeners]) listener();
}

/** Tells the deeper checks card whether this analysis holds the single elevated slot. */
function publishSlot() {
  holdSlot("quick", ["preparing", "review", "running", "cancelling", "orphaned"].includes(quick.status) || poller != null);
}

function set(patch) {
  Object.assign(quick, patch);
  window.clearInterval(ticker);
  ticker = quick.status === "running" ? window.setInterval(changed, 1000) : null;
  changed();
  publishSlot();
}

/** Waits until the elevated slot is free again, e.g. after a cancellation or an error. */
function waitForFreeSlot(then) {
  window.clearInterval(poller);
  poller = window.setInterval(async () => {
    try {
      if ((await call("elevation_status")) == null) {
        window.clearInterval(poller);
        poller = null;
        then();
        publishSlot();
      }
    } catch {
      // Keep waiting; the next poll retries.
    }
  }, 1000);
  publishSlot();
}

/**
 * Drive letters the helper can read, C: first: the fixed NTFS volumes of the live measurements,
 * and fixed ones whose file system could not be read (the helper checks again). C: before the
 * first measurement; none when this PC has no fixed NTFS drive.
 */
function volumes() {
  const shown = state.live.dash?.volumes || [];
  if (!shown.length) return capable("ntfs_volumes") ? ["C"] : [];
  return shown
    .filter((v) => (v.kind ?? "fixed") === "fixed" && (v.filesystem == null || /^ntfs$/i.test(v.filesystem)))
    .map((v) => driveName(v.root).replace(":", ""))
    .filter((l) => /^[A-Z]$/i.test(l))
    .sort();
}

async function prepare(volume) {
  set({ status: "preparing", volume, error: null });
  try {
    const request = await call("elevation_prepare", { task: { kind: "mft_inventory", volume }, budgetMs: BUDGET_MS });
    set({ status: "review", request });
  } catch (error) {
    set({ status: "failed", error });
    waitForFreeSlot(() => quick.status === "failed" && changed());
  }
}

async function approve() {
  const request = quick.request;
  if (!request) return;
  set({ status: "running", startedAt: Date.now(), result: null, error: null });
  try {
    const result = await call("elevation_run", { requestId: request.request_id });
    // A result that arrives after Stop was clicked is still a complete measurement: show it.
    window.clearInterval(poller);
    poller = null;
    set({ status: "done", result, finishedAt: Date.now(), request: null });
  } catch (error) {
    // After Stop, the cancellation flow reports the outcome once the slot is free.
    if (quick.status === "cancelling") return;
    // An error does not prove the slot is free: wait for elevation_status before another try.
    set({ status: "failed", error, request: null });
    waitForFreeSlot(() => changed());
  }
}

async function cancel() {
  const request = quick.request;
  if (!request) {
    set({ status: "idle" });
    return;
  }
  const wasRunning = quick.status === "running";
  set({ status: "cancelling" });
  try {
    await call("elevation_cancel", { requestId: request.request_id });
  } catch (error) {
    toast(failureText(error), "bad");
  }
  waitForFreeSlot(() => {
    if (quick.status === "cancelling") set({ status: wasRunning ? "cancelled" : "idle", request: null });
  });
}

/** Recovers a prepared or running task after the page was reloaded. */
export async function recoverQuickScan() {
  if (quick.status !== "idle") return;
  try {
    const current = await call("elevation_status");
    if (current?.task?.kind !== "mft_inventory") return;
    const volume = current.task.volume;
    if (current.state === "awaiting_local_approval") set({ status: "review", volume, request: current });
    else if (current.state === "running" || current.state === "preparing") {
      // The result belongs to the page that started it; this page can only follow or cancel it.
      set({ status: "orphaned", volume, request: current });
      waitForFreeSlot(() => set({ status: "idle", request: null }));
    } else if (current.state === "cancelling") {
      set({ status: "cancelling", volume, request: current });
      waitForFreeSlot(() => set({ status: "idle", request: null }));
    }
  } catch {
    // Without a status the card simply offers to prepare a new analysis.
  }
}

function observation(result, metric) {
  return (result?.observations || []).find((o) => o.metric === metric && typeof o.value === "number") || null;
}

function itemSize(item) {
  return item.data_cluster_bytes_known ?? item.logical_bytes_known;
}

function treeList(items, volume, kind) {
  if (!items?.length) return h("p", { class: "muted" }, t("quick.empty"));
  const max = Math.max(1, ...items.map(itemSize));
  return h(
    "div",
    { class: "list" },
    items.map((item) => {
      const path = `${volume}:\\${item.path.replace(/^[\\/]+/, "")}`;
      const name = path.split("\\").filter(Boolean).pop() || path;
      const unknown = item.unknown_logical_files + item.unknown_allocation_files;
      return h(
        "div",
        { class: "list-row quick-row" },
        h("div", { class: "list-name" }, icon(kind === "dirs" ? "folder" : "file"), h("div", { class: "text" }, h("strong", { title: path }, name), h("span", { title: path }, path))),
        bar(itemSize(item) / max, kind === "dirs" ? "" : "mint"),
        h(
          "div",
          { class: "list-value" },
          bytes(itemSize(item)),
          unknown ? h("span", { class: "partial-mark", title: t("quick.unknownFiles", { count: number(unknown) }), "aria-label": t("quick.unknownFiles", { count: number(unknown) }) }, icon("info")) : null,
        ),
        h("div", { class: "quick-files" }, kind === "dirs" ? t("quick.files", { count: number(item.files) }) : ""),
      );
    }),
  );
}

function resultView(onRender) {
  const r = quick.result;
  const tree = r.local_mft_tree;
  const files = observation(r, "ntfs_files_examined");
  const data = observation(r, "ntfs_supported_data_allocation_bytes_known") || observation(r, "ntfs_supported_logical_bytes_known");
  const walked = observation(r, "ntfs_record_slots_walked")?.value;
  const left = observation(r, "ntfs_record_slots_not_walked")?.value;
  const share = walked != null && left != null && walked + left > 0 ? walked / (walked + left) : null;
  const complete = share === 1;
  const notes = (r.coverage || []).filter((c) => c.status !== "ok").map((c) => c.detail);
  return h(
    "section",
    { class: "card card-pad section quick-result" },
    h(
      "div",
      { class: "row wrap", style: { "justify-content": "space-between", "margin-bottom": "10px" } },
      h("div", null, h("h3", { class: "quick-title" }, t("quick.result.title", { drive: `${quick.volume}:` })), h("span", { class: "hint" }, t("quick.result.when", { at: clock(r.measured_at_unix_ms), took: duration((quick.finishedAt ?? Date.now()) - (quick.startedAt ?? Date.now())) }))),
      button(t("quick.again"), { iconName: "refresh", size: "sm", onClick: () => prepare(quick.volume) }),
    ),
    h(
      "div",
      { class: "grid grid-metrics grid-stats" },
      h("div", { class: "card stat" }, h("span", { class: "stat-label" }, t("quick.stat.files")), h("span", { class: "stat-value" }, files ? number(files.value) : "—")),
      h("div", { class: "card stat" }, h("span", { class: "stat-label" }, t("quick.stat.data")), h("span", { class: "stat-value" }, data ? bytes(data.value) : "—"), h("span", { class: "hint" }, t("quick.stat.dataNote"))),
      h(
        "div",
        { class: "card stat" },
        h("span", { class: "stat-label" }, t("quick.stat.coverage")),
        h("span", { class: "stat-value" }, share != null ? percent(share) : "—"),
        share != null ? bar(share, complete ? "" : "warn") : null,
        h("span", null, badge(t(complete ? "quick.complete" : "quick.partial"), complete ? "good" : "warn", complete ? "check" : "info")),
      ),
    ),
    tree
      ? [
          h(
            "div",
            { class: "tabs", role: "tablist", style: { margin: "14px 0 6px" } },
            ["dirs", "files"].map((k) =>
              h("button", { type: "button", role: "tab", "aria-selected": String(quick.tab === k), onclick: () => ((quick.tab = k), onRender()) }, t(`storage.tab.${k}`)),
            ),
          ),
          treeList(quick.tab === "dirs" ? tree.directories : tree.files, quick.volume, quick.tab),
          h("p", { class: "note", style: { "margin-top": "10px" } }, t("quick.overlap")),
        ]
      : h("p", { class: "muted" }, t("quick.noTree")),
    notes.length ? h("details", { class: "more" }, h("summary", null, icon("chevron"), t("quick.limits")), h("ul", { class: "bullets" }, notes.map((n) => h("li", null, n)))) : null,
  );
}

/** The quick analysis card of the Storage view; returns its element and a cleanup. */
export function quickScanCard() {
  const host = h("section", { class: "card card-pad section quick-card" });
  let volume = quick.volume || volumes()[0];
  const render = () => {
    const busy = ["preparing", "running", "cancelling", "orphaned"].includes(quick.status);
    const head = h(
      "div",
      { class: "quick-head" },
      h("span", { class: "metric-icon quick-icon" }, icon("bolt")),
      h("div", { class: "quick-text" }, h("h3", { class: "quick-title" }, t("quick.title")), h("p", { class: "hint" }, t("quick.lead"))),
    );
    let body;
    if (quick.status === "review") {
      body = h(
        "div",
        { class: "quick-review" },
        h("strong", null, t("quick.review.title", { drive: `${quick.volume}:` })),
        h(
          "ul",
          { class: "bullets" },
          [t("quick.review.helper"), t("quick.review.reads"), t("quick.review.changes"), t("quick.review.consent"), t("quick.review.budget", { time: duration(quick.request?.budget_ms ?? BUDGET_MS) }), t("quick.review.local")].map((x) => h("li", null, x)),
        ),
        h("div", { class: "row" }, button(t("common.cancel"), { onClick: cancel }), button(t("quick.approve"), { variant: "primary", iconName: "shield", onClick: approve })),
      );
    } else if (quick.status === "running" || quick.status === "orphaned" || quick.status === "cancelling") {
      body = h(
        "div",
        { class: "quick-running" },
        h("div", { class: "row wrap", style: { "justify-content": "space-between" } }, h("strong", null, t(quick.status === "cancelling" ? "quick.cancelling" : "quick.running", { drive: `${quick.volume}:` })), quick.status === "cancelling" ? null : button(t("storage.cancel"), { variant: "danger", iconName: "stop", onClick: cancel })),
        h("div", { class: "indeterminate", style: { margin: "10px 0" } }),
        h("span", { class: "hint" }, quick.status === "running" && Date.now() - quick.startedAt >= 1000 ? t("quick.elapsed", { time: duration(Math.floor((Date.now() - quick.startedAt) / 1000) * 1000) }) : t("quick.consentHint")),
      );
    } else if (!volumes().length) {
      body = h("p", { class: "hint" }, t("quick.noDrive"));
    } else {
      const letters = volumes();
      if (!letters.includes(volume)) volume = letters[0];
      // The deeper checks card holds the single elevated slot: wait for it instead of failing.
      const blocked = slotHeldElsewhere("quick");
      body = h(
        "div",
        { class: "row wrap" },
        h(
          "label",
          { class: "quick-drive" },
          h("span", { class: "hint" }, t("quick.drive")),
          h(
            "select",
            { class: "select", value: volume, onchange: (e) => (volume = e.target.value), disabled: busy },
            letters.map((l) => h("option", { value: l, selected: l === volume ? true : null }, `${l}:`)),
          ),
        ),
        button(t(quick.status === "preparing" ? "quick.preparing" : "quick.prepare"), { variant: quick.result ? "" : "primary", iconName: "bolt", disabled: busy || blocked, onClick: () => prepare(volume) }),
        blocked && !busy ? h("span", { class: "hint" }, t("admin.slotBusy")) : null,
        quick.status === "failed" ? h("div", { class: "callout bad", style: { "flex-basis": "100%" } }, icon("alert"), h("div", null, h("strong", null, t("quick.failed")), h("span", { title: failureDetail(quick.error) }, failureText(quick.error)))) : null,
        quick.status === "cancelled" ? h("span", { class: "hint" }, t("quick.cancelled")) : null,
      );
    }
    fill(host, head, body);
  };
  const resultHost = h("div");
  const renderAll = () => {
    render();
    fill(resultHost, quick.status === "done" && quick.result ? resultView(renderAll) : null);
  };
  listeners.add(renderAll);
  const stopSlot = onSlotChange(renderAll);
  // The drive list arrives with the first measurements; redraw only when it changes, so an open
  // drive menu is not rebuilt every five seconds.
  let letters = volumes().join();
  const stopLive = subscribe((topic) => {
    if (topic !== "live" && topic !== "info") return;
    if (volumes().join() === letters) return;
    letters = volumes().join();
    render();
  });
  renderAll();
  recoverQuickScan();
  return {
    el: h("div", null, host, resultHost),
    dispose: () => {
      listeners.delete(renderAll);
      stopSlot();
      stopLive();
    },
  };
}
