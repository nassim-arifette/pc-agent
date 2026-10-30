// Storage: pick or drop a folder and watch the scan. Its results come first: the scanned folder's
// totals and its largest folders and files, beside the inspector of the selected one, which can be
// explained or investigated (action bar under the row, inspector). After them, under their own
// heading, two cards ask for administrator consent: a quick analysis of a whole drive from its file
// system index (quickscan.js) and the deeper read-only checks (elevation/checks.js).
import { h, fill, announce } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call, onDrop } from "../api.js";
import { askWithAi, cloudReady } from "../chat/composer.js";
import { runSelection } from "../selection/actions.js";
import { actionBar, keepFocus, once, selectionKeys } from "../actionbar.js";
import { state, subscribe, startScan, cancelScan } from "../store.js";
import { badge, bar, button, iconButton, toast } from "../ui.js";
import { bytes, date, duration, number, percent } from "../format.js";
import { quickScanCard } from "./quickscan.js";
import { adminChecksCard } from "../elevation/checks.js";
import { failureDetail, failureText } from "../failure.js";
import { capable } from "../capabilities.js";
import { folderName, isDriveRoot, itemCoverage, itemFacts, itemNotes, shareShown } from "./items.js";
import { hierarchyBrowser } from "./browser.js";
import { watchProposal } from "./treemap.js";

let requestWatch = null;
import("../watches-controller.js").then((module) => { requestWatch = module.requestWatch; }).catch(() => {});

const QUICK_ICONS = { downloads: "drop", desktop: "pc", documents: "file", temp: "clock", profile: "folder", system_drive: "drive" };
const REASONS = ["denied_dirs", "denied_files", "errors", "placeholders", "reparse_skipped", "other_volume_skipped", "changed_during_scan", "vanished"];
// The badge of an item's coverage in the inspector: tone and icon.
const COVERAGE = { ok: ["good", "check"], partial: ["warn", "info"], denied: ["warn", "lock"] };
// The narrowest results (CSS pixels) that hold the inspector's column (320 px) beside a list whose
// rows still fit the action bar (about 530 px); narrower, the inspector opens under the row.
const BESIDE = 920;

/**
 * The host-issued reference of a scan item. Each page of results is published as a selection
 * source that expires after a few minutes; when it is gone, reading the same page of the same
 * scan again (scan_page) publishes it anew. Null when the scan was replaced.
 */
async function storageReference(view, kind, item) {
  const target = { kind: "storage", scope_id: view.scope_id, item_ref: item.item_ref };
  try {
    const sources = await call("selection_sources");
    const existing = sources.find((source) => source.reference.source_id === item.selection_source_id);
    if (existing) return existing.reference;
    const facts = await call("scan_item_facts", { scopeId: view.scope_id, itemRef: item.item_ref });
    item.selection_source_id = facts.selection_source_id;
    return facts.selection_source_id ? { source_id: facts.selection_source_id, target } : null;
  } catch {
    return null;
  }
}

/** What the action bar, the inspector and the shortcuts do for a scanned folder or file. */
function storageItem(view, kind, item) {
  const proposal = watchProposal(view);
  const monitor = requestWatch && proposal.rule ? () => requestWatch(proposal.rule) : null;
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
    ready: Boolean(item.selection_source_id),
    unavailable: t("storage.select.unavailable"),
    explain: () => run("explain"),
    investigate: () => run("investigate"),
    monitor,
    monitorUnavailable: t(proposal.reason || "storage.watch.unavailable"),
  };
}

/** Opens Explorer on the item: the Rust side resolves its reference, never a path from the page. */
async function reveal(view, item) {
  try {
    await call("reveal", { scopeId: view.scope_id, itemRef: item.item_ref });
  } catch (error) {
    toast(failureText(error), "bad");
  }
}

