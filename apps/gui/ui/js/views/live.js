// Live PC as a full view: resource cards (each system battery included) and applications on the
// left, the inspector of the selected item on the right. Selecting an item shows the action bar under it (Explain,
// Investigate). A selection names a kind and a key; its measurements are always the latest ones,
// and the Rust side freezes them when a preview is prepared, so what the inspector shows is what
// the preview starts from.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { applicationName, processName, slowdownText } from "../phrases.js";
import { state, subscribe, setLivePaused, watchActivity, pokeActivity } from "../store.js";
import { almostFull, batteryInspector, driveName, gpuName, measureText, notReported, sparkline } from "../live.js";
import { batteryItems } from "../battery.js";
import { ago, basisPoints, bytes, number, percent } from "../format.js";
import { runSelection } from "../select.js";
import { actionBar, selectionKeys } from "../actionbar.js";
import { currentRoute } from "../router.js";

// Kept across visits so coming back to the view shows the same selection.
let selected = null;
let sort = "memory";

const RESOURCE_ICONS = { memory: "memory", cpu: "cpu", disk: "drive", gpu: "gpu", battery: "battery" };

/**
 * The resource cards: memory, processor, each fixed drive, each graphics card and each system
 * battery. A battery card carries its own reference (the battery read's source) and read time.
 */
function resources(dash, history) {
  const cards = [];
  const m = dash.memory;
  const total = m.total.value;
  const available = m.available.value;
  cards.push({
    key: "memory",
    resource: "memory",
    label: t("metric.memory"),
    value: m.used_ratio != null ? percent(m.used_ratio) : null,
    detail: total != null && available != null ? t("live.memory.detail", { used: bytes(total - available), total: bytes(total) }) : "",
    warn: m.used_ratio >= 0.9,
    history: history.map((d) => d.memory.used_ratio),
  });
  const busy = dash.cpu.busy;
  cards.push({
    key: "cpu",
    resource: "cpu",
    label: t("metric.cpu"),
    // A rate needs two samples: the first one says it is still measuring.
    value: busy.value != null ? basisPoints(busy.value) : busy.status === "partial" ? t("metric.measuring") : null,
    detail: dash.cpu.busiest_core.value != null ? t("metric.cpu.core", { value: basisPoints(dash.cpu.busiest_core.value) }) : "",
    warn: busy.value >= 9000,
    history: history.map((d) => (d.cpu.busy.value != null ? d.cpu.busy.value / 10000 : null)),
  });
  for (const v of dash.volumes || []) {
    cards.push({
      key: `disk:${v.root}`,
      resource: "disk",
      label: t("live.drive", { drive: driveName(v.root) }),
      value: percent(1 - v.free_ratio, 1),
      detail: t("live.drive.detail", { free: bytes(v.free_bytes), total: bytes(v.total_bytes) }),
      warn: almostFull(v),
      history: history.map((d) => {
        const same = (d.volumes || []).find((x) => x.root === v.root);
        return same ? 1 - same.free_ratio : null;
      }),
      volume: v,
    });
  }
  for (const [index, g] of dash.gpus.entries()) {
    const load = g.utilization.value;
    cards.push({
      key: `gpu:${index}`,
      resource: "gpu",
      label: gpuName(g.name),
      value: load != null ? basisPoints(load, 0) : g.temperature.value != null ? `${g.temperature.value} °C` : null,
      detail: [g.memory_used.value != null && g.memory_total.value != null ? t("metric.gpu.memory", { used: bytes(g.memory_used.value), total: bytes(g.memory_total.value) }) : null, g.temperature.value != null ? `${g.temperature.value} °C` : null].filter(Boolean).join(" · "),
      warn: Boolean(g.slowdown.value),
      history: history.map((d) => (d.gpus[index]?.utilization.value != null ? d.gpus[index].utilization.value / 10000 : null)),
      gpu: g,
    });
  }
  cards.push(...batteryItems(dash, history));
  return cards;
}

/** The member a selected application is explained through: its largest measured process. */
function mainMember(app) {
  return [...(app.members || [])].sort((a, b) => (b.working_set.value ?? -1) - (a.working_set.value ?? -1))[0] || null;
}

function rankValue(app) {
  return (sort === "cpu" ? app.cpu.value : app.working_set.value) ?? -1;
}

