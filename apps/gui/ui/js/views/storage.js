// Storage: pick or drop a folder, watch the scan, then browse the largest folders and files and
// select one to explain it or investigate (action bar); or analyse a whole drive quickly from its
// file system index (quickscan.js), or run one of the deeper read-only checks (adminchecks.js);
// both ask for administrator consent.
import { h, fill, announce } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call, onDrop } from "../api.js";
import { askWithAi, cloudReady } from "../ask.js";
import { findReference, runSelection } from "../select.js";
import { actionBar, keepFocus, selectionKeys } from "../actionbar.js";
import { state, subscribe, startScan, cancelScan, morePage } from "../store.js";
import { badge, bar, button, iconButton, toast } from "../ui.js";
import { bytes, date, duration, number, percent } from "../format.js";
import { quickScanCard } from "../quickscan.js";
import { adminChecksCard } from "../adminchecks.js";
import { failureDetail, failureText } from "../failure.js";

const QUICK_ICONS = { downloads: "drop", desktop: "pc", documents: "file", temp: "clock", profile: "folder", system_drive: "drive" };
const REASONS = ["denied_dirs", "denied_files", "errors", "placeholders", "reparse_skipped", "other_volume_skipped", "changed_during_scan", "vanished"];

function folderName(path) {
  const trimmed = (path || "").replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}
function isDriveRoot(path) {
  return /^[A-Za-z]:\\?$/.test(path || "");
}

/**
 * The host-issued reference of a scan item. Each page of results is published as a selection
 * source that expires after a few minutes; when it is gone, reading the same page of the same
 * scan again (scan_page) publishes it anew. Null when the scan was replaced.
 */
async function storageReference(view, kind, item) {
  const target = { kind: "storage", scope_id: view.scope_id, item_ref: item.item_ref };
  const live = await findReference((x) => x.kind === "storage" && x.scope_id === target.scope_id && x.item_ref === target.item_ref);
  if (live) return live;
  try {
    const page = await call("scan_page", { scopeId: view.scope_id, kind, offset: item.page_offset ?? 0 });
    return page.selection_source_id && page.items.some((x) => x.item_ref === item.item_ref) ? { source_id: page.selection_source_id, target } : null;
  } catch {
    return null;
  }
}

/** What the action bar and the shortcuts do for a scanned folder or file. */
function storageItem(view, kind, item) {
  const run = async (intent) => {
    const reference = await storageReference(view, kind, item);
    if (!reference) {
      toast(t("storage.select.unavailable"), "bad");
      return;
    }
    // The name stays on this PC: the question and the preview name the item generically.
    const what = kind === "dirs" ? "folder" : "file";
    return runSelection({ reference, label: item.name, subject: t(`select.what.${what}`), sent: t(`select.sent.${what}`) }, intent);
  };
  return {
    key: `storage:${view.scope_id}:${item.item_ref}`,
    label: item.name,
    // The host publishes each page it returns; without a source the scan cannot be selected.
    ready: Boolean("selection_source_id" in item ? item.selection_source_id : view[kind].selection_source_id),
    unavailable: t("storage.select.unavailable"),
    explain: () => run("explain"),
    investigate: () => run("investigate"),
  };
}

