//! Windows capture and local inference. This module has no HTTP client.
mod sapi;

use crate::{
    audio::{rms, Resampler, Utterance, RATE},
    model, Device, Engine, ErrorCode, Language, Phase, Result, Settings, VoiceError, VoiceState,
};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn device_id(name: &str, ordinal: usize) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{name}\0{ordinal}").as_bytes())
    )
}

fn inputs() -> Result<Vec<(Device, cpal::Device)>> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    let mut names = std::collections::HashMap::new();
    host.input_devices()
        .map_err(|_| ErrorCode::NoDevice)?
        .map(|device| {
            let name = device.name().map_err(|_| ErrorCode::DeviceUnavailable)?;
            let ordinal = names.entry(name.clone()).or_insert(0);
            let item = Device {
                id: device_id(&name, *ordinal),
                is_default: default.as_ref() == Some(&name) && *ordinal == 0,
                name,
            };
            *ordinal += 1;
            Ok((item, device))
        })
        .collect()
}
pub fn devices() -> Result<Vec<Device>> {
    Ok(inputs()?.into_iter().map(|(d, _)| d).collect())
}

fn capture_error(error: impl std::fmt::Display) -> VoiceError {
    let text = error.to_string().to_lowercase();
    if text.contains("access") || text.contains("denied") || text.contains("80070005") {
        ErrorCode::PermissionDenied.into()
    } else {
        ErrorCode::DeviceUnavailable.into()
    }
}

fn stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    tx: mpsc::SyncSender<Result<Vec<f32>>>,
    stop: Arc<AtomicBool>,
    overrun: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    tail: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let mut resampler = Resampler::new(config.sample_rate.0, config.channels as usize);
    let err_tx = tx.clone();
    let err_stop = stop.clone();
    let err_overrun = overrun.clone();
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                let mut pending = tail.lock().unwrap_or_else(|p| p.into_inner());
                resampler.push(
                    data.iter().map(|&v| <f32 as cpal::Sample>::from_sample(v)),
                    &mut pending,
                );
                while pending.len() >= RATE / 10 {
                    let next = pending.split_off(RATE / 10);
                    let frame = std::mem::replace(&mut *pending, next);
                    level.store(rms(&frame).to_bits(), Ordering::Release);
                    if tx.try_send(Ok(frame)).is_err() {
                        overrun.store(true, Ordering::Release);
                        stop.store(true, Ordering::Release);
                        return;
                    }
                }
            },
            move |error| {
                if err_tx.try_send(Err(capture_error(error))).is_err() {
                    err_overrun.store(true, Ordering::Release);
                }
                err_stop.store(true, Ordering::Release);
            },
            None,
        )
        .map_err(capture_error)
}

