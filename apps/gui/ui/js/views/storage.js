// Storage: pick or drop a folder, watch the scan, then browse the largest folders and files.
import { h, fill, announce } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call, onDrop } from "../api.js";
import { mascotWithBubble } from "../mascot.js";
import { ask, cloudReady } from "../ask.js";
import { state, subscribe, startScan, cancelScan, morePage } from "../store.js";
import { badge, bar, button, iconButton, toast } from "../ui.js";
import { bytes, date, duration, number, percent } from "../format.js";

const QUICK_ICONS = { downloads: "drop", desktop: "pc", documents: "file", temp: "clock", profile: "folder", system_drive: "drive" };
const REASONS = ["denied_dirs", "denied_files", "errors", "placeholders", "reparse_skipped", "other_volume_skipped", "changed_during_scan", "vanished"];

function folderName(path) {
  const trimmed = (path || "").replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}
function isDriveRoot(path) {
  return /^[A-Za-z]:\\?$/.test(path || "");
}

export function mount(page) {
  const iris = mascotWithBubble({ size: 96, side: true, label: t("mascot.label") });
  const picker = h("section", { class: "dropzone" });
  const body = h("div", { class: "section" });
  let tab = "dirs";
  let quick = [];

  const choose = async () => {
    try {
      const path = await call("pick_folder");
      if (path) startScan(path);
    } catch (error) {
      toast(error.message, "bad");
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

  const itemRow = (item, max, kind) =>
    h(
      "div",
      { class: "list-row item-row" },
      h(
        "div",
        { class: "list-name" },
        icon(kind === "dirs" ? "folder" : "file"),
        h("div", { class: "text" }, h("strong", { title: item.path }, item.name), h("span", { title: item.path }, item.parent ? `…\\${item.parent}` : folderName(state.scan.view?.root))),
      ),
      bar(max ? item.size / max : 0, kind === "dirs" ? "" : "mint"),
      h(
        "div",
        { class: "list-value" },
        item.size_text,
        item.status !== "ok" ? h("span", { class: "partial-mark", title: t("storage.itemPartial"), "aria-label": t("storage.itemPartial") }, icon("info")) : null,
      ),
      h("div", { class: "date" }, item.modified_unix_ms ? date(item.modified_unix_ms) : ""),
      iconButton("reveal", t("storage.reveal"), async () => {
        try {
          await call("reveal", { scopeId: state.scan.view.scope_id, itemRef: item.item_ref });
        } catch (error) {
          toast(error.message, "bad");
        }
      }),
    );

  const renderResults = (view) => {
    const scanned = view.scanned;
    const volume = view.volume;
    const used = volume.total_bytes && volume.free_bytes != null ? 1 - volume.free_bytes / volume.total_bytes : null;
    const why = reasons(view.coverage);
    const complete = view.status === "ok";
    const current = view[tab];
    const max = current.items.reduce((m, i) => Math.max(m, i.size), 0);
    const askButton = cloudReady()
      ? button(t("storage.ask"), { iconName: "sparkle", variant: "primary", onClick: () => ask(t("storage.askQuestion", { folder: folderName(view.root) }), { cloud: true }) })
      : button(t("storage.ask"), { iconName: "sparkle", disabled: true, title: t("mode.cloud.setup") });
    return [
      h(
        "div",
        { class: "grid grid-metrics grid-stats" },
        h(
          "div",
          { class: "card stat" },
          h("span", { class: "stat-label" }, t("storage.size")),
          h("span", { class: "stat-value" }, view.scanned_text ?? "—"),
          h("span", { class: "hint" }, scanned ? t("storage.files", { count: number(scanned.files) }) : ""),
        ),
        h(
          "div",
          { class: "card stat" },
          h("span", { class: "stat-label" }, t("storage.drive")),
          h("span", { class: "stat-value" }, used != null ? percent(used) : "—"),
          used != null ? bar(used, used > 0.95 ? "warn" : "") : null,
          h("span", { class: "hint" }, volume.free_text && volume.total_text ? t("storage.drive.free", { free: volume.free_text, total: volume.total_text }) : ""),
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
                    render();
                  },
                },
                t(`storage.tab.${k}`),
              ),
            ),
          ),
          askButton,
        ),
        current.items.length ? h("div", { class: "list" }, current.items.map((item) => itemRow(item, max, tab))) : h("p", { class: "muted" }, t("storage.empty")),
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
                    toast(error.message, "bad");
                  }
                },
              }),
            )
          : null,
      ),
    ];
  };

  // The scanning card is built once and its counters updated in place: rebuilding it on every
  // progress event would restart Iris's animations and hide her speech bubble.
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
      h("div", { class: "row wrap", style: { "justify-content": "space-between" } }, iris.el, button(t("storage.cancel"), { variant: "danger", iconName: "stop", onClick: () => cancelScan() })),
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
        iris.set("working");
        iris.say(t("say.storage.scan"));
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
    if (status === "idle") {
      iris.set("idle");
      iris.say(t("say.storage.idle"));
      fill(body, h("section", { class: "card live-line" }, iris.el));
      return;
    }
    if (status === "failed") {
      iris.set("error");
      iris.say(t("say.storage.failed"));
      fill(body, h("section", { class: "card card-pad" }, h("div", { class: "row" }, iris.el), h("div", { class: "callout bad", style: { "margin-top": "12px" } }, icon("alert"), h("div", null, scan.error))));
      return;
    }
    iris.set("happy");
    fill(
      body,
      h(
        "div",
        { class: "row wrap", style: { "justify-content": "space-between", "margin-bottom": "14px" } },
        h("div", { class: "row", style: { "min-width": "0" } }, icon("folder"), h("strong", { style: { "word-break": "break-all" } }, scan.view.root)),
        button(t("storage.again"), { iconName: "refresh", size: "sm", onClick: () => startScan(scan.view.root) }),
      ),
      ...renderResults(scan.view),
    );
  };

  fill(
    page,
    h("div", { class: "page-head" }, h("div", null, h("h1", { class: "page-title" }, t("storage.title")), h("p", { class: "page-sub" }, t("storage.sub")))),
    picker,
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
    if (state.scan.status !== lastStatus) {
      lastStatus = state.scan.status;
      if (lastStatus === "done") announce(t("say.storage.done"));
    }
    render();
  });
  const stopDrop = onDrop((event) => {
    const type = event?.type;
    picker.classList.toggle("drag", type === "enter" || type === "over");
    if (type === "drop" && event.paths?.length) startScan(event.paths[0]);
  });
  return () => {
    unsubscribe();
    stopDrop?.();
  };
}
