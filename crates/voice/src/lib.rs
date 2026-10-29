//! Audio stays in process memory. Only reviewed text reaches the conversation.
#![deny(unsafe_op_in_unsafe_fn)]

mod audio;
mod model;
#[cfg(windows)]
mod native;
mod service;

pub use model::{ModelInfo, MODEL_BYTES, MODEL_SHA256, MODEL_URL};
use serde::{Deserialize, Serialize};
pub use service::VoiceService;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Auto,
    Whisper,
    Windows,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    #[default]
    Auto,
    En,
    Fr,
}
impl Language {
    pub fn code(self) -> Option<&'static str> {
        match self {
            Self::Auto => None,
            Self::En => Some("en"),
            Self::Fr => Some("fr"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub engine: Engine,
    pub language: Language,
    /// An opaque, session-local device ID, not a path or a name sent to a provider.
    pub device_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Listening,
    Finalizing,
    Review,
    Cancelled,
    Error,
}
impl Phase {
    fn active(self) -> bool {
        matches!(self, Self::Starting | Self::Listening | Self::Finalizing)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VoiceState {
    pub session_id: Option<String>,
    pub sequence: u64,
    pub phase: Phase,
    pub engine: Engine,
    pub language: Option<String>,
    pub level: f32,
    /// Completed utterances in this session; never saved by the voice service.
    pub text: String,
    pub partial: String,
    pub error: Option<VoiceError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Unsupported,
    Busy,
    ConsentRequired,
    NoDevice,
    PermissionDenied,
    DeviceUnavailable,
    AudioOverrun,
    ModelMissing,
    ModelInvalid,
    DownloadFailed,
    StorageUnavailable,
    EngineUnavailable,
    RecognitionFailed,
    StaleSession,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceError {
    pub code: ErrorCode,
    pub message: &'static str,
}
impl From<ErrorCode> for VoiceError {
    fn from(code: ErrorCode) -> Self {
        let message = match code {
            ErrorCode::Unsupported => "Microphone capture is not supported on this platform.",
            ErrorCode::Busy => "Stop dictation or the model operation first.",
            ErrorCode::ConsentRequired => "Explicit consent is required.",
            ErrorCode::NoDevice => "No microphone is available.",
            ErrorCode::PermissionDenied => "Microphone access was denied.",
            ErrorCode::DeviceUnavailable => "The selected microphone is unavailable.",
            ErrorCode::AudioOverrun => "Audio arrived faster than it could be processed. Stop other heavy work and retry.",
            ErrorCode::ModelMissing => "Download the local multilingual model first.",
            ErrorCode::ModelInvalid => "The local model failed its integrity check. Delete it and download again.",
            ErrorCode::DownloadFailed => "The model could not be downloaded. Retry when connected.",
            ErrorCode::StorageUnavailable => "The local voice settings or model could not be accessed.",
            ErrorCode::EngineUnavailable => "No compatible offline Windows speech engine is installed. Download the Whisper model.",
            ErrorCode::RecognitionFailed => "Local speech recognition failed.",
            ErrorCode::StaleSession => "This dictation session is no longer active.",
            ErrorCode::Cancelled => "The operation was cancelled.",
        };
        Self { code, message }
    }
}
impl std::fmt::Display for VoiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for VoiceError {}
pub type Result<T> = std::result::Result<T, VoiceError>;

pub fn devices() -> Result<Vec<Device>> {
    #[cfg(windows)]
    return native::devices();
    #[cfg(not(windows))]
    Err(ErrorCode::Unsupported.into())
}
