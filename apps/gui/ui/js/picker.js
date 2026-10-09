// The model and reasoning pickers next to Send, as in chat apps: one button for the Nemotron
// model (or Auto), another for how much it reasons, each opening a small menu. Models are listed
// from the most capable to the lightest. A model is saved with save_cloud_settings; Auto is the
// engine's calibrated Super → Ultra routing (model_routing_set), offered only once a reviewed
// calibration exists. Both commands invalidate any reviewed preview on the Rust side.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { call } from "./api.js";
import { state, notify } from "./store.js";
import { toast } from "./ui.js";
import { modelName } from "./models.js";

export const REASONING = ["on", "low", "off"];

/** Larger first: Ultra, Super, Nano, then Lightning; unknown IDs last. */
function rank(id) {
  const name = (id || "").toLowerCase();
  return ["ultra", "super", "nano", "lightning"].findIndex((family) => name.includes(family)) >>> 0;
}

/** The routing policy: configured model, always Super/Ultra, or calibrated automatic. */
export async function refreshRouting() {
  const before = JSON.stringify(state.routing ?? null);
  try {
    state.routing = await call("model_routing_get");
  } catch {
    state.routing = null;
  }
  // Redraw only on a change: a redraw replaces the picker button the menu is attached to.
  if (JSON.stringify(state.routing) !== before) notify("info");
  return state.routing;
}

let openMenu = null;

function closeMenu() {
  if (!openMenu) return;
  const { menu, button, cleanup } = openMenu;
  openMenu = null;
  cleanup();
  menu.remove();
  button.setAttribute("aria-expanded", "false");
}

function familyId(family) {
  return (state.info?.models || []).map((m) => m.id).find((id) => id.toLowerCase().includes(family)) || null;
}

/** The model a question uses now, under the routing policy. */
function activeModel() {
  const mode = state.routing?.settings?.mode;
  if (mode === "always_super") return state.routing.settings.super_model_id || familyId("super");
  if (mode === "always_ultra") return state.routing.settings.ultra_model_id || familyId("ultra");
  return state.info?.cloud?.model_id;
}

async function chooseModel(modelId) {
  closeMenu();
  const cloud = state.info?.cloud;
  try {
    // A fixed choice replaces Auto or an "always" policy.
    if (state.routing && state.routing.settings.mode !== "configured") {
      state.routing = await call("model_routing_set", { settings: { mode: "configured", super_model_id: null, ultra_model_id: null } });
    }
    if (modelId !== cloud.model_id) state.info.cloud = await call("save_cloud_settings", { apiKey: null, modelId, reasoning: cloud.reasoning });
    notify("info");
  } catch (error) {
    toast(error.message, "bad");
  }
}

async function chooseAuto() {
  closeMenu();
  try {
    state.routing = await call("model_routing_set", { settings: { mode: "automatic", super_model_id: familyId("super"), ultra_model_id: familyId("ultra") } });
    notify("info");
  } catch (error) {
    toast(error.message, "bad");
  }
}

async function chooseReasoning(reasoning) {
  closeMenu();
  const cloud = state.info?.cloud;
  if (!cloud || reasoning === cloud.reasoning) return;
  try {
    state.info.cloud = await call("save_cloud_settings", { apiKey: null, modelId: cloud.model_id, reasoning });
    notify("info");
  } catch (error) {
    toast(error.message, "bad");
  }
}

function item({ label, note, checked, disabled = false, onPick }) {
  return h(
    "button",
    { type: "button", role: "menuitemradio", class: "pick-item", "aria-checked": String(checked), "aria-disabled": disabled ? "true" : null, onclick: disabled ? null : onPick },
    h("span", { class: "pick-check" }, checked ? icon("check") : null),
    h("span", { class: "pick-text" }, h("span", null, label), note ? h("span", { class: "pick-note" }, note) : null),
  );
}

