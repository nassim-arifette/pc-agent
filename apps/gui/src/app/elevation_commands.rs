//! Explicit approved inspections and elevated-worker lifecycle commands.
use super::AppState;
use crate::elevation;
use crate::failure::Failure;
use std::time::Duration;
use tauri::State;

/// Prepare a fixed task for local review without opening the Windows consent dialog.
#[tauri::command]
pub(super) async fn elevation_prepare(
    state: State<'_, AppState>,
    task: pcagent_inspect::elevation::ElevatedTask,
    budget_ms: u64,
) -> Result<elevation::PreparedView, Failure> {
    let slot = state.elevation.clone();
    // Native creation and failed-creation cleanup belong off the GUI thread. Slot reserves
    // admission before that work and keeps status/cancellation responsive while it is pending.
    tauri::async_runtime::spawn_blocking(move || {
        slot.prepare(task, Duration::from_millis(budget_ms))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) fn elevation_status(
    state: State<'_, AppState>,
) -> Result<Option<elevation::PreparedView>, Failure> {
    state.elevation.status()
}

/// The local approval click consumes one prepared request. No model tool invokes this command.
#[tauri::command]
pub(super) async fn elevation_run(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<elevation::InspectionResult, Failure> {
    let run = state.elevation.begin(&request_id)?;
    let result = tauri::async_runtime::spawn_blocking(move || run.execute()).await;
    result.map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) fn elevation_cancel(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), Failure> {
    state.elevation.cancel(&request_id)
}
