import { h } from "../dom.js";
import { call, on } from "../api.js";
import { t } from "../i18n.js";
import { button } from "../ui.js";
import { navigate } from "../router.js";
import { createVoiceDraft } from "./voice-draft.js";

export function voiceError(error) {
  const key = `voice.error.${error?.code || "recognition_failed"}`;
  const text = t(key);
  return text === key ? t("voice.error.recognition_failed") : text;
}

export function voiceComposer({ area, changed, grow, unavailable }) {
  const status = h("span", { role: "status", "aria-live": "polite" });
  const transcript = h("p", { class: "voice-transcript" });
  const meter = h("meter", { min: 0, max: 1, value: 0, "aria-label": t("voice.level") });
  const mic = button(t("voice.start"), { onClick: () => toggle() });
  mic.setAttribute("aria-keyshortcuts", "Control+Shift+M");
  mic.title = t("voice.shortcut");
  const cancel = button(t("voice.cancel"), { onClick: () => draft.stop(true) });
  const reviewed = button(t("voice.reviewed"), { onClick: () => { draft.reviewed(); area.focus(); } });
  const setup = button(t("voice.settings"), { onClick: () => navigate("settings") });
  const controls = h("div", { class: "row" }, meter, cancel, reviewed, setup);
  const el = h("div", { class: "voice-draft" }, status, transcript, controls);
  let disposed = false;
  let off = () => {};
  const draft = createVoiceDraft({
    invoke: call,
    apply: (text) => { area.value = text; grow(); },
    changed: () => { draw(); changed(); },
  });
  const ready = on("voice:state", (event) => { if (!disposed) draft.accept(event); })
    .then((unlisten) => { if (disposed) unlisten(); else off = unlisten; });
  const draw = () => {
    const value = draft.state;
    mic.textContent = t(draft.busy ? "voice.stop" : "voice.start");
    mic.setAttribute("aria-pressed", String(draft.busy));
    mic.disabled = unavailable() || value.phase === "starting" || value.phase === "finalizing" || draft.reviewRequired;
    status.textContent = value.error ? voiceError(value.error)
      : t(draft.reviewRequired ? "voice.review" : `voice.${value.phase}`);
    if (value.language) status.textContent += ` · ${value.language.toUpperCase()} · ${value.engine === "windows" ? "Windows SAPI" : "Whisper"}`;
    transcript.textContent = draft.busy ? [value.text, value.partial].filter(Boolean).join(" ") : "";
    meter.value = Math.min(1, Math.max(0, value.level || 0));
    meter.hidden = !draft.busy;
    cancel.hidden = !draft.busy && !draft.reviewRequired;
    reviewed.hidden = !draft.reviewRequired;
    setup.hidden = !value.error;
  };
  const toggle = async () => {
    if (disposed || unavailable()) return;
    await ready;
    if (disposed) return;
    if (draft.busy) await draft.stop();
    else await draft.start(area.value);
  };
  const shortcut = (event) => {
    if (event.ctrlKey && event.shiftKey && event.code === "KeyM") {
      event.preventDefault();
      void toggle();
    }
  };
  area.addEventListener("keydown", shortcut);
  const visibility = () => {
    if (document.hidden && draft.busy) void draft.stop(true);
  };
  document.addEventListener("visibilitychange", visibility);
  draw();
  return {
    el, mic,
    get busy() { return draft.busy; },
    get blocked() { return draft.busy || draft.reviewRequired; },
    bind: (owner) => draft.bind(owner),
    refresh: draw,
    dispose() {
      disposed = true;
      off();
      draft.dispose();
      area.removeEventListener("keydown", shortcut);
      document.removeEventListener("visibilitychange", visibility);
    },
  };
}