function findApp(ref) {
  return state.live.activity?.applications.find((a) => a.entity_ref === ref) || null;
}

/** Resolves the selection against the latest measurements: what to show and what to send. */
function resolve(cards) {
  if (!selected) return null;
  const dash = state.live.dash;
  const activity = state.live.activity;
  if (selected.kind === "resource") {
    const card = cards.find((c) => c.key === selected.key && c.selectable !== false);
    if (!card) return null;
    return {
      kind: "resource",
      card,
      label: card.label,
      measuredAt: card.measuredAt ?? dash.collected_at_unix_ms,
      // A battery has its own source; without one it never falls back to the dashboard's.
      reference: "reference" in card ? card.reference : dash.selection_source_id ? { source_id: dash.selection_source_id, target: { kind: "resource", resource: card.resource } } : null,
      sent: card.sent,
      subject: card.subject,
    };
  }
  const app = findApp(selected.app);
  if (!app) return null;
  const member = selected.kind === "process" ? app.members.find((m) => m.entity_ref === selected.key) : mainMember(app);
  if (selected.kind === "process" && !member) return null;
  const reference = member && activity.selection_source_id ? { source_id: activity.selection_source_id, target: { kind: "process", entity_ref: member.entity_ref } } : null;
  return {
    kind: selected.kind,
    app,
    member,
    label: selected.kind === "process" ? `${processName(member)} (PID ${member.pid})` : applicationName(app),
    name: selected.kind === "process" ? member.name : app.name,
    measuredAt: activity.collected_at_unix_ms,
    reference,
  };
}

function kv(rows) {
  return h(
    "dl",
    { class: "insp-kv" },
    rows.filter(Boolean).map(([label, value]) => [h("dt", null, label), h("dd", null, value ?? h("span", { class: "unknown" }, t("metric.notMeasured")))]),
  );
}

function topApp(by) {
  const apps = state.live.activity?.applications || [];
  const value = (a) => (by === "cpu" ? a.cpu.value : a.working_set.value) ?? -1;
  const top = [...apps].sort((a, b) => value(b) - value(a))[0];
  if (!top || value(top) < 0) return null;
  return `${applicationName(top)} · ${by === "cpu" ? basisPoints(top.cpu.value) : bytes(top.working_set.value)}`;
}

function resourceDetails(card) {
  const dash = state.live.dash;
  if (card.resource === "battery") return batteryInspector(card);
  if (card.resource === "memory") {
    const m = dash.memory;
    return kv([
      [t("inspect.used"), m.total.value != null && m.available.value != null ? t("live.memory.detail", { used: bytes(m.total.value - m.available.value), total: bytes(m.total.value) }) : null],
      [t("inspect.available"), m.available.value != null ? bytes(m.available.value) : null],
      [t("inspect.commit"), m.commit?.value != null && m.commit_limit?.value != null ? t("inspect.commit.value", { used: bytes(m.commit.value), limit: bytes(m.commit_limit.value) }) : null],
      [t("inspect.top.memory"), topApp("memory")],
    ]);
  }
  if (card.resource === "cpu") {
    return kv([
      [t("inspect.cpu.busy"), dash.cpu.busy.value != null ? basisPoints(dash.cpu.busy.value) : null],
      [t("inspect.cpu.core"), dash.cpu.busiest_core.value != null ? basisPoints(dash.cpu.busiest_core.value) : null],
      [t("inspect.top.cpu"), topApp("cpu")],
    ]);
  }
  if (card.resource === "disk") {
    // Latencies arrive in microseconds.
    const latency = (measure) => (measure?.value != null ? `${number(measure.value / 1000, 1)} ms` : null);
    return [
      kv([
        [t("inspect.disk.free", { drive: driveName(card.volume.root) }), t("live.drive.detail", { free: bytes(card.volume.free_bytes), total: bytes(card.volume.total_bytes) })],
        [t("inspect.disk.read"), latency(dash.disk.read_latency)],
        [t("inspect.disk.write"), latency(dash.disk.write_latency)],
        [t("inspect.disk.idle"), dash.disk.idle.value != null ? basisPoints(dash.disk.idle.value) : null],
      ]),
      h("p", { class: "note" }, t("inspect.disk.note")),
    ];
  }
  const g = card.gpu;
  return kv([
    [t("inspect.gpu.load"), g.utilization.value != null ? basisPoints(g.utilization.value) : null],
    [t("inspect.gpu.memory"), g.memory_used.value != null && g.memory_total.value != null ? t("metric.gpu.memory", { used: bytes(g.memory_used.value), total: bytes(g.memory_total.value) }) : null],
    [t("inspect.gpu.temp"), g.temperature.value != null ? `${g.temperature.value} °C` : null],
    [t("inspect.gpu.slowdown"), g.slowdown.value == null ? null : g.slowdown.value ? slowdownText(g.slowdown_codes) || (g.slowdown_reasons || []).join(", ") || null : t("inspect.gpu.none")],
  ]);
}

