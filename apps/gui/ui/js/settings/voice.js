import { h, fill } from "../dom.js";
import { call, on } from "../api.js";
import { t } from "../i18n.js";
import { button } from "../ui.js";
import { bytes } from "../format.js";
import { voiceError } from "../chat/voice.js";

export function voiceSettingsCard() {
  let disposed = false;
  let off = () => {};
  let settings = { engine: "auto", language: "auto", device_id: null };
  let model = null;
  const error = h("p", { class: "note", role: "alert" });
  const modelHost = h("div", { class: "stack" });
  const progress = h("progress", { max: 1, value: 0, "aria-label": t("voice.downloadProgress") });
  const progressText = h("span", { class: "note" });
  const engine = h("select", { "aria-label": t("voice.engine") },
    ["auto", "whisper", "windows"].map((value) => h("option", { value }, t(`voice.engine.${value}`))));
  const language = h("select", { "aria-label": t("voice.language") },
    ["auto", "fr", "en"].map((value) => h("option", { value }, t(`voice.language.${value}`))));
  const device = h("select", { "aria-label": t("voice.device") });
  const report = (failure) => { if (!disposed) error.textContent = voiceError(failure); };
  const run = async (command, args) => {
    error.textContent = "";
    try { await call(command, args); }
    catch (failure) { report(failure); }
  };
  const sync = () => {
    engine.value = settings.engine;
    language.value = settings.language;
    device.value = settings.device_id || "";
  };
  const save = async () => {
    const next = { engine: engine.value, language: language.value, device_id: device.value || null };
    engine.disabled = language.disabled = device.disabled = true;
    try { await call("voice_settings_set", { settings: next }); settings = next; error.textContent = ""; }
    catch (failure) { report(failure); sync(); }
    finally { engine.disabled = language.disabled = device.disabled = false; }
  };
  for (const control of [engine, language, device]) control.addEventListener("change", save);
  const devices = async () => {
    fill(device, h("option", { value: "" }, t("voice.defaultDevice")));
    try {
      const values = await call("voice_devices");
      if (disposed) return;
      for (const value of values) device.append(h("option", { value: value.id }, value.name));
      if (settings.device_id && !values.some((value) => value.id === settings.device_id)) {
        device.append(h("option", { value: settings.device_id }, t("voice.missingDevice")));
      }
      if (!values.length) report({ code: "no_device" });
      sync();
    } catch (failure) { report(failure); }
  };
  const drawModel = () => {
    if (disposed || !model) return;
    progress.value = model.bytes ? model.downloaded_bytes / model.bytes : 0;
    progressText.textContent = `${bytes(model.downloaded_bytes)} / ${bytes(model.bytes)}`;
    if (model.downloading && progress.parentNode === modelHost) return;
    const consent = h("input", { type: "checkbox" });
    const download = button(t("voice.download"), {
      disabled: true,
      onClick: async () => {
        if (!consent.checked) return;
        download.disabled = true;
        await run("voice_model_download", { consent: true });
        model = await call("voice_model_state");
        drawModel();
      },
    });
    consent.addEventListener("change", () => { download.disabled = !consent.checked; });
    fill(modelHost,
      h("strong", null, `${model.name} · ${bytes(model.bytes)}`),
      h("p", { class: "note", role: "status" }, t(model.downloading ? "voice.downloading" : model.installed ? "voice.installed" : "voice.notInstalled")),
      model.error ? h("p", { role: "alert" }, voiceError(model.error)) : null,
      model.downloading ? [
        progress,
        progressText,
        button(t("voice.cancelDownload"), { onClick: () => run("voice_model_cancel") }),
      ] : model.installed
        ? button(t("voice.delete"), { onClick: async () => { await run("voice_model_delete"); model = await call("voice_model_state"); drawModel(); } })
        : [h("label", { class: "row" }, consent, t("voice.downloadConsent", { size: bytes(model.bytes) })), download],
      h("details", null, h("summary", null, t("voice.integrity")),
        h("p", { class: "note" }, t("voice.integrityNote")),
        h("code", { class: "voice-digest" }, `SHA-256: ${model.sha256}`),
        h("p", { class: "voice-digest" }, model.url)),
    );
  };
  const row = (label, control) => h("label", { class: "setting-row" }, h("strong", null, label), control);
  const el = h("section", { class: "card card-pad" },
    h("h2", { class: "card-title" }, t("voice.title")),
    h("p", { class: "note" }, t("voice.privacy")),
    row(t("voice.engine"), engine),
    h("p", { class: "note" }, t("voice.fallback")),
    row(t("voice.language"), language),
    row(t("voice.device"), device),
    button(t("voice.refreshDevices"), { onClick: devices }),
    error, modelHost,
  );
  void (async () => {
    try {
      const unlisten = await on("voice:model", (value) => { model = value; drawModel(); });
      if (disposed) { unlisten(); return; }
      off = unlisten;
      settings = await call("voice_settings_get");
      model = await call("voice_model_state");
      if (disposed) return;
      sync();
      drawModel();
      await devices();
    } catch (failure) { report(failure); }
  })();
  return { el, dispose() { disposed = true; off(); } };
}
