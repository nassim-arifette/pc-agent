// The model and reasoning picker next to Send, as in chat apps: one button showing the Nemotron
// model and how much it reasons, opening a small menu. A choice is saved on the Rust side with the
// model ID (save_cloud_settings), which also invalidates any reviewed preview.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { call } from "./api.js";
import { state, notify } from "./store.js";
import { toast } from "./ui.js";
import { modelName } from "./models.js";

export const REASONING = ["on", "low", "off"];

let openMenu = null;

function closeMenu() {
  if (!openMenu) return;
  const { menu, button, cleanup } = openMenu;
  openMenu = null;
  cleanup();
  menu.remove();
  button.setAttribute("aria-expanded", "false");
}

async function choose(modelId, reasoning) {
  closeMenu();
  const cloud = state.info?.cloud;
  if (!cloud || (modelId === cloud.model_id && reasoning === cloud.reasoning)) return;
  try {
    state.info.cloud = await call("save_cloud_settings", { apiKey: null, modelId, reasoning });
    notify("info");
  } catch (error) {
    toast(error.message, "bad");
  }
}

function item({ label, note, checked, onPick }) {
  return h(
    "button",
    { type: "button", role: "menuitemradio", class: "pick-item", "aria-checked": String(checked), onclick: onPick },
    h("span", { class: "pick-check" }, checked ? icon("check") : null),
    h("span", { class: "pick-text" }, h("span", null, label), note ? h("span", { class: "pick-note" }, note) : null),
  );
}

function openPicker(button) {
  if (openMenu) {
    closeMenu();
    return;
  }
  const cloud = state.info.cloud;
  const models = state.info.models || [];
  const current = cloud.model_id;
  const listed = models.some((m) => m.id === current);
  const menu = h(
    "div",
    { class: "pick-menu", role: "menu", "aria-label": t("picker.label") },
    h("div", { class: "pick-section" }, t("picker.model")),
    models.map((m) =>
      item({ label: modelName(m.id), note: t(`picker.note.${m.note}`), checked: m.id === current, onPick: () => choose(m.id, cloud.reasoning) }),
    ),
    // A custom ID set in Settings stays visible and selected.
    listed || !current ? null : item({ label: modelName(current), note: t("picker.custom"), checked: true, onPick: () => closeMenu() }),
    h("div", { class: "pick-section" }, t("picker.reasoning")),
    REASONING.map((r) =>
      item({ label: t(`picker.reasoning.${r}`), note: t(`picker.reasoning.${r}.note`), checked: cloud.reasoning === r, onPick: () => choose(current, r) }),
    ),
  );
  button.parentElement.append(menu);
  button.setAttribute("aria-expanded", "true");
  // Open upwards when there is room inside the view, otherwise downwards; never clipped.
  const area = (button.closest(".main") || document.body).getBoundingClientRect();
  const at = button.getBoundingClientRect();
  const above = at.top - area.top - 12;
  const below = area.bottom - at.bottom - 12;
  const up = above >= Math.min(menu.scrollHeight, 320) || above >= below;
  menu.classList.toggle("down", !up);
  menu.style.setProperty("max-height", `${Math.max(160, Math.min(420, up ? above : below))}px`);
  const onDown = (e) => {
    if (!menu.contains(e.target) && !button.contains(e.target)) closeMenu();
  };
  const onKey = (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      closeMenu();
      button.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const items = [...menu.querySelectorAll(".pick-item")];
      const at = items.indexOf(document.activeElement);
      const next = e.key === "ArrowDown" ? (at + 1) % items.length : (at - 1 + items.length) % items.length;
      items[next]?.focus();
    }
  };
  document.addEventListener("pointerdown", onDown, true);
  document.addEventListener("keydown", onKey, true);
  openMenu = {
    menu,
    button,
    cleanup: () => {
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
    },
  };
  menu.querySelector('.pick-item[aria-checked="true"]')?.focus();
}

/**
 * The picker button, or null when the AI is not configured. `disabled` while a turn runs: the
 * running turn keeps the model and reasoning it started with.
 */
export function modelPicker({ disabled = false } = {}) {
  const cloud = state.info?.cloud;
  if (!cloud?.ready) return null;
  const reasoning = cloud.reasoning || "on";
  const host = h("span", { class: "pick-host" });
  const button = h(
    "button",
    {
      type: "button",
      class: "pick-button",
      "aria-haspopup": "menu",
      "aria-expanded": "false",
      title: t("picker.label"),
      disabled,
      onclick: () => openPicker(button),
    },
    h("span", null, modelName(cloud.model_id)),
    h("span", { class: "pick-faint" }, t(`picker.short.${reasoning}`)),
    icon("chevron"),
  );
  fill(host, button);
  return host;
}

/** Closes an open picker, e.g. when the composer redraws. */
export function closePicker() {
  closeMenu();
}
