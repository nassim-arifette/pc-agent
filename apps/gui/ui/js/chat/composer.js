// The question composer and the flow that sends it. Toggles under the text say what leaves for
// Nebius (program names, web lookups, an attached folder and its file names), on by default and
// remembered; "See what leaves" shows the exact first message. No dialog stands before a send:
// each cloud question still takes its exact preview just before, bound to a single-use ticket.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { state, notify, sendMessage, stopInvestigation, setPrivacy, setWebSearch, setFileNames, webSearchActive, waitForPrivacy, beginTurnPreparation, endTurnPreparation } from "../store.js";
import { button, dialog, toast } from "../ui.js";
import { bytes } from "../format.js";
import { navigate } from "../router.js";
import { closePicker, modelPicker } from "./picker.js";
import { webConsentNote } from "./web.js";
import { failureText } from "../failure.js";
import { createImageDraft, pastedImages } from "./image-draft.js";
import { imagePreview, showImage } from "./images.js";

const attachment = createImageDraft({
  prepare: (dataBase64) => call("image_prepare", { dataBase64 }),
  discard: (imageId) => call("image_discard", { imageId }),
  changed: () => notify("image"),
});

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
    preview?.message?.previous_turns?.length ? t(preview.message.previous_turns.length === 1 ? "consent.sent.history.one" : "consent.sent.history", { count: preview.message.previous_turns.length }) : null,
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

function folderName(path) {
  const trimmed = (path || "").replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || trimmed;
}

/** The finished scan goes with the questions unless the person detached it in the composer. */
export function scanAttached() {
  const scan = state.scan;
  return scan.status === "done" && Boolean(scan.scopeId) && state.scanDetached !== scan.scopeId;
}

/** The names of the attached folder's files go too: complete sharing and its toggle on. */
function fileNamesShared() {
  return scanAttached() && namesShared() && state.privacy.file_names !== false;
}

/** The exact preview of a question as it would leave now, with its single-use ticket. */
function previewOf(question, conversationId, imageId = null) {
  return call("cloud_preview", { question, names: namesShared(), conversationId, fileNames: fileNamesShared(), imageId });
}

let shownPreview = null;

function previewKey(question, conversationId, imageId = null) {
  return JSON.stringify([question, conversationId, state.privacyRevision, state.privacy,
    scanAttached() ? state.scan.scopeId : null, state.info?.cloud?.model_id,
    state.info?.cloud?.reasoning, state.routing, imageId, state.conversationRevision]);
}

/**
 * "See what leaves": the exact first message for the text in the composer (or a sample question
 * when it is empty), what it holds and what never leaves. Nothing is sent from here.
 */
export async function showPreview(text, { followUp = false, imageId = null } = {}) {
  if (state.preparing || attachment.pending) return;
  const typed = (text || "").trim();
  const question = typed || t("consent.sample");
  let preview;
  try {
    await waitForPrivacy();
    const conversationId = followUp ? (state.conversation?.id ?? null) : null;
    const key = previewKey(question, conversationId, imageId);
    preview = await previewOf(question, conversationId, imageId);
    // Reuse the reviewed bytes once, only if the typed question and choices are unchanged.
    shownPreview = typed && key === previewKey(question, conversationId, imageId) ? { key, preview } : null;
  } catch (error) {
    toast(failureText(error), "bad");
    return;
  }
  const sent = preview.message?.question ?? question;
  const files = preview.file_name_sharing?.names || [];
  await dialog(
    (close) => [
      h("div", { class: "dialog-head" }, h("div", { class: "metric-icon" }, icon("eye")), h("h2", { class: "dialog-title" }, t("consent.preview.title"))),
      h("p", null, t("consent.preview.lead")),
      h(
        "div",
        { class: "sent-question" },
        h("span", { class: "caption" }, t(typed ? "consent.question" : "consent.question.sample")),
        h("q", null, sent),
        sent.trim() !== question.trim() ? h("span", { class: "note" }, icon("shield"), t("consent.question.cleaned")) : null,
      ),
      disclosure(preview),
      preview.image ? h("div", null, imagePreview(preview.image), h("p", { class: "note" }, t("image.warning"))) : null,
      webConsentNote(),
      files.length
        ? h(
            "div",
            { class: "file-names" },
            h("span", { class: "caption" }, t("consent.files.list", { count: files.length })),
            h("ul", null, files.slice(0, 40).map((n) => h("li", null, h("span", { class: "mono" }, n.relative_path || n.name)))),
            files.length > 40 ? h("span", { class: "note" }, t("consent.files.more", { count: files.length - 40 })) : null,
          )
        : null,
      h(
        "details",
        { class: "more" },
        h("summary", null, icon("chevron"), t("consent.show")),
        h("div", null, h("p", { class: "note" }, t("consent.size", { size: bytes(preview.bytes) })), h("pre", { class: "code" }, JSON.stringify(preview.message, null, 2))),
      ),
      h("div", { class: "dialog-actions" }, button(t("common.close"), { variant: "primary", onClick: () => close(null) })),
    ],
    { label: t("consent.preview.title"), dismissValue: null },
  );
}

