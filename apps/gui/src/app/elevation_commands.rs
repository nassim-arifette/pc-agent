//! Explicit approved inspections and elevated-worker lifecycle commands.
use super::{lock, AppState};
use crate::elevation;
use crate::failure::Failure;
use pcagent_shared::unix_ms;
use serde::Serialize;
use std::time::Duration;
use tauri::State;

#[tauri::command]
pub(super) async fn elevation_options(
    state: State<'_, AppState>,
) -> Result<elevation::ReviewOptions, Failure> {
    if !lock(&state.probes).elevated_helper {
        return Err("The administrator helper is not available on this PC.".into());
    }
    let dashboard =
        super::measurements::ask_measure(&state, super::state::Measure::Dashboard).await?;
    let snapshot = super::measurements::ask_measure(&state, super::state::Measure::Preview).await?;
    let battery = lock(&state.probes).battery != Some(false);
    Ok(elevation::review_options(
        &dashboard.volumes,
        &snapshot,
        battery,
    ))
}

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
) -> Result<CompletedView, Failure> {
    let run = state.elevation.begin(&request_id)?;
    let history = state.history.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let task = run.task().clone();
        let result = run.execute()?;
        let saved = lock(&history)
            .as_ref()
            .ok_or_else(super::history_commands::history_unavailable)
            .and_then(|store| {
                store
                    .save_elevated_inspection(
                        &task,
                        &result.inspection,
                        &result.local_details(),
                        unix_ms(),
                    )
                    .and_then(|record| {
                        for section in &result.sections {
                            store.save_elevated_inspection(
                                &section.task,
                                &section.result.inspection,
                                &section.result.local_details(),
                                unix_ms(),
                            )?;
                        }
                        Ok(record)
                    })
                    .map_err(crate::failure::history)
            });
        let (stored_id, persistence_error) = match saved {
            Ok(Some(record)) => (Some(record.id), None),
            Ok(None) => (
                None,
                Some("The measurement is outside the 24-hour retention window.".into()),
            ),
            Err(error) => (None, Some(error)),
        };
        Ok(CompletedView {
            result,
            stored_id,
            persistence_error,
        })
    })
    .await;
    result.map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub(super) struct CompletedView {
    #[serde(flatten)]
    result: elevation::InspectionResult,
    stored_id: Option<String>,
    persistence_error: Option<Failure>,
}

#[tauri::command]
pub(super) fn elevation_results(
    state: State<'_, AppState>,
) -> Result<Vec<pcagent_store::ElevatedInspection>, Failure> {
    lock(&state.history)
        .as_ref()
        .ok_or_else(super::history_commands::history_unavailable)?
        .elevated_inspections(unix_ms())
        .map_err(crate::failure::history)
}

#[tauri::command]
pub(super) fn elevation_select(
    state: State<'_, AppState>,
    inspection_id: String,
) -> Result<pcagent_agent::selection::SelectionRef, Failure> {
    use pcagent_agent::selection::{SelectionRef, SelectionSource, SelectionTarget};
    let history = lock(&state.history);
    let record = history
        .as_ref()
        .ok_or_else(super::history_commands::history_unavailable)?
        .elevated_inspection(&inspection_id, unix_ms())
        .map_err(crate::failure::history)?
        .ok_or("The administrator inspection has expired or was removed.")?;
    let source_id = lock(&state.selections).publish(SelectionSource::ElevatedInspection {
        id: record.id.clone(),
        inspection: record.inspection,
    })?;
    Ok(SelectionRef {
        source_id,
        target: SelectionTarget::ElevatedInspection {
            inspection_id: record.id,
        },
    })
}

#[tauri::command]
pub(super) fn elevation_cancel(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), Failure> {
    state.elevation.cancel(&request_id)
}
