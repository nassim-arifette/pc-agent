// The action bar: a small toolbar anchored under the selected item, with Explain, Investigate and
// Monitor (not available yet). Views own their selection and draw the bar inside the selected
// item's `.actbar-anchor`; this module draws it, keeps it in view, gives it one tab stop with the
// arrow keys between its buttons, and handles E (Explain) and Enter (Investigate) while an item is
// selected. Explain and Investigate go through runSelection and its exact preview.
import { h } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { dialogOpen } from "./ui.js";

// Typing in these never triggers a shortcut.
const TYPING = "input:not([type='checkbox']):not([type='radio']):not([type='button']):not([type='submit']):not([type='range']), textarea, select, [contenteditable]:not([contenteditable='false'])";
// Enter on these belongs to them, unless it is the selected item itself.
const CONTROL = "button, a[href], input, select, textarea, summary, [role='button'], [role='tab'], [role='switch'], [role='menuitem'], [role='option']";

let pending = false;
// The last item a bar was drawn for: views redraw every few seconds and the bar must not replay
// its entrance each time.
let shownKey = null;

/**
 * Runs one action at a time, from the bar, the shortcuts or a view's own buttons (the Storage
 * inspector): a second E while a preview is being prepared does nothing.
 */
export async function once(run) {
  if (pending || !run) return;
  pending = true;
  try {
    await run();
  } finally {
    pending = false;
  }
}

/**
 * Keeps the bar inside the visible part of the page: on the item's other side when it would
 * overflow its bounds, above the item when there is no room below.
 */
function place(bar, align) {
  if (!bar.isConnected) return;
  const anchor = bar.parentElement;
  const scroller = bar.closest(".main") || document.documentElement;
  const view = scroller === document.documentElement ? { top: 0, bottom: window.innerHeight } : scroller.getBoundingClientRect();
  const bounds = (bar.closest("[data-actbar-bounds]") || bar.closest(".page") || scroller).getBoundingClientRect();
  bar.classList.remove("actbar-end", "actbar-above");
  if (align === "end") bar.classList.add("actbar-end");
  const rect = bar.getBoundingClientRect();
  if (align === "end" ? rect.left < bounds.left : rect.right > bounds.right) bar.classList.toggle("actbar-end");
  const item = anchor.getBoundingClientRect();
  const room = { below: view.bottom - item.bottom, above: item.top - view.top };
  if (rect.bottom > view.bottom - 8 && room.above > room.below) bar.classList.add("actbar-above");
}

/**
 * The bar for the selected item: { key, label, ready, unavailable, explain, investigate, align }.
 * `ready` is false while the item has no host-issued reference yet; `unavailable` then says why.
 * `explain` and `investigate` are async functions; the bar runs one at a time. `align: "end"`
 * puts it under the item's right end, for full-width rows whose left side carries their names.
 */
export function actionBar({ key, label, ready, unavailable, explain, investigate, align = "start" }) {
  const tool = (name, { iconName, text, tip, shortcut, enabled, run, tone }) =>
    h(
      "button",
      {
        type: "button",
        class: ["actbar-btn", tone && `actbar-${tone}`],
        "data-key": `actbar:${name}`,
        "aria-disabled": enabled ? null : "true",
        "aria-keyshortcuts": enabled && shortcut ? shortcut : null,
        title: tip,
        tabindex: "-1",
        onclick: () => {
          if (enabled) once(run);
        },
      },
      icon(iconName),
      h("span", null, text),
    );
  const buttons = [
    tool("explain", { iconName: "sparkle", text: t("inspect.explain"), tip: ready ? t("actbar.explain.tip") : unavailable, shortcut: "E", enabled: ready, run: explain, tone: "primary" }),
    tool("investigate", { iconName: "search", text: t("inspect.investigate"), tip: ready ? t("actbar.investigate.tip") : unavailable, shortcut: "Enter", enabled: ready, run: investigate }),
    // Watching a resource is not built yet: shown so the place is known, never faked.
    tool("monitor", { iconName: "bell", text: t("actbar.monitor"), tip: t("actbar.monitor.soon"), enabled: false, tone: "muted" }),
  ];
  const enabled = buttons.filter((b) => !b.hasAttribute("aria-disabled"));
  // One tab stop: the first enabled button, or Explain so its reason stays reachable.
  (enabled[0] || buttons[0]).tabIndex = 0;
  const bar = h(
    "div",
    {
      class: ["actbar", key !== shownKey && "actbar-in", !ready && "actbar-idle"],
      role: "toolbar",
      "aria-label": t("actbar.label", { what: label }),
      onfocusin: (e) => {
        const focused = e.target.closest(".actbar-btn");
        if (!focused || !enabled.includes(focused)) return;
        for (const b of buttons) b.tabIndex = b === focused ? 0 : -1;
      },
      onkeydown: (e) => {
        if (!enabled.length) return;
        const at = enabled.indexOf(document.activeElement);
        const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
        let next = null;
        if (step && at < 0) next = step > 0 ? enabled[0] : enabled[enabled.length - 1];
        else if (step) next = enabled[(at + step + enabled.length) % enabled.length];
        else if (e.key === "Home") next = enabled[0];
        else if (e.key === "End") next = enabled[enabled.length - 1];
        if (!next) return;
        e.preventDefault();
        next.focus();
      },
    },
    buttons[0],
    buttons[1],
    h("span", { class: "actbar-sep", role: "separator", "aria-orientation": "vertical" }),
    buttons[2],
    ready ? h("span", { class: "actbar-hint", "aria-hidden": "true" }, t("actbar.keys")) : null,
  );
  shownKey = key;
  // Before the next paint, once the bar is in the page and can be measured.
  requestAnimationFrame(() => place(bar, align));
  return bar;
}

/**
 * E explains and Enter investigates the selected item; Escape clears the selection. `current()`
 * returns { ready, explain, investigate } for the selected item, or null. The selected item's own
 * control carries `data-actbar-item`. Returns the function that removes the listener.
 */
export function selectionKeys({ current, clear }) {
  const onKey = (e) => {
    if (e.defaultPrevented || e.ctrlKey || e.altKey || e.metaKey || e.isComposing || dialogOpen()) return;
    const item = current();
    if (!item) return;
    const target = e.target instanceof Element ? e.target : null;
    if (e.key === "Escape") {
      e.preventDefault();
      // Leaving the bar: focus goes back to the item, which the view keeps after redrawing.
      if (target?.closest(".actbar")) document.querySelector("[data-actbar-item]")?.focus({ preventScroll: true });
      clear();
      return;
    }
    if (e.repeat || target?.closest(TYPING)) return;
    if (e.key === "e" || e.key === "E") {
      e.preventDefault();
      if (item.ready) once(item.explain);
    } else if (e.key === "Enter") {
      // Enter on a button of the bar, or on another control, keeps its own meaning.
      if (target?.closest(".actbar")) return;
      if (target && !target.closest("[data-actbar-item]") && target.closest(CONTROL)) return;
      e.preventDefault();
      if (item.ready) once(item.investigate);
    }
  };
  document.addEventListener("keydown", onKey);
  return () => document.removeEventListener("keydown", onKey);
}

/**
 * Redraws `host` with `draw()` and gives keyboard focus back to the element with the same
 * data-key, so selecting an item or a periodic refresh does not drop it.
 */
export function keepFocus(host, draw) {
  const active = document.activeElement;
  const key = active && host.contains(active) ? active.closest("[data-key]")?.dataset.key : null;
  draw();
  if (key) host.querySelector(`[data-key="${CSS.escape(key)}"]`)?.focus({ preventScroll: true });
}
