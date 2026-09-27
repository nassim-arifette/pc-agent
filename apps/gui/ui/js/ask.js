// The question composer and the flow that starts an investigation, including consent before
// anything is sent to Nebius.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { call } from "./api.js";
import { getPrefs, setPref } from "./prefs.js";
import { state, startInvestigation } from "./store.js";
import { button, dialog, segmented, toast } from "./ui.js";
import { bytes } from "./format.js";
import { navigate } from "./router.js";

export const SUGGESTIONS = [
  { key: "suggest.slow", icon: "activity" },
  { key: "suggest.ram", icon: "memory" },
  { key: "suggest.space", icon: "drive" },
  { key: "suggest.upgrade", icon: "cpu" },
];

export function cloudReady() {
  return Boolean(state.info?.cloud?.ready);
}

/** Lists of what AI mode sends and never sends, shared by the consent dialog and Settings. */
export function disclosure(preview) {
  const sent = [
    t("consent.sent.question"),
    preview ? t("consent.sent.measures", { count: preview.measured }) : null,
    preview ? t("consent.sent.findings", { count: preview.findings }) : null,
    t("consent.sent.refs"),
  ].filter(Boolean);
  const never = [t("consent.never.names"), t("consent.never.paths"), t("consent.never.ids"), t("consent.never.key")];
  return h(
    "div",
    { class: "two-lists" },
    h("div", null, h("h4", null, icon("cloud"), t("consent.sent.title")), h("ul", null, sent.map((x) => h("li", null, x)))),
    h("div", null, h("h4", null, icon("lock"), t("consent.never.title")), h("ul", null, never.map((x) => h("li", null, x)))),
  );
}

function folderName(path) {
  const trimmed = (path || "").replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}

/** Resolves with { cloud, useStorage } or null when the person cancels. */
async function consent(question) {
  let preview = null;
  try {
    preview = await call("cloud_preview", { question });
  } catch (error) {
    toast(error.message, "bad");
    return null;
  }
  let useStorage = Boolean(preview.storage_folder);
  let dontAsk = false;
  return dialog(
    (close) => {
      const storageRow = preview.storage_folder
        ? h(
            "label",
            { class: "check" },
            h("input", {
              type: "checkbox",
              checked: true,
              onchange: (e) => {
                useStorage = e.target.checked;
              },
            }),
            t("consent.storage", { folder: folderName(preview.storage_folder) }),
          )
        : null;
      return [
        h("div", { class: "dialog-head" }, h("div", { class: "metric-icon" }, icon("cloud")), h("h2", { class: "dialog-title" }, t("consent.title"))),
        h("p", null, t("consent.lead")),
        disclosure(preview),
        storageRow,
        h(
          "details",
          { class: "more" },
          h("summary", null, icon("chevron"), t("consent.show")),
          h("div", null, h("p", { class: "note" }, t("consent.size", { size: bytes(preview.bytes) })), h("pre", { class: "code" }, JSON.stringify(preview.message, null, 2))),
        ),
        h(
          "label",
          { class: "check" },
          h("input", {
            type: "checkbox",
            onchange: (e) => {
              dontAsk = e.target.checked;
            },
          }),
          t("consent.dontAsk"),
        ),
        h(
          "div",
          { class: "dialog-actions" },
          button(t("common.cancel"), { variant: "ghost", onClick: () => close(null) }),
          button(t("consent.local"), { iconName: "pc", onClick: () => close({ cloud: false, useStorage: false }) }),
          h(
            "button",
            {
              type: "button",
              class: "btn btn-primary",
              "data-autofocus": true,
              onclick: () => {
                if (dontAsk) {
                  state.skipConsent = true;
                  state.lastUseStorage = useStorage;
                }
                close({ cloud: true, useStorage });
              },
            },
            icon("send"),
            t("consent.send"),
          ),
        ),
      ];
    },
    { label: t("consent.title"), dismissValue: null },
  );
}

/** Starts an investigation from any screen. */
export async function ask(question, { cloud }) {
  const text = (question || "").trim();
  if (!text) return;
  if (state.inv && (state.inv.status === "running" || state.inv.status === "stopping")) {
    navigate("investigate");
    return;
  }
  let useStorage = false;
  if (cloud) {
    if (!cloudReady()) {
      toast(t("mode.cloud.setup"), "bad");
      navigate("settings");
      return;
    }
    if (state.skipConsent) {
      // A scanned folder is shared only if the person ticked it in a consent dialog.
      useStorage = state.lastUseStorage ?? false;
    } else {
      const decision = await consent(text);
      if (!decision) return;
      cloud = decision.cloud;
      useStorage = decision.useStorage;
    }
  }
  navigate("investigate");
  await startInvestigation(text, { cloud, useStorage });
}

/**
 * The composer: a growing text area, the local/AI switch and the send button.
 * `onFocusChange(focused)` lets a view react (Iris listens while you type).
 */
export function composer({ autofocus = false, onFocusChange, initial = "" } = {}) {
  const area = h("textarea", {
    rows: 2,
    placeholder: t("ask.placeholder"),
    "aria-label": t("ask.label"),
    maxlength: 4000,
    value: initial,
  });
  const grow = () => {
    area.style.setProperty("height", "auto");
    area.style.setProperty("height", `${Math.min(area.scrollHeight, 180)}px`);
  };
  const modeHost = h("div");
  const note = h("span", { class: "ask-note" });
  const renderMode = () => {
    const ready = cloudReady();
    let mode = getPrefs().mode === "cloud" && ready ? "cloud" : "local";
    fill(
      modeHost,
      segmented(
        [
          { value: "local", label: t("mode.local"), icon: "pc", title: t("mode.local.note") },
          { value: "cloud", label: t("mode.cloud"), icon: ready ? "sparkle" : "lock", title: ready ? t("mode.cloud.note") : t("mode.cloud.setup"), locked: !ready },
        ],
        mode,
        (value) => {
          if (value === "cloud" && !ready) {
            toast(t("mode.cloud.setup"), "bad");
            navigate("settings");
            return;
          }
          setPref("mode", value);
          renderMode();
        },
        t("mode.label"),
      ),
    );
    note.textContent = t(mode === "cloud" ? "mode.cloud.note" : "mode.local.note");
    return mode;
  };
  const submit = () => {
    const cloud = getPrefs().mode === "cloud" && cloudReady();
    ask(area.value, { cloud });
  };
  area.addEventListener("input", grow);
  area.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
  });
  area.addEventListener("focus", () => onFocusChange?.(true));
  area.addEventListener("blur", () => onFocusChange?.(false));
  renderMode();
  const el = h(
    "div",
    { class: "ask" },
    area,
    h("div", { class: "ask-bar" }, modeHost, note, h("span", { class: "spacer" }), button(t("ask.send"), { variant: "primary", iconName: "send", onClick: submit })),
  );
  if (autofocus) requestAnimationFrame(() => area.focus());
  return {
    el,
    focus: () => area.focus(),
    setText(text) {
      area.value = text;
      grow();
      area.focus();
    },
    refresh: renderMode,
  };
}

/** Suggestion chips; clicking one asks right away in the chosen mode. */
export function suggestions(onPick) {
  return h(
    "div",
    { class: "suggestions" },
    SUGGESTIONS.map((s) => h("button", { type: "button", class: "chip", onclick: () => onPick(t(s.key)) }, icon(s.icon), t(s.key))),
  );
}
