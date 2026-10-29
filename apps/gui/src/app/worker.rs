//! Read-only investigation execution, provider routing and extension waits.
use super::lock;
use super::state::{Ask, ExtensionEvent, StepEvent};
use crate::model_routing::RoutingReview;
use crate::session::{ObservedModel, ObservedTools, SeenProcesses, Step};
use crate::{actions, protocol, view};
use pcagent_agent::credentials::Credentials;
use pcagent_agent::image_input::MODEL_ID as IMAGE_MODEL_ID;
use pcagent_agent::routing::{RoutedModel, RoutingMode, RoutingSettings};
use pcagent_agent::selection::{SelectionIntent, GET_SELECTED_EVIDENCE};
use pcagent_agent::{
    converse_with_image, converse_with_privacy, converse_with_selection, local_report, AgentError,
    ExtensionRequest, Investigation, SnapshotTools,
};
use pcagent_nebius::{Config, Nebius};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{unix_ms, Snapshot};
use pcagent_storage::StorageService;
use pcagent_store::sampler::Sampler;
use pcagent_telemetry::{Collector, ProgramNames};
use pcagent_tools::Workstation;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Runs one investigation, reporting each real step; returns the report, or why it could not
/// start (error code and message).
#[allow(clippy::too_many_arguments)]
pub(super) fn investigation(
    app: &AppHandle,
    id: u64,
    ask: &Ask<'_>,
    storage: Arc<StorageService>,
    stop: &Arc<AtomicBool>,
    answer: &Arc<Mutex<Option<mpsc::Sender<bool>>>>,
    extension: &Mutex<Option<Value>>,
    sampler: Option<&Sampler>,
    proposals: &actions::Slot,
    names: &ProgramNames,
) -> Result<view::Report, (String, String)> {
    let readable = |pid: u32, created: u64| names.name(pid, created);
    // Every process the turn sees, so its draft names programs as its answer will.
    let seen = SeenProcesses::default();
    let emit = |mut step: Step| {
        let at_unix_ms = unix_ms();
        if let Step::AnswerDraft { text, .. } = &mut step {
            *text = seen.localize(text, &readable);
        }
        let _ = app.emit("turn:step", protocol::step(ask.turn_id, at_unix_ms, &step));
        if let Step::AnswerDraft { request, text } = &step {
            let _ = app.emit(
                "turn:delta",
                serde_json::json!({"turn_id":ask.turn_id,"request":request,
                "text":text,"replace":true,"unverified":true}),
            );
        }
        let _ = app.emit(
            "inv:step",
            StepEvent {
                id,
                at_unix_ms,
                step,
            },
        );
    };
    let explain = ask
        .selected
        .is_some_and(|selected| selected.intent() == SelectionIntent::Explain);
    let mut collector = (!explain).then(Collector::new);
    if let Some(selected) = ask.selected {
        emit(Step::SelectedEvidenceLoaded {
            intent: selected.intent(),
            measured_at_unix_ms: selected.output().cloud["measured_at_unix_ms"].as_u64(),
        });
    } else {
        emit(Step::SnapshotStarted);
    }
    let started = Instant::now();
    let initial = ask.initial.cloned().unwrap_or_else(|| {
        collector
            .as_mut()
            .expect("ordinary turn owns a collector")
            .snapshot(true, 10)
    });
    // What the investigation starts from, then each tool output (`record`), in its own order.
    seen.add(&initial.processes);
    if let Some(selected) = ask.selected {
        seen.add(&selected.output().processes);
    }
    let record = |output: &pcagent_agent::ToolOutput| seen.add(&output.processes);
    if ask.selected.is_none() {
        emit(Step::SnapshotFinished {
            duration_ms: started.elapsed().as_millis() as u64,
            measured: initial
                .evidence
                .iter()
                .filter(|e| e.usable_value().is_some())
                .count(),
        });
    }
    let question = ask.question;
    let report: Investigation = if let Some(sharing) = ask.cloud {
        let routing = ask.routing.ok_or_else(|| {
            (
                "preview_required".into(),
                "Open the model sharing preview before sending.".into(),
            )
        })?;
        let model = match build_routed_model(routing, ask.image.is_some()) {
            Ok(model) => model,
            Err(e) => {
                let message = match &e {
                    AgentError::MissingKey
                    | AgentError::MissingModel
                    | AgentError::InvalidModel => {
                        format!("{e}. Add your Nebius key and model in Settings, or analyze on this PC.")
                    }
                    other => other.to_string(),
                };
                return Err((view::error_code(&e), message));
            }
        };
        let mut model = ObservedModel::new(model, stop.clone(), emit);
        let privacy = ask.privacy.ok_or_else(|| {
            (
                "preview_required".into(),
                "Open the sharing preview before sending.".into(),
            )
        })?;
        if explain {
            let selected = ask.selected.expect("Explain owns selected evidence");
            let mut no_collectors =
                SnapshotTools::new(|_, _| Snapshot::empty("unavailable-collector"));
            let selected_tools =
                pcagent_agent::selection::SelectedTools::new(&mut no_collectors, selected);
            let mut tools = ObservedTools::new(
                selected_tools,
                |tool: &str| emit(Step::ToolStarted { tool: tool.into() }),
                record,
            );
            let report = converse_with_selection(
                &mut model,
                ask.past,
                question,
                selected,
                &mut tools,
                privacy,
                |event| {
                    emit(Step::ToolFinished {
                        tool: event.tool.clone(),
                        status: event.status,
                        duration_ms: event.duration_ms,
                        query: event.query.clone(),
                    })
                },
                |partial, request| {
                    ask_extension(
                        app,
                        id,
                        ask.turn_id,
                        partial,
                        request,
                        stop,
                        answer,
                        extension,
                    )
                },
            );
            return Ok(view::report(&report, question, &readable));
        }
        let (policy, prefs) = (Policy::from_env(), Preferences::default());
        let mut workstation = Workstation::new(
            collector.as_mut().expect("Investigate owns a collector"),
            storage,
            &policy,
            &prefs,
        );
        workstation.set_sharing(sharing);
        workstation.set_allowed_scopes(ask.scope_ids.clone());
        workstation.set_elevated_evidence(ask.elevated_evidence.clone());
        // Web lookups (C6): only after the separate consent and with a saved Tavily key.
        if ask.web_search {
            if let Some(tavily) = Credentials::load()
                .get("TAVILY_API_KEY")
                .and_then(|key| pcagent_tools::tavily::Tavily::new(&key))
            {
                let cache_path = pcagent_agent::credentials::file_path()
                    .and_then(|path| path.parent().map(|dir| dir.join("web-cache.sqlite")));
                let page_cache = cache_path
                    .as_ref()
                    .and_then(|path| pcagent_store::web_cache::WebCache::open(path).ok());
                let tavily = match page_cache {
                    Some(cache) => tavily.with_page_cache(cache),
                    None => tavily,
                };
                workstation.enable_web_search(Box::new(tavily), pcagent_inspect::region_country());
                if let Some(cache) = cache_path
                    .as_ref()
                    .and_then(|path| pcagent_store::web_cache::WebCache::open(path).ok())
                {
                    workstation.set_web_cache(cache);
                }
            }
        }
        if let Some(sampler) = sampler {
            workstation.set_history(sampler.clone(), stop.clone());
        }
        if let Some(context) = ask.storage_context {
            workstation.set_storage_context(context.clone());
        }
        // The model may propose actions; they wait for a click in this window to run.
        if let Some(desk) = actions::desk() {
            workstation.enable_action_proposals(desk);
        }
        // Only the tools that work on this PC and in this turn are declared, before Investigate
        // copies the list.
        workstation.offer_available(ask.probes);
        let (report, mut workstation) = if let Some(selected) = ask.selected {
            let selected_tools =
                pcagent_agent::selection::SelectedTools::new(&mut workstation, selected);
            let mut tools = ObservedTools::new(
                selected_tools,
                |tool: &str| emit(Step::ToolStarted { tool: tool.into() }),
                record,
            );
            let report = converse_with_selection(
                &mut model,
                ask.past,
                question,
                selected,
                &mut tools,
                privacy,
                |event| {
                    emit(Step::ToolFinished {
                        tool: event.tool.clone(),
                        status: event.status,
                        duration_ms: event.duration_ms,
                        query: event.query.clone(),
                    })
                },
                |partial, request| {
                    ask_extension(
                        app,
                        id,
                        ask.turn_id,
                        partial,
                        request,
                        stop,
                        answer,
                        extension,
                    )
                },
            );
            drop(tools);
            (report, workstation)
        } else {
            let mut tools = ObservedTools::new(
                workstation,
                |tool: &str| emit(Step::ToolStarted { tool: tool.into() }),
                record,
            );
            let on_event = |event: &pcagent_agent::ToolEvent| {
                emit(Step::ToolFinished {
                    tool: event.tool.clone(),
                    status: event.status,
                    duration_ms: event.duration_ms,
                    query: event.query.clone(),
                })
            };
            let on_extension = |partial: &Investigation, request: &ExtensionRequest| {
                ask_extension(
                    app,
                    id,
                    ask.turn_id,
                    partial,
                    request,
                    stop,
                    answer,
                    extension,
                )
            };
            let report = if let Some(image) = ask.image {
                converse_with_image(
                    &mut model,
                    ask.past,
                    question,
                    &initial,
                    &mut tools,
                    privacy,
                    image,
                    on_event,
                    on_extension,
                )
            } else {
                converse_with_privacy(
                    &mut model,
                    ask.past,
                    question,
                    &initial,
                    &mut tools,
                    privacy,
                    on_event,
                    on_extension,
                )
            };
            (report, tools.into_inner())
        };
        if let Some((_, pending)) = workstation.take_actions() {
            actions::keep(proposals, ask.turn_id, pending, &readable);
        }
        let jobs = workstation.take_jobs();
        for job in &jobs {
            job.cancel();
        }
        for job in jobs {
            let _ = job.wait();
        }
        report
    } else {
        let mut report = local_report(&initial);
        if let Some(selected) = ask.selected {
            report.mode = "local_evidence".into();
            report.record_local_observation(&GET_SELECTED_EVIDENCE, selected.output().clone());
            report.limitations.push(view::LOCAL_EVIDENCE_NOTE.into());
        }
        report
    };
    Ok(view::report(&report, question, &readable))
}

