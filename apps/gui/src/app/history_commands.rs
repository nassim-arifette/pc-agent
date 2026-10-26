//! Local history, recordings and stored conversations.
use super::selection_commands::retain_recording;
use super::{lock, AppState};
use crate::failure::{Code, Failure};
use crate::{failure, preview, protocol};
use pcagent_agent::selection::SelectionSource;
use pcagent_agent::{credentials, PastTurn};
use pcagent_shared::unix_ms;
use pcagent_store::sampler::{Sampler, SamplerStatus};
use pcagent_store::{
    Conversation, ConversationSummary, HistoryQuery, HistorySeries, Recording, Store,
    DEFAULT_RETENTION_DAYS,
};
use serde_json::Value;
use std::sync::atomic::Ordering;
use tauri::State;

/// The history file next to the settings file, with conversations older than the retention
/// period removed.
pub(super) fn open_history() -> Option<Store> {
    let dir = credentials::file_path()?.parent()?.to_path_buf();
    let store = Store::open(&dir.join("pcagent.sqlite")).ok()?;
    let retention_ms = DEFAULT_RETENTION_DAYS * 24 * 3_600_000;
    let _ = store.prune(unix_ms().saturating_sub(retention_ms));
    Some(store)
}

pub(super) fn open_sampler() -> Option<Sampler> {
    let dir = credentials::file_path()?.parent()?.to_path_buf();
    Sampler::start(&dir.join("pcagent.sqlite")).ok()
}

pub(super) fn history_unavailable() -> Failure {
    Failure::new(Code::HistoryUnavailable, "Local history is unavailable.")
}

#[tauri::command]
pub(super) fn history_status(state: State<'_, AppState>) -> Option<SamplerStatus> {
    state.sampler.as_ref().map(Sampler::status)
}

#[tauri::command]
pub(super) async fn metric_history(
    state: State<'_, AppState>,
    metric: pcagent_shared::Metric,
    from_unix_ms: u64,
    to_unix_ms: u64,
    entity_ref: Option<String>,
) -> Result<HistorySeries, Failure> {
    let sampler = state.sampler.clone().ok_or_else(history_unavailable)?;
    let selections = state.selections.clone();
    let revision = lock(&selections).history_revision();
    tauri::async_runtime::spawn_blocking(move || {
        let series = sampler
            .history(&HistoryQuery {
                metric,
                from_unix_ms,
                to_unix_ms,
                entity_ref: entity_ref.unwrap_or_else(|| "system".into()),
            })
            .map_err(failure::history)?;
        let _ =
            lock(&selections).publish_history(SelectionSource::History(series.clone()), revision);
        Ok(series)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn record_activity_start(
    state: State<'_, AppState>,
    duration_seconds: u64,
) -> Result<Recording, Failure> {
    let sampler = state.sampler.clone().ok_or_else(history_unavailable)?;
    let selections = state.selections.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let recording = sampler
            .start_recording(duration_seconds)
            .map_err(failure::history)?;
        retain_recording(&selections, &sampler, &recording.id);
        Ok(recording)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read the battery points of an existing local problem recording without collecting again.
#[tauri::command]
pub(super) async fn battery_recording(
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<Option<pcagent_store::BatteryRecording>, Failure> {
    let sampler = state
        .sampler
        .as_ref()
        .ok_or_else(history_unavailable)?
        .clone();
    let selections = state.selections.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let battery = sampler.battery_recording(&recording_id)?;
        retain_recording(&selections, &sampler, &recording_id);
        Ok::<_, pcagent_store::StoreError>(battery)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(failure::history)
}

#[tauri::command]
pub(super) fn record_activity_cancel(
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<(), Failure> {
    state
        .sampler
        .as_ref()
        .ok_or_else(history_unavailable)?
        .cancel_recording(&recording_id)
        .map_err(failure::history)
}

#[tauri::command]
pub(super) async fn history_clear(state: State<'_, AppState>) -> Result<usize, Failure> {
    let sampler = state.sampler.clone().ok_or_else(history_unavailable)?;
    let selections = state.selections.clone();
    lock(&selections).begin_history_clear();
    tauri::async_runtime::spawn_blocking(move || {
        let result = sampler.clear().map_err(failure::history);
        lock(&selections).finish_history_clear();
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Only typed model context is reusable. Legacy display-only reports remain visible locally;
/// their localized prose has no trustworthy name mapping or measured sources to resend.
pub(super) fn past_turns(conversation: &Conversation) -> Vec<PastTurn> {
    conversation
        .turns
        .iter()
        .filter_map(|turn| serde_json::from_value(turn.report.get("context")?.clone()).ok())
        .collect()
}

#[tauri::command]
pub(super) fn conversations_list(
    state: State<'_, AppState>,
) -> Result<Vec<ConversationSummary>, String> {
    match lock(&state.history).as_ref() {
        Some(store) => store.list(200).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
pub(super) fn conversation_get(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<Value>, String> {
    match lock(&state.history).as_ref() {
        Some(store) => store
            .get(&id)
            .map(|conversation| {
                conversation.map(|conversation| {
                    let mut value =
                        serde_json::to_value(&conversation).expect("serializable conversation");
                    value["turns"] = serde_json::json!(conversation
                        .turns
                        .iter()
                        .map(|turn| {
                            let mut shown = protocol::turn(
                                &turn.id,
                                &turn.question,
                                turn.asked_at_unix_ms,
                                &turn.report,
                            );
                            if let Some(image) = &turn.image {
                                shown["image"] = serde_json::json!(image);
                            }
                            shown
                        })
                        .collect::<Vec<_>>());
                    value
                })
            })
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[tauri::command]
pub(super) fn conversation_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    // Use the same history -> ticket order as preview publication and consumption. Delete
    // only while both ticket stores are held, so a completed deletion cannot resend its past.
    let history = lock(&state.history);
    let mut images = lock(&state.images);
    let mut preview = lock(&state.preview);
    let mut selections = lock(&state.selections);
    let deleted = match history.as_ref() {
        Some(store) => store.delete(&id).map_err(|e| e.to_string())?,
        None => false,
    };
    preview::invalidate_conversation(&mut preview, &id);
    images.invalidate_conversation(&id);
    selections.invalidate_conversation(&id);
    Ok(deleted)
}

#[tauri::command]
pub(super) fn conversations_clear(state: State<'_, AppState>) -> Result<usize, String> {
    let history = lock(&state.history);
    let mut images = lock(&state.images);
    let mut preview = lock(&state.preview);
    let mut selections = lock(&state.selections);
    let deleted = match history.as_ref() {
        Some(store) => store.clear().map_err(|e| e.to_string())?,
        None => 0,
    };
    state.preview_revision.fetch_add(1, Ordering::SeqCst);
    *preview = None;
    images.begin();
    selections.invalidate_tickets();
    Ok(deleted)
}

pub(super) fn read_past(state: &AppState, id: Option<&str>) -> Result<Vec<PastTurn>, Failure> {
    let history = lock(&state.history);
    read_past_from(&history, id)
}

pub(super) fn read_past_from(
    history: &Option<Store>,
    id: Option<&str>,
) -> Result<Vec<PastTurn>, Failure> {
    let Some(id) = id else { return Ok(Vec::new()) };
    let conversation = history
        .as_ref()
        .ok_or_else(|| {
            Failure::new(
                Code::HistoryUnavailable,
                "Conversation history is unavailable.",
            )
        })?
        .get(id)
        .map_err(failure::history)?
        .ok_or_else(|| {
            Failure::new(
                Code::ConversationMissing,
                "This conversation no longer exists.",
            )
        })?;
    Ok(past_turns(&conversation))
}
