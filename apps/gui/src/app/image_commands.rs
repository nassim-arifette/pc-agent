use super::{lock, AppState};
use crate::{
    failure::{Code, Failure},
    images::{self, ImageView},
};
use pcagent_agent::image_input::{ImageInput, MAX_INPUT_BYTES};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::State;

struct DecodePermit(Arc<AtomicBool>);
impl Drop for DecodePermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn invalidate_preview(state: &AppState) {
    state.preview_revision.fetch_add(1, Ordering::SeqCst);
    *lock(&state.preview) = None;
}

#[tauri::command]
pub async fn image_prepare(
    state: State<'_, AppState>,
    data_base64: String,
) -> Result<ImageView, Failure> {
    if data_base64.len() > MAX_INPUT_BYTES.div_ceil(3) * 4 {
        return Err(Failure::new(
            Code::ImageTooLarge,
            "The image exceeds the input limit.",
        ));
    }
    if state.image_decoding.swap(true, Ordering::SeqCst) {
        return Err(Failure::new(
            Code::ImagePreparing,
            "Wait for the current image to finish preparing.",
        ));
    }
    let permit = DecodePermit(state.image_decoding.clone());
    let revision = {
        let mut images = lock(&state.images);
        let revision = images.begin();
        invalidate_preview(&state);
        revision
    };
    let image = tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        ImageInput::prepare_base64(&data_base64)
    })
    .await
    .map_err(|_| Failure::new(Code::ImageInvalid, "The image could not be prepared."))?
    .map_err(images::preparation_error)?;
    let mut images = lock(&state.images);
    let view = images.finish(revision, image)?;
    invalidate_preview(&state);
    Ok(view)
}

#[tauri::command]
pub fn image_discard(state: State<'_, AppState>, image_id: String) {
    let mut images = lock(&state.images);
    if images.discard(&image_id) {
        invalidate_preview(&state);
    }
}

#[tauri::command]
pub fn conversation_image_get(
    state: State<'_, AppState>,
    conversation_id: String,
    turn_id: String,
) -> Result<Option<ImageView>, Failure> {
    lock(&state.history)
        .as_ref()
        .ok_or_else(|| Failure::new(Code::HistoryUnavailable, "Local history is unavailable."))?
        .image(&conversation_id, &turn_id)
        .map(|image| image.map(|image| ImageView::new(&turn_id, &ImageInput::from_record(image))))
        .map_err(crate::failure::history)
}
