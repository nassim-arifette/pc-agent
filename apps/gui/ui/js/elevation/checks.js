// One local review and one UAC prompt for a closed read-only batch. Targets are resolved by the
// host; the model proposes only families. Historical measurements never authorize another run.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { cloudReady } from "../chat/composer.js";
import { runSelection } from "../selection/actions.js";
import { keepFocus } from "../actionbar.js";
import { badge, button, toast } from "../ui.js";
import { bytes, clock, duration, number, percent } from "../format.js";
import { holdSlot, onSlotChange, slotHeldElsewhere } from "./slot.js";
import { failureDetail, failureText } from "../failure.js";
import { capable } from "../capabilities.js";
import { subscribe } from "../store.js";

export const CHECKS = ["system_caches", "shadow_storage", "disk_reliability", "battery_energy", "mft_inventory", "process_images"];
const ICONS = { system_caches: "folder", shadow_storage: "history", disk_reliability: "disk", battery_energy: "battery", mft_inventory: "drive", process_images: "cpu" };
/** What a check needs beyond the helper (capabilities.js); without it the row says why. */
const NEEDS = { battery_energy: "battery", mft_inventory: "ntfs_volumes" };
/** Maximum read time per section; consent has its own separate native deadline. */
export const BUDGET_MS = 120000;
const CHAT_RETENTION_MS = 24 * 60 * 60 * 1000;
/** Groups shown before the rest folds away (a battery history can list up to 32 apps). */
const VISIBLE_GROUPS = 6;
const UUID = /-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

// Kept across visits to the Storage view: a running check outlives the page that started it.
// `results` keeps the last result of each check; `shown` is the one displayed.
const admin = { status: "idle", kind: null, request: null, options: null, selected: new Set(), error: null, startedAt: null, results: {}, shown: null };
const listeners = new Set();
let ticker = null;
let poller = null;

function changed() {
  for (const listener of [...listeners]) listener();
}

function publishSlot() {
  holdSlot("admin", ["preparing", "review", "running", "cancelling", "orphaned"].includes(admin.status) || poller != null);
}

