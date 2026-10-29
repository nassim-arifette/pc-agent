//! Local-only microphone controls. No audio bytes cross the webview bridge.
use pcagent_voice::{Device, ErrorCode, ModelInfo, Result, Settings, VoiceService, VoiceState};
use tauri::State;

async fn blocking<T: Send + 'static>(
    operation: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(operation)
        .await
        .map_err(|_| ErrorCode::RecognitionFailed)?
}

#[tauri::command]
pub(super) async fn voice_start(
    state: State<'_, VoiceService>,
    consent: bool,
) -> Result<VoiceState> {
    let service = state.inner().clone();
    blocking(move || service.start(consent)).await
}
#[tauri::command]
pub(super) async fn voice_stop(
    state: State<'_, VoiceService>,
    session_id: String,
    cancel: bool,
) -> Result<VoiceState> {
    let service = state.inner().clone();
    blocking(move || service.stop(&session_id, cancel)).await
}
#[tauri::command]
pub(super) async fn voice_state(state: State<'_, VoiceService>) -> Result<VoiceState> {
    let service = state.inner().clone();
    blocking(move || Ok(service.state())).await
}
#[tauri::command]
pub(super) async fn voice_devices() -> Result<Vec<Device>> {
    blocking(pcagent_voice::devices).await
}
#[tauri::command]
pub(super) async fn voice_settings_get(state: State<'_, VoiceService>) -> Result<Settings> {
    let service = state.inner().clone();
    blocking(move || Ok(service.settings())).await
}
#[tauri::command]
pub(super) async fn voice_settings_set(
    state: State<'_, VoiceService>,
    settings: Settings,
) -> Result<()> {
    let service = state.inner().clone();
    blocking(move || service.set_settings(settings)).await
}
#[tauri::command]
pub(super) async fn voice_model_state(state: State<'_, VoiceService>) -> Result<ModelInfo> {
    let service = state.inner().clone();
    blocking(move || Ok(service.model())).await
}
#[tauri::command]
pub(super) async fn voice_model_download(
    state: State<'_, VoiceService>,
    consent: bool,
) -> Result<()> {
    let service = state.inner().clone();
    blocking(move || service.download_model(consent)).await
}
#[tauri::command]
pub(super) async fn voice_model_cancel(state: State<'_, VoiceService>) -> Result<()> {
    let service = state.inner().clone();
    blocking(move || {
        service.cancel_download();
        Ok(())
    })
    .await
}
#[tauri::command]
pub(super) async fn voice_model_delete(state: State<'_, VoiceService>) -> Result<()> {
    let service = state.inner().clone();
    blocking(move || service.delete_model()).await
}
