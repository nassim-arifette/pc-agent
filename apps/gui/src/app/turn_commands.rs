//! Start, reattach, stop and extend a consented conversation turn.
use super::history_commands::{past_turns, read_past_from};
use super::preview_commands::{checked_question, preview_required, sharing_of};
use super::settings_commands::configured_model_id;
use super::state::{
    ApprovedCapture, Ask, CurrentTurn, DoneEvent, FailedEvent, Running, SelectionStart,
    StartRequest, Started,
};
use super::worker::investigation;
use super::{lock, AppState};
use crate::failure::{Code, Failure};
use crate::preview::Reviewed;
use crate::selection::ReviewedSelection;
use crate::settings::{PrivacyMode, PrivacySettings};
use crate::{failure, images, preview, protocol, settings, view};
use pcagent_agent::credentials::Credentials;
use pcagent_agent::image_input::ImageInput;
use pcagent_agent::selection::SelectionTarget;
use pcagent_agent::{local_report, Sharing};
use pcagent_nebius::Config;
use pcagent_shared::{unix_ms, Snapshot};
use pcagent_storage::StorageService;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, State};

/// C3 entry point. The stored mode is a preference; a cloud request also needs the exact
/// single-use preview ticket reviewed for this question and conversation.
#[tauri::command]
pub(super) fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: Option<String>,
    text: String,
    scope_ids: Vec<String>,
    preview_id: Option<String>,
    image_id: Option<String>,
) -> Result<Started, Failure> {
    if scope_ids.len() > 1 {
        return Err("Attach one scanned folder to a message.".into());
    }
    if let Some(scope) = scope_ids.first() {
        let scan = lock(&state.scan);
        if scan.running || scan.scope_id.as_ref() != Some(scope) {
            return Err(Failure::new(
                Code::ScanUnavailable,
                "The selected folder is no longer the current completed scan.",
            ));
        }
    }
    let privacy = lock(&state.privacy).clone();
    // A cloud mode without a usable key and model runs locally, as the window says it will.
    let sharing = privacy.mode.sharing().filter(|_| {
        let creds = Credentials::load();
        Config::from_lookup(|name| creds.get(name)).is_ok()
    });
    if image_id.is_some() && privacy.mode != PrivacyMode::Complete {
        return Err(Failure::new(
            Code::ImageSharingRequired,
            "Images require Complete sharing.",
        ));
    }
    if image_id.is_some() && sharing.is_none() {
        return Err(Failure::new(
            Code::ImageModelUnavailable,
            "Configure Nebius before sending an image.",
        ));
    }
    // No confirmation dialog stands before a send (C5, 9 October): a cloud send still needs the
    // single-use ticket of its exact preview, checked when the turn starts.
    let file_names = sharing.is_some()
        && lock(&state.preview)
            .as_ref()
            .filter(|p| Some(p.id.as_str()) == preview_id.as_deref())
            .is_some_and(|p| p.file_names);
    start_investigation(
        app,
        state,
        StartRequest {
            question: text,
            cloud: sharing.is_some(),
            use_storage: !scope_ids.is_empty(),
            names: sharing == Some(Sharing::Complete),
            conversation_id,
            preview_id,
            file_names,
            image_id,
        },
    )
}

#[tauri::command]
pub(super) fn start_investigation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartRequest,
) -> Result<Started, Failure> {
    launch_investigation(app, &state, request, None)
}

