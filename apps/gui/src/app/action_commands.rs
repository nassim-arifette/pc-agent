//! Local action proposals and explicit approval commands.
use super::AppState;
use crate::failure::Failure;
use crate::{actions, view};
use tauri::State;

/// Resolve only a reference from the last local browser collection, then prepare (never run).
#[tauri::command]
pub(super) async fn browser_manager_prepare(
    state: State<'_, AppState>,
    application_ref: String,
    process_ref: String,
) -> Result<actions::StartupRestoration, Failure> {
    let targets =
        super::measurements::ask_measure(&state, |reply| super::state::Measure::BrowserTargets {
            application_ref,
            reply,
        })
        .await?;
    let instance = targets
        .into_iter()
        .find(|target| target.instance.entity_ref == process_ref)
        .map(|target| target.instance)
        .ok_or_else(|| {
            Failure::refused(
                Some(crate::failure::Refusal::Stale),
                "Refresh local browser windows first.",
            )
        })?;
    let slot = state.actions.clone();
    let names = state.program_names.clone();
    tauri::async_runtime::spawn_blocking(move || {
        actions::prepare_browser_manager(&slot, &instance, &|pid, created| names.name(pid, created))
    })
    .await
    .map_err(|_| {
        Failure::refused(
            Some(crate::failure::Refusal::Stale),
            "Browser plan preparation stopped.",
        )
    })?
}

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
