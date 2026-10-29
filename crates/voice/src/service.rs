use crate::{model, ErrorCode, ModelInfo, Phase, Result, Settings, VoiceState};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

type VoiceSink = Arc<dyn Fn(VoiceState) + Send + Sync>;
type ModelSink = Arc<dyn Fn(ModelInfo) + Send + Sync>;

struct Inner {
    settings: Settings,
    state: VoiceState,
    model: ModelInfo,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    download_cancel: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct VoiceService {
    root: Arc<PathBuf>,
    inner: Arc<Mutex<Inner>>,
    voice_sink: VoiceSink,
    model_sink: ModelSink,
}
impl VoiceService {
    pub fn new(
        root: PathBuf,
        voice_sink: impl Fn(VoiceState) + Send + Sync + 'static,
        model_sink: impl Fn(ModelInfo) + Send + Sync + 'static,
    ) -> Self {
        let settings = fs::read(root.join("settings.json"))
            .ok()
            .filter(|bytes| bytes.len() <= 4096)
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let model = ModelInfo::new(&root);
        Self {
            root: Arc::new(root),
            inner: Arc::new(Mutex::new(Inner {
                settings,
                state: VoiceState::default(),
                model,
                stop: Arc::new(AtomicBool::new(false)),
                cancel: Arc::new(AtomicBool::new(false)),
                download_cancel: Arc::new(AtomicBool::new(false)),
            })),
            voice_sink: Arc::new(voice_sink),
            model_sink: Arc::new(model_sink),
        }
    }
    pub fn state(&self) -> VoiceState {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .state
            .clone()
    }
    pub fn settings(&self) -> Settings {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .settings
            .clone()
    }
    pub fn model(&self) -> ModelInfo {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .model
            .clone()
    }
    pub fn set_settings(&self, settings: Settings) -> Result<()> {
        let mut inner = self.inner.lock().map_err(|_| ErrorCode::Busy)?;
        if inner.state.phase.active() || inner.model.downloading {
            return Err(ErrorCode::Busy.into());
        }
        if let Some(id) = &settings.device_id {
            if !crate::devices()?.iter().any(|d| &d.id == id) {
                return Err(ErrorCode::DeviceUnavailable.into());
            }
        }
        fs::create_dir_all(&*self.root).map_err(|_| ErrorCode::StorageUnavailable)?;
        let bytes = serde_json::to_vec(&settings).map_err(|_| ErrorCode::StorageUnavailable)?;
        let path = self.root.join("settings.json");
        fs::write(&path, bytes).map_err(|_| ErrorCode::StorageUnavailable)?;
        inner.settings = settings;
        Ok(())
    }
    pub fn download_model(&self, consent: bool) -> Result<()> {
        if !consent {
            return Err(ErrorCode::ConsentRequired.into());
        }
        let cancel = {
            let mut inner = self.inner.lock().map_err(|_| ErrorCode::Busy)?;
            if inner.state.phase.active() || inner.model.downloading {
                return Err(ErrorCode::Busy.into());
            }
            if inner.model.installed {
                return Err(ErrorCode::Busy.into());
            }
            inner.model.downloading = true;
            inner.model.error = None;
            inner.model.downloaded_bytes = 0;
            inner.download_cancel = Arc::new(AtomicBool::new(false));
            inner.download_cancel.clone()
        };
        (self.model_sink)(self.model());
        let this = self.clone();
        std::thread::spawn(move || {
            let mut last_progress = std::time::Instant::now();
            let result = model::download(&this.root, &cancel, |count| {
                if count != model::MODEL_BYTES && last_progress.elapsed().as_millis() < 100 {
                    return;
                }
                last_progress = std::time::Instant::now();
                let state = {
                    let mut inner = this.inner.lock().unwrap_or_else(|p| p.into_inner());
                    inner.model.downloaded_bytes = count;
                    inner.model.clone()
                };
                (this.model_sink)(state);
            });
            let state = {
                let mut inner = this.inner.lock().unwrap_or_else(|p| p.into_inner());
                inner.model.downloading = false;
                inner.model.installed = result.is_ok();
                inner.model.error = result.err();
                inner.model.clone()
            };
            (this.model_sink)(state);
        });
        Ok(())
    }
    pub fn cancel_download(&self) {
        if let Ok(inner) = self.inner.lock() {
            inner.download_cancel.store(true, Ordering::Release);
        }
    }
    pub fn delete_model(&self) -> Result<()> {
        let state = {
            let mut inner = self.inner.lock().map_err(|_| ErrorCode::Busy)?;
            if inner.state.phase.active() || inner.model.downloading {
                return Err(ErrorCode::Busy.into());
            }
            let path = model::path(&self.root);
            if path.exists() {
                fs::remove_file(path).map_err(|_| ErrorCode::StorageUnavailable)?;
            }
            inner.model = ModelInfo::new(&self.root);
            inner.model.clone()
        };
        (self.model_sink)(state);
        Ok(())
    }
    pub fn start(&self, consent: bool) -> Result<VoiceState> {
        if !consent {
            return Err(ErrorCode::ConsentRequired.into());
        }
        #[cfg(not(windows))]
        return Err(ErrorCode::Unsupported.into());
        #[cfg(windows)]
        {
            let (settings, stop, cancel, id) = {
                let mut inner = self.inner.lock().map_err(|_| ErrorCode::Busy)?;
                if inner.state.phase.active() || inner.model.downloading {
                    return Err(ErrorCode::Busy.into());
                }
                let id = uuid::Uuid::new_v4().to_string();
                inner.stop = Arc::new(AtomicBool::new(false));
                inner.cancel = Arc::new(AtomicBool::new(false));
                inner.state = VoiceState {
                    phase: Phase::Starting,
                    session_id: Some(id.clone()),
                    ..VoiceState::default()
                };
                (
                    inner.settings.clone(),
                    inner.stop.clone(),
                    inner.cancel.clone(),
                    id,
                )
            };
            let state = self.state();
            (self.voice_sink)(state.clone());
            let this = self.clone();
            std::thread::spawn(move || {
                let result = crate::native::run(
                    &settings,
                    &model::path(&this.root),
                    stop,
                    cancel.clone(),
                    |event| {
                        this.update(&id, event);
                    },
                );
                let mut final_state = this.state();
                final_state.level = 0.0;
                final_state.partial.clear();
                if cancel.load(Ordering::Acquire) {
                    final_state.phase = Phase::Cancelled;
                    final_state.text.clear();
                    final_state.error = None;
                } else if let Err(error) = result {
                    final_state.phase = Phase::Error;
                    final_state.error = Some(error);
                } else {
                    final_state.phase = Phase::Review;
                }
                this.update(&id, final_state);
            });
            Ok(state)
        }
    }
    #[cfg(windows)]
    fn update(&self, id: &str, mut state: VoiceState) {
        let mut inner = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if inner.state.session_id.as_deref() != Some(id) {
            return;
        }
        if inner.cancel.load(Ordering::Acquire) {
            state.text.clear();
            state.partial.clear();
            if !state.phase.active() {
                state.phase = Phase::Cancelled;
                state.error = None;
            }
        }
        if state.phase.active() && inner.stop.load(Ordering::Acquire) {
            state.phase = Phase::Finalizing;
        }
        state.session_id = Some(id.to_owned());
        state.sequence = inner.state.sequence.saturating_add(1);
        inner.state = state.clone();
        drop(inner);
        (self.voice_sink)(state);
    }
    pub fn stop(&self, session_id: &str, cancel: bool) -> Result<VoiceState> {
        let state = {
            let mut inner = self.inner.lock().map_err(|_| ErrorCode::Busy)?;
            if inner.state.session_id.as_deref() != Some(session_id) {
                return Err(ErrorCode::StaleSession.into());
            }
            if cancel {
                inner.cancel.store(true, Ordering::Release);
                inner.state.text.clear();
                inner.state.partial.clear();
            }
            inner.stop.store(true, Ordering::Release);
            if inner.state.phase.active() {
                inner.state.phase = Phase::Finalizing;
            } else if cancel {
                inner.state.phase = Phase::Cancelled;
                inner.state.text.clear();
            }
            inner.state.sequence = inner.state.sequence.saturating_add(1);
            inner.state.clone()
        };
        (self.voice_sink)(state.clone());
        Ok(state)
    }
    pub fn cancel(&self) {
        if let Some(id) = self.state().session_id {
            let _ = self.stop(&id, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    fn service() -> VoiceService {
        VoiceService::new(
            std::env::temp_dir().join(format!("pcagent-voice-{}", uuid::Uuid::new_v4())),
            |_| {},
            |_| {},
        )
    }
    #[test]
    fn consent_and_identity_are_required() {
        let service = service();
        assert_eq!(
            service.start(false).unwrap_err().code,
            ErrorCode::ConsentRequired
        );
        assert_eq!(
            service.download_model(false).unwrap_err().code,
            ErrorCode::ConsentRequired
        );
        assert_eq!(
            service.stop("unknown", true).unwrap_err().code,
            ErrorCode::StaleSession
        );
        assert!(!service.model().installed);
    }
    #[test]
    fn settings_have_no_audio_or_transcript_and_round_trip() {
        let service = service();
        service
            .set_settings(Settings {
                engine: Engine::Whisper,
                ..Settings::default()
            })
            .unwrap();
        let loaded = VoiceService::new((*service.root).clone(), |_| {}, |_| {});
        assert_eq!(loaded.settings().engine, Engine::Whisper);
        assert_eq!(fs::read_dir(&*service.root).unwrap().count(), 1);
        fs::remove_dir_all(&*service.root).unwrap();
    }
    #[test]
    fn active_capture_blocks_model_network_and_settings_changes() {
        let service = service();
        service.inner.lock().unwrap().state.phase = Phase::Listening;
        assert_eq!(
            service.download_model(true).unwrap_err().code,
            ErrorCode::Busy
        );
        assert_eq!(service.delete_model().unwrap_err().code, ErrorCode::Busy);
        assert_eq!(
            service.set_settings(Settings::default()).unwrap_err().code,
            ErrorCode::Busy
        );
    }

    #[test]
    fn cancelling_clears_text_and_orders_events_without_releasing_the_capture_slot() {
        let service = service();
        {
            let mut inner = service.inner.lock().unwrap();
            inner.state = VoiceState {
                session_id: Some("session".into()),
                phase: Phase::Listening,
                text: "private completed words".into(),
                partial: "private partial words".into(),
                ..VoiceState::default()
            };
        }
        let stopped = service.stop("session", true).unwrap();
        assert_eq!(stopped.phase, Phase::Finalizing);
        assert!(stopped.text.is_empty() && stopped.partial.is_empty());
        assert!(stopped.sequence > 0);
        assert_eq!(service.delete_model().unwrap_err().code, ErrorCode::Busy);
        #[cfg(windows)]
        {
            service.update("old-session", VoiceState::default());
            assert_eq!(service.state().sequence, stopped.sequence);
            service.update(
                "session",
                VoiceState {
                    phase: Phase::Review,
                    text: "late private result".into(),
                    ..VoiceState::default()
                },
            );
            assert_eq!(service.state().phase, Phase::Cancelled);
            assert!(service.state().text.is_empty());
            assert!(service.state().sequence > stopped.sequence);
        }
    }
}