pub(super) fn launch_investigation(
    app: AppHandle,
    state: &AppState,
    request: StartRequest,
    selected_start: Option<SelectionStart>,
) -> Result<Started, Failure> {
    let StartRequest {
        question,
        cloud,
        use_storage,
        names,
        conversation_id,
        preview_id,
        file_names,
        image_id,
    } = request;
    let question = checked_question(&question)?;
    if image_id.is_some()
        && (!cloud
            || !names
            || selected_start.is_some()
            || lock(&state.privacy).mode != PrivacyMode::Complete)
    {
        return Err(Failure::new(
            Code::ImageSharingRequired,
            "Images require a Complete-sharing chat turn.",
        ));
    }
    if file_names && ((!cloud && selected_start.is_none()) || !use_storage || !names) {
        return Err(
            "File names require cloud sharing, complete mode and the approved folder.".into(),
        );
    }
    let mut slot = lock(&state.investigation);
    if slot
        .as_ref()
        .is_some_and(|r| !r.finished.load(Ordering::SeqCst))
    {
        return Err(Failure::new(
            Code::TurnRunning,
            "An investigation is already running.",
        ));
    }
    let history_guard = lock(&state.history);
    let prior = read_past_from(&history_guard, conversation_id.as_deref())?;
    let prepared = if let Some(selected) = selected_start {
        // Scan replacement holds this same lock through source invalidation. Keep it until
        // the reviewed ticket is consumed so the retired scope cannot win that race.
        let selected_scan = matches!(&selected.reference.target, SelectionTarget::Storage { .. })
            .then(|| lock(&state.scan));
        if let SelectionTarget::Storage { scope_id, .. } = &selected.reference.target {
            let scan = selected_scan
                .as_ref()
                .expect("storage selection owns the scan lock");
            if scan.running
                || scan.scope_id.as_ref() != Some(scope_id)
                || state.storage.index(scope_id).is_err()
            {
                return Err(Failure::new(
                    Code::ScanUnavailable,
                    "The selected storage scan is no longer available.",
                ));
            }
        }
        let mut selections = lock(&state.selections);
        let (sharing, approved_file_names, routing) = selections
            .choices(&selected.preview_id)
            .ok_or_else(|| Failure::new(Code::PreviewExpired, "The selected preview expired."))?;
        // Match the preview's registry -> routing lock order. A policy update must not
        // slip between validation and consumption of its reviewed ticket.
        let model_routing = lock(&state.model_routing);
        if approved_file_names != file_names
            || sharing != sharing_of(names)
            || !model_routing.matches(&routing, configured_model_id().as_deref())
        {
            return Err(Failure::new(
                Code::PreviewChanged,
                "Sharing or model settings changed. Open the selected preview again.",
            ));
        }
        Some(ApprovedCapture::Selected(Box::new(selections.take(
            ReviewedSelection {
                preview_id: &selected.preview_id,
                reference: &selected.reference,
                intent: selected.intent,
                question: Some(&question),
                conversation_id: conversation_id.as_deref(),
                past: &prior,
                sharing,
                file_names,
                context_revision: state.selection_context_revision.load(Ordering::SeqCst),
            },
        )?)))
    } else if cloud {
        let scan = lock(&state.scan);
        let scope = if use_storage {
            Some(
                scan.scope_id
                    .as_deref()
                    .filter(|s| !scan.running && state.storage.index(s).is_ok())
                    .ok_or_else(|| {
                        Failure::new(
                            Code::ScanUnavailable,
                            "The approved scan is no longer available. Open the preview again.",
                        )
                    })?,
            )
        } else {
            None
        };
        let images = lock(&state.images);
        let mut preview = lock(&state.preview);
        if preview
            .as_ref()
            .and_then(|p| p.image.as_ref())
            .is_some_and(|image| !images.matches(image))
        {
            return Err(images::changed());
        }
        let routing = preview
            .as_ref()
            .and_then(|prepared| prepared.routing.clone())
            .ok_or_else(preview_required)?;
        // The preview owns its ticket before the routing guard, as in cloud_preview.
        // Keep both until take so a concurrent policy change cannot approve stale settings.
        let model_routing = lock(&state.model_routing);
        if !model_routing.matches(&routing, configured_model_id().as_deref()) {
            return Err(Failure::new(
                Code::PreviewChanged,
                "Model settings changed. Open the sharing preview again.",
            ));
        }
        Some(ApprovedCapture::Snapshot(Box::new(preview::take(
            &mut preview,
            Reviewed {
                id: preview_id.as_deref().ok_or_else(preview_required)?,
                question: &question,
                conversation_id: conversation_id.as_deref(),
                past: &prior,
                sharing: sharing_of(names),
                storage_scope: scope,
                file_names,
                image_id: image_id.as_deref(),
            },
        )?)))
    } else {
        None
    };
    drop(history_guard);
    // A follow-up reads its conversation again; a first question starts one.
    let kept = lock(&state.privacy).clone();
    let remembered = PrivacySettings {
        mode: if !cloud {
            PrivacyMode::Local
        } else if names {
            PrivacyMode::Complete
        } else {
            PrivacyMode::Anonymized
        },
        confirmed: true,
        // The web and file-name toggles are their own choices and survive a change of mode.
        web_search: kept.web_search,
        file_names: kept.file_names,
    };
    settings::save(&remembered).map_err(failure::saving)?;
    if lock(&state.privacy).mode != remembered.mode {
        state.preview_revision.fetch_add(1, Ordering::SeqCst);
        state
            .selection_context_revision
            .fetch_add(1, Ordering::SeqCst);
        *lock(&state.preview) = None;
        lock(&state.selections).invalidate_tickets();
    }
    *lock(&state.privacy) = remembered;
    let (conversation_id, past) = {
        let history = lock(&state.history);
        match history.as_ref() {
            None => (None, Vec::new()),
            Some(store) => {
                match conversation_id {
                    Some(id) => {
                        // A deletion after ticket consumption cannot retract the approved
                        // capture, but must not recreate a thread containing its deleted past.
                        let past = store
                            .get(&id)
                            .map_err(failure::history)?
                            .map(|conversation| past_turns(&conversation))
                            .unwrap_or_default();
                        (Some(id), past)
                    }
                    None => (store.create(&question, unix_ms()).ok(), Vec::new()),
                }
            }
        }
    };
    let past = prepared
        .as_ref()
        .map_or(past, |prepared| prepared.past().to_vec());
    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    let turn_id = pcagent_shared::new_id("turn");
    let running = Running {
        id,
        turn_id: turn_id.clone(),
        stop: Arc::new(AtomicBool::new(false)),
        answer: Arc::new(Mutex::new(None)),
        finished: Arc::new(AtomicBool::new(false)),
        question: question.clone(),
        conversation_id: conversation_id.clone(),
        started_at_unix_ms: unix_ms(),
        extension: Arc::new(Mutex::new(None)),
    };
    let (stop, answer, finished, extension) = (
        running.stop.clone(),
        running.answer.clone(),
        running.finished.clone(),
        running.extension.clone(),
    );
    // Without permission, the model gets an empty storage service: no folder is visible.
    let storage = if use_storage {
        state.storage.clone()
    } else {
        Arc::new(StorageService::new())
    };
    let history = state.history.clone();
    let sampler = state.sampler.clone();
    let proposals = state.actions.clone();
    let elevated_evidence = state.elevation.completed_evidence();
    let web_search = cloud && lock(&state.privacy).web_search;
    let probes = *lock(&state.probes);
    let program_names = state.program_names.clone();
    let saved_in = conversation_id.clone();
    let worker_turn_id = turn_id.clone();
    thread::Builder::new()
        .name("investigation".into())
        .spawn(move || {
            let asked_at = unix_ms();
            // Local analysis, or the model with the person's choice of program names.
            let ask = Ask {
                turn_id: &worker_turn_id,
                question: &question,
                past: &past,
                cloud: cloud.then(|| sharing_of(names)),
                initial: prepared.as_ref().map(ApprovedCapture::snapshot),
                privacy: prepared.as_ref().map(ApprovedCapture::privacy),
                storage_context: prepared.as_ref().map(ApprovedCapture::context),
                scope_ids: if use_storage { prepared.as_ref().and_then(ApprovedCapture::scope).map(str::to_owned).into_iter().collect() } else { Vec::new() },
                selected: prepared.as_ref().and_then(ApprovedCapture::selected),
                routing: prepared.as_ref().and_then(ApprovedCapture::routing),
                image: prepared.as_ref().and_then(ApprovedCapture::image),
                elevated_evidence,
                web_search,
                probes,
            };
            let outcome = investigation(&app, id, &ask, storage, &stop, &answer, &extension, sampler.as_ref(), &proposals, &program_names);
            let stored_report = match &outcome {
                Ok(report) => report.clone(),
                Err((code, message)) => {
                    let mut report = view::report(&local_report(&Snapshot::empty("failed-turn")), &question, &view::no_readable_names);
                    report.execution = pcagent_agent::ExecutionStatus::Failed;
                    report.mode = if cloud { "nebius" } else { "local_rules" }.into();
                    report.error = Some(view::ErrorView {code:code.clone(),message:message.clone()});
                    report
                }
            };
            let mut value = serde_json::to_value(&stored_report).unwrap_or_default();
            value["schema_version"] = serde_json::json!(1);
            let mut completed_turn = protocol::turn(&worker_turn_id, &question, asked_at, &value);
            if let Some(image) = ask.image {
                completed_turn["image"] = serde_json::json!(image.info());
            }
            // Every finished turn is kept, unless the conversation was deleted meanwhile.
            let conversation_id = match &saved_in {
                Some(conversation) => {
                    lock(&history)
                        .as_mut()
                        .and_then(|store| {
                            store
                                .add_turn_with_image(conversation, &worker_turn_id, &question, asked_at, &value, ask.image.map(ImageInput::record))
                                .ok()
                                .flatten()
                        })
                        .map(|_| conversation.clone())
                }
                _ => None,
            };
            // Finished before the last event, so the next question can start right away.
            finished.store(true, Ordering::SeqCst);
            if let Some(error) = &stored_report.error {
                let _ = app.emit("turn:failed", serde_json::json!({"turn_id":worker_turn_id,"conversation_id":conversation_id,
                    "code":error.code,"message":error.message,"turn":completed_turn}));
            } else {
                let _ = app.emit("turn:done", serde_json::json!({"turn_id":worker_turn_id,"conversation_id":conversation_id,"turn":completed_turn}));
            }
            let _ = match outcome {
                Ok(report) => app.emit(
                    "inv:done",
                    DoneEvent {
                        id,
                        report,
                        conversation_id,
                    },
                ),
                Err((code, message)) => app.emit("inv:failed", FailedEvent { id, code, message }),
            };
        })
        .map_err(|e| format!("Cannot start the investigation: {e}"))?;
    *slot = Some(running);
    if let Some(image_id) = image_id {
        lock(&state.images).discard(&image_id);
    }
    Ok(Started {
        id,
        conversation_id,
        turn_id,
    })
}