/** A fact of the inspector: a value the scan did not give reads as unknown, never as zero. */
function factValue(value) {
  if (value && "bytes" in value) return bytes(value.bytes);
  if (value && "count" in value) return number(value.count);
  if (value && "date" in value) return date(value.date);
  const shown = value && shareShown(value.share);
  return shown ? `${shown.below ? "< " : ""}${percent(shown.ratio, shown.digits)}` : h("span", { class: "unknown" }, t("storage.insp.unknown"));
}

/** A path that wraps after its separators rather than inside a name. */
function pathText(path) {
  return path.split("\\").flatMap((part, i, parts) => (i < parts.length - 1 ? [part, "\\", h("wbr")] : [part]));
}

/** A selection's summary is shown in the panel, in the inspector's place (as in Live PC). */
function summaryOpen() {
  return state.panel.open && state.panel.tab === "selection";
}

export function mount(page) {
  const picker = h("section", { class: "dropzone" });
  const recent = h("section", { class: "section storage-recent" });
  const body = h("div", { class: "section" });
  let quick = [];
  // The selected result stays tied to its scan and exact host-issued facts source.
  let selected = null;
  // A redraw that only moves the selection keeps the size bars still.
  let still = false;
  // Whether the last results were drawn without the inspector (a summary in the panel).
  let inspectorHidden = false;
  // Whether the results are wide enough for the inspector's column (see BESIDE).
  let beside = true;
  let factsRequest = 0;
  let factsResult = null;
  let factsError = null;
  let disposed = false;
  let other = false;
  const browser = hierarchyBrowser(() => { if (!disposed) redraw(); }, () => { selected = null; other = false; factsRequest++; });

  /** The selected result of the current results: { view, kind, item }, or null. */
  const selectedEntry = () => {
    const view = state.scan.status === "done" ? state.scan.view : null;
    if (!view || !selected || selected.scope !== view.scope_id) return null;
    return { view, kind: selected.kind, item: selected.item };
  };
  /** The selected item's actions, or null. */
  const selectedItem = () => {
    const entry = selectedEntry();
    return entry ? storageItem(entry.view, entry.kind, entry.item) : null;
  };
  /** Redraws the same data: only the selection, the room or the panel changed, so the bars stay still. */
  const redraw = () => {
    still = true;
    try {
      render();
    } finally {
      still = false;
    }
  };
  const select = (item) => {
    const request = ++factsRequest;
    other = false;
    factsResult = null;
    factsError = null;
    selected = item ? { scope: state.scan.view.scope_id, kind: item.kind, ref: item.item_ref, item } : null;
    if (item) {
      item.selection_source_id = null;
      call("scan_item_facts", { scopeId: selected.scope, itemRef: selected.ref }).then((result) => {
        if (disposed || request !== factsRequest || state.scan.view?.scope_id !== selected?.scope) return;
        factsResult = result.item_facts;
        item.selection_source_id = result.selection_source_id;
        redraw();
      }).catch((error) => {
        if (disposed || request !== factsRequest) return;
        factsError = error; redraw();
      });
    }
    redraw();
  };
  /** Clears the selection from the inspector; keyboard focus goes back to the item's row. */
  const clearToRow = () => {
    const ref = selected?.ref;
    select(null);
    if (ref) body.querySelector(`[data-key="${CSS.escape(`item:${ref}`)}"]`)?.focus();
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
      h("span", { class: "metric-icon" }, icon("folder")),
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
    const actions = isSelected ? storageItem(view, kind, selected.item) : null;
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
        h("span", { class: "text", "data-i18n-source": "local_name" }, h("strong", null, item.name), h("span", null, item.parent ? `…\\${item.parent}` : folderName(view.root)),
          item.system ? h("span", { class: "hint" }, t("storage.system")) : null),
      ),
      bar(max ? item.size / max : 0, kind === "dirs" ? "" : "mint", { animate: !still }),
      h(
        "div",
        { class: "list-value" },
        bytes(item.size),
        item.status !== "ok" ? h("span", { class: "partial-mark", title: t("storage.itemPartial"), "aria-label": t("storage.itemPartial") }, icon("info")) : null,
      ),
      h("div", { class: "date" }, item.modified_unix_ms ? date(item.modified_unix_ms) : ""),
      iconButton("reveal", t("storage.reveal"), () => reveal(view, item)),
      actions ? actionBar(actions) : null,
    );
  };

  /**
   * The inspector of the selected result, or its empty state. Everything it shows comes from the
   * scan already on the page (items.js); the location is shown here only. `under` draws it
   * under the selected row instead of in the column beside the results.
   */
  const inspector = (entry, under = false) => {
    const safe = h("p", { class: "storage-insp-safe" }, icon("shield"), h("span", null, t("storage.insp.safe")));
    if (other) return h("aside", { class: "insp storage-insp" }, h("h2", null, t("storage.other")), h("p", null, t("storage.other.detail")), safe);
    if (!entry) {
      return h(
        "aside",
        { class: "insp insp-empty storage-insp", "aria-label": t("inspect.label") },
        h("span", { class: "insp-empty-icon" }, icon("folder")),
        h("strong", null, t("inspect.empty.title")),
        h("p", null, t("storage.insp.empty")),
        safe,
      );
    }
    const { view, kind, item } = entry;
    const actions = storageItem(view, kind, item);
    const coverage = itemCoverage(item);
    const [tone, coverageIcon] = COVERAGE[coverage];
    const facts = itemFacts(kind, item, view);
    const link = (name, iconName, text, onclick, enabled = true) => h("button", { type: "button", class: "insp-more", disabled: !enabled, "data-key": `insp:${name}`, onclick }, icon(iconName), text);
    return h(
      "aside",
      { class: ["insp", "storage-insp", under && "storage-insp-under"], "aria-label": t("inspect.label") },
      h(
        "div",
        { class: "insp-head" },
        h(
          "div",
          { class: "insp-titles" },
          h("div", { class: "storage-insp-kind" }, h("span", { class: "caption" }, t(`storage.insp.kind.${kind}`)), badge(t(`storage.insp.coverage.${coverage}`), tone, coverageIcon)),
          h("h2", { class: "insp-title", "data-i18n-source": "local_name" }, item.name),
        ),
        h("button", { type: "button", class: "icon-btn", title: t("inspect.clear"), "aria-label": t("inspect.clear"), "data-key": "insp:clear", onclick: clearToRow }, icon("x")),
      ),
      h(
        "div",
        { class: "insp-body" },
        h(
          "dl",
          { class: "insp-kv" },
          facts.map((row) => [h("dt", null, t(row.key, row.vars)), h("dd", null, factValue(row.value))]),
        ),
        itemNotes(kind, item, view, facts).map((key) => h("p", { class: "note" }, t(key))),
        factsResult ? h("div", { class: "storage-facts" },
          h("p", { class: "note" }, t("storage.facts.at", { at: date(factsResult.scanned_at_unix_ms, true) })),
          h("dl", { class: "insp-kv" },
            h("dt", null, t("storage.facts.untouched")), h("dd", null, `${number(factsResult.untouched.files_count)} · ${factsResult.untouched.allocated_bytes == null ? t("storage.insp.unknown") : bytes(factsResult.untouched.allocated_bytes)}`),
            h("dt", null, t("storage.facts.undated")), h("dd", null, number(factsResult.undated_files_count)),
            h("dt", null, t("storage.facts.largest")), h("dd", null, factsResult.largest_file
              ? `${folderName(factsResult.largest_file.path || factsResult.largest_file.alias)} · ${bytes(factsResult.largest_file.allocated_bytes ?? factsResult.largest_file.logical_bytes)}`
              : t("storage.insp.unknown"))),
          h("h3", null, t("storage.facts.types")),
          h("dl", { class: "insp-kv" }, factsResult.file_types.map((type) => [
            h("dt", null, t(`storage.type.${type.file_type}`)),
            h("dd", null, `${number(type.files_count)} · ${type.allocated_bytes == null ? t("storage.insp.unknown") : bytes(type.allocated_bytes)}`)])),
          factsResult.status !== "ok" ? h("p", { class: "note" }, t("storage.map.partial")) : null)
          : h("div", { role: "status" }, factsError ? failureText(factsError) : t("storage.loading"),
            factsError ? link("retry", "refresh", t("storage.retry"), () => select(item)) : null),
      ),
      h(
        "div",
        { class: "storage-insp-where" },
        h("span", { class: "caption" }, t("storage.insp.where")),
        h("p", { class: "storage-insp-path", "data-i18n-source": "local_name" }, pathText(item.path)),
        h("p", { class: "note" }, t("storage.insp.whereNote")),
        link("reveal", "reveal", t("storage.reveal"), () => reveal(view, item)),
      ),
      // The same actions as the bar under the row, which remains their main place; one at a time
      // with the bar and the shortcuts.
      h(
        "div",
        { class: "insp-next" },
        h("p", null, actions.ready ? t("storage.insp.next") : actions.unavailable),
        h(
          "div",
          { class: "storage-insp-actions" },
          link("explain", "sparkle", t("inspect.explain"), () => once(actions.explain), actions.ready),
          link("investigate", "search", t("inspect.investigate"), () => once(actions.investigate), actions.ready),
          kind === "dirs" ? link("open", "folder", t("storage.open"), () => browser.zoom(item)) : null,
          link("monitor", "bell", t("actbar.monitor"), () => once(actions.monitor), Boolean(actions.monitor)),
        ),
        !actions.monitor ? h("p", { class: "note" }, actions.monitorUnavailable) : null,
      ),
      safe,
    );
  };

  const renderResults = (view) => {
    const scanned = view.scanned;
    const volume = view.volume;
    const used = volume.total_bytes && volume.free_bytes != null ? 1 - volume.free_bytes / volume.total_bytes : null;
    const why = reasons(view.coverage);
    const complete = view.status === "ok";
    const askButton = cloudReady()
      ? button(t("storage.ask"), { iconName: "sparkle", variant: "primary", onClick: () => askWithAi(t("storage.askQuestion", { folder: folderName(view.root) }), { followUp: false }) })
      : button(t("storage.ask"), { iconName: "sparkle", disabled: true, title: t("mode.cloud.setup") });
    inspectorHidden = summaryOpen();
    const entry = inspectorHidden ? null : selectedEntry();
    const side = beside && !inspectorHidden;
    return h(
      "div",
      { class: ["storage-layout", side && "beside"] },
      h(
        "div",
        { class: "storage-main" },
        h(
          "div",
          { class: "row wrap", style: { "justify-content": "space-between" } },
          h("div", { class: "row", style: { "min-width": "0" } }, icon("folder"), h("strong", { style: { "word-break": "break-all" } }, view.root)),
          button(t("storage.again"), { iconName: "refresh", size: "sm", onClick: () => startScan(view.root) }),
        ),
        h("p", { class: "hint" }, t("storage.facts.at", { at: date(view.scanned_at_unix_ms, true) })),
        h(
          "div",
          { class: "grid grid-metrics grid-stats" },
          h(
            "div",
            { class: "card stat" },
            h("span", { class: "stat-label" }, t("storage.size")),
            h("span", { class: "stat-value" }, scanned ? bytes(Math.max(scanned.allocated_bytes, scanned.logical_bytes)) : "—"),
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
          { class: "card card-pad storage-results", "data-actbar-bounds": true },
          browser.render(view, {
            selected: selected?.ref,
            select,
            other: () => { select(null); other = true; redraw(); },
            row: (item, max, kind) => h("div", { class: "storage-browser-row" }, itemRow(item, max, kind, view),
              !beside && entry?.item.item_ref === item.item_ref ? inspector(entry, true) : null),
          }),
          askButton,
          !side && other ? inspector(null, true) : null,
        ),
      ),
      side ? inspector(entry) : null,
    );
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
    fill(recent, state.recentScans.length ? [
      h("h2", { class: "caption" }, t("storage.recent")),
      state.recentScans.map((entry) => h("div", { class: "row wrap" },
        h("span", { class: "hint" }, `${entry.root} · ${date(entry.at, true)} · ${entry.size == null ? t("storage.insp.unknown") : bytes(entry.size)} · ${t(entry.status === "ok" ? "storage.complete" : "storage.partial")}`),
        button(t("storage.recent.rescan"), { size: "sm", disabled: locked, onClick: () => startScan(entry.root) })))
    ] : []);
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
    keepFocus(body, () => fill(body, renderResults(scan.view)));
  };

  // The two administrator cards need pcagent-elevated next to the app (capabilities.js); without
  // it the page says so instead of offering checks that cannot start.
  const quickCard = quickScanCard();
  const adminCard = adminChecksCard();
  const helperNote = h("div", { class: "callout info section" }, icon("shield"), h("div", null, h("strong", null, t("storage.helper.title")), h("span", null, t("storage.helper.missing"))));
  const showHelper = () => {
    const helper = capable("elevated_helper");
    quickCard.el.hidden = !helper;
    adminCard.el.hidden = !helper;
    helperNote.hidden = helper;
  };
  showHelper();
  fill(
    page,
    h("div", { class: "page-head" }, h("div", null, h("h1", { class: "page-title" }, t("storage.title")), h("p", { class: "page-sub" }, t("storage.sub")))),
    picker,
    h("p", { class: "note" }, t("storage.readOnly")),
    body,
    recent,
    // After the results: what needs administrator consent, each card still complete.
    h(
      "section",
      { class: "storage-admin", "aria-labelledby": "storage-admin-title" },
      h("h2", { class: "storage-admin-title", id: "storage-admin-title" }, t("storage.admin.title")),
      h("p", { class: "hint storage-admin-lead" }, t("storage.admin.lead")),
      helperNote,
      quickCard.el,
      adminCard.el,
    ),
  );
  // The room for the inspector's column follows the window and the panel, which can open or
  // close at any time and animate the width: watch it rather than measuring once.
  beside = body.clientWidth >= BESIDE;
  const room = new ResizeObserver(() => {
    const next = body.clientWidth >= BESIDE;
    if (next === beside) return;
    beside = next;
    if (state.scan.status === "done") redraw();
  });
  room.observe(body);
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
    // Opening or closing a selection's summary moves the inspector out of the page or back.
    if (topic === "panel") {
      if (state.scan.status === "done" && summaryOpen() !== inspectorHidden) redraw();
      return;
    }
    if (topic !== "scan" && topic !== "info") return;
    if (topic === "info") showHelper();
    const active = (status) => status === "starting" || status === "scanning";
    const started = active(state.scan.status) && !active(lastStatus);
    if (state.scan.status !== lastStatus) {
      lastStatus = state.scan.status;
      if (lastStatus === "done") announce(t("say.storage.done"));
    }
    render();
    // A folder dropped while the administrator cards are in view: bring the scan's progress back.
    if (started) {
      const reduced = document.documentElement.classList.contains("reduce-motion");
      body.scrollIntoView({ block: "nearest", behavior: reduced ? "auto" : "smooth" });
    }
  });
  // E explains and Enter investigates the selected result; Escape clears it (from the inspector
  // too, whose focus then goes back to the item's row).
  const stopKeys = selectionKeys({
    current: selectedItem,
    clear: () => {
      if (document.activeElement?.closest?.(".storage-insp")) clearToRow();
      else select(null);
    },
  });
  const stopDrop = onDrop((event) => {
    const type = event?.type;
    picker.classList.toggle("drag", type === "enter" || type === "over");
    if (type === "drop" && event.paths?.length) startScan(event.paths[0]);
  });
  return () => {
    disposed = true;
    factsRequest++;
    browser.dispose();
    unsubscribe();
    stopKeys();
    stopDrop?.();
    room.disconnect();
    quickCard.dispose();
    adminCard.dispose();
  };
}
