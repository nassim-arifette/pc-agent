//! The Tauri shell: commands the page may call and the events it listens to. The page is
//! untrusted input: every argument is validated here, paths and item references are resolved on
//! this side, and the Nebius key never reaches the page.
use crate::{
    actions, elevation,
    failure::{self, Code, Failure},
    model_routing::{ModelRouting, RoutingReview, RoutingView},
    preview::{self, PreparedTurn, Reviewed},
    protocol,
    selection::{PreparedSelection, ReviewedSelection, SelectionPreview, SelectionRegistry},
    session::{ObservedModel, ObservedTools, SeenProcesses, Step},
    settings::{self, PrivacyMode, PrivacySettings},
    shell, view,
};
use pcagent_agent::{
    capabilities::Probes,
    converse_with_privacy, converse_with_selection,
    credentials::{self, Credentials},
    local_report, AgentError, Config, ExtensionRequest, Investigation, Nebius, PastTurn,
    PrivacyContext, Sharing, StorageContext, Workstation,
};
use pcagent_agent::{
    routing::{RoutedModel, RoutingMode, RoutingSettings},
    selection::{
        FrozenSelection, RecordingSource, SelectionIntent, SelectionRef, SelectionSource,
        SelectionTarget, StorageSource, GET_SELECTED_EVIDENCE,
    },
    SnapshotTools,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{unix_ms, Snapshot};
use pcagent_storage::{ItemKind, ScanOptions, StorageService, MAX_PAGE};
use pcagent_store::{
    sampler::{Sampler, SamplerStatus},
    HistoryQuery, HistorySeries, Recording,
};
use pcagent_store::{Conversation, ConversationSummary, Store, DEFAULT_RETENTION_DAYS};
use pcagent_telemetry::{Collector, ProgramNames};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

const PAGE: usize = 50;

/// Requests for the thread that owns the live collector (its native handles stay on it).
enum Measure {
    Dashboard(mpsc::Sender<view::Dashboard>),
    Activity(mpsc::Sender<view::ActivityView>),
    Preview(mpsc::Sender<Snapshot>),
}

struct Running {
    id: u64,
    turn_id: String,
    stop: Arc<AtomicBool>,
    answer: Arc<Mutex<Option<mpsc::Sender<bool>>>>,
    finished: Arc<AtomicBool>,
    /// What a reloaded window needs to reattach this turn (`current_turn`).
    question: String,
    conversation_id: Option<String>,
    started_at_unix_ms: u64,
    /// The extension request waiting for an answer, as `turn:extension` sent it.
    extension: Arc<Mutex<Option<Value>>>,
}

/// The running turn, so a reloaded window can show it, stop it and answer its extension request.
#[derive(Serialize)]
struct CurrentTurn {
    turn_id: String,
    conversation_id: Option<String>,
    question: String,
    started_at_unix_ms: u64,
    stopping: bool,
    extension: Option<Value>,
}

#[derive(Default)]
struct ScanSlot {
    scope_id: Option<String>,
    root: Option<String>,
    running: bool,
}

struct AppState {
    measure: Mutex<mpsc::Sender<Measure>>,
    /// Shared by the Storage screen and investigations: a folder scanned there is authorized for
    /// the model (sizes only, names hidden) when the person allows it for an investigation.
    storage: Arc<StorageService>,
    scan: Arc<Mutex<ScanSlot>>,
    investigation: Mutex<Option<Running>>,
    next_id: AtomicU64,
    /// Local conversation history; `None` when its file cannot be opened.
    history: Arc<Mutex<Option<Store>>>,
    sampler: Option<Sampler>,
    preview: Mutex<Option<PreparedTurn>>,
    preview_revision: AtomicU64,
    /// Changes of consent/model policy, independent of ordinary dashboard/preview refreshes.
    selection_context_revision: AtomicU64,
    privacy: Mutex<PrivacySettings>,
    selections: Arc<Mutex<SelectionRegistry>>,
    model_routing: Mutex<ModelRouting>,
    /// The last finished turn's action proposals; they run only after a click in this window.
    actions: Arc<actions::Slot>,
    elevation: Arc<elevation::Slot>,
    /// What this PC offers (`pcagent_agent::capabilities`), from the probes the window already
    /// runs: the helper next to the app at start, NVIDIA GPUs and NTFS volumes from the live
    /// measurements, the battery from the live battery read. Turns and `app_info` read it.
    probes: Arc<Mutex<Probes>>,
    /// Readable program names for the window, read and cached on the threads that measure or
    /// investigate, never on the window's thread. They stay local (C5, note of 26 October).
    program_names: Arc<ProgramNames>,
}

/// The history file next to the settings file, with conversations older than the retention
/// period removed.
fn open_history() -> Option<Store> {
    let dir = credentials::file_path()?.parent()?.to_path_buf();
    let store = Store::open(&dir.join("pcagent.sqlite")).ok()?;
    let retention_ms = DEFAULT_RETENTION_DAYS * 24 * 3_600_000;
    let _ = store.prune(unix_ms().saturating_sub(retention_ms));
    Some(store)
}

fn open_sampler() -> Option<Sampler> {
    let dir = credentials::file_path()?.parent()?.to_path_buf();
    Sampler::start(&dir.join("pcagent.sqlite")).ok()
}

fn history_unavailable() -> Failure {
    Failure::new(Code::HistoryUnavailable, "Local history is unavailable.")
}

#[tauri::command]
fn history_status(state: State<'_, AppState>) -> Option<SamplerStatus> {
    state.sampler.as_ref().map(Sampler::status)
}

#[tauri::command]
async fn metric_history(
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
async fn record_activity_start(
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
async fn battery_recording(
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
fn record_activity_cancel(state: State<'_, AppState>, recording_id: String) -> Result<(), Failure> {
    state
        .sampler
        .as_ref()
        .ok_or_else(history_unavailable)?
        .cancel_recording(&recording_id)
        .map_err(failure::history)
}

#[tauri::command]
async fn history_clear(state: State<'_, AppState>) -> Result<usize, Failure> {
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

#[tauri::command]
fn startup_get() -> Result<bool, Failure> {
    pcagent_inspect::autostart::get().map_err(failure::startup)
}

#[tauri::command]
fn startup_set(enabled: bool) -> Result<bool, Failure> {
    let executable = std::env::current_exe().map_err(failure::startup)?;
    pcagent_inspect::autostart::set(enabled, &executable).map_err(failure::startup)?;
    startup_get()
}

/// Only typed model context is reusable. Legacy display-only reports remain visible locally;
/// their localized prose has no trustworthy name mapping or measured sources to resend.
fn past_turns(conversation: &Conversation) -> Vec<PastTurn> {
    conversation
        .turns
        .iter()
        .filter_map(|turn| serde_json::from_value(turn.report.get("context")?.clone()).ok())
        .collect()
}

#[tauri::command]
fn conversations_list(state: State<'_, AppState>) -> Result<Vec<ConversationSummary>, String> {
    match lock(&state.history).as_ref() {
        Some(store) => store.list(200).map_err(|e| e.to_string()),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
fn conversation_get(state: State<'_, AppState>, id: String) -> Result<Option<Value>, String> {
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
                        .map(|turn| protocol::turn(
                            &turn.id,
                            &turn.question,
                            turn.asked_at_unix_ms,
                            &turn.report
                        ))
                        .collect::<Vec<_>>());
                    value
                })
            })
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[tauri::command]
fn conversation_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    // Use the same history -> ticket order as preview publication and consumption. Delete
    // only while both ticket stores are held, so a completed deletion cannot resend its past.
    let history = lock(&state.history);
    let mut preview = lock(&state.preview);
    let mut selections = lock(&state.selections);
    let deleted = match history.as_ref() {
        Some(store) => store.delete(&id).map_err(|e| e.to_string())?,
        None => false,
    };
    preview::invalidate_conversation(&mut preview, &id);
    selections.invalidate_conversation(&id);
    Ok(deleted)
}

#[tauri::command]
fn conversations_clear(state: State<'_, AppState>) -> Result<usize, String> {
    let history = lock(&state.history);
    let mut preview = lock(&state.preview);
    let mut selections = lock(&state.selections);
    let deleted = match history.as_ref() {
        Some(store) => store.clear().map_err(|e| e.to_string())?,
        None => 0,
    };
    state.preview_revision.fetch_add(1, Ordering::SeqCst);
    *preview = None;
    selections.invalidate_tickets();
    Ok(deleted)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn configured_model_id() -> Option<String> {
    Credentials::load()
        .get("NEBIUS_MODEL_ID")
        .filter(|id| pcagent_agent::validate_model_id(id).is_ok())
}

fn retain_capture(registry: &Mutex<SelectionRegistry>, source: SelectionSource) -> Option<String> {
    let mut registry = lock(registry);
    if let Some(session) = source.process_session() {
        registry.invalidate_process_session(session);
    }
    registry.publish(source).ok()
}

fn retain_recording(registry: &Mutex<SelectionRegistry>, sampler: &Sampler, id: &str) {
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

fn retain_storage_page(
    registry: &Mutex<SelectionRegistry>,
    storage: &StorageService,
    scan: &Mutex<ScanSlot>,
    page: pcagent_storage::LargestItemsPage,
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
fn model_routing_get(state: State<'_, AppState>) -> RoutingView {
    let mut routing = lock(&state.model_routing);
    // A campaign reviewed while the window is open makes Auto available without a restart.
    routing.refresh();
    routing.view()
}

#[tauri::command]
fn model_routing_set(
    state: State<'_, AppState>,
    settings: RoutingSettings,
) -> Result<RoutingView, Failure> {
    let view = lock(&state.model_routing).set(settings)?;
    state.preview_revision.fetch_add(1, Ordering::SeqCst);
    state
        .selection_context_revision
        .fetch_add(1, Ordering::SeqCst);
    *lock(&state.preview) = None;
    lock(&state.selections).invalidate_tickets();
    Ok(view)
}

#[tauri::command]
fn selection_sources(state: State<'_, AppState>) -> Vec<crate::selection::SelectionDescriptor> {
    lock(&state.selections).sources()
}

#[tauri::command]
fn selection_preview(
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

struct SelectionStart {
    reference: SelectionRef,
    intent: SelectionIntent,
    preview_id: String,
}

#[tauri::command]
fn selection_send(
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
        },
        Some(SelectionStart {
            reference,
            intent,
            preview_id,
        }),
    )
}

/// Free space changes slowly; the inventory is read again at most this often.
const VOLUMES_MAX_AGE: Duration = Duration::from_secs(30);

/// Reads the battery on its own thread, off the measurement thread, each time it is asked.
/// The read runs on `pcagent_inspect::live_battery`'s bounded worker, never on the shared
/// inspection worker used by agent tools and recordings.
fn battery_thread(
    pokes: mpsc::Receiver<()>,
    latest: Arc<Mutex<view::LiveBattery>>,
    selections: Arc<Mutex<SelectionRegistry>>,
    probes: Arc<Mutex<Probes>>,
) {
    for () in pokes {
        let mut read = view::battery_read(pcagent_inspect::live_battery());
        // Published before `latest` is locked, so the two locks are never nested.
        let source_id = read
            .source
            .take()
            .and_then(|source| retain_capture(&selections, SelectionSource::Battery(source)));
        if let Some(listed) = read.battery_listed() {
            lock(&probes).battery = Some(listed);
        }
        lock(&latest).record(read, source_id, Instant::now());
    }
}

fn measure_thread(
    requests: mpsc::Receiver<Measure>,
    selections: Arc<Mutex<SelectionRegistry>>,
    battery: Arc<Mutex<view::LiveBattery>>,
    poke: mpsc::SyncSender<()>,
    probes: Arc<Mutex<Probes>>,
    names: Arc<ProgramNames>,
) {
    let mut collector = Collector::new();
    let mut volumes: Option<(Instant, Vec<view::VolumeView>)> = None;
    // NVML lists the NVIDIA GPUs in every snapshot this thread takes anyway.
    let nvidia = |snapshot: &Snapshot| lock(&probes).nvidia_gpu = Some(!snapshot.gpus.is_empty());
    for request in requests {
        match request {
            Measure::Dashboard(reply) => {
                // Asked before the snapshot's one-second window, so the read usually lands in
                // this very dashboard. This thread never waits for the battery.
                {
                    let mut live = lock(&battery);
                    let now = Instant::now();
                    if live.due(now) && poke.try_send(()).is_ok() {
                        live.requested(now);
                    }
                }
                let snapshot = collector.snapshot(true, 8);
                nvidia(&snapshot);
                let mut dashboard = view::dashboard(&snapshot);
                dashboard.selection_source_id =
                    retain_capture(&selections, SelectionSource::Snapshot(snapshot));
                if volumes
                    .as_ref()
                    .is_none_or(|(at, _)| at.elapsed() > VOLUMES_MAX_AGE)
                {
                    let inventory = collector.hardware_inventory();
                    // One file-system name per shown volume (GetVolumeInformationW).
                    let list = view::volumes(&inventory, |root| {
                        pcagent_storage::volume_space(Path::new(root)).filesystem
                    });
                    if inventory.volumes.value.is_some() {
                        lock(&probes).ntfs_volumes = view::ntfs_volumes(&list);
                    }
                    volumes = Some((Instant::now(), list));
                }
                if let Some((_, list)) = &volumes {
                    dashboard.volumes = list.clone();
                }
                lock(&battery).show(&mut dashboard, Instant::now());
                let _ = reply.send(dashboard);
            }
            Measure::Activity(reply) => {
                // Two seconds between samples: long enough for rates, short enough to feel live.
                let activity = collector.process_activity(2, 50);
                // Readable names are read here, once per process and per executable, and only
                // for the view: the selection source keeps the activity as measured.
                let mut shown = view::activity(
                    &activity,
                    |reference| collector.instance(reference).cloned(),
                    &|pid, created| names.name(pid, created),
                );
                shown.selection_source_id =
                    retain_capture(&selections, SelectionSource::ProcessActivity(activity));
                let _ = reply.send(shown);
            }
            Measure::Preview(reply) => {
                let snapshot = collector.snapshot(true, 10);
                nvidia(&snapshot);
                let _ = retain_capture(&selections, SelectionSource::Snapshot(snapshot.clone()));
                let _ = reply.send(snapshot);
            }
        }
    }
}

async fn ask_measure<T: Send + 'static>(
    state: &AppState,
    request: impl FnOnce(mpsc::Sender<T>) -> Measure,
) -> Result<T, Failure> {
    let (tx, rx) = mpsc::channel();
    lock(&state.measure).send(request(tx)).map_err(|_| {
        Failure::new(
            Code::LiveUnavailable,
            "The measurement thread stopped; restart PC Agent.",
        )
    })?;
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| Failure::new(Code::LiveUnavailable, "Measurements took too long."))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> view::AppInfo {
    let probes = *lock(&state.probes);
    view::app_info(
        &Credentials::load(),
        view::capabilities(state.sampler.is_some(), probes),
    )
}

#[tauri::command]
async fn dashboard(state: State<'_, AppState>) -> Result<view::Dashboard, Failure> {
    ask_measure(&state, Measure::Dashboard).await
}

/// The proposals of a finished turn, as the person reviews them before deciding.
#[tauri::command]
fn action_plans(state: State<'_, AppState>, turn_id: String) -> Vec<view::PlanView> {
    actions::plans(&state.actions, &turn_id)
}

/// Prepare local journal-based recovery; the existing action_run command still requires a click.
#[tauri::command]
async fn startup_restore_prepare(
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
fn action_decline(
    state: State<'_, AppState>,
    turn_id: String,
    plan_id: String,
) -> Result<(), Failure> {
    actions::decline(&state.actions, &turn_id, &plan_id)
}

/// Runs one reviewed proposal. Only this window calls it, after the person's click on that plan;
/// the broker revalidates the target and refuses anything protected or changed.
#[tauri::command]
async fn action_run(
    state: State<'_, AppState>,
    turn_id: String,
    plan_id: String,
) -> Result<view::ActionOutcome, Failure> {
    let slot = state.actions.clone();
    tauri::async_runtime::spawn_blocking(move || actions::run(&slot, &turn_id, &plan_id))
        .await
        .map_err(|e| e.to_string())?
}

/// Application groups, their members and services over a short fresh window (local only).
#[tauri::command]
async fn process_activity(state: State<'_, AppState>) -> Result<view::ActivityView, Failure> {
    ask_measure(&state, Measure::Activity).await
}

#[derive(Serialize)]
struct Preview {
    id: String,
    message: Value,
    bytes: usize,
    measured: usize,
    findings: usize,
    /// Program names in the message; zero when the person chose anonymized sharing.
    names: usize,
    storage_folder: Option<String>,
    /// Exact finite names/relative paths authorized for subsequent storage tool calls.
    file_name_sharing: Value,
    model_routing: RoutingView,
    configured_model_id: Option<String>,
}

/// The person's choice in the interface: program names or opaque references (contract C5).
fn sharing_of(names: bool) -> Sharing {
    if names {
        Sharing::Complete
    } else {
        Sharing::Anonymized
    }
}

/// Freeze the first data message, including historical context, until its matching approval.
#[tauri::command]
async fn cloud_preview(
    state: State<'_, AppState>,
    question: String,
    names: bool,
    conversation_id: Option<String>,
    file_names: Option<bool>,
) -> Result<Preview, Failure> {
    let revision = state.preview_revision.fetch_add(1, Ordering::SeqCst) + 1;
    let question = checked_question(&question)?;
    let sharing = sharing_of(names);
    let file_names = file_names.unwrap_or(false);
    if file_names && sharing != Sharing::Complete {
        return Err("File names require complete sharing.".into());
    }
    let past = read_past(&state, conversation_id.as_deref())?;
    let snapshot = ask_measure(&state, Measure::Preview).await?;
    let (storage_folder, storage_scope) = {
        let slot = lock(&state.scan);
        let scanned = slot
            .scope_id
            .as_ref()
            .is_some_and(|scope| !slot.running && state.storage.index(scope).is_ok());
        (
            slot.root.clone().filter(|_| scanned),
            slot.scope_id.clone().filter(|_| scanned),
        )
    };
    let mut prepared = PreparedTurn::new(
        question,
        conversation_id,
        past,
        snapshot,
        sharing,
        storage_scope,
    );
    let routing = lock(&state.model_routing).review(configured_model_id());
    prepared.routing = Some(routing.clone());
    if let Some(secret) = Credentials::load().get("NEBIUS_API_KEY") {
        prepared.protect_secret(&secret);
    }
    if file_names {
        let scope = prepared.storage_scope.as_deref().ok_or_else(|| {
            Failure::new(
                Code::ScanUnavailable,
                "Scan and authorize a folder before previewing its names.",
            )
        })?;
        let index = state
            .storage
            .index(scope)
            .map_err(|e| failure::storage(&e, storage_folder.as_deref().unwrap_or_default()))?;
        prepared
            .storage_context
            .authorize_useful_names_with_privacy(&index, &prepared.privacy)
            .map_err(|e| e.to_string())?;
        prepared.file_names = true;
    }
    let file_name_sharing = prepared.storage_context.sharing_preview();
    let message = prepared.message.clone();
    let text = message.to_string();
    let measured = message["initial_observation"]["measurements"]["values"]
        .as_object()
        .map_or(0, |entities| {
            entities
                .values()
                .filter_map(Value::as_object)
                .map(|metrics| metrics.len())
                .sum()
        });
    let id = prepared.id.clone();
    // The collector wait above cannot retain a mutex guard. Re-read under the history
    // lock and keep it through publication, rejecting a deleted or changed conversation.
    let history = lock(&state.history);
    if read_past_from(&history, prepared.conversation_id.as_deref())? != prepared.past {
        return Err(Failure::new(
            Code::PreviewChanged,
            "Conversation history changed. Open the sharing preview again.",
        ));
    }
    let mut slot = lock(&state.preview);
    if revision != state.preview_revision.load(Ordering::SeqCst) {
        return Err(Failure::new(
            Code::PreviewChanged,
            "A newer sharing preview was requested.",
        ));
    }
    if !lock(&state.model_routing).matches(&routing, configured_model_id().as_deref()) {
        return Err(Failure::new(
            Code::PreviewChanged,
            "Model settings changed. Open the preview again.",
        ));
    }
    *slot = Some(prepared);
    Ok(Preview {
        id,
        bytes: text.len(),
        measured,
        findings: message["findings"].as_array().map_or(0, Vec::len),
        names: message["process_names"].as_object().map_or(0, |n| n.len()),
        storage_folder,
        file_name_sharing,
        message,
        model_routing: lock(&state.model_routing).view(),
        configured_model_id: routing.configured_model_id,
    })
}

fn read_past(state: &AppState, id: Option<&str>) -> Result<Vec<PastTurn>, Failure> {
    let history = lock(&state.history);
    read_past_from(&history, id)
}

fn read_past_from(history: &Option<Store>, id: Option<&str>) -> Result<Vec<PastTurn>, Failure> {
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

fn checked_question(question: &str) -> Result<String, Failure> {
    let question = question.trim();
    failure::check_question(question)?;
    Ok(question.to_owned())
}

#[tauri::command]
fn privacy_get(state: State<'_, AppState>) -> PrivacySettings {
    lock(&state.privacy).clone()
}

#[tauri::command]
fn privacy_set(
    state: State<'_, AppState>,
    settings: PrivacySettings,
) -> Result<PrivacySettings, Failure> {
    settings::save(&settings).map_err(failure::saving)?;
    // Every sharing toggle belongs to the frozen preview; changing one needs a new ticket.
    let invalidates_preview = settings.invalidates_preview(&lock(&state.privacy));
    *lock(&state.privacy) = settings.clone();
    if invalidates_preview {
        state.preview_revision.fetch_add(1, Ordering::SeqCst);
        state
            .selection_context_revision
            .fetch_add(1, Ordering::SeqCst);
        *lock(&state.preview) = None;
        lock(&state.selections).invalidate_tickets();
    }
    Ok(settings)
}

/// C3 entry point. The stored mode is a preference; a cloud request also needs the exact
/// single-use preview ticket reviewed for this question and conversation.
#[tauri::command]
fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_id: Option<String>,
    text: String,
    scope_ids: Vec<String>,
    preview_id: Option<String>,
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
        },
    )
}

#[derive(Clone, Serialize)]
struct StepEvent {
    id: u64,
    at_unix_ms: u64,
    step: Step,
}
#[derive(Clone, Serialize)]
struct ExtensionEvent {
    id: u64,
    request: ExtensionRequest,
    usage: view::Usage,
}
#[derive(Clone, Serialize)]
struct DoneEvent {
    id: u64,
    report: view::Report,
    /// The conversation the turn was saved in; `None` without a history file.
    conversation_id: Option<String>,
}
#[derive(Serialize)]
struct Started {
    id: u64,
    conversation_id: Option<String>,
    turn_id: String,
}
/// The question of one turn, what the model reads again, and where it may go.
struct Ask<'a> {
    turn_id: &'a str,
    question: &'a str,
    past: &'a [PastTurn],
    /// `None` for a local analysis; otherwise the person's choice of program names.
    cloud: Option<Sharing>,
    initial: Option<&'a Snapshot>,
    privacy: Option<&'a PrivacyContext>,
    storage_context: Option<&'a StorageContext>,
    scope_ids: Vec<String>,
    selected: Option<&'a FrozenSelection>,
    routing: Option<&'a RoutingReview>,
    elevated_evidence: Vec<pcagent_agent::elevated_tools::ApprovedInspection>,
    /// The person consented to web lookups (C6); a key is still required.
    web_search: bool,
    /// What this PC offered when the turn started: the model is offered only what works.
    probes: Probes,
}

enum ApprovedCapture {
    Snapshot(Box<PreparedTurn>),
    Selected(Box<PreparedSelection>),
}
impl ApprovedCapture {
    fn snapshot(&self) -> &Snapshot {
        match self {
            Self::Snapshot(prepared) => &prepared.snapshot,
            Self::Selected(prepared) => prepared.selection.snapshot(),
        }
    }
    fn privacy(&self) -> &PrivacyContext {
        match self {
            Self::Snapshot(prepared) => &prepared.privacy,
            Self::Selected(prepared) => &prepared.privacy,
        }
    }
    fn past(&self) -> &[PastTurn] {
        match self {
            Self::Snapshot(prepared) => &prepared.past,
            Self::Selected(prepared) => &prepared.past,
        }
    }
    fn context(&self) -> &StorageContext {
        match self {
            Self::Snapshot(prepared) => &prepared.storage_context,
            Self::Selected(prepared) => prepared.selection.storage_context(),
        }
    }
    fn scope(&self) -> Option<&str> {
        match self {
            Self::Snapshot(prepared) => prepared.storage_scope.as_deref(),
            Self::Selected(prepared) => match &prepared.reference.target {
                SelectionTarget::Storage { scope_id, .. } => Some(scope_id),
                _ => None,
            },
        }
    }
    fn selected(&self) -> Option<&FrozenSelection> {
        match self {
            Self::Snapshot(_) => None,
            Self::Selected(prepared) => Some(&prepared.selection),
        }
    }
    fn routing(&self) -> Option<&RoutingReview> {
        match self {
            Self::Snapshot(prepared) => prepared.routing.as_ref(),
            Self::Selected(prepared) => prepared.routing.as_ref(),
        }
    }
}
#[derive(Clone, Serialize)]
struct FailedEvent {
    id: u64,
    code: String,
    message: String,
}

fn preview_required() -> Failure {
    Failure::new(
        Code::PreviewRequired,
        "Open the sharing preview before sending.",
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StartRequest {
    question: String,
    cloud: bool,
    use_storage: bool,
    names: bool,
    conversation_id: Option<String>,
    preview_id: Option<String>,
    #[serde(default)]
    file_names: bool,
}

#[tauri::command]
fn start_investigation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartRequest,
) -> Result<Started, Failure> {
    launch_investigation(app, &state, request, None)
}

fn launch_investigation(
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
    } = request;
    let question = checked_question(&question)?;
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
        let mut preview = lock(&state.preview);
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
            let completed_turn = protocol::turn(&worker_turn_id, &question, asked_at, &value);
            // Every finished turn is kept, unless the conversation was deleted meanwhile.
            let conversation_id = match &saved_in {
                Some(conversation) => {
                    lock(&history)
                        .as_mut()
                        .and_then(|store| {
                            store
                                .add_turn_with_id(conversation, &worker_turn_id, &question, asked_at, &value)
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
    Ok(Started {
        id,
        conversation_id,
        turn_id,
    })
}

/// Runs one investigation, reporting each real step; returns the report, or why it could not
/// start (error code and message).
#[allow(clippy::too_many_arguments)]
fn investigation(
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
        let model = match build_routed_model(routing) {
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
                .and_then(|key| pcagent_agent::tavily::Tavily::new(&key))
            {
                workstation.enable_web_search(Box::new(tavily), pcagent_inspect::region_country());
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
            let report = converse_with_privacy(
                &mut model,
                ask.past,
                question,
                &initial,
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

fn build_routed_model(review: &RoutingReview) -> Result<RoutedModel<Nebius>, AgentError> {
    let credentials = Credentials::load();
    let config = Config::from_lookup(|name| credentials.get(name))?;
    if review.configured_model_id.as_deref() != Some(config.model_id()) {
        return Err(AgentError::InvalidModel);
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
fn ask_extension(
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

/// The running turn, or nothing: a window that reloaded reattaches it instead of leaving it
/// running unseen (and blocking every new question).
#[tauri::command]
fn current_turn(state: State<'_, AppState>) -> Option<CurrentTurn> {
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
fn answer_extension(
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
fn stop_investigation(state: State<'_, AppState>, id: u64) {
    if let Some(running) = lock(&state.investigation).as_ref().filter(|r| r.id == id) {
        running.stop.store(true, Ordering::SeqCst);
        if let Some(sender) = lock(&running.answer).take() {
            let _ = sender.send(false);
        }
    }
}

#[tauri::command]
fn stop_turn(state: State<'_, AppState>, turn_id: String) -> Result<(), String> {
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

#[derive(Serialize)]
struct QuickFolder {
    id: &'static str,
    path: String,
}

/// Common places, from the user's environment; only existing folders are offered.
#[tauri::command]
fn quick_folders() -> Vec<QuickFolder> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let profile = var("USERPROFILE");
    // SystemDrive is "C:"; the folder to scan is its root, "C:\".
    let drive = std::env::var_os("SystemDrive").map(|mut d| {
        d.push("\\");
        PathBuf::from(d)
    });
    [
        ("downloads", profile.as_ref().map(|p| p.join("Downloads"))),
        ("desktop", profile.as_ref().map(|p| p.join("Desktop"))),
        ("documents", profile.as_ref().map(|p| p.join("Documents"))),
        ("temp", var("TEMP")),
        ("profile", profile.clone()),
        ("system_drive", drive),
    ]
    .into_iter()
    .filter_map(|(id, path)| {
        let path = path?;
        path.is_dir().then(|| QuickFolder {
            id,
            path: path.display().to_string(),
        })
    })
    .collect()
}

#[tauri::command]
async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let texts = shell::shell_texts(pcagent_inspect::elevation::windows_ui_is_french());
        app.dialog()
            .file()
            .set_title(texts.folder_dialog)
            .blocking_pick_folder()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
struct ScanStarted {
    scope_id: String,
    root: String,
}
/// `code` translates `message` for the page (failure.rs); the message stays for diagnosis.
#[derive(Clone, Serialize)]
struct ScanFailed {
    scope_id: String,
    code: Code,
    message: String,
}

#[tauri::command]
fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<ScanStarted, Failure> {
    let root = PathBuf::from(path.trim());
    let shown = root.display().to_string();
    if !root.is_absolute() {
        return Err(Failure::new(
            Code::NotAFolder,
            "Choose a complete folder path, such as C:\\Users\\you\\Downloads.",
        ));
    }
    if !root.exists() {
        return Err(Failure::new(
            Code::FolderMissing,
            format!("{shown} does not exist."),
        ));
    }
    let mut slot = lock(&state.scan);
    if slot.running {
        return Err(Failure::new(
            Code::ScanRunning,
            "A scan is already running.",
        ));
    }
    let info = state
        .storage
        .authorize(&root)
        .map_err(|e| failure::storage(&e, &shown))?;
    // Only the latest folder stays authorized and indexed; its memory is freed otherwise.
    if let Some(old) = slot.scope_id.replace(info.scope_id.clone()) {
        state.storage.revoke(&old);
        lock(&state.selections).invalidate_storage(&old);
    }
    slot.root = Some(info.root.clone());
    slot.running = true;
    drop(slot);
    let (storage, scan, selections) = (
        state.storage.clone(),
        state.scan.clone(),
        state.selections.clone(),
    );
    let started = ScanStarted {
        scope_id: info.scope_id.clone(),
        root: info.root.clone(),
    };
    thread::Builder::new()
        .name("storage-scan".into())
        .spawn(move || {
            let scope = info.scope_id;
            let mut last = Instant::now();
            let result = storage
                .scan_blocking(&scope, &ScanOptions::default(), &mut |p| {
                    if last.elapsed() >= Duration::from_millis(150) {
                        last = Instant::now();
                        let _ = app.emit("scan:progress", view::progress(&scope, p));
                    }
                })
                .and_then(|summary| {
                    let overview = storage.get_storage_overview(&scope)?;
                    let dirs = storage.get_largest_items(&scope, ItemKind::Dirs, 0, PAGE)?;
                    let files = storage.get_largest_items(&scope, ItemKind::Files, 0, PAGE)?;
                    let dir_source =
                        retain_storage_page(&selections, &storage, &scan, dirs.clone());
                    let file_source =
                        retain_storage_page(&selections, &storage, &scan, files.clone());
                    let mut shown = view::storage(&info.root, &summary, &overview, &dirs, &files);
                    shown.dirs.selection_source_id = dir_source;
                    shown.files.selection_source_id = file_source;
                    Ok(shown)
                });
            lock(&scan).running = false;
            let _ = match result {
                Ok(done) => app.emit("scan:done", done),
                Err(e) => {
                    let failed = failure::storage(&e, &info.root);
                    app.emit(
                        "scan:failed",
                        ScanFailed {
                            scope_id: scope,
                            code: failed.code,
                            message: failed.message,
                        },
                    )
                }
            };
        })
        .map_err(|e| {
            lock(&state.scan).running = false;
            Failure::new(Code::ScanFailed, format!("Cannot start the scan: {e}"))
        })?;
    Ok(started)
}

#[tauri::command]
fn cancel_scan(state: State<'_, AppState>) {
    if let Some(scope) = lock(&state.scan).scope_id.clone() {
        let _ = state.storage.cancel(&scope);
    }
}

#[tauri::command]
fn scan_page(
    state: State<'_, AppState>,
    scope_id: String,
    kind: ItemKind,
    offset: usize,
) -> Result<view::Page, Failure> {
    let root = lock(&state.scan).root.clone().unwrap_or_default();
    state
        .storage
        .get_largest_items(&scope_id, kind, offset, PAGE.min(MAX_PAGE))
        .map(|p| {
            let selection_source_id =
                retain_storage_page(&state.selections, &state.storage, &state.scan, p.clone());
            let mut shown = view::page(&p, &root);
            shown.selection_source_id = selection_source_id;
            shown
        })
        .map_err(|e| failure::storage(&e, &root))
}

/// Opens Explorer on an item of the latest scan. The page names the item by reference only.
#[tauri::command]
fn reveal(state: State<'_, AppState>, scope_id: String, item_ref: String) -> Result<(), Failure> {
    let item = state
        .storage
        .index(&scope_id)
        .and_then(|index| index.item(&item_ref))
        .map_err(|_| {
            Failure::new(
                Code::ItemUnavailable,
                "This item is no longer in the latest scan.",
            )
        })?;
    let path = item
        .path
        .ok_or_else(|| Failure::new(Code::ItemUnavailable, "This item has no local path."))?;
    pcagent_actions::show_in_explorer(Path::new(&path))
        .map_err(|e| Failure::new(Code::OpenFailed, e.detail))
}

#[tauri::command]
fn save_cloud_settings(
    state: State<'_, AppState>,
    api_key: Option<String>,
    model_id: String,
    reasoning: Option<pcagent_agent::Reasoning>,
) -> Result<view::Cloud, Failure> {
    let model_id = model_id.trim().to_owned();
    // The same rule as the CLI: an explicit nvidia/ Nemotron ID, never a silent fallback.
    Config::from_lookup(|name| match name {
        "NEBIUS_API_KEY" => Some("checked-separately".into()),
        "NEBIUS_MODEL_ID" => Some(model_id.clone()),
        _ => None,
    })
    .map_err(|e| e.to_string())?;
    let key = api_key
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    let mut updates = vec![("NEBIUS_MODEL_ID", model_id.as_str())];
    if let Some(key) = &key {
        updates.push(("NEBIUS_API_KEY", key.as_str()));
    }
    // Absent keeps the saved choice; the composer sends it with the model it shows.
    if let Some(reasoning) = reasoning {
        updates.push(("NEBIUS_REASONING", reasoning.as_str()));
    }
    let file = credentials::file_path()
        .ok_or_else(|| failure::saving("The APPDATA folder is unknown."))?;
    credentials::save(&file, &updates).map_err(failure::saving)?;
    // The preview's protected credentials are frozen; changing them requires a fresh ticket.
    state.preview_revision.fetch_add(1, Ordering::SeqCst);
    state
        .selection_context_revision
        .fetch_add(1, Ordering::SeqCst);
    *lock(&state.preview) = None;
    lock(&state.selections).invalidate_tickets();
    Ok(view::cloud(&Credentials::load()))
}

/// Saves or removes the Tavily key for web lookups. The page sends it once and never reads it back.
#[tauri::command]
fn save_search_key(api_key: Option<String>) -> Result<view::Cloud, Failure> {
    let file = credentials::file_path()
        .ok_or_else(|| failure::saving("The APPDATA folder is unknown."))?;
    let key = api_key.map(|k| k.trim().to_owned()).unwrap_or_default();
    if key.is_empty() {
        credentials::remove(&file, &["TAVILY_API_KEY"]).map_err(failure::saving)?;
    } else if !key.starts_with("tvly-") {
        return Err(Failure::new(
            Code::SearchKeyInvalid,
            "A Tavily key starts with tvly-.",
        ));
    } else {
        credentials::save(&file, &[("TAVILY_API_KEY", key.as_str())]).map_err(failure::saving)?;
    }
    Ok(view::cloud(&Credentials::load()))
}

/// Opens a cited web source in the browser. The URL is read from the stored turn, never taken
/// from the page, and only an https page opens.
#[tauri::command]
fn open_web_source(
    state: State<'_, AppState>,
    conversation_id: String,
    turn_id: String,
    reference: String,
) -> Result<(), Failure> {
    let gone = |message: &str| Failure::new(Code::WebSourceUnavailable, message);
    let conversation = lock(&state.history)
        .as_ref()
        .ok_or_else(|| {
            Failure::new(
                Code::HistoryUnavailable,
                "The local history is unavailable.",
            )
        })?
        .get(&conversation_id)
        .map_err(failure::history)?
        .ok_or_else(|| gone("This conversation no longer exists."))?;
    let turn = conversation
        .turns
        .iter()
        .find(|turn| turn.id == turn_id)
        .ok_or_else(|| gone("This answer no longer exists."))?;
    let url = turn.report["answer"]["citations"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["reference"] == reference.as_str() && c["kind"] == "web")
        .and_then(|c| c["web"]["url"].as_str())
        .ok_or_else(|| gone("This answer has no such web source."))?;
    // Even a stored report that was altered can only open a site the lookups search.
    let host = pcagent_agent::tavily::domain(url).ok_or("Only https web pages can be opened.")?;
    if !pcagent_agent::external_tools::searchable(&host) {
        return Err("This address is not one of the sites web lookups search.".into());
    }
    pcagent_actions::open_web_page(url).map_err(|e| Failure::new(Code::OpenFailed, e.detail))
}

#[derive(Serialize)]
struct CloudCheck {
    price: Option<view::PriceView>,
}

/// A local setup click opens only the official console, never a caller-provided address.
#[tauri::command]
fn open_nebius_console() -> Result<(), String> {
    pcagent_actions::open_web_page("https://tokenfactory.nebius.com/")
        .map_err(|_| "The browser could not be opened.".to_owned())
}

/// Lists the saved account's catalog; the page receives metadata, never the local key.
#[tauri::command]
async fn cloud_models() -> Result<pcagent_agent::ModelCatalog, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let credentials = Credentials::load();
        Nebius::account_catalog(|name| credentials.get(name), Duration::from_secs(10))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Prepare a fixed task for local review without opening the Windows consent dialog.
#[tauri::command]
async fn elevation_prepare(
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
fn elevation_status(
    state: State<'_, AppState>,
) -> Result<Option<elevation::PreparedView>, Failure> {
    state.elevation.status()
}

/// The local approval click consumes one prepared request. No model tool invokes this command.
#[tauri::command]
async fn elevation_run(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<elevation::InspectionResult, Failure> {
    let run = state.elevation.begin(&request_id)?;
    let result = tauri::async_runtime::spawn_blocking(move || run.execute()).await;
    result.map_err(|e| e.to_string())?
}

#[tauri::command]
fn elevation_cancel(state: State<'_, AppState>, request_id: String) -> Result<(), Failure> {
    state.elevation.cancel(&request_id)
}

/// Explicit catalog check with the saved key; sends no telemetry.
#[tauri::command]
async fn test_cloud() -> Result<CloudCheck, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let creds = Credentials::load();
        let mut client = Config::from_lookup(|name| creds.get(name))
            .and_then(Nebius::new)
            .map_err(|e| e.to_string())?;
        client
            .check_catalog(Duration::from_secs(10))
            .map_err(|e| e.to_string())?;
        Ok(CloudCheck {
            price: client.catalog_pricing().map(view::price),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn run() {
    if pcagent_inspect::elevation::ensure_unelevated_host().is_err() {
        eprintln!("Start PC Agent from a standard, unelevated session.");
        pcagent_inspect::elevation::notify_elevated_host();
        std::process::exit(2);
    }
    let background = std::env::args_os().any(|arg| arg == "--background");
    let mut context = tauri::generate_context!();
    if background {
        for window in &mut context.config_mut().app.windows {
            window.visible = false;
        }
    }
    let (tx, rx) = mpsc::channel();
    let selections = Arc::new(Mutex::new(SelectionRegistry::default()));
    let measured_selections = selections.clone();
    // One pending request at most: a read still running absorbs further asks.
    let (battery_poke, battery_pokes) = mpsc::sync_channel(1);
    let battery = Arc::new(Mutex::new(view::LiveBattery::default()));
    let (battery_latest, battery_selections) = (battery.clone(), selections.clone());
    // One file check, no launch: the helper is only ever started from the app's own folder.
    let probes = Arc::new(Mutex::new(Probes {
        elevated_helper: pcagent_inspect::elevation::helper_installed(),
        ..Probes::default()
    }));
    let (battery_probes, measured_probes) = (probes.clone(), probes.clone());
    let program_names = Arc::new(ProgramNames::new());
    let measured_names = program_names.clone();
    thread::Builder::new()
        .name("battery".into())
        .spawn(move || {
            battery_thread(
                battery_pokes,
                battery_latest,
                battery_selections,
                battery_probes,
            )
        })
        .expect("the battery thread starts");
    thread::Builder::new()
        .name("measurements".into())
        .spawn(move || {
            measure_thread(
                rx,
                measured_selections,
                battery,
                battery_poke,
                measured_probes,
                measured_names,
            )
        })
        .expect("the measurement thread starts");
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            measure: Mutex::new(tx),
            storage: Arc::new(StorageService::new()),
            scan: Arc::new(Mutex::new(ScanSlot::default())),
            investigation: Mutex::new(None),
            next_id: AtomicU64::new(1),
            history: Arc::new(Mutex::new(open_history())),
            sampler: open_sampler(),
            privacy: Mutex::new(settings::load()),
            actions: Arc::new(Mutex::new(None)),
            elevation: Arc::new(elevation::Slot::default()),
            preview: Mutex::new(None),
            preview_revision: AtomicU64::new(0),
            selection_context_revision: AtomicU64::new(0),
            selections,
            model_routing: Mutex::new(ModelRouting::load()),
            probes,
            program_names,
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            current_turn,
            dashboard,
            process_activity,
            action_plans,
            startup_restore_prepare,
            action_decline,
            action_run,
            cloud_preview,
            selection_sources,
            selection_preview,
            selection_send,
            model_routing_get,
            model_routing_set,
            start_investigation,
            answer_extension,
            stop_investigation,
            quick_folders,
            pick_folder,
            start_scan,
            cancel_scan,
            scan_page,
            reveal,
            save_cloud_settings,
            save_search_key,
            open_web_source,
            test_cloud,
            cloud_models,
            open_nebius_console,
            elevation_prepare,
            elevation_status,
            elevation_run,
            elevation_cancel,
            conversations_list,
            conversation_get,
            conversation_delete,
            conversations_clear,
            history_status,
            metric_history,
            record_activity_start,
            battery_recording,
            record_activity_cancel,
            history_clear,
            startup_get,
            startup_set,
            privacy_get,
            privacy_set,
            send_message,
            stop_turn
        ])
        .setup(|app| {
            use tauri::{
                menu::{Menu, MenuItem},
                tray::{TrayIconBuilder, TrayIconEvent},
            };
            // Drawn by Windows, outside the page: in the language of Windows itself.
            let texts = shell::shell_texts(pcagent_inspect::elevation::windows_ui_is_french());
            let open = MenuItem::with_id(app, "open", texts.tray_open, true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", texts.tray_quit, true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("pcagent")
                .tooltip(texts.tray_tooltip)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        shutdown(app);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(event, TrayIconEvent::DoubleClick { .. }) {
                        if let Some(window) = tray.app_handle().get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("PC Agent {}", env!("CARGO_PKG_VERSION")));
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if pcagent_inspect::autostart::get().unwrap_or(false) {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    // Tauri exits the process without Rust destructors. Wait for the helper first.
                    api.prevent_close();
                    shutdown(window.app_handle());
                }
            }
        })
        .run(context)
        .expect("PC Agent's window could not start (is Microsoft Edge WebView2 installed?)");
}

fn shutdown(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Some(shutdown) = state.elevation.begin_shutdown() else {
        return;
    };
    if let Some(running) = lock(&state.investigation).as_ref() {
        running.stop.store(true, Ordering::SeqCst);
        if let Some(answer) = lock(&running.answer).take() {
            let _ = answer.send(false);
        }
    }
    if let Some(sampler) = &state.sampler {
        sampler.stop();
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if shutdown.wait(Duration::from_secs(5)) {
            app.exit(0);
        } else {
            eprintln!("PC Agent is waiting for its elevated inspection to stop; try Quit again.");
        }
    });
}