function appDetails(item) {
  const { app, member } = item;
  if (item.kind === "process") {
    return kv([
      [t("metric.cpu"), member.cpu.value != null ? basisPoints(member.cpu.value) : null],
      [t("metric.memory"), member.working_set.value != null ? bytes(member.working_set.value) : null],
    ]);
  }
  return [
    kv([
      [t("inspect.app.processes"), String(app.process_count)],
      [t("metric.cpu"), app.cpu.value != null ? basisPoints(app.cpu.value) : null],
      [t("metric.memory"), app.working_set.value != null ? bytes(app.working_set.value) : null],
      [t("inspect.app.private"), app.private_commit?.value != null ? bytes(app.private_commit.value) : null],
      app.working_set_increase?.value ? [t("live.apps.grew"), `+${bytes(app.working_set_increase.value)}`] : null,
      [t("live.apps.gpu"), app.gpu?.value != null ? basisPoints(app.gpu.value) : null],
    ]),
    app.services.length ? h("div", { class: "insp-tags" }, h("span", { class: "caption" }, t("live.apps.services")), app.services.map((sv) => h("span", { class: "tag", title: sv.name }, sv.display_name || sv.name))) : null,
    app.members.length
      ? h(
          "div",
          { class: "insp-members" },
          h("span", { class: "caption" }, t("live.apps.members")),
          app.members.map((m) =>
            h(
              "button",
              { type: "button", class: "insp-member", "data-key": `process:${m.entity_ref}`, onclick: () => select({ kind: "process", app: app.entity_ref, key: m.entity_ref }) },
              h("span", { class: "insp-member-name" }, `${processName(m)} · PID ${m.pid}`),
              h("span", null, measureText(m.cpu, "cpu")),
              h("span", null, measureText(m.working_set, "bytes")),
            ),
          ),
          app.process_count > app.members.length ? h("span", { class: "note" }, t("live.apps.others", { count: app.process_count - app.members.length })) : null,
        )
      : null,
    member ? h("p", { class: "note" }, t("inspect.app.target", { name: processName(member), pid: member.pid })) : h("p", { class: "note" }, t("inspect.app.none")),
  ];
}

/**
 * Explain or Investigate on the resolved item, through the exact preview (select.js). `sent` and
 * `subject` say when the item leaves with others (one battery target covers every shown battery).
 */
function act(item, intent) {
  return runSelection({ reference: item.reference, label: item.label, name: item.name, sent: item.sent, subject: item.subject }, intent);
}

/** The action bar of the resolved item, drawn under the card or application row it belongs to. */
function itemBar(item, key) {
  return actionBar({
    key,
    label: item.label,
    ready: Boolean(item.reference),
    unavailable: t("inspect.unavailable"),
    explain: () => act(item, "explain"),
    investigate: () => act(item, "investigate"),
  });
}

