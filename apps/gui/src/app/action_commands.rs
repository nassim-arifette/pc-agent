//! Local action proposals and explicit approval commands.
use super::AppState;
use crate::failure::Failure;
use crate::{actions, view};
use tauri::State;

/// The proposals of a finished turn, as the person reviews them before deciding.
#[tauri::command]
pub(super) fn action_plans(state: State<'_, AppState>, turn_id: String) -> Vec<view::PlanView> {
    actions::plans(&state.actions, &turn_id)
}

/// Prepare local journal-based recovery; the existing action_run command still requires a click.
#[tauri::command]
pub(super) async fn startup_restore_prepare(
    state: State<'_, AppState>,
    source_plan_id: String,
    item_index: usize,
) -> Result<actions::StartupRestoration, Failure> {
    let slot = state.actions.clone();
    let names = state.program_names.clone();
    tauri::async_runtime::spawn_blocking(move || {
        actions::prepare_startup_restore(&slot, &source_plan_id, item_index, &|pid, created| {
            names.name(pid, created)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) fn action_decline(
    state: State<'_, AppState>,
    turn_id: String,
    plan_id: String,
) -> Result<(), Failure> {
    actions::decline(&state.actions, &turn_id, &plan_id)
}

/// Runs one reviewed proposal. Only this window calls it, after the person's click on that plan;
/// the broker revalidates the target and refuses anything protected or changed.
#[tauri::command]
pub(super) async fn action_run(
    state: State<'_, AppState>,
    turn_id: String,
    plan_id: String,
) -> Result<view::ActionOutcome, Failure> {
    let slot = state.actions.clone();
    tauri::async_runtime::spawn_blocking(move || actions::run(&slot, &turn_id, &plan_id))
        .await
        .map_err(|e| e.to_string())?
}
