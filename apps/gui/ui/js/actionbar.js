// The action bar: an in-flow toolbar under the selected item, with Explain, Investigate and
// an optional local Monitor callback. Views own their selection and draw the bar inside the selected
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
let nextReasonId = 0;

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
 * `ready` is false while the item has no host-issued reference yet; `unavailable` then says why.
 * `monitor` opens the view's local creation form; its readiness is independent of the cloud
 * reference. Without it, `monitorUnavailable` is visible and describes the disabled control.
 * The callbacks may be async; the bar runs one at a time. `align: "end"`
 * puts it under the item's right end, for full-width rows whose left side carries their names.
 */
export function actionBar({ key, label, ready, unavailable, explain, investigate, monitor, monitorUnavailable, align = "start" }) {
  const reason = monitor ? null : monitorUnavailable || t("actbar.monitor.soon");
  const reasonId = reason ? `actbar-monitor-reason-${++nextReasonId}` : null;
  const tool = (name, { iconName, text, tip, shortcut, enabled, run, tone }) =>
    h(
      "button",
      {
        type: "button",
        class: ["actbar-btn", tone && `actbar-${tone}`],
        "data-key": `actbar:${name}`,
        "aria-disabled": enabled ? null : "true",
        "aria-describedby": name === "monitor" ? reasonId : null,
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
    tool("monitor", { iconName: "bell", text: t("actbar.monitor"), tip: reason || t("actbar.monitor"), enabled: Boolean(monitor), run: monitor, tone: monitor ? null : "muted" }),
  ];
  const enabled = buttons.filter((b) => !b.hasAttribute("aria-disabled"));
  // One tab stop: the first enabled button, or Explain so its reason stays reachable.
  (enabled[0] || buttons[0]).tabIndex = 0;
  const bar = h(
    "div",
    {
      class: ["actbar", key !== shownKey && "actbar-in", !ready && "actbar-idle", align === "end" && "actbar-end"],
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
    reason ? h("span", { class: "actbar-hint", id: reasonId }, reason) : null,
    ready ? h("span", { class: "actbar-hint", "aria-hidden": "true" }, t("actbar.keys")) : null,
  );
  shownKey = key;
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