/**
 * Sends a question from any screen: a follow-up of the open conversation, or a new one. A cloud
 * question leaves as the composer's toggles say, without a dialog (C5, 9 October): its exact
 * preview is taken just before, bound to a single-use ticket, and kept in the turn's steps.
 */
export async function ask(question, { followUp = false, imageId = null } = {}) {
  const text = (question || "").trim();
  if (!text) return false;
  if (state.inv && (state.inv.status === "running" || state.inv.status === "stopping")) {
    navigate("chat");
    return false;
  }
  if (!beginTurnPreparation()) return false;
  try {
    await waitForPrivacy();
    if (imageId && effectiveMode() !== "complete") {
      toast(t("failure.image_sharing_required"), "bad");
      return false;
    }
    const conversationId = followUp ? (state.conversation?.id ?? null) : null;
    const key = previewKey(text, conversationId, imageId);
    const scopeId = scanAttached() ? state.scan.scopeId : null;
    let previewId = null;
    let scopeIds = [];
    let sent = null;
    const reviewed = shownPreview?.key === key ? shownPreview.preview : null;
    shownPreview = null;
    if (effectiveMode() !== "local") {
      const preview = reviewed ?? await previewOf(text, conversationId, imageId);
      previewId = preview.id;
      sent = preview.message ?? null;
      scopeIds = scopeId && preview.storage_folder ? [scopeId] : [];
    }
    if (key !== previewKey(text, followUp ? (state.conversation?.id ?? null) : null, imageId)) {
      toast(t("share.changed"));
      return false;
    }
    navigate("chat");
    return await sendMessage(text, { followUp, previewId, scopeIds, sent, imageId });
  } catch (error) {
    toast(failureText(error), "bad");
    return false;
  } finally {
    endTurnPreparation();
  }
}

/** From a local result or Storage: switch back to the AI when it is set up, then ask. */
export async function askWithAi(question, { followUp = true } = {}) {
  if (effectiveMode() === "local") {
    if (!cloudReady()) {
      toast(t("mode.cloud.setup"), "bad");
      navigate("settings");
      return;
    }
    await setPrivacy("complete");
  }
  return ask(question, { followUp });
}

/** Answers locally once, after a cloud failure, by switching the stored choice to local. */
export async function askLocally(question, { followUp = true } = {}) {
  await setPrivacy("local");
  toast(t("privacy.switchedLocal"));
  return ask(question, { followUp });
}

/** One toggle under the composer: pressed when that part leaves with the question. */
function shareToggle({ key, iconName, label, title, pressed, onClick, unavailable = false }) {
  return h(
    "button",
    { type: "button", class: ["share-toggle", unavailable ? "unavailable" : ""], "data-share-key": key, "aria-pressed": String(pressed), "aria-disabled": state.privacyPending > 0 ? "true" : null, title, onclick: () => { if (!state.privacyPending) onClick(); } },
    icon(iconName),
    h("span", null, label),
  );
}

