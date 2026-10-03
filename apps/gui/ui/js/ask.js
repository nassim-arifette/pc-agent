// The question composer and the flow that sends it: the stored sharing choice (complete,
// anonymized or local), and before anything leaves for Nebius, an exact preview of the first
// message, including the cleaned question and any file names, approved with a single-use ticket.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { call } from "./api.js";
import { state, sendMessage, stopInvestigation, setPrivacy } from "./store.js";
import { button, dialog, toast } from "./ui.js";
import { bytes } from "./format.js";
import { navigate } from "./router.js";

export const MODES = ["complete", "anonymized", "local"];
const MODE_ICONS = { complete: "cloud", anonymized: "shield", local: "pc" };

export function cloudReady() {
  return Boolean(state.info?.cloud?.ready);
}

/** Whether program names go to the AI: only in complete sharing. */
export function namesShared() {
  return state.privacy.mode === "complete";
}

/** The mode a question will actually use: a cloud mode needs a usable key and model. */
export function effectiveMode() {
  return state.privacy.mode !== "local" && cloudReady() ? state.privacy.mode : "local";
}

/** Lists of what is sent and never sent, shared by the consent dialog and Settings. */
export function disclosure(preview, mode = state.privacy.mode) {
  const names = mode === "complete";
  const sent = [
    t("consent.sent.question"),
    preview?.message?.previous_turns?.length ? t("consent.sent.history", { count: preview.message.previous_turns.length }) : null,
    preview ? t("consent.sent.measures", { count: preview.measured }) : null,
    preview ? t("consent.sent.findings", { count: preview.findings }) : null,
    names ? (preview?.names ? t("consent.sent.names", { count: preview.names }) : t("consent.sent.namesAny")) : t("consent.sent.refs"),
    preview?.file_name_sharing?.names?.length ? t("consent.sent.files", { count: preview.file_name_sharing.names.length }) : null,
  ].filter(Boolean);
  const filesShared = Boolean(preview?.file_name_sharing?.names?.length);
  const never = [
    names ? t("consent.never.pids") : t("consent.never.names"),
    t(filesShared ? "consent.never.contents" : "consent.never.paths"),
    t("consent.never.ids"),
    t("consent.never.key"),
  ];
  return h(
    "div",
    { class: "two-lists" },
    h("div", null, h("h4", null, icon("cloud"), t("consent.sent.title")), h("ul", null, sent.map((x) => h("li", null, x)))),
    h("div", null, h("h4", null, icon("lock"), t("consent.never.title")), h("ul", null, never.map((x) => h("li", null, x)))),
  );
}

/** The three sharing choices as selectable cards; `onPick(mode)` runs when one is chosen. */
export function modeCards(selected, onPick) {
  return h(
    "div",
    { class: "mode-cards", role: "radiogroup", "aria-label": t("privacy.choose") },
    MODES.map((mode) => {
      const locked = mode !== "local" && !cloudReady();
      return h(
        "button",
        {
          type: "button",
          role: "radio",
          class: "mode-card",
          "aria-checked": String(mode === selected),
          title: locked ? t("mode.cloud.setup") : null,
          onclick: () => onPick(mode),
        },
        h("span", { class: "mode-card-icon" }, icon(MODE_ICONS[mode])),
        h("span", { class: "mode-card-text" }, h("strong", null, t(`privacy.${mode}`)), h("span", null, t(`privacy.${mode}.note`))),
        mode === "complete" ? h("span", { class: "mode-card-tag" }, t("privacy.recommended")) : null,
      );
    }),
  );
}

/** Opens the sharing choice; resolves with the stored settings, or null when dismissed. */
export async function choosePrivacy() {
  let picked = state.privacy.mode;
  const host = h("div");
  const draw = () => fill(host, modeCards(picked, (mode) => {
    picked = mode;
    draw();
  }));
  draw();
  const mode = await dialog(
    (close) => [
      h("h2", { class: "dialog-title" }, t("privacy.title")),
      h("p", null, t("privacy.lead")),
      host,
      h(
        "div",
        { class: "dialog-actions" },
        button(t("common.cancel"), { onClick: () => close(null) }),
        button(t("privacy.save"), { variant: "primary", onClick: () => close(picked) }),
      ),
    ],
    { label: t("privacy.title"), dismissValue: null },
  );
  if (!mode) return null;
  if (mode !== "local" && !cloudReady()) {
    toast(t("mode.cloud.setup"), "bad");
    navigate("settings");
    return null;
  }
  // Choosing a cloud mode here is not consent: the first preview still has to be approved.
  return setPrivacy(mode, mode === "local" ? true : state.privacy.mode === mode && state.privacy.confirmed);
}