function inspector(item) {
  if (!item) {
    return h(
      "aside",
      { class: "insp insp-empty", "aria-label": t("inspect.label") },
      h("span", { class: "insp-empty-icon" }, icon("activity")),
      h("strong", null, t("inspect.empty.title")),
      h("p", null, t("inspect.empty.text")),
    );
  }
  const kindLabel = t(`inspect.kind.${item.kind}`);
  const ready = Boolean(item.reference);
  return h(
    "aside",
    { class: "insp", "aria-label": t("inspect.label") },
    h(
      "div",
      { class: "insp-head" },
      h("div", { class: "insp-titles" }, h("span", { class: "caption" }, kindLabel), h("h2", { class: "insp-title" }, item.label)),
      h("button", { type: "button", class: "icon-btn", title: t("inspect.clear"), "aria-label": t("inspect.clear"), onclick: () => select(null) }, icon("x")),
    ),
    h("p", { class: "insp-measured" }, t("inspect.measured", { ago: ago(item.measuredAt) })),
    h("div", { class: "insp-body" }, item.kind === "resource" ? resourceDetails(item.card) : appDetails(item)),
    // The action bar under the item is the main place for the actions; this is the way further.
    h(
      "div",
      { class: "insp-next" },
      h("p", null, ready ? t("inspect.note") : t("inspect.unavailable")),
      h(
        "button",
        { type: "button", class: "insp-more", disabled: !ready, "data-key": "insp:investigate", onclick: () => act(item, "investigate") },
        icon("search"),
        t("inspect.more"),
      ),
    ),
  );
}

/** A resource that can't be selected (a battery Windows lists but did not read): it says why. */
function staticCard(c) {
  return h(
    "div",
    { class: "livepage-card-slot" },
    h(
      "div",
      { class: ["livepage-card", "static"], "data-key": `resource:${c.key}` },
      h("span", { class: "livepage-card-head" }, h("span", { class: "livepage-card-icon" }, icon(RESOURCE_ICONS[c.resource])), h("span", { class: "livepage-card-label" }, c.label)),
      h("span", { class: "livepage-card-value" }, notReported()),
      h("span", { class: "livepage-card-detail" }, c.detail),
    ),
  );
}

function card(c, item) {
  if (c.selectable === false) return staticCard(c);
  const pressed = selected?.kind === "resource" && selected.key === c.key;
  const bar = pressed && item ? itemBar(item, `resource:${c.key}`) : null;
  return h(
    "div",
    { class: "actbar-anchor livepage-card-slot" },
    h(
      "button",
      {
        type: "button",
        class: ["livepage-card", c.warn ? "warn" : "", pressed ? "selected" : ""],
        "aria-pressed": String(pressed),
        "aria-keyshortcuts": bar && item.reference ? "E Enter" : null,
        "data-key": `resource:${c.key}`,
        "data-actbar-item": Boolean(bar),
        onclick: () => select(pressed ? null : { kind: "resource", key: c.key }),
      },
      h("span", { class: "livepage-card-head" }, h("span", { class: "livepage-card-icon" }, icon(RESOURCE_ICONS[c.resource])), h("span", { class: "livepage-card-label" }, c.label)),
      h("span", { class: "livepage-card-value" }, c.value ?? (c.resource === "battery" ? notReported() : t("metric.notMeasured"))),
      h("span", { class: "livepage-card-detail" }, c.detail),
      sparkline(c.history, c.warn ? "warn" : ""),
    ),
    bar,
  );
}

function applications(item) {
  const activity = state.live.activity;
  const head = h(
    "div",
    { class: "livepage-apps-head" },
    h("h2", { class: "caption" }, t("live.apps")),
    h("span", { class: "spacer" }),
    h(
      "div",
      { class: "mini-tabs", role: "group", "aria-label": t("live.apps.sort") },
      ["memory", "cpu"].map((key) =>
        h("button", { type: "button", "aria-pressed": String(sort === key), onclick: () => ((sort = key), render()) }, t(`live.apps.by.${key}`)),
      ),
    ),
  );
  if (!activity) return h("section", { class: "livepage-apps" }, head, h("div", { class: "skeleton", style: { height: "160px" } }));
  const apps = [...activity.applications].sort((a, b) => rankValue(b) - rankValue(a)).slice(0, 12);
  const max = Math.max(1, ...apps.map(rankValue));
  return h(
    "section",
    { class: "livepage-apps" },
    head,
    h(
      "div",
      { class: "livepage-app-list" },
      apps.map((app) => {
        const pressed = selected && selected.kind !== "resource" && selected.app === app.entity_ref;
        // A process chosen in the inspector keeps its application's row: the bar acts on it.
        const bar = pressed && item ? itemBar(item, `${selected.kind}:${selected.key ?? app.entity_ref}`) : null;
        return h(
          "div",
          { class: "actbar-anchor livepage-app-slot" },
          h(
            "button",
            {
              type: "button",
              class: ["livepage-app", pressed ? "selected" : ""],
              "aria-pressed": String(pressed),
              "aria-keyshortcuts": bar && item.reference ? "E Enter" : null,
              "data-key": `app:${app.entity_ref}`,
              "data-actbar-item": Boolean(bar),
              onclick: () => select(pressed && selected.kind === "app" ? null : { kind: "app", app: app.entity_ref }),
            },
            h("span", { class: "livepage-app-name", title: applicationName(app) }, applicationName(app)),
            h("span", { class: "livepage-app-count" }, t(app.process_count === 1 ? "live.apps.one" : "live.apps.many", { count: app.process_count })),
            h("span", { class: "livepage-app-value" }, measureText(app.cpu, "cpu")),
            h("span", { class: "livepage-app-value" }, measureText(app.working_set, "bytes")),
            h("span", { class: "meter" }, h("span", { style: { width: `${(Math.max(0, rankValue(app)) / max) * 100}%` } })),
          ),
          bar,
        );
      }),
    ),
    activity.applications_total > apps.length ? h("p", { class: "note" }, t("live.apps.total", { shown: apps.length, total: activity.applications_total })) : null,
    h("p", { class: "note" }, t("live.apps.note")),
  );
}

