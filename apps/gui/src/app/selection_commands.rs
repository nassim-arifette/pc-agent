//! Host-owned selections and single-use selected-evidence tickets.
use super::history_commands::read_past_from;
use super::preview_commands::{checked_question, sharing_of};
use super::settings_commands::configured_model_id;
use super::state::{ScanSlot, SelectionStart, StartRequest, Started};
use super::turn_commands::launch_investigation;
use super::{lock, AppState};
use crate::failure::{Code, Failure};
use crate::selection::{SelectionPreview, SelectionRegistry};
use pcagent_agent::credentials::Credentials;
use pcagent_agent::selection::{
    RecordingSource, SelectionIntent, SelectionRef, SelectionSource, SelectionTarget, StorageSource,
};
use pcagent_agent::{PrivacyContext, Sharing, StorageContext};
use pcagent_nebius::Config;
use pcagent_shared::unix_ms;
use pcagent_storage::StorageService;
use pcagent_store::sampler::Sampler;
use pcagent_store::HistoryQuery;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use tauri::{AppHandle, State};

pub(super) fn retain_capture(
    registry: &Mutex<SelectionRegistry>,
    source: SelectionSource,
) -> Option<String> {
    let mut registry = lock(registry);
    if let Some(session) = source.process_session() {
        registry.invalidate_process_session(session);
    }
    registry.publish(source).ok()
}

pub(super) fn retain_recording(registry: &Mutex<SelectionRegistry>, sampler: &Sampler, id: &str) {
    let revision = lock(registry).history_revision();
    let Ok(Some(recording)) = sampler.recording(id) else {
        return;
    };
    let to = recording.ended_at_unix_ms.unwrap_or_else(unix_ms);
    if to <= recording.started_at_unix_ms {
        return;
    }
    let histories = [
        pcagent_shared::Metric::CpuBusyBasisPoints,
        pcagent_shared::Metric::PhysicalAvailableBytes,
        pcagent_shared::Metric::DiskReadLatencyMicroseconds,
        pcagent_shared::Metric::DiskWriteLatencyMicroseconds,
    ]
    .into_iter()
    .filter_map(|metric| {
        let mut series = sampler
            .history(&HistoryQuery {
                metric,
                entity_ref: "system".into(),
                from_unix_ms: recording.started_at_unix_ms,
                to_unix_ms: to,
            })
            .ok()?;
        series.limit_points(120);
        Some(series)
    })
    .collect();
    let source = RecordingSource {
        cpu_peaks: sampler
            .process_peaks(recording.started_at_unix_ms, to, 10)
            .ok(),
        resource_peaks: sampler
            .process_resource_peaks(recording.started_at_unix_ms, to, 10)
            .ok(),
        battery: sampler.battery_recording(id).ok().flatten(),
        recording,
        histories,
    };
    let _ = lock(registry).publish_history(SelectionSource::Recording(source), revision);
}

pub(super) fn retain_storage_page(
    registry: &Mutex<SelectionRegistry>,
    storage: &StorageService,
    scan: &Mutex<ScanSlot>,
    page: pcagent_storage::LargestItemsPage,
) -> Option<String> {
    retain_storage_facts(registry, storage, scan, page, None)
}

pub(super) fn retain_storage_facts(
    registry: &Mutex<SelectionRegistry>,
    storage: &StorageService,
    scan: &Mutex<ScanSlot>,
    page: pcagent_storage::LargestItemsPage,
    facts: Option<pcagent_storage::StorageItemEvidence>,
) -> Option<String> {
    let current = lock(scan);
    if current.scope_id.as_deref() != Some(page.scope_id.as_str()) {
        return None;
    }
    let index = storage.index(&page.scope_id).ok()?;
    if index.scan_id() != page.scan_id {
        return None;
    }
    let mut context = StorageContext::default();
    let _ = context.authorize_useful_names_with_privacy(&index, &PrivacyContext::local());
    let source = StorageSource {
        facts: facts.map(Box::new),
        page,
        window: index.measurement_window().clone(),
        origin: pcagent_shared::Origin::Live,
        context,
    };
    lock(registry)
        .publish(SelectionSource::Storage(source))
        .ok()
}

#[tauri::command]
pub(super) fn selection_sources(
    state: State<'_, AppState>,
) -> Vec<crate::selection::SelectionDescriptor> {
    lock(&state.selections).sources()
}

#[tauri::command]
pub(super) fn selection_preview(
    state: State<'_, AppState>,
    reference: SelectionRef,
    intent: SelectionIntent,
    question: Option<String>,
    conversation_id: Option<String>,
    names: bool,
    file_names: Option<bool>,
) -> Result<Value, Failure> {
    let question = question
        .map(|question| checked_question(&question))
        .transpose()?;
    let history = lock(&state.history);
    let past = read_past_from(&history, conversation_id.as_deref())?;
    let mut privacy = PrivacyContext::local();
    if let Some(secret) = Credentials::load().get("NEBIUS_API_KEY") {
        privacy.protect_secret(&secret);
    }
    let revision = state.selection_context_revision.load(Ordering::SeqCst);
    let routing = lock(&state.model_routing).review(configured_model_id());
    let model_routing = lock(&state.model_routing).view();
    let mut registry = lock(&state.selections);
    let preview: SelectionPreview = registry.prepare(
        reference,
        intent,
        question,
        conversation_id,
        past,
        sharing_of(names),
        file_names.unwrap_or(false),
        privacy,
        revision,
    )?;
    if revision != state.selection_context_revision.load(Ordering::SeqCst)
        || !lock(&state.model_routing).matches(&routing, configured_model_id().as_deref())
    {
        registry.invalidate_tickets();
        return Err(Failure::new(
            Code::PreviewChanged,
            "Settings changed. Open the selected preview again.",
        ));
    }
    registry.bind_routing(&preview.preview_id, routing.clone())?;
    let mut value =
        serde_json::to_value(preview).map_err(|_| "The selected preview could not be encoded.")?;
    value["model_routing"] = serde_json::json!(model_routing);
    value["configured_model_id"] = serde_json::json!(routing.configured_model_id);
    Ok(value)
}

#[tauri::command]
pub(super) fn selection_send(
    app: AppHandle,
    state: State<'_, AppState>,
    reference: SelectionRef,
    intent: SelectionIntent,
    question: Option<String>,
    conversation_id: Option<String>,
    preview_id: String,
) -> Result<Started, Failure> {
    let privacy = lock(&state.privacy).clone();
    let cloud = privacy.mode.sharing().is_some()
        && Config::from_lookup(|name| Credentials::load().get(name)).is_ok();
    // No confirmation dialog stands before a send (C5, 9 October): the single-use ticket of the
    // exact preview is what a cloud send needs.
    let (sharing, file_names, _) =
        lock(&state.selections)
            .choices(&preview_id)
            .ok_or_else(|| {
                Failure::new(
                    Code::PreviewRequired,
                    "Open the selected-evidence sharing preview before sending.",
                )
            })?;
    let use_storage = matches!(reference.target, SelectionTarget::Storage { .. });
    let question = question.unwrap_or_else(|| reference.target.default_question(intent));
    launch_investigation(
        app,
        &state,
        StartRequest {
            question,
            cloud,
            use_storage,
            names: sharing == Sharing::Complete,
            conversation_id,
            preview_id: Some(preview_id.clone()),
            file_names,
            image_id: None,
        },
        Some(SelectionStart {
            reference,
            intent,
            preview_id,
        }),
    )
}