struct Capture {
    receiver: mpsc::Receiver<Result<Vec<f32>>>,
    stop: Arc<AtomicBool>,
    overrun: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Capture {
    fn start(settings: &Settings, stop: Arc<AtomicBool>) -> Result<Self> {
        let (tx, receiver) = mpsc::sync_channel(300);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let overrun = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let id = settings.device_id.clone();
        let (stop_worker, overrun_worker, meter) = (stop.clone(), overrun.clone(), level.clone());
        let thread = std::thread::spawn(move || {
            let tail = Arc::new(Mutex::new(Vec::new()));
            let tail_tx = tx.clone();
            let tail_overrun = overrun_worker.clone();
            let build = || -> Result<cpal::Stream> {
                let devices = inputs()?;
                let (_, device) = devices
                    .into_iter()
                    .find(|(d, _)| match &id {
                        Some(id) => &d.id == id,
                        None => d.is_default,
                    })
                    .ok_or(if id.is_some() {
                        ErrorCode::DeviceUnavailable
                    } else {
                        ErrorCode::NoDevice
                    })?;
                let supported = device.default_input_config().map_err(capture_error)?;
                let config = supported.config();
                let args = (tx, stop_worker.clone(), overrun_worker, meter, tail.clone());
                macro_rules! make {
                    ($sample:ty) => {
                        stream::<$sample>(&device, &config, args.0, args.1, args.2, args.3, args.4)
                    };
                }
                let stream = match supported.sample_format() {
                    cpal::SampleFormat::I8 => make!(i8),
                    cpal::SampleFormat::I16 => make!(i16),
                    cpal::SampleFormat::I32 => make!(i32),
                    cpal::SampleFormat::I64 => make!(i64),
                    cpal::SampleFormat::U8 => make!(u8),
                    cpal::SampleFormat::U16 => make!(u16),
                    cpal::SampleFormat::U32 => make!(u32),
                    cpal::SampleFormat::U64 => make!(u64),
                    cpal::SampleFormat::F32 => make!(f32),
                    cpal::SampleFormat::F64 => make!(f64),
                    _ => Err(ErrorCode::DeviceUnavailable.into()),
                }?;
                stream.play().map_err(capture_error)?;
                Ok(stream)
            };
            match build() {
                Ok(stream) => {
                    let _ = ready_tx.send(Ok(()));
                    while !stop_worker.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    drop(stream);
                    let remaining =
                        std::mem::take(&mut *tail.lock().unwrap_or_else(|p| p.into_inner()));
                    if !remaining.is_empty() && tail_tx.try_send(Ok(remaining)).is_err() {
                        tail_overrun.store(true, Ordering::Release);
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
            }
        });
        let capture = Self {
            receiver,
            stop,
            overrun,
            level,
            thread: Some(thread),
        };
        ready_rx
            .recv()
            .map_err(|_| ErrorCode::DeviceUnavailable)??;
        Ok(capture)
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

enum Backend {
    Whisper(WhisperContext),
    Windows(sapi::Recognizer),
}
impl Backend {
    fn new(settings: &Settings, path: &Path, cancel: &AtomicBool) -> Result<Self> {
        if settings.engine == Engine::Windows
            || (settings.engine == Engine::Auto && !path.is_file())
        {
            return sapi::Recognizer::new(settings.language).map(Self::Windows);
        }
        model::verify(path, cancel)?;
        // No log backend is enabled: native model paths/text must not reach application logs.
        whisper_rs::install_logging_hooks();
        let path = path.to_str().ok_or(ErrorCode::StorageUnavailable)?;
        let mut params = WhisperContextParameters::default();
        params.use_gpu(false);
        WhisperContext::new_with_params(path, params)
            .map(Self::Whisper)
            .map_err(|_| ErrorCode::ModelInvalid.into())
    }
    fn transcribe(
        &self,
        samples: &[f32],
        language: Language,
        cancel: Arc<AtomicBool>,
    ) -> Result<(String, String)> {
        match self {
            Self::Windows(engine) => engine.transcribe(samples, &cancel),
            Self::Whisper(context) => {
                let mut state = context
                    .create_state()
                    .map_err(|_| ErrorCode::RecognitionFailed)?;
                let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
                params.set_n_threads(
                    std::thread::available_parallelism()
                        .map_or(2, |n| n.get().saturating_sub(1).clamp(1, 8))
                        as i32,
                );
                params.set_no_context(true);
                params.set_language(language.code());
                params.set_print_realtime(false);
                params.set_print_progress(false);
                params.set_print_timestamps(false);
                params.set_print_special(false);
                params.set_abort_callback_safe(move || cancel.load(Ordering::Acquire));
                // Whisper expects at least one second, even for a short utterance.
                let padded;
                let samples = if samples.len() < RATE {
                    padded = [samples, &vec![0.0; RATE - samples.len()]].concat();
                    &padded
                } else {
                    samples
                };
                state
                    .full(params, samples)
                    .map_err(|_| ErrorCode::RecognitionFailed)?;
                let mut text = String::new();
                for i in 0..state
                    .full_n_segments()
                    .map_err(|_| ErrorCode::RecognitionFailed)?
                {
                    text.push_str(
                        &state
                            .full_get_segment_text(i)
                            .map_err(|_| ErrorCode::RecognitionFailed)?,
                    );
                }
                let language = state
                    .full_lang_id_from_state()
                    .ok()
                    .and_then(whisper_rs::get_lang_str)
                    .unwrap_or("und")
                    .to_owned();
                Ok((text.trim().to_owned(), language))
            }
        }
    }
}

pub fn run(
    settings: &Settings,
    path: &Path,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    mut emit: impl FnMut(VoiceState),
) -> Result<()> {
    let backend = Backend::new(settings, path, &cancel)?;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let capture = Capture::start(settings, stop.clone())?;
    let mut state = VoiceState {
        phase: Phase::Listening,
        engine: match &backend {
            Backend::Whisper(_) => Engine::Whisper,
            Backend::Windows(_) => Engine::Windows,
        },
        ..VoiceState::default()
    };
    emit(state.clone());
    let mut utterance = Utterance::new();
    let mut partial_at = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(());
        }
        if capture.overrun.load(Ordering::Acquire) {
            return Err(ErrorCode::AudioOverrun.into());
        }
        match capture.receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(samples) => {
                let complete = utterance.push(&samples?);
                state.level = f32::from_bits(capture.level.load(Ordering::Acquire));
                if complete
                    || (utterance.has_speech()
                        && utterance.samples.len() >= RATE * 2
                        && partial_at.elapsed() >= Duration::from_secs(3))
                {
                    let (text, language) = backend.transcribe(
                        &utterance.samples,
                        settings.language,
                        cancel.clone(),
                    )?;
                    state.language = Some(language);
                    if complete {
                        if !text.is_empty() {
                            if !state.text.is_empty() {
                                state.text.push(' ');
                            }
                            state.text.push_str(&text);
                        }
                        state.partial.clear();
                        utterance.take();
                    } else {
                        state.partial = text;
                    }
                    partial_at = Instant::now();
                }
                if stop.load(Ordering::Acquire) {
                    state.phase = Phase::Finalizing;
                }
                emit(state.clone());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if stop.load(Ordering::Acquire) {
                    state.phase = Phase::Finalizing;
                }
                emit(state.clone());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    state.phase = Phase::Finalizing;
    state.level = 0.0;
    emit(state.clone());
    if utterance.has_speech() && !cancel.load(Ordering::Acquire) {
        let (text, language) = backend.transcribe(&utterance.take(), settings.language, cancel)?;
        if !text.is_empty() {
            if !state.text.is_empty() {
                state.text.push(' ');
            }
            state.text.push_str(&text);
        }
        state.language = Some(language);
    }
    state.partial.clear();
    emit(state);
    Ok(())
}