let host = null;

function select(next) {
  selected = next;
  render();
}

function render() {
  if (!host) return;
  // Measurements redraw the view every few seconds: keep keyboard focus on the same item.
  const focusedKey = document.activeElement?.closest?.("[data-key]")?.dataset.key;
  const live = state.live;
  const dash = live.dash;
  const status = h(
    "div",
    { class: "livepage-status" },
    h("span", { class: ["dot", live.paused ? "neutral" : "live"] }),
    h("span", null, live.paused ? t("live.paused") : dash ? `${t("live.updated", { ago: ago(dash.collected_at_unix_ms) })} · ${t("live.every")}` : t("health.measuring")),
    h("button", { type: "button", class: "icon-btn", title: t(live.paused ? "live.resume" : "live.pause"), "aria-label": t(live.paused ? "live.resume" : "live.pause"), onclick: () => setLivePaused(!live.paused) }, icon(live.paused ? "play" : "pause")),
  );
  const head = h("div", { class: "page-head" }, h("div", null, h("h1", { class: "page-title" }, t("livepage.title")), h("p", { class: "page-sub" }, t("livepage.sub"))), status);
  if (!dash) {
    fill(host, head, live.error ? h("p", { class: "note" }, t("live.error", { message: live.error })) : h("div", { class: "skeleton", style: { height: "220px" } }));
    return;
  }
  const cards = resources(dash, live.history);
  const item = resolve(cards);
  const hidden = dash.process_coverage.permission_denied;
  fill(
    host,
    head,
    h(
      "div",
      { class: ["livepage", state.panel.open && state.panel.tab === "selection" && "with-summary"] },
      h(
        "div",
        { class: "livepage-main", "data-actbar-bounds": true },
        h("h2", { class: "caption" }, t("livepage.resources")),
        h("div", { class: "livepage-cards" }, cards.map((c) => card(c, item))),
        applications(item),
        hidden ? h("p", { class: "note" }, t("procs.hidden", { count: hidden })) : null,
      ),
      state.panel.open && state.panel.tab === "selection" ? null : inspector(item),
    ),
  );
  if (focusedKey) host.querySelector(`[data-key="${CSS.escape(focusedKey)}"]`)?.focus({ preventScroll: true });
}

export function mount(page) {
  host = page;
  const stopWatching = watchActivity(() => currentRoute() === "live");
  // E explains and Enter investigates the selected item; Escape clears the selection.
  const stopKeys = selectionKeys({
    current: () => {
      const item = selected && state.live.dash ? resolve(resources(state.live.dash, state.live.history)) : null;
      return item && { ready: Boolean(item.reference), explain: () => act(item, "explain"), investigate: () => act(item, "investigate") };
    },
    clear: () => select(null),
  });
  render();
  pokeActivity();
  const unsubscribe = subscribe((topic) => {
    if (topic === "live" || topic === "panel") render();
  });
  return () => {
    host = null;
    stopWatching();
    unsubscribe();
    stopKeys();
  };
}