/// The running turn, or nothing: a window that reloaded reattaches it instead of leaving it
/// running unseen (and blocking every new question).
#[tauri::command]
pub(super) fn current_turn(state: State<'_, AppState>) -> Option<CurrentTurn> {
    lock(&state.investigation)
        .as_ref()
        .filter(|r| !r.finished.load(Ordering::SeqCst))
        .map(|r| CurrentTurn {
            turn_id: r.turn_id.clone(),
            conversation_id: r.conversation_id.clone(),
            question: r.question.clone(),
            started_at_unix_ms: r.started_at_unix_ms,
            stopping: r.stop.load(Ordering::SeqCst),
            extension: lock(&r.extension).clone(),
        })
}

#[tauri::command]
pub(super) fn answer_extension(
    state: State<'_, AppState>,
    id: Option<u64>,
    turn_id: Option<String>,
    accept: bool,
) -> Result<(), String> {
    let slot = lock(&state.investigation);
    let running = slot
        .as_ref()
        .filter(|r| protocol::identity_matches(r.id, &r.turn_id, id, turn_id.as_deref()))
        .ok_or("This investigation is over.")?;
    let sender = lock(&running.answer).take();
    sender
        .ok_or("No question is waiting for an answer.")?
        .send(accept)
        .map_err(|_| "This investigation is over.".into())
}

#[tauri::command]
pub(super) fn stop_investigation(state: State<'_, AppState>, id: u64) {
    if let Some(running) = lock(&state.investigation).as_ref().filter(|r| r.id == id) {
        running.stop.store(true, Ordering::SeqCst);
        if let Some(sender) = lock(&running.answer).take() {
            let _ = sender.send(false);
        }
    }
}

#[tauri::command]
pub(super) fn stop_turn(state: State<'_, AppState>, turn_id: String) -> Result<(), String> {
    let slot = lock(&state.investigation);
    let running = slot
        .as_ref()
        .filter(|r| r.turn_id == turn_id && !r.finished.load(Ordering::SeqCst))
        .ok_or("This turn is over.")?;
    running.stop.store(true, Ordering::SeqCst);
    if let Some(sender) = lock(&running.answer).take() {
        let _ = sender.send(false);
    }
    Ok(())
}