export function mount(page) {
  const picker = h("section", { class: "dropzone" });
  const body = h("div", { class: "section" });
  let tab = "dirs";
  let quick = [];
  // The selected result: { scope, kind, ref }; cleared by another tab or another scan.
  let selected = null;
  // A redraw that only moves the selection keeps the size bars still.
  let still = false;

  /** The selected item of the current results, or null. */
  const selectedItem = () => {
    const view = state.scan.status === "done" ? state.scan.view : null;
    if (!view || !selected || selected.scope !== view.scope_id || selected.kind !== tab) return null;
    const item = view[tab].items.find((x) => x.item_ref === selected.ref);
    return item ? storageItem(view, tab, item) : null;
  };
  const select = (item) => {
    selected = item ? { scope: state.scan.view.scope_id, kind: tab, ref: item.item_ref } : null;
    still = true;
    try {
      render();
    } finally {
      still = false;
    }
  };

  const choose = async () => {
    try {
      const path = await call("pick_folder");
      if (path) startScan(path);
    } catch (error) {
      toast(failureText(error), "bad");
    }
  };

  const renderPicker = () => {
    const scanning = state.scan.status === "scanning" || state.scan.status === "starting";
    fill(
      picker,
      h("span", { class: "metric-icon", style: { width: "46px", height: "46px", "border-radius": "14px" } }, icon("folder")),
      h(
        "div",
        { style: { flex: "1", "min-width": "0" } },
        h("div", { class: "row wrap" }, button(t("storage.choose"), { variant: "primary", iconName: "folder", onClick: choose, disabled: scanning }), h("span", { class: "hint" }, t("storage.drop"))),
        quick.length
          ? h(
              "div",
              { class: "suggestions", style: { "margin-top": "12px" } },
              quick.map((q) =>
                h(
                  "button",
                  {
                    type: "button",
                    class: "chip",
                    disabled: scanning,
                    title: q.id === "system_drive" ? `${q.path} — ${t("storage.whole")}` : q.path,
                    onclick: () => startScan(q.path),
                  },
                  icon(QUICK_ICONS[q.id] || "folder"),
                  t(`storage.quick.${q.id}`),
                ),
              ),
            )
          : null,
      ),
    );
  };

  const reasons = (coverage) =>
    REASONS.filter((k) => coverage[k] > 0)
      .map((k) => t(`storage.reason.${k}`, { n: number(coverage[k]) }))
      .concat(coverage.truncated ? [t("storage.reason.truncated")] : []);

  const itemRow = (item, max, kind, view) => {
    const isSelected = Boolean(selected && selected.scope === view.scope_id && selected.kind === kind && selected.ref === item.item_ref);
    const actions = isSelected ? storageItem(view, kind, item) : null;
    return h(
      "div",
      { class: ["list-row", "item-row", "actbar-anchor", isSelected ? "selected" : ""] },
      // The name is the row's control; its hit area covers the row, the reveal button stays apart.
      h(
        "button",
        {
          type: "button",
          class: "list-name item-select",
          title: item.path,
          "aria-pressed": String(isSelected),
          "aria-keyshortcuts": actions?.ready ? "E Enter" : null,
          "data-key": `item:${item.item_ref}`,
          "data-actbar-item": isSelected,
          onclick: () => select(isSelected ? null : item),
        },
        icon(kind === "dirs" ? "folder" : "file"),
        h("span", { class: "text" }, h("strong", null, item.name), h("span", null, item.parent ? `…\\${item.parent}` : folderName(view.root))),
      ),
      bar(max ? item.size / max : 0, kind === "dirs" ? "" : "mint", { animate: !still }),
      h(
        "div",
        { class: "list-value" },
        bytes(item.size),
        item.status !== "ok" ? h("span", { class: "partial-mark", title: t("storage.itemPartial"), "aria-label": t("storage.itemPartial") }, icon("info")) : null,
      ),
      h("div", { class: "date" }, item.modified_unix_ms ? date(item.modified_unix_ms) : ""),
      iconButton("reveal", t("storage.reveal"), async () => {
        try {
          await call("reveal", { scopeId: state.scan.view.scope_id, itemRef: item.item_ref });
        } catch (error) {
          toast(failureText(error), "bad");
        }
      }),
      actions ? actionBar(actions) : null,
    );
  };

  const renderResults = (view) => {
    const scanned = view.scanned;
    const volume = view.volume;
    const used = volume.total_bytes && volume.free_bytes != null ? 1 - volume.free_bytes / volume.total_bytes : null;
    const why = reasons(view.coverage);
    const complete = view.status === "ok";
    const current = view[tab];
    const max = current.items.reduce((m, i) => Math.max(m, i.size), 0);
    const askButton = cloudReady()
      ? button(t("storage.ask"), { iconName: "sparkle", variant: "primary", onClick: () => askWithAi(t("storage.askQuestion", { folder: folderName(view.root) }), { followUp: false }) })
      : button(t("storage.ask"), { iconName: "sparkle", disabled: true, title: t("mode.cloud.setup") });
    return [
      h(
        "div",
        { class: "grid grid-metrics grid-stats" },
        h(
          "div",
          { class: "card stat" },
          h("span", { class: "stat-label" }, t("storage.size")),
          h("span", { class: "stat-value" }, view.scanned ? bytes(Math.max(view.scanned.allocated_bytes, view.scanned.logical_bytes)) : "—"),
          h("span", { class: "hint" }, scanned ? t("storage.files", { count: number(scanned.files) }) : ""),
        ),
        h(
          "div",
          { class: "card stat" },
          h("span", { class: "stat-label" }, t("storage.drive")),
          h("span", { class: "stat-value" }, used != null ? percent(used) : "—"),
          used != null ? bar(used, used > 0.95 ? "warn" : "", { animate: !still }) : null,
          h("span", { class: "hint" }, volume.free_bytes != null && volume.total_bytes != null ? t("storage.drive.free", { free: bytes(volume.free_bytes), total: bytes(volume.total_bytes) }) : ""),
        ),
        h(
          "div",
          { class: "card stat" },
          h("span", { class: "stat-label" }, t("storage.coverage")),
          h("span", null, view.coverage.cancelled ? badge(t("storage.cancelled"), "warn", "x") : badge(t(complete ? "storage.complete" : "storage.partial"), complete ? "good" : "warn", complete ? "check" : "info")),
          why.length
            ? h("details", { class: "more", style: { "margin-top": "6px", "padding-top": "6px" } }, h("summary", null, icon("chevron"), t("storage.why")), h("div", null, h("ul", { class: "bullets" }, why.map((x) => h("li", null, x)))))
            : null,
          h("span", { class: "hint" }, t("storage.took", { time: duration(view.duration_ms) })),
        ),
      ),
      h(
        "section",
        { class: "card card-pad section" },
        h(
          "div",
          { class: "row wrap", style: { "justify-content": "space-between", "margin-bottom": "6px" } },
          h(
            "div",
            { class: "tabs", role: "tablist" },
            ["dirs", "files"].map((k) =>
              h(
                "button",
                {
                  type: "button",
                  role: "tab",
                  "aria-selected": String(tab === k),
                  onclick: () => {
                    tab = k;
                    selected = null;
                    render();
                  },
                },
                t(`storage.tab.${k}`),
              ),
            ),
          ),
          askButton,
        ),
        current.items.length
          ? [h("p", { class: "hint storage-select-hint" }, t("storage.select.hint")), h("div", { class: "list" }, current.items.map((item) => itemRow(item, max, tab, view)))]
          : h("p", { class: "muted" }, t("storage.empty")),
        current.next_offset != null
          ? h(
              "div",
              { class: "row", style: { "justify-content": "center", "margin-top": "12px" } },
              button(t("storage.more"), {
                iconName: "chevron",
                onClick: async () => {
                  try {
                    await morePage(tab);
                  } catch (error) {
                    toast(failureText(error), "bad");
                  }
                },
              }),
            )
          : null,
      ),
    ];
  };

  // The scanning card is built once and its counters updated in place: rebuilding it on every
  // progress event would make the counters flicker.
  let shownStatus = null;
  let scanUi = null;
  const buildScanning = () => {
    const count = h("div", { class: "progress-count" }, "0");
    const size = h("div", { class: "progress-count" }, "0 B");
    const time = h("div", { class: "progress-count" }, "0 s");
    const title = h("h3", { style: { margin: "14px 0 10px", "font-size": "16px", "word-break": "break-all" } });
    const note = h("p", { class: "note", style: { "margin-top": "12px" } });
    const card = h(
      "section",
      { class: "card card-pad" },
      h("div", { class: "row wrap", style: { "justify-content": "space-between" } }, h("span", { class: "hint" }, t("say.storage.scan")), button(t("storage.cancel"), { variant: "danger", iconName: "stop", onClick: () => cancelScan() })),
      title,
      h("div", { class: "indeterminate" }),
      h(
        "div",
        { class: "grid grid-metrics", style: { "margin-top": "16px" } },
        h("div", null, count, h("div", { class: "hint" }, t("storage.progress.items"))),
        h("div", null, size, h("div", { class: "hint" }, t("storage.progress.size"))),
        h("div", null, time, h("div", { class: "hint" }, t("storage.progress.time"))),
      ),
      note,
    );
    return { card, count, size, time, title, note };
  };

  let pickerLocked = null;
  const render = () => {
    const scan = state.scan;
    const status = scan.status === "starting" ? "scanning" : scan.status;
    const locked = status === "scanning";
    if (locked !== pickerLocked) {
      pickerLocked = locked;
      renderPicker();
    }
    if (status === "scanning") {
      if (shownStatus !== "scanning") {
        scanUi = buildScanning();
        fill(body, scanUi.card);
      }
      const p = scan.progress;
      scanUi.title.textContent = t("storage.scanning", { folder: scan.root || "" });
      scanUi.count.textContent = p ? number(p.entries) : "0";
      scanUi.size.textContent = p ? bytes(p.bytes) : "0 B";
      scanUi.time.textContent = p ? duration(p.elapsed_ms) : "0 s";
      scanUi.note.textContent = isDriveRoot(scan.root) ? t("storage.whole") : "";
      shownStatus = "scanning";
      return;
    }
    shownStatus = status;
    if (status !== "done") selected = null;
    if (status === "idle") {
      fill(body, h("p", { class: "hint" }, t("say.storage.idle")));
      return;
    }
    if (status === "failed") {
      fill(body, h("div", { class: "callout bad" }, icon("alert"), h("div", null, h("strong", null, t("say.storage.failed")), h("span", { title: failureDetail(scan.error) }, failureText(scan.error)))));
      return;
    }
    keepFocus(body, () =>
      fill(
        body,
        h(
          "div",
          { class: "row wrap", style: { "justify-content": "space-between", "margin-bottom": "14px" } },
          h("div", { class: "row", style: { "min-width": "0" } }, icon("folder"), h("strong", { style: { "word-break": "break-all" } }, scan.view.root)),
          button(t("storage.again"), { iconName: "refresh", size: "sm", onClick: () => startScan(scan.view.root) }),
        ),
        ...renderResults(scan.view),
      ),
    );
  };

  // The elevated helper exists on Windows only; elsewhere the backend reports unsupported.
  const quickCard = quickScanCard();
  const adminCard = adminChecksCard();
  fill(
    page,
    h("div", { class: "page-head" }, h("div", null, h("h1", { class: "page-title" }, t("storage.title")), h("p", { class: "page-sub" }, t("storage.sub")))),
    picker,
    quickCard.el,
    adminCard.el,
    body,
  );
  render();
  call("quick_folders")
    .then((folders) => {
      quick = folders;
      pickerLocked = null;
      render();
    })
    .catch(() => {});

  let lastStatus = state.scan.status;
  const unsubscribe = subscribe((topic) => {
    if (topic !== "scan" && topic !== "info") return;
    const active = (status) => status === "starting" || status === "scanning";
    const started = active(state.scan.status) && !active(lastStatus);
    if (state.scan.status !== lastStatus) {
      lastStatus = state.scan.status;
      if (lastStatus === "done") announce(t("say.storage.done"));
    }
    render();
    // The scan follows the two administrator cards: bring its progress into view when it starts.
    if (started) {
      const still = document.documentElement.classList.contains("reduce-motion");
      body.scrollIntoView({ block: "start", behavior: still ? "auto" : "smooth" });
    }
  });
  // E explains and Enter investigates the selected result; Escape clears it.
  const stopKeys = selectionKeys({ current: selectedItem, clear: () => select(null) });
  const stopDrop = onDrop((event) => {
    const type = event?.type;
    picker.classList.toggle("drag", type === "enter" || type === "over");
    if (type === "drop" && event.paths?.length) startScan(event.paths[0]);
  });
  return () => {
    unsubscribe();
    stopKeys();
    stopDrop?.();
    quickCard.dispose();
    adminCard.dispose();
  };
}