function folderName(path) {
  const trimmed = (path || "").replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}

/** Previews and asks; resolves with { previewId, scopeIds } or null when the person cancels. */
async function consent(question, conversationId) {
  const names = namesShared();
  let fileNames = false;
  let preview = null;
  try {
    preview = await call("cloud_preview", { question, names, conversationId, fileNames });
  } catch (error) {
    toast(error.message, "bad");
    return null;
  }
  let useStorage = Boolean(preview.storage_folder);
  let dontAsk = false;
  let revision = 0;
  let pending = false;
  let closed = false;
  let sendButton = null;
  const listHost = h("div");
  const questionHost = h("div");
  const filesHost = h("div");
  const messageHost = h("div");
  const draw = () => {
    fill(listHost, disclosure(preview));
    // The question as it will be sent, after identities and secrets were removed.
    const sent = preview.message?.question ?? question;
    fill(
      questionHost,
      h(
        "div",
        { class: "sent-question" },
        h("span", { class: "caption" }, t("consent.question")),
        h("q", null, sent),
        sent.trim() !== question.trim() ? h("span", { class: "note" }, icon("shield"), t("consent.question.cleaned")) : null,
      ),
    );
    const shared = preview.file_name_sharing?.names || [];
    fill(
      filesHost,
      fileNames && shared.length
        ? h(
            "div",
            { class: "file-names" },
            h("span", { class: "caption" }, t("consent.files.list", { count: shared.length })),
            h("ul", null, shared.slice(0, 40).map((n) => h("li", null, h("span", { class: "mono" }, n.relative_path || n.name)))),
            shared.length > 40 ? h("span", { class: "note" }, t("consent.files.more", { count: shared.length - 40 })) : null,
          )
        : null,
    );
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
  /** A new preview replaces the ticket; sending waits for it and ignores older answers. */
  const repreview = async () => {
    const request = ++revision;
    pending = true;
    if (sendButton) sendButton.disabled = true;
    try {
      const next = await call("cloud_preview", { question, names, conversationId, fileNames });
      if (closed || request !== revision) return;
      preview = next;
      pending = false;
      if (sendButton) sendButton.disabled = false;
      draw();
    } catch (error) {
      if (closed || request !== revision) return;
      preview = null;
      fill(messageHost, h("p", { class: "note" }, error.message));
      toast(error.message, "bad");
    }
  };
  const storageRow = preview.storage_folder
    ? h(
        "label",
        { class: "check" },
        h("input", {
          type: "checkbox",
          checked: true,
          onchange: (e) => {
            useStorage = e.target.checked;
            if (!useStorage && fileNames) {
              fileNames = false;
              filesRow.querySelector("input").checked = false;
              repreview();
            }
          },
        }),
        t("consent.storage", { folder: folderName(preview.storage_folder) }),
      )
    : null;
  // File names are a separate consent, only in complete sharing and for the scanned folder.
  const filesRow =
    preview.storage_folder && names
      ? h(
          "label",
          { class: "check" },
          h("input", {
            type: "checkbox",
            onchange: (e) => {
              fileNames = e.target.checked;
              if (fileNames && !useStorage) {
                useStorage = true;
                storageRow.querySelector("input").checked = true;
              }
              repreview();
            },
          }),
          h("span", null, h("strong", null, t("consent.files")), h("span", { class: "hint" }, ` — ${t("consent.files.note")}`)),
        )
      : null;
  const modeRow = h(
    "div",
    { class: "consent-mode" },
    icon(MODE_ICONS[state.privacy.mode]),
    h("span", null, h("strong", null, t(`privacy.${state.privacy.mode}`)), " · ", t(`privacy.${state.privacy.mode}.note`)),
    h("span", { class: "spacer" }),
  );
  const decision = await dialog(
    (close) => {
      modeRow.append(
        h("button", { type: "button", class: "link-btn", onclick: () => close({ change: true }) }, t("privacy.change")),
      );
      return [
        h("div", { class: "dialog-head" }, h("div", { class: "metric-icon" }, icon("cloud")), h("h2", { class: "dialog-title" }, t("consent.title"))),
        h("p", null, t("consent.lead")),
        modeRow,
        questionHost,
        listHost,
        storageRow,
        filesRow,
        filesHost,
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
          button(t("common.cancel"), { onClick: () => close(null) }),
          (sendButton = h(
            "button",
            {
              type: "button",
              class: "btn btn-primary",
              "data-autofocus": true,
              onclick: () => {
                if (pending || !preview) return;
                close({ send: true });
              },
            },
            icon("send"),
            t("consent.send"),
          )),
        ),
      ];
    },
    { label: t("consent.title"), dismissValue: null },
  );
  closed = true;
  if (!decision) return null;
  if (decision.change) {
    const settings = await choosePrivacy();
    return settings ? { retry: true } : null;
  }
  // Approving this preview confirms the stored choice; the same mode keeps the ticket valid.
  if (!state.privacy.confirmed) await setPrivacy(state.privacy.mode, true);
  if (dontAsk) {
    state.skipConsent = true;
    state.lastUseStorage = useStorage;
    state.lastStorageFolder = useStorage ? preview.storage_folder : null;
    state.lastFileNames = fileNames;
  }
  return { previewId: preview.id, scopeIds: useStorage && state.scan.scopeId ? [state.scan.scopeId] : [] };
}