/** What leaves with a cloud question, each part one click away from being left out. */
function shareToggles({ followUp, text }) {
  const names = namesShared();
  const webKey = Boolean(state.info?.cloud?.search_key_configured);
  const web = webSearchActive();
  const attached = scanAttached();
  const folder = folderName(state.scan.root);
  const run = (change) => async () => {
    try {
      await change();
    } catch (error) {
      toast(failureText(error), "bad");
    }
  };
  return [
    shareToggle({
      key: "names",
      iconName: names ? "cloud" : "shield",
      label: t("share.names"),
      title: t(names ? "share.names.on" : "share.names.off"),
      pressed: names,
      onClick: run(() => setPrivacy(names ? "anonymized" : "complete")),
    }),
    shareToggle({
      key: "web",
      iconName: "globe",
      label: t("share.web"),
      title: t(!webKey ? "share.web.noKey" : web ? "share.web.on" : "share.web.off"),
      pressed: web,
      unavailable: !webKey,
      onClick: webKey
        ? run(() => setWebSearch(!state.privacy.web_search))
        : () => {
            toast(t("share.web.noKey"));
            navigate("settings");
          },
    }),
    state.scan.status === "done" && state.scan.scopeId
      ? shareToggle({
          key: "folder",
          iconName: "folder",
          label: folder,
          title: t(attached ? "share.folder.on" : "share.folder.off", { folder }),
          pressed: attached,
          onClick: () => {
            state.scanDetached = attached ? state.scan.scopeId : null;
            notify("privacy");
          },
        })
      : null,
    attached && names
      ? shareToggle({
          key: "files",
          iconName: "file",
          label: t("share.files"),
          title: t(state.privacy.file_names !== false ? "share.files.on" : "share.files.off", { folder }),
          pressed: state.privacy.file_names !== false,
          onClick: run(() => setFileNames(state.privacy.file_names === false)),
        })
      : null,
    h(
      "button",
      { type: "button", class: "icon-btn share-preview", title: t("share.preview"), "aria-label": t("share.preview"), onclick: () => showPreview(text(), { followUp, imageId: attachment.image?.id ?? null }) },
      icon("eye"),
    ),
  ];
}

/** Local analysis: nothing leaves. A click goes back to the AI, or to Settings to set it up. */
function localPill() {
  const noKey = state.privacy.mode !== "local";
  return h(
    "button",
    {
      type: "button",
      class: "mode-pill",
      title: t(cloudReady() ? "share.local.back" : "mode.cloud.setup"),
      onclick: async () => {
        if (!cloudReady()) {
          navigate("settings");
          return;
        }
        try {
          await setPrivacy("complete");
        } catch (error) {
          toast(failureText(error), "bad");
        }
      },
    },
    icon(MODE_ICONS.local),
    h("span", null, t("mode.pill.local")),
    noKey ? h("span", { class: "faint" }, `· ${t("mode.pill.noKey")}`) : null,
  );
}