/** The model menu: Auto, then the models from the most capable to the lightest. */
function modelItems() {
  const routing = state.routing;
  const automatic = routing?.settings?.mode === "automatic";
  const current = activeModel();
  const models = [...(state.info.models || [])].sort((a, b) => rank(a.id) - rank(b.id));
  const listed = models.some((m) => m.id === current);
  return [
    h("div", { class: "pick-section" }, t("picker.model")),
    item({
      label: t("picker.auto"),
      note: routing?.automatic_available ? t("picker.auto.note") : t("picker.auto.unavailable"),
      checked: automatic,
      disabled: !routing?.automatic_available && !automatic,
      onPick: chooseAuto,
    }),
    models.map((m) =>
      item({ label: modelName(m.id), note: t(`picker.note.${m.note}`), checked: !automatic && m.id === current, onPick: () => chooseModel(m.id) }),
    ),
    // A custom ID set in Settings stays visible and selected.
    listed || !current || automatic ? null : item({ label: modelName(current), note: t("picker.custom"), checked: true, onPick: () => closeMenu() }),
  ];
}

/** The reasoning menu: how long the model thinks before answering. */
function reasoningItems() {
  const reasoning = state.info.cloud.reasoning || "on";
  return [
    h("div", { class: "pick-section" }, t("picker.reasoning")),
    REASONING.map((r) =>
      item({ label: t(`picker.reasoning.${r}`), note: t(`picker.reasoning.${r}.note`), checked: reasoning === r, onPick: () => chooseReasoning(r) }),
    ),
  ];
}

const MENUS = {
  model: { label: "picker.model", items: modelItems },
  reasoning: { label: "picker.reasoning", items: reasoningItems },
};

async function openPicker(button, kind) {
  if (openMenu) {
    const same = openMenu.button === button;
    closeMenu();
    if (same) return;
  }
  await refreshRouting();
  // A changed policy redrew the composer: open the menu on the new button.
  if (!button.isConnected) {
    document.querySelector(`.pick-button[data-picker="${kind}"]`)?.click();
    return;
  }
  const menu = h("div", { class: "pick-menu", role: "menu", "aria-label": t(MENUS[kind].label) }, MENUS[kind].items());
  button.parentElement.append(menu);
  button.setAttribute("aria-expanded", "true");
  // Open upwards when there is room inside the view, otherwise downwards; never clipped.
  const area = (button.closest(".main") || document.body).getBoundingClientRect();
  const at = button.getBoundingClientRect();
  const above = at.top - area.top - 12;
  const below = area.bottom - at.bottom - 12;
  const up = above >= Math.min(menu.scrollHeight, 320) || above >= below;
  menu.classList.toggle("down", !up);
  menu.style.setProperty("max-height", `${Math.max(160, Math.min(440, up ? above : below))}px`);
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
      const index = items.indexOf(document.activeElement);
      const next = e.key === "ArrowDown" ? (index + 1) % items.length : (index - 1 + items.length) % items.length;
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

function pickerButton(kind, text, { disabled }) {
  const host = h("span", { class: "pick-host" });
  const button = h(
    "button",
    {
      type: "button",
      class: ["pick-button", `pick-${kind}`],
      dataset: { picker: kind },
      "aria-haspopup": "menu",
      "aria-expanded": "false",
      title: t(`picker.${kind}.title`),
      disabled,
      onclick: () => openPicker(button, kind),
    },
    h("span", null, text),
    icon("chevron"),
  );
  fill(host, button);
  return host;
}

/**
 * The model and reasoning buttons, or nothing when the AI is not configured. `disabled` while a
 * turn runs: the running turn keeps the model and reasoning it started with.
 */
export function modelPicker({ disabled = false } = {}) {
  const cloud = state.info?.cloud;
  if (!cloud?.ready) return null;
  const automatic = state.routing?.settings?.mode === "automatic";
  return [
    pickerButton("model", automatic ? t("picker.auto") : modelName(activeModel()), { disabled }),
    pickerButton("reasoning", t(`picker.short.${cloud.reasoning || "on"}`), { disabled }),
  ];
}

/** Closes an open picker, e.g. when the composer redraws. */
export function closePicker() {
  closeMenu();
}
