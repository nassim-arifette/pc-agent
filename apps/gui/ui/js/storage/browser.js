import { h } from "../dom.js";
import { call } from "../api.js";
import { t } from "../i18n.js";
import { bytes } from "../format.js";
import { failureText } from "../failure.js";
import { squarify, otherSize } from "./treemap.js";

/** Session-local hierarchy navigation. Children never grant a selection source. */
export function hierarchyBrowser(changed, deselect) {
  let scope = null, mode = "map", kind = "dirs", trail = [], disposed = false;
  const pages = new Map(), expanded = new Set();
  const key = (ref, k) => `${ref || ""}/${k}`;
  const parentRef = () => trail.at(-1)?.item_ref ?? null;
  async function load(ref, k, more = false) {
    const id = key(ref, k);
    const previous = pages.get(id);
    if (previous?.loading || (!more && previous && !previous.error)) return;
    const atScope = scope;
    const offset = more ? previous?.next_offset : 0;
    if (offset == null) return;
    pages.set(id, { ...previous, loading: true, error: null });
    try {
      const next = await call("scan_children", { scopeId: scope, itemRef: ref, kind: k, offset });
      if (scope !== atScope || disposed) return;
      pages.set(id, { ...next, items: more ? [...previous.items, ...next.items] : next.items });
    } catch (error) {
      if (scope !== atScope || disposed) return;
      pages.set(id, { ...previous, error, loading: false });
    }
    changed();
  }
  function zoom(item) {
    trail.push(item);
    deselect();
    changed();
  }
  const control = (text, click, props = {}) => h("button", { type: "button", class: "chip", onclick: click, ...props }, text);
  function group(ref, k, options, tree = false, depth = 0) {
    const id = key(ref, k);
    if (!pages.has(id)) load(ref, k);
    const page = pages.get(id);
    if (page?.error) return h("div", { role: "status" }, failureText(page.error),
      control(t("storage.retry"), () => load(ref, k)));
    if (!page?.items) return h("p", { role: "status" }, t("storage.loading"));
    const max = Math.max(0, ...page.items.map((item) => item.size));
    const rows = page.items.map((item) => {
      const folder = item.kind === "dirs";
      const open = expanded.has(item.item_ref);
      const row = options.row(item, max, item.kind);
      return h("li", { class: "storage-node" },
        h("div", { class: "storage-node-line" },
          folder ? control(tree ? (open ? "−" : "+") : t("storage.open"), () => {
            if (!tree) zoom(item);
            else { open ? expanded.delete(item.item_ref) : expanded.add(item.item_ref); changed(); }
          }, { "aria-label": t(tree ? "storage.expand" : "storage.openFolder", { name: item.name }),
            "aria-expanded": tree ? String(open) : null, "data-key": `open:${item.item_ref}` }) : null,
          row),
        tree && open ? h("div", { class: "storage-branch" },
          group(item.item_ref, "dirs", options, true, depth + 1),
          group(item.item_ref, "files", options, true, depth + 1)) : null);
    });
    const more = page.next_offset != null
      ? control(t("storage.more"), () => { load(ref, k, true); changed(); }, { disabled: page.loading, "data-key": `more:${id}` })
      : null;
    return h("div", null, h("ul", { class: "storage-nodes" }, rows),
      !tree && !rows.length ? h("p", { class: "hint" }, t("storage.empty")) : null, more);
  }
  function map(options) {
    const ref = parentRef(), id = key(ref, kind);
    if (!pages.has(id)) load(ref, kind);
    const page = pages.get(id);
    if (!page?.items || page.error) return group(ref, kind, options);
    const remainder = otherSize(page.parent, page.items);
    const values = [...page.items, ...(remainder ? [{ item_ref: "other", name: t("storage.other"), size: remainder }] : [])];
    const rects = squarify(values, 1000, 520);
    const tiles = rects.map(({ item, x, y, width, height }, index) => h("button", {
      type: "button", class: ["storage-tile", item.item_ref === "other" && "storage-other", options.selected === item.item_ref && "selected"],
      style: { left: `${x / 10}%`, top: `${y / 5.2}%`, width: `${width / 10}%`, height: `${height / 5.2}%`, "--tile-hue": (index * 41 + 190) % 360 },
      title: `${item.name} · ${bytes(item.size)}`,
      "aria-label": `${item.name} · ${bytes(item.size)}${item.status && item.status !== "ok" ? ` · ${t("storage.partial")}` : ""}`,
      "aria-pressed": String(options.selected === item.item_ref),
      "data-key": `tile:${item.item_ref}`,
      onclick: () => item.item_ref === "other" ? options.other() : options.select(item),
      ondblclick: () => { if (item.kind === "dirs") zoom(item); },
      onkeydown: (event) => {
        const direction = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[event.key];
        if (direction || event.key === "Home" || event.key === "End") {
          event.preventDefault();
          const next = event.key === "Home" ? 0 : event.key === "End" ? tiles.length - 1 : (index + direction + tiles.length) % tiles.length;
          tiles[next]?.focus();
        }
      },
    }, h("span", null, item.name), h("strong", null, bytes(item.size))));
    return h("div", null,
      h("p", { class: "hint" }, t("storage.map.help")),
      h("div", { class: "storage-map", "aria-label": t("storage.mode.map") }, tiles),
      page.parent_partial ? h("p", { class: "note" }, t("storage.map.partial")) : null,
      h("p", { class: "hint" }, t("storage.map.legend")),
      group(ref, kind, options));
  }
  return {
    zoom,
    dispose() { disposed = true; },
    render(view, options) {
      if (scope !== view.scope_id) {
        scope = view.scope_id; pages.clear(); expanded.clear(); trail = [];
      }
      return h("div", { class: "storage-browser" },
        h("div", { class: "row wrap" },
          ["map", "tree", "list"].map((value) => control(t(`storage.mode.${value}`), () => { mode = value; changed(); },
            { "aria-pressed": String(mode === value), "data-key": `mode:${value}` })),
          mode !== "tree" ? ["dirs", "files"].map((value) => control(t(`storage.tab.${value}`), () => { kind = value; deselect(); changed(); },
            { "aria-pressed": String(kind === value), "data-key": `kind:${value}` })) : null),
        h("nav", { class: "row wrap storage-crumbs", "aria-label": t("storage.location") },
          control(view.root, () => { trail = []; deselect(); changed(); }, { "data-key": "storage-root" }),
          trail.map((item, index) => control(item.name, () => { trail = trail.slice(0, index + 1); deselect(); changed(); },
            { "data-key": `crumb:${item.item_ref}` }))),
        mode === "map" ? map(options) : mode === "tree"
          ? h("div", null, group(parentRef(), "dirs", options, true), group(parentRef(), "files", options, true))
          : group(parentRef(), kind, options));
    },
  };
}
