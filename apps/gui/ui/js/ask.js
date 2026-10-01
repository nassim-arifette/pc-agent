// The question composer and the flow that starts an investigation, including consent before
// anything is sent to Nebius.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { call } from "./api.js";
import { getPrefs, setPref } from "./prefs.js";
import { state, startInvestigation, stopInvestigation } from "./store.js";
import { button, dialog, toast } from "./ui.js";
import { bytes } from "./format.js";
import { navigate } from "./router.js";

export function cloudReady() {
  return Boolean(state.info?.cloud?.ready);
}

/** Whether program names go to the AI: the person's choice, on unless turned off. */
export function namesShared() {
  return getPrefs().names !== false;
}

/** Lists of what AI mode sends and never sends, shared by the consent dialog and Settings. */
export function disclosure(preview, names = namesShared()) {
  const sent = [
    t("consent.sent.question"),
    preview?.message?.previous_turns?.length ? t("consent.sent.history", { count: preview.message.previous_turns.length }) : null,
    preview ? t("consent.sent.measures", { count: preview.measured }) : null,
    preview ? t("consent.sent.findings", { count: preview.findings }) : null,
    names ? (preview?.names ? t("consent.sent.names", { count: preview.names }) : t("consent.sent.namesAny")) : t("consent.sent.refs"),
  ].filter(Boolean);
  const never = [names ? t("consent.never.pids") : t("consent.never.names"), t("consent.never.paths"), t("consent.never.ids"), t("consent.never.key")];
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

/** Resolves with { cloud, useStorage, names } or null when the person cancels. */
async function consent(question, conversationId) {
  let names = namesShared();
  let preview = null;
  try {
    preview = await call("cloud_preview", { question, names, conversationId });
  } catch (error) {
    toast(error.message, "bad");
    return null;
  }
  let useStorage = Boolean(preview.storage_folder);
  let dontAsk = false;
  let revision = 0;
  let pending = false;
  let closed = false;
  let sendButton;
  // What is sent depends on the names choice: both lists and the exact message are redrawn.
  const listHost = h("div");
  const messageHost = h("div");
  const draw = () => {
    fill(listHost, disclosure(preview, names));
    fill(
      messageHost,
      h(
        "details",
        { class: "more" },
        h("summary", null, icon("chevron"), t("consent.show")),
        h("div", null, h("p", { class: "note" }, t("consent.size", { size: bytes(preview.bytes) })), h("pre", { class: "code" }, JSON.stringify(preview.message, null, 2))),
      ),
    );
  };
  draw();
  const namesRow = h(
    "label",
    { class: "check" },
    h("input", {
      type: "checkbox",
      checked: names,
      onchange: async (e) => {
        names = e.target.checked;
        const request = ++revision;
        pending = true;
        sendButton.disabled = true;
        try {
          const next = await call("cloud_preview", { question, names, conversationId });
          if (closed || request !== revision) return;
          preview = next;
          pending = false;
          sendButton.disabled = false;
          draw();
        } catch (error) {
          if (closed || request !== revision) return;
          preview = null;
          fill(messageHost, h("p", { class: "note" }, error.message));
          toast(error.message, "bad");
        }
      },
    }),
    t("consent.names"),
  );
  const decision = await dialog(
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
        listHost,
        namesRow,
        storageRow,
        messageHost,
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
          sendButton = h(
            "button",
            {
              type: "button",
              class: "btn btn-primary",
              "data-autofocus": true,
              onclick: () => {
                if (pending || !preview) return;
                setPref("names", names);
                if (dontAsk) {
                  state.skipConsent = true;
                  state.lastUseStorage = useStorage;
                  state.lastStorageFolder = useStorage ? preview.storage_folder : null;
                }
                close({ cloud: true, useStorage, names, previewId: preview.id });
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
  closed = true;
  return decision;
}

/** Starts an investigation from any screen: a follow-up of the open conversation, or a new one. */
export async function ask(question, { cloud, followUp = false }) {
  const text = (question || "").trim();
  if (!text) return;
  if (state.inv && (state.inv.status === "running" || state.inv.status === "stopping")) {
    navigate("chat");
    return;
  }
  let useStorage = false;
  let names = namesShared();
  let previewId = null;
  const conversationId = followUp ? (state.conversation?.id ?? null) : null;
  if (cloud) {
    if (!cloudReady()) {
      toast(t("mode.cloud.setup"), "bad");
      navigate("settings");
      return;
    }
    if (state.skipConsent) {
      try {
        const preview = await call("cloud_preview", { question: text, names, conversationId });
        previewId = preview.id;
        // Session consent applies to the folder actually approved, not a later different scan.
        useStorage = Boolean(state.lastUseStorage && preview.storage_folder && preview.storage_folder === state.lastStorageFolder);
      } catch (error) {
        toast(error.message, "bad");
        return;
      }
    } else {
      const decision = await consent(text, conversationId);
      if (!decision) return;
      cloud = decision.cloud;
      useStorage = decision.useStorage;
      names = decision.names ?? names;
      previewId = decision.previewId ?? null;
    }
  }
  navigate("chat");
  await startInvestigation(text, { cloud, useStorage, names, followUp, previewId });
}

/** The composer: a growing text area, the analysis mode, and Send or Stop while a run is going. */
export function composer({ autofocus = false, followUp = false, big = false } = {}) {
  const area = h("textarea", {
    rows: big ? 3 : 1,
    placeholder: t(followUp ? "ask.followUp" : "ask.placeholder"),
    "aria-label": t("ask.label"),
    maxlength: 4000,
  });
  const grow = () => {
    area.style.setProperty("height", "auto");
    area.style.setProperty("height", `${Math.min(area.scrollHeight, 200)}px`);
  };
  const modeHost = h("span", { class: "mode-host" });
  const sendHost = h("span", { class: "send-host" });
  const running = () => Boolean(state.inv && (state.inv.status === "running" || state.inv.status === "stopping"));
  const submit = () => {
    if (running()) return;
    const cloud = getPrefs().mode === "cloud" && cloudReady();
    const text = area.value;
    if (!text.trim()) return;
    area.value = "";
    grow();
    ask(text, { cloud, followUp });
  };
  const refresh = () => {
    const ready = cloudReady();
    const cloud = getPrefs().mode === "cloud" && ready;
    fill(
      modeHost,
      h(
        "button",
        {
          type: "button",
          class: "mode-pill",
          title: t("mode.pill.title"),
          onclick: () => {
            if (!cloud && !ready) {
              toast(t("mode.cloud.setup"), "bad");
              navigate("settings");
              return;
            }
            setPref("mode", cloud ? "local" : "cloud");
            refresh();
            window.dispatchEvent(new Event("pcagent:mode"));
          },
        },
        icon(cloud ? "shield" : "pc"),
        cloud ? h("span", null, t("mode.pill.cloud")) : h("span", null, t("mode.pill.local")),
        cloud ? h("span", { class: "faint" }, `· ${t(namesShared() ? "mode.pill.names" : "mode.pill.hidden")}`) : null,
      ),
    );
    fill(
      sendHost,
      running()
        ? h(
            "button",
            { type: "button", class: "send stop", title: t("inv.stop"), "aria-label": t("inv.stop"), disabled: state.inv.status === "stopping", onclick: () => stopInvestigation() },
            icon("stop"),
          )
        : h("button", { type: "button", class: "send", title: t("ask.send"), "aria-label": t("ask.send"), onclick: submit }, icon("send")),
    );
  };
  area.addEventListener("input", grow);
  area.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
  });
  refresh();
  const el = h(
    "div",
    { class: ["ask", big ? "big" : ""] },
    area,
    h(
      "div",
      { class: "ask-bar" },
      h("button", { type: "button", class: "icon-btn", title: t("ask.folder"), "aria-label": t("ask.folder"), onclick: () => navigate("storage") }, icon("folder")),
      modeHost,
      h("span", { class: "spacer" }),
      sendHost,
    ),
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
    refresh,
  };
}
