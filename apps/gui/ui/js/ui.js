// Shared interface pieces: buttons, badges, bars, dialogs and toasts.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";

export function button(label, { variant = "", iconName, onClick, title, size = "", disabled = false, type = "button" } = {}) {
  return h(
    "button",
    {
      type,
      class: ["btn", variant && `btn-${variant}`, size && `btn-${size}`],
      onclick: onClick,
      title,
      "aria-label": label ? null : title,
      disabled,
    },
    iconName && icon(iconName),
    label,
  );
}

export function iconButton(iconName, title, onClick, variant = "ghost") {
  return h("button", { type: "button", class: ["btn", "btn-icon", `btn-${variant}`], title, "aria-label": title, onclick: onClick }, icon(iconName));
}

export function badge(text, tone = "", iconName) {
  return h("span", { class: ["badge", tone] }, iconName && icon(iconName), text);
}

/** A horizontal bar for a 0..1 ratio. */
export function bar(ratio, tone = "") {
  const inner = h("span");
  const el = h("div", { class: ["bar", tone], role: "presentation" }, inner);
  // Set after insertion so the width animates in.
  requestAnimationFrame(() => {
    inner.style.setProperty("width", `${Math.max(0, Math.min(1, ratio ?? 0)) * 100}%`);
  });
  return el;
}

export function segmented(options, value, onChange, label) {
  const group = h("div", { class: "segmented", role: "group", "aria-label": label });
  for (const option of options) {
    group.append(
      h(
        "button",
        {
          type: "button",
          "aria-pressed": String(option.value === value),
          class: option.locked ? "locked" : null,
          disabled: option.disabled || false,
          title: option.title,
          onclick: () => {
            if (option.value === value) return;
            onChange(option.value);
          },
        },
        option.icon && icon(option.icon),
        option.label,
      ),
    );
  }
  return group;
}

export function toggle(checked, onChange, label) {
  const el = h("button", {
    type: "button",
    class: "switch",
    role: "switch",
    "aria-checked": String(checked),
    "aria-label": label,
    onclick: () => {
      const next = el.getAttribute("aria-checked") !== "true";
      el.setAttribute("aria-checked", String(next));
      onChange(next);
    },
  });
  return el;
}

export function card(...children) {
  return h("section", { class: "card card-pad" }, ...children);
}

export function supportTone(support) {
  return { supported: "good", plausible: "warn", insufficient: "" }[support] ?? "";
}

// ---------- Dialogs ----------
let openDialogs = 0;

/**
 * Opens a modal dialog. `render(close)` returns its content. Resolves with the value passed to
 * close(). Escape and clicking outside resolve with `dismissValue`.
 */
export function dialog(render, { label, dismissValue = null, dismissible = true } = {}) {
  return new Promise((resolve) => {
    const host = document.getElementById("dialogs");
    const previous = document.activeElement;
    let done = false;
    const close = (value) => {
      if (done) return;
      done = true;
      openDialogs -= 1;
      overlay.remove();
      document.removeEventListener("keydown", onKey, true);
      if (previous && previous.focus) previous.focus();
      resolve(value);
    };
    const box = h("div", { class: "dialog", role: "dialog", "aria-modal": "true", "aria-label": label });
    const overlay = h("div", {
      class: "overlay",
      onclick: (e) => {
        if (e.target === overlay && dismissible) close(dismissValue);
      },
    }, box);
    const onKey = (e) => {
      if (e.key === "Escape" && dismissible) {
        e.preventDefault();
        close(dismissValue);
      } else if (e.key === "Tab") {
        // Keep keyboard focus inside the dialog.
        const focusable = [...box.querySelectorAll("button, input, select, textarea, summary, [tabindex]")].filter((el) => !el.disabled);
        if (!focusable.length) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    fill(box, render(close));
    host.append(overlay);
    openDialogs += 1;
    document.addEventListener("keydown", onKey, true);
    const primary = box.querySelector("[data-autofocus]") || box.querySelector(".btn-primary") || box.querySelector("button");
    primary?.focus();
  });
}

export function dialogOpen() {
  return openDialogs > 0;
}

// ---------- Toasts ----------
export function toast(text, tone = "good") {
  const host = document.getElementById("toasts");
  const el = h("div", { class: ["toast", tone] }, icon(tone === "bad" ? "alert" : "check"), h("span", null, text));
  host.append(el);
  window.setTimeout(() => {
    el.classList.add("leaving");
    window.setTimeout(() => el.remove(), 260);
  }, 3200);
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    toast(t("common.copied"));
  } catch {
    const area = h("textarea", { class: "sr-only" });
    area.value = text;
    document.body.append(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    toast(ok ? t("common.copied") : t("common.copyFailed"), ok ? "good" : "bad");
  }
}
