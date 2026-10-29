/** Synthetic text only: the design mock never accesses the browser microphone or a network. */
export function createVoiceFixture({ params = new URLSearchParams(), emit, schedule = setTimeout, unschedule = clearTimeout }) {
  let settings = { engine: "auto", language: "auto", device_id: null };
  let state = { phase: "idle", sequence: 0, session_id: null, text: "", partial: "", level: 0 };
  let sequence = 0;
  let timer;
  let downloadTimer;
  const model = {
    name: "Whisper base multilingual", bytes: 147951465,
    sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
    installed: params.get("voiceModel") !== "missing", downloading: false, downloaded_bytes: 0, error: null,
  };
  const busy = () => ["starting", "listening", "finalizing"].includes(state.phase);
  const fail = (code) => { throw { code, message: code }; };
  const guard = () => { if (busy() || model.downloading) fail("busy"); };
  const update = (patch) => {
    state = { ...state, ...patch, sequence: state.sequence + 1 };
    emit("voice:state", { ...state });
    return { ...state };
  };
  const modelEvent = () => emit("voice:model", { ...model });
  const stopDownload = (cancelled = false) => {
    unschedule(downloadTimer);
    model.downloading = false;
    model.error = cancelled ? { code: "cancelled" } : null;
    modelEvent();
  };
  const fullText = () => settings.language === "en"
    ? "Why is my computer slow today?" : "Pourquoi mon ordinateur est-il lent aujourd’hui ?";
  const commands = {
    voice_state: () => ({ ...state }),
    voice_settings_get: () => ({ ...settings }),
    voice_settings_set: ({ settings: next }) => { guard(); settings = { ...next }; },
    voice_devices: () => params.get("voice") === "no_device" ? [] : [{ id: "synthetic-mic", name: "Synthetic microphone (mock)", is_default: true }],
    voice_model_state: () => ({ ...model }),
    voice_start: ({ consent }) => {
      if (!consent) fail("consent_required");
      guard();
      const scenario = params.get("voice");
      if (["no_device", "permission_denied", "device_unavailable", "engine_unavailable"].includes(scenario)) fail(scenario);
      if (!model.installed && settings.engine !== "windows") fail("model_missing");
      const initial = update({
        phase: "listening", session_id: `voice-${++sequence}`, text: "", partial: "",
        engine: settings.engine === "windows" ? "windows" : "whisper",
        language: settings.language === "auto" ? "fr" : settings.language, level: 0.13, error: null,
      });
      timer = schedule(() => update({ partial: fullText(), level: 0.3 }), 450);
      return initial;
    },
    voice_stop: ({ sessionId, cancel }) => {
      if (sessionId !== state.session_id) fail("stale_session");
      unschedule(timer);
      if (cancel) return update({ phase: "cancelled", text: "", partial: "", level: 0, error: null });
      const next = update({ phase: "finalizing", level: 0 });
      timer = schedule(() => update({ phase: "review", text: fullText(), partial: "" }), 200);
      return next;
    },
    voice_model_download: ({ consent }) => {
      if (!consent) fail("consent_required");
      guard();
      if (model.installed) fail("busy");
      model.downloading = true;
      model.downloaded_bytes = 0;
      model.error = null;
      modelEvent();
      const tick = () => {
        model.downloaded_bytes = Math.min(model.bytes, model.downloaded_bytes + Math.ceil(model.bytes / 5));
        if (model.downloaded_bytes === model.bytes) {
          model.installed = true;
          stopDownload();
        } else { modelEvent(); downloadTimer = schedule(tick, 250); }
      };
      downloadTimer = schedule(tick, 250);
    },
    voice_model_cancel: () => stopDownload(true),
    voice_model_delete: () => { guard(); model.installed = false; model.downloaded_bytes = 0; modelEvent(); },
  };
  return { commands };
}