function set(patch) {
  Object.assign(admin, patch);
  window.clearInterval(ticker);
  ticker = admin.status === "running" ? window.setInterval(changed, 1000) : null;
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

async function prepare(kinds = CHECKS) {
  const requested = kinds === "deep_analysis" ? CHECKS : Array.isArray(kinds) ? kinds : [kinds];
  set({ status: "preparing", kind: "deep_analysis", error: null, request: null });
  try {
    const options = await call("elevation_options");
    const selected = new Set(options.tasks.flatMap((task, index) => requested.includes(task.kind) ? [index] : []));
    set({ status: "review", options, selected });
  } catch (error) {
    set({ status: "failed", error });
    waitForFreeSlot(() => admin.status === "failed" && changed());
  }
}

async function approve() {
  const tasks = (admin.options?.tasks || []).filter((_, i) => admin.selected.has(i));
  if (admin.status !== "review" || !tasks.length) return;
  const startedAt = Date.now();
  const old = admin.request;
  set({ status: "preparing", startedAt, error: null });
  try {
    if (old) await call("elevation_cancel", { requestId: old.request_id });
    const request = await call("elevation_prepare", { task: { kind: "deep_analysis", tasks }, budgetMs: tasks.length * (BUDGET_MS + 5000) });
    set({ status: "running", request });
    const result = await call("elevation_run", { requestId: request.request_id });
    // A result that arrives after Stop was clicked is still a complete measurement: show it.
    window.clearInterval(poller);
    poller = null;
    const id = result.stored_id || `memory-${startedAt}`;
    const entry = { kind: "deep_analysis", result, startedAt, finishedAt: Date.now() };
    set({ status: "done", request: null, results: { ...admin.results, [id]: entry }, shown: id });
    await restoreResults();
  } catch (error) {
    // After Stop, the cancellation flow reports the outcome once the slot is free.
    if (admin.status === "cancelling") return;
    // An error does not prove the slot is free: wait for elevation_status before another try.
    set({ status: "failed", error, request: null });
    waitForFreeSlot(() => changed());
  }
}

async function cancel() {
  const request = admin.request;
  if (!request) {
    set({ status: "idle" });
    return;
  }
  const wasRunning = admin.status === "running";
  set({ status: "cancelling" });
  try {
    await call("elevation_cancel", { requestId: request.request_id });
  } catch (error) {
    toast(failureText(error), "bad");
  }
  waitForFreeSlot(() => {
    if (admin.status === "cancelling") set({ status: wasRunning ? "cancelled" : "idle", request: null });
  });
}

async function restoreResults() {
  try {
    const records = await call("elevation_results");
    const results = {};
    for (const record of records) {
      const result = { ...record.inspection, ...record.local_details, stored_id: record.id };
      results[record.id] = { kind: record.task.kind, result, startedAt: result.window?.started_at_unix_ms ?? result.measured_at_unix_ms, finishedAt: result.measured_at_unix_ms };
    }
    // A disk failure must not hide the in-memory result or its persistence warning.
    for (const [id, entry] of Object.entries(admin.results)) {
      if (id.startsWith("memory-")) results[id] = entry;
    }
    const shown = results[admin.shown] ? admin.shown : Object.keys(results).sort((a, b) => results[b].finishedAt - results[a].finishedAt)[0] ?? null;
    set({ results, shown });
  } catch {
    // The last successful in-memory measurement remains viewable if history is unavailable.
  }
}

async function selectResult(entry, intent, question) {
  try {
    const reference = await call("elevation_select", { inspectionId: entry.result.stored_id });
    const title = t(`admin.check.${entry.kind}`);
    await runSelection({ reference, label: title, subject: title }, intent, { question });
  } catch (error) {
    toast(failureText(error), "bad");
  }
}

/** Recovers a prepared or running check after the page was reloaded. */
export async function recoverAdminChecks() {
  await restoreResults();
  if (admin.status !== "idle") return;
  try {
    const current = await call("elevation_status");
    const kind = current?.task?.kind;
    if (!CHECKS.includes(kind) && kind !== "deep_analysis") return;
    const tasks = kind === "deep_analysis" ? current.task.tasks : [current.task];
    if (current.state === "awaiting_local_approval") set({ status: "review", kind, request: current, options: { tasks, unavailable: [] }, selected: new Set(tasks.map((_, i) => i)) });
    else if (current.state === "running" || current.state === "preparing") {
      // The result belongs to the page that started it; this page can only follow or cancel it.
      set({ status: "orphaned", kind, request: current });
      waitForFreeSlot(() => { set({ status: "idle", request: null }); restoreResults(); });
    } else if (current.state === "cancelling") {
      set({ status: "cancelling", kind, request: current });
      waitForFreeSlot(() => set({ status: "idle", request: null }));
    }
  } catch {
    // Without a status the card simply offers the checks.
  }
}

// ---------- Result formatting ----------

/** "disk_reliability_read_errors_total" → "Disk reliability read errors total". */
function humanize(name) {
  const text = String(name).replace(/[_-]+/g, " ").trim();
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** Translation for `key`, or `fallback` when the dictionaries have none. */
function label(key, fallback, vars) {
  const text = t(key, vars);
  return text === key ? fallback : text;
}

function metricLabel(metric) {
  return label(`admin.metric.${metric}`, humanize(metric));
}

/** A coverage scope: a task name, or a system cache category shared with the group headings. */
function scopeLabel(scope) {
  return label(`admin.scope.${scope}`, label(`admin.value.${scope}`, humanize(scope)));
}

/** Millijoules in the unit a person reads for a battery: Wh, then J, then mJ. */
function energy(millijoules) {
  if (millijoules === 0 || millijoules >= 36000) return `${number(millijoules / 3600000, 2)} Wh`;
  if (millijoules >= 1000) return `${number(millijoules / 1000, 1)} J`;
  return `${number(millijoules)} mJ`;
}

/** Milliseconds, with hours for long totals (summed battery row durations). */
function span(ms) {
  return ms >= 3600000 ? `${number(ms / 3600000, 1)} h` : duration(ms);
}

/** One observation's value as text, by its unit; null means the source did not report it. */
function formatValue(observation) {
  const { value, unit } = observation;
  if (value == null) return null;
  if (typeof value === "boolean") return t(value ? "admin.yes" : "admin.no");
  if (typeof value === "number") {
    switch (unit) {
      case "bytes":
        return bytes(value);
      case "count":
        return number(value);
      case "percent":
        return percent(value / 100);
      case "celsius":
        return `${number(value)} °C`;
      case "hours":
        return `${number(value)} h`;
      case "ms":
        return span(value);
      case "mJ":
        return energy(value);
      default:
        return unit ? `${number(value, 2)} ${unit}` : number(value, 2);
    }
  }
  if (unit === "provider_civil_time") {
    // "2026-10-03 21:14:05.000000000 (provider civil time; zone unknown)": the metric's label
    // already says the zone is unknown, so the date and minute are enough.
    const match = /^(\d{4}-\d{2}-\d{2}) (\d{2}:\d{2})/.exec(String(value));
    return match ? `${match[1]} ${match[2]}` : String(value);
  }
  if (unit === "category") return label(`admin.value.${value}`, humanize(value));
  return String(value);
}

/** Observations grouped by entity, in the order the helper reported them, with a heading each. */
function groups(result) {
  const byRef = new Map();
  for (const o of result.observations || []) {
    if (!byRef.has(o.entity_ref)) byRef.set(o.entity_ref, { kind: String(o.entity_ref).replace(UUID, ""), rows: [], heading: null, sortKey: null });
    const group = byRef.get(o.entity_ref);
    // A category names its group (system caches) rather than being a row of its own.
    if (o.unit === "category" && typeof o.value === "string") group.heading = formatValue(o);
    else group.rows.push(o);
    if (o.metric === "battery_app_energy_estimated" && typeof o.value === "number") group.sortKey = o.value;
  }
  const list = [...byRef.values()];
  // Battery apps by estimated energy, largest first; everything else keeps the helper's order.
  const apps = list.filter((g) => g.kind === "battery-energy-app").sort((a, b) => (b.sortKey ?? -1) - (a.sortKey ?? -1));
  const ordered = list.map((g) => (g.kind === "battery-energy-app" ? apps.shift() : g));
  const totals = {};
  for (const g of ordered) totals[g.kind] = (totals[g.kind] || 0) + 1;
  const seen = {};
  for (const g of ordered) {
    seen[g.kind] = (seen[g.kind] || 0) + 1;
    if (g.heading) continue;
    // "Disk 2" when there are several; just "Disk" when there is one.
    const n = totals[g.kind] > 1 ? number(seen[g.kind]) : "";
    g.heading = label(`admin.entity.${g.kind}`, humanize(g.kind), { n }).replace(/\s+$/, "");
  }
  return ordered;
}

function groupCard(group) {
  return h(
    "div",
    { class: "admin-group" },
    h("strong", { class: "admin-group-title" }, group.heading),
    h(
      "dl",
      { class: "admin-kv" },
      group.rows.map((o) => {
        const text = formatValue(o);
        const partial = text != null && o.status !== "ok";
        return [
          h("dt", null, metricLabel(o.metric)),
          h(
            "dd",
            { class: text == null ? "admin-missing" : null },
            text ?? t("admin.notReported"),
            partial ? h("span", { class: "partial-mark", title: t("admin.valuePartial"), "aria-label": t("admin.valuePartial") }, icon("info")) : null,
          ),
        ];
      }),
    ),
  );
}

const STATUS_TONE = { ok: "good", partial: "warn", cancelled: "warn", unsupported: "" };

function resultView(kind, entry, { question, askLabel } = {}) {
  const { result, startedAt, finishedAt } = entry;
  const title = t(`admin.check.${kind}`);
  const list = groups(result);
  const notes = (result.coverage || []).filter((c) => c.status !== "ok");
  const age = Date.now() - result.measured_at_unix_ms;
  const fresh = age >= 0 && age < CHAT_RETENTION_MS;
  const selectable = fresh && Boolean(result.stored_id) && !busySelection();
  const busy = ["preparing", "review", "running", "cancelling", "orphaned"].includes(admin.status) || slotHeldElsewhere("admin");
  const status = result.status || "partial";
  return h(
    "section",
    { class: "card card-pad section admin-result" },
    h(
      "div",
      { class: "row wrap", style: { "justify-content": "space-between", "margin-bottom": "12px" } },
      h(
        "div",
        { class: "admin-text" },
        h("h3", { class: "admin-title" }, t("admin.result.title", { check: title })),
        h("span", { class: "hint" }, t("admin.result.when", { at: clock(result.measured_at_unix_ms || finishedAt), took: duration(Math.max(0, finishedAt - startedAt)) })),
      ),
      h(
        "div",
        { class: "row wrap" },
        badge(label(`admin.status.${status}`, humanize(status)), STATUS_TONE[status] ?? "bad", status === "ok" ? "check" : "info"),
        button(t("admin.again"), { iconName: "refresh", size: "sm", disabled: busy, onClick: () => prepare(kind) }),
      ),
    ),
    h(
      "div",
      { class: ["callout", fresh ? "info" : "", "admin-chat"] },
      icon(fresh ? "chat" : "clock"),
      h("div", null, h("span", null, t(fresh ? "admin.result.chat" : "admin.result.chatExpired"))),
      fresh
        ? cloudReady()
          ? button(askLabel ?? t("admin.ask"), { iconName: "sparkle", size: "sm", disabled: !selectable, onClick: () => selectResult(entry, "investigate", question ?? t("admin.askQuestion", { check: title })) })
          : button(askLabel ?? t("admin.ask"), { iconName: "sparkle", size: "sm", disabled: true, title: t("mode.cloud.setup") })
        : null,
    ),
    result.persistence_error ? h("p", { class: "callout warn" }, failureText(result.persistence_error)) : null,
    h("div", { class: "row wrap" },
      button(t("admin.explain"), { disabled: !selectable, onClick: () => selectResult(entry, "explain") }),
      button(t("admin.investigate"), { disabled: !selectable, onClick: () => selectResult(entry, "investigate") })),
    result.sections?.length ? h("div", { class: "admin-sections" }, result.sections.map((section) =>
      h("details", { class: "more admin-more", open: section.status !== "ok" },
        h("summary", null, taskLabel(section.task), " · ", label(`admin.status.${section.status}`, humanize(section.status))),
        h("p", { class: "hint" }, t("admin.result.when", { at: clock(section.measured_at_unix_ms), took: duration(section.window?.duration_ms ?? 0) })),
        h("div", { class: "admin-groups" }, groups(section).map(groupCard)),
        (section.coverage || []).filter((c) => c.status !== "ok").map((c) => h("p", { class: "hint" }, c.detail))))) : null,
    !result.sections?.length && list.length
      ? [
          h("div", { class: "admin-groups" }, list.slice(0, VISIBLE_GROUPS).map(groupCard)),
          list.length > VISIBLE_GROUPS
            ? h(
                "details",
                { class: "more admin-more" },
                h("summary", null, icon("chevron"), t("admin.moreGroups", { count: number(list.length - VISIBLE_GROUPS) })),
                h("div", { class: "admin-groups" }, list.slice(VISIBLE_GROUPS).map(groupCard)),
              )
            : null,
        ]
      : !result.sections?.length ? h("p", { class: "muted" }, t("admin.empty")) : null,
    notes.length
      ? h(
          "details",
          { class: "more admin-more" },
          h("summary", null, icon("chevron"), t("admin.limits")),
          h(
            "div",
            null,
            h(
              "ul",
              { class: "bullets" },
              notes.map((c) =>
                h(
                  "li",
                  null,
                  h("strong", null, `${scopeLabel(c.scope)} · ${label(`admin.coverage.${c.status}`, humanize(c.status))}`),
                  h("span", { class: "admin-note" }, c.detail),
                ),
              ),
            ),
          ),
        )
      : null,
  );
}

// ---------- Card ----------

function busySelection() {
  return ["preparing", "review", "running", "cancelling", "orphaned"].includes(admin.status);
}

function taskLabel(task) {
  return `${t(`admin.check.${task.kind}`)}${task.volume ? ` · ${task.volume}:\\` : ""}`;
}

function reviewRow(task, index) {
  return h("label", { class: "admin-check" },
    h("input", { type: "checkbox", checked: admin.selected.has(index), "data-key": `admin-section-${index}`, onchange: (event) => {
      if (event.target.checked) admin.selected.add(index);
      else admin.selected.delete(index);
      changed();
    } }),
    h("div", { class: "admin-check-text" },
      h("strong", null, taskLabel(task)),
      h("span", { class: "hint" }, t(`admin.review.reads.${task.kind}`)),
      task.targets ? h("span", { class: "hint" }, t("admin.review.processTargets", { pids: task.targets.map((target) => target.pid).join(", ") })) : null));
}

function checkRow(kind, { busy, blocked }) {
  const preparing = admin.status === "preparing" && admin.kind === kind;
  const missing = NEEDS[kind] && !capable(NEEDS[kind]) ? NEEDS[kind] : null;
  return h(
    "div",
    { class: "admin-check" },
    h("span", { class: "metric-icon" }, icon(ICONS[kind])),
    h("div", { class: "admin-check-text" }, h("strong", null, t(`admin.check.${kind}`)), h("span", { class: "hint" }, missing ? t(`admin.unavailable.${missing}`) : t(`admin.lead.${kind}`))),
    h(
      "div",
      { class: "admin-check-actions" },
      button(t(preparing ? "admin.preparing" : "admin.prepare"), { size: "sm", disabled: busy || blocked || Boolean(missing), title: missing ? t(`admin.unavailable.${missing}`) : t("admin.prepareHint"), onClick: () => prepare(kind) }),
    ),
  );
}

/**
 * The deeper checks card of the Storage view; returns its element and a cleanup. The chat shows
 * the same card under an answer that lacked administrator readings, limited to the checks that
 * turn lacked (`kinds`), with its own title, lead and "ask again" action. Both cards follow the
 * same single flow: a check prepared from one is visible, and cancellable, in the other.
 */
export function adminChecksCard({ kinds = CHECKS, title, lead, question, askLabel } = {}) {
  const offered = CHECKS.filter((kind) => kinds.includes(kind));
  const host = h("section", { class: "card card-pad section admin-card" });
  const resultHost = h("div");
  const render = () => {
    const busy = ["preparing", "running", "cancelling", "orphaned"].includes(admin.status);
    const blocked = slotHeldElsewhere("admin") || !capable("elevated_helper");
    const check = admin.kind ? t(`admin.check.${admin.kind}`) : "";
    const head = h(
      "div",
      { class: "admin-head" },
      h("span", { class: "metric-icon admin-icon" }, icon("shield")),
      h("div", { class: "admin-text" }, h("h3", { class: "admin-title" }, title ?? t("admin.title")), h("p", { class: "hint" }, lead ?? t("admin.lead"))),
    );
    let body;
    if (admin.status === "review") {
      body = h(
        "div",
        { class: "admin-review" },
        h("strong", null, t("admin.review.title", { check })),
        h(
          "ul",
          { class: "bullets" },
          [
            t("admin.review.helper"),
            t("admin.review.changes"),
            t("admin.review.consent"),
            t("admin.review.budget", { time: duration(BUDGET_MS) }),
            t("admin.review.local"),
          ].map((x) => h("li", null, x)),
        ),
        h("div", { class: "admin-checks" }, (admin.options?.tasks || []).map(reviewRow)),
        (admin.options?.unavailable || []).map((kind) => h("p", { class: "hint" }, t("admin.review.unavailable", { check: t(`admin.check.${kind}`) }))),
        admin.options?.process_targets_omitted ? h("p", { class: "hint" }, t("admin.review.omitted", { count: number(admin.options.process_targets_omitted) })) : null,
        h("div", { class: "row" }, button(t("common.cancel"), { onClick: cancel }), button(t("admin.approve"), { variant: "primary", iconName: "shield", disabled: !admin.selected.size || blocked, onClick: approve })),
      );
    } else if (admin.status === "running" || admin.status === "orphaned" || admin.status === "cancelling") {
      const cancelling = admin.status === "cancelling";
      body = h(
        "div",
        { class: "admin-running" },
        h(
          "div",
          { class: "row wrap", style: { "justify-content": "space-between" } },
          h("strong", null, t(cancelling ? "admin.cancelling" : "admin.running", { check })),
          cancelling ? null : button(t("storage.cancel"), { variant: "danger", iconName: "stop", onClick: cancel }),
        ),
        h("div", { class: "indeterminate", style: { margin: "10px 0" } }),
        h("span", { class: "hint" }, admin.status === "running" && Date.now() - admin.startedAt >= 1000 ? t("admin.elapsed", { time: duration(Math.floor((Date.now() - admin.startedAt) / 1000) * 1000) }) : t("admin.consentHint")),
      );
    } else {
      body = h(
        "div",
        { class: "admin-list" },
        button(t("admin.deep"), { variant: "primary", iconName: "shield", disabled: busy || blocked, onClick: () => prepare(offered) }),
        h("div", { class: "admin-checks" }, offered.map((kind) => checkRow(kind, { busy, blocked }))),
        blocked && !busy ? h("p", { class: "hint admin-blocked" }, icon("clock"), t("admin.slotBusy")) : null,
        admin.status === "failed"
          ? h("div", { class: "callout bad" }, icon("alert"), h("div", null, h("strong", null, t("admin.failed", { check })), h("span", { title: failureDetail(admin.error) }, failureText(admin.error))))
          : null,
        admin.status === "cancelled" ? h("p", { class: "hint" }, t("admin.cancelled")) : null,
      );
    }
    fill(host, head, body, h("div", { class: "row wrap admin-history" },
      Object.entries(admin.results).sort(([, a], [, b]) => b.finishedAt - a.finishedAt).map(([id, entry]) =>
        button(`${t(`admin.check.${entry.kind}`)} · ${clock(entry.finishedAt)}`, { size: "sm", onClick: () => set({ shown: id }) }))));
  };
  let drawn = null;
  let expiry = null;
  const renderAll = () => {
    keepFocus(host, render);
    // The chat card shows only a result of a check it offered; the Storage card shows any.
    const entry = admin.shown ? admin.results[admin.shown] : null;
    // Rebuild the result only when what it shows changes: the running check ticks every second,
    // and a rebuild would close the folded groups and limits the person opened.
    const busy = ["preparing", "review", "running", "cancelling", "orphaned"].includes(admin.status) || slotHeldElsewhere("admin");
    const fresh = entry ? Date.now() - entry.finishedAt < CHAT_RETENTION_MS : false;
    const key = entry ? [admin.shown, entry.finishedAt, busy, fresh, cloudReady()].join("|") : null;
    if (key !== drawn) {
      drawn = key;
      fill(resultHost, entry ? resultView(entry.kind, entry, { question, askLabel }) : null);
    }
    // Redraw when the shown result stops being readable by the chat.
    window.clearTimeout(expiry);
    const left = entry ? entry.finishedAt + CHAT_RETENTION_MS - Date.now() : 0;
    expiry = left > 0 ? window.setTimeout(renderAll, left + 50) : null;
  };
  listeners.add(renderAll);
  const stopSlot = onSlotChange(renderAll);
  // The battery probe answers with the first measurements (capabilities.js).
  const stopInfo = subscribe((topic) => topic === "info" && renderAll());
  renderAll();
  recoverAdminChecks();
  return {
    el: h("div", null, host, resultHost),
    dispose: () => {
      listeners.delete(renderAll);
      stopSlot();
      stopInfo();
      window.clearTimeout(expiry);
    },
  };
}