/** The composer: a growing text area, the sharing toggles, and Send or Stop while a run is going. */
export function composer({ autofocus = false, followUp = false, big = false } = {}) {
  attachment.bind(state.conversationRevision);
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
  const pickHost = h("span", { class: "pick-slot" });
  const sendHost = h("span", { class: "send-host" });
  const imageHost = h("div", { class: "image-draft", "aria-live": "polite" });
  const running = () => Boolean(state.inv && (state.inv.status === "running" || state.inv.status === "stopping"));
  const attach = async (files) => {
    if (running() || state.preparing) return;
    try { await attachment.attach(Array.from(files)); }
    catch (error) { toast(failureText(error), "bad"); }
  };
  const fileInput = h("input", { type: "file", accept: "image/png,image/jpeg", hidden: true, onchange: () => {
    if (fileInput.files.length) void attach(fileInput.files);
    fileInput.value = "";
  } });
  const attachButton = h("button", { type: "button", class: "icon-btn", title: t("image.attach"), "aria-label": t("image.attach"), onclick: () => fileInput.click() }, icon("image"));
  const submit = async () => {
    if (running() || state.preparing || attachment.pending) return;
    const text = area.value;
    if (!text.trim()) return;
    const imageId = attachment.image?.id ?? null;
    if (await ask(text, { followUp, imageId }) && area.value === text) {
      if (imageId) attachment.consume(imageId);
      area.value = "";
      grow();
    }
  };
  const refresh = () => {
    attachment.bind(state.conversationRevision);
    // The welcome composer is replaced by the thread when a send starts. Keep its text stable
    // until that handoff; otherwise edits made while preparing would be lost with the old node.
    area.readOnly = state.preparing;
    area.setAttribute("aria-busy", String(state.preparing));
    const focusedShare = modeHost.contains(document.activeElement) ? document.activeElement.dataset.shareKey : null;
    const mode = effectiveMode();
    fill(modeHost, mode === "local" ? localPill() : shareToggles({ followUp, text: () => area.value }));
    if (focusedShare) modeHost.querySelector(`[data-share-key="${CSS.escape(focusedShare)}"]`)?.focus({ preventScroll: true });
    // The model and its reasoning only matter when the question goes to the AI.
    closePicker();
    fill(pickHost, attachment.image ? h("span", { class: "image-model" }, t("image.model")) : mode === "local" ? null : modelPicker({ disabled: running() || state.preparing }));
    attachButton.disabled = running() || state.preparing || attachment.pending;
    const image = attachment.image;
    fill(imageHost, attachment.pending
      ? h("p", { class: "note", role: "status" }, t("image.preparing"))
      : image ? [
        h("div", { class: "image-draft-row" },
          h("button", { type: "button", class: "image-thumb", "aria-label": t("image.preview"), onclick: () => showImage(image) }, h("img", { src: image.data_url, alt: t("image.alt") })),
          h("div", null, h("strong", null, t("image.ready")), h("p", { class: "note" }, t("image.details", { width: image.width, height: image.height, size: bytes(image.bytes) }))),
          h("button", { type: "button", class: "icon-btn", "aria-label": t("image.remove"), title: t("image.remove"), disabled: state.preparing, onclick: () => { attachment.clear(); area.focus(); } }, icon("x")),
        ),
        h("p", { class: "image-warning" }, t(mode === "complete" ? "image.warning" : "failure.image_sharing_required")),
      ] : null);
    fill(
      sendHost,
      running()
        ? h(
            "button",
            { type: "button", class: "send stop", title: t("inv.stop"), "aria-label": t("inv.stop"), disabled: state.inv.status === "stopping", onclick: () => stopInvestigation() },
            icon("stop"),
          )
        : h("button", { type: "button", class: "send", title: t("ask.send"), "aria-label": t("ask.send"), disabled: state.preparing || attachment.pending || Boolean(image && mode !== "complete"), onclick: submit }, icon("send")),
    );
  };
  area.addEventListener("input", grow);
  area.addEventListener("paste", (event) => {
    const files = pastedImages(event.clipboardData);
    if (!files.length) return;
    event.preventDefault();
    void attach(files);
  });
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
    fileInput,
    imageHost,
    area,
    h(
      "div",
      { class: "ask-bar" },
      attachButton,
      h("button", { type: "button", class: "icon-btn", title: t("ask.folder"), "aria-label": t("ask.folder"), onclick: () => navigate("storage") }, icon("folder")),
      modeHost,
      h("span", { class: "spacer" }),
      pickHost,
      sendHost,
    ),
  );
  if (autofocus) requestAnimationFrame(() => area.focus());
  return {
    el,
    text: () => area.value,
    focus: () => area.focus(),
    setText(text) {
      area.value = text;
      grow();
      area.focus();
    },
    refresh,
  };
}