pub(super) fn build_routed_model(
    review: &RoutingReview,
    image: bool,
) -> Result<RoutedModel<Nebius>, AgentError> {
    let credentials = Credentials::load();
    let config = Config::from_lookup(|name| credentials.get(name))?;
    if review.configured_model_id.as_deref() != Some(config.model_id())
        || review.reasoning != Some(config.reasoning())
    {
        return Err(AgentError::InvalidModel);
    }
    if image {
        return RoutedModel::new(
            Nebius::new(config.for_model(IMAGE_MODEL_ID)?)?,
            None,
            &RoutingSettings::default(),
            None,
        )
        .map_err(|_| AgentError::ImageNotAllowed);
    }
    let primary =
        Nebius::new(config.for_model(review.settings.initial_model(config.model_id())?)?)?;
    let ultra = if review.settings.mode == RoutingMode::Automatic {
        Some(Nebius::new(
            config.for_model(
                review
                    .settings
                    .ultra_model_id
                    .as_deref()
                    .ok_or(AgentError::MissingModel)?,
            )?,
        )?)
    } else {
        None
    };
    RoutedModel::new(
        primary,
        ultra,
        &review.settings,
        review.calibration.as_ref(),
    )
    .map_err(|_| AgentError::InvalidModel)
}

/// Blocks the investigation until the person answers in the window, stops, or closes it.
#[allow(clippy::too_many_arguments)]
pub(super) fn ask_extension(
    app: &AppHandle,
    id: u64,
    turn_id: &str,
    partial: &Investigation,
    request: &ExtensionRequest,
    stop: &AtomicBool,
    answer: &Mutex<Option<mpsc::Sender<bool>>>,
    pending: &Mutex<Option<Value>>,
) -> bool {
    if stop.load(Ordering::SeqCst) {
        return false;
    }
    let (tx, rx) = mpsc::channel();
    *lock(answer) = Some(tx);
    let event = serde_json::json!({"turn_id":turn_id,"request":request,"usage":view::usage(&partial.stats)});
    // Kept until answered, so a reloaded window can ask again (current_turn).
    *lock(pending) = Some(event.clone());
    let _ = app.emit("turn:extension", event);
    let _ = app.emit(
        "inv:extension",
        ExtensionEvent {
            id,
            request: request.clone(),
            usage: view::usage(&partial.stats),
        },
    );
    let accepted = loop {
        if stop.load(Ordering::SeqCst) {
            break false;
        }
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(accepted) => break accepted,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break false,
        }
    };
    *lock(answer) = None;
    *lock(pending) = None;
    accepted
}