/** Sends a question from any screen: a follow-up of the open conversation, or a new one. */
export async function ask(question, { followUp = false } = {}) {
  const text = (question || "").trim();
  if (!text) return;
  if (state.inv && (state.inv.status === "running" || state.inv.status === "stopping")) {
    navigate("chat");
    return;
  }
  // First cloud question: the choice itself is made deliberately, once. Without a usable key the
  // question is answered locally, as the composer and status already say.
  if (effectiveMode() !== "local" && !state.privacy.confirmed && !state.privacyAsked) {
    state.privacyAsked = true;
    const settings = await choosePrivacy();
    if (!settings) return;
  }
  const conversationId = followUp ? (state.conversation?.id ?? null) : null;
  let previewId = null;
  let scopeIds = [];
  if (effectiveMode() !== "local") {
    if (state.skipConsent && state.privacy.confirmed) {
      try {
        const fileNames = Boolean(state.lastFileNames && namesShared());
        const preview = await call("cloud_preview", { question: text, names: namesShared(), conversationId, fileNames });
        previewId = preview.id;
        // Session consent applies to the folder actually approved, not to a later different scan.
        const sameFolder = Boolean(state.lastUseStorage && preview.storage_folder && preview.storage_folder === state.lastStorageFolder);
        scopeIds = sameFolder && state.scan.scopeId ? [state.scan.scopeId] : [];
      } catch (error) {
        toast(error.message, "bad");
        return;
      }
    } else {
      const decision = await consent(text, conversationId);
      if (!decision) return;
      if (decision.retry) return ask(text, { followUp });
      previewId = decision.previewId;
      scopeIds = decision.scopeIds;
    }
  } else if (state.scan.status === "done" && state.scan.scopeId && state.attachScan) {
    scopeIds = [state.scan.scopeId];
  }
  navigate("chat");
  await sendMessage(text, { followUp, previewId, scopeIds });
}

/** From a local result or Storage: choose a cloud mode first if needed, then ask. */
export async function askWithAi(question, { followUp = true } = {}) {
  if (effectiveMode() === "local") {
    const settings = await choosePrivacy();
    if (!settings || settings.mode === "local") return;
  }
  return ask(question, { followUp });
}

/** Answers locally once, after a cloud failure, by switching the stored choice to local. */
export async function askLocally(question, { followUp = true } = {}) {
  await setPrivacy("local", true);
  toast(t("privacy.switchedLocal"));
  return ask(question, { followUp });
}

/** The composer: a growing text area, the sharing choice, and Send or Stop while a run is going. */
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
    const text = area.value;
    if (!text.trim()) return;
    area.value = "";
    grow();
    ask(text, { followUp });
  };
  const refresh = () => {
    const mode = effectiveMode();
    fill(
      modeHost,
      h(
        "button",
        { type: "button", class: "mode-pill", title: t("mode.pill.title"), onclick: () => choosePrivacy() },
        icon(MODE_ICONS[mode]),
        h("span", null, t(`mode.pill.${mode}`)),
        mode === "local" && state.privacy.mode !== "local" ? h("span", { class: "faint" }, `· ${t("mode.pill.noKey")}`) : null,
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
