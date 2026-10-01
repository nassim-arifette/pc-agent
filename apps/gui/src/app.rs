//! The Tauri shell: commands the page may call and the events it listens to. The page is
//! untrusted input: every argument is validated here, paths and item references are resolved on
//! this side, and the Nebius key never reaches the page.
use crate::{
    preview::{self, PreparedTurn, Reviewed},
    session::{ObservedModel, ObservedTools, Step},
    view,
};
use pcagent_agent::{
    converse,
    credentials::{self, Credentials},
    local_report, AgentError, Config, ExtensionRequest, Investigation, Nebius, PastTurn, Sharing,
    Workstation, MAX_QUESTION_BYTES,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{unix_ms, Snapshot};
use pcagent_storage::{ItemKind, ScanOptions, StorageError, StorageService, MAX_PAGE};
use pcagent_store::{Conversation, ConversationSummary, Store, DEFAULT_RETENTION_DAYS};
use pcagent_telemetry::Collector;
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
    Preview(mpsc::Sender<Snapshot>),
}

struct Running {
    id: u64,
    stop: Arc<AtomicBool>,
    answer: Arc<Mutex<Option<mpsc::Sender<bool>>>>,
    finished: Arc<AtomicBool>,
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
    preview: Mutex<Option<PreparedTurn>>,
    preview_revision: AtomicU64,
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
fn conversation_get(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<Conversation>, String> {
    match lock(&state.history).as_ref() {
        Some(store) => store.get(&id).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[tauri::command]
fn conversation_delete(state: State<'_, AppState>, id: String) -> Result<bool, String> {
    match lock(&state.history).as_ref() {
        Some(store) => store.delete(&id).map_err(|e| e.to_string()),
        None => Ok(false),
    }
}

#[tauri::command]
fn conversations_clear(state: State<'_, AppState>) -> Result<usize, String> {
    match lock(&state.history).as_ref() {
        Some(store) => store.clear().map_err(|e| e.to_string()),
        None => Ok(0),
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Free space changes slowly; the inventory is read again at most this often.
const VOLUMES_MAX_AGE: Duration = Duration::from_secs(30);

fn measure_thread(requests: mpsc::Receiver<Measure>) {
    let mut collector = Collector::new();
    let mut volumes: Option<(Instant, Vec<view::VolumeView>)> = None;
    for request in requests {
        match request {
            Measure::Dashboard(reply) => {
                let mut dashboard = view::dashboard(&collector.snapshot(true, 8));
                if volumes
                    .as_ref()
                    .is_none_or(|(at, _)| at.elapsed() > VOLUMES_MAX_AGE)
                {
                    volumes = Some((
                        Instant::now(),
                        view::volumes(&collector.hardware_inventory()),
                    ));
                }
                if let Some((_, list)) = &volumes {
                    dashboard.volumes = list.clone();
                }
                let _ = reply.send(dashboard);
            }
            Measure::Preview(reply) => {
                let snapshot = collector.snapshot(true, 10);
                let _ = reply.send(snapshot);
            }
        }
    }
}

async fn ask_measure<T: Send + 'static>(
    state: &AppState,
    request: impl FnOnce(mpsc::Sender<T>) -> Measure,
) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    lock(&state.measure)
        .send(request(tx))
        .map_err(|_| "The measurement thread stopped; restart PC Agent.".to_owned())?;
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(Duration::from_secs(30))
            .map_err(|_| "Measurements took too long.".to_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn app_info() -> view::AppInfo {
    view::app_info(&Credentials::load())
}

#[tauri::command]
async fn dashboard(state: State<'_, AppState>) -> Result<view::Dashboard, String> {
    ask_measure(&state, Measure::Dashboard).await
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
) -> Result<Preview, String> {
    let revision = state.preview_revision.fetch_add(1, Ordering::SeqCst) + 1;
    let question = checked_question(&question)?;
    let sharing = sharing_of(names);
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
    let prepared = PreparedTurn::new(
        question,
        conversation_id,
        past,
        snapshot,
        sharing,
        storage_scope,
    );
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
    let mut slot = lock(&state.preview);
    if revision != state.preview_revision.load(Ordering::SeqCst) {
        return Err("A newer sharing preview was requested.".into());
    }
    *slot = Some(prepared);
    Ok(Preview {
        id,
        bytes: text.len(),
        measured,
        findings: message["findings"].as_array().map_or(0, Vec::len),
        names: message["process_names"].as_object().map_or(0, |n| n.len()),
        storage_folder,
        message,
    })
}

fn read_past(state: &AppState, id: Option<&str>) -> Result<Vec<PastTurn>, String> {
    let Some(id) = id else { return Ok(Vec::new()) };
    let history = lock(&state.history);
    let conversation = history
        .as_ref()
        .ok_or("Conversation history is unavailable.")?
        .get(id)
        .map_err(|e| e.to_string())?
        .ok_or("This conversation no longer exists.")?;
    Ok(past_turns(&conversation))
}

fn checked_question(question: &str) -> Result<String, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("Type a question first.".into());
    }
    if question.len() > MAX_QUESTION_BYTES {
        return Err(format!(
            "The question is longer than {MAX_QUESTION_BYTES} bytes; shorten it."
        ));
    }
    Ok(question.to_owned())
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
}
/// The question of one turn, what the model reads again, and where it may go.
struct Ask<'a> {
    question: &'a str,
    past: &'a [PastTurn],
    /// `None` for a local analysis; otherwise the person's choice of program names.
    cloud: Option<Sharing>,
    initial: Option<&'a Snapshot>,
}
#[derive(Clone, Serialize)]
struct FailedEvent {
    id: u64,
    code: String,
    message: String,
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
}

#[tauri::command]
fn start_investigation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartRequest,
) -> Result<Started, String> {
    let StartRequest {
        question,
        cloud,
        use_storage,
        names,
        conversation_id,
        preview_id,
    } = request;
    let question = checked_question(&question)?;
    let mut slot = lock(&state.investigation);
    if slot
        .as_ref()
        .is_some_and(|r| !r.finished.load(Ordering::SeqCst))
    {
        return Err("An investigation is already running.".into());
    }
    let prior = read_past(&state, conversation_id.as_deref())?;
    let prepared = if cloud {
        let scan = lock(&state.scan);
        let scope = if use_storage {
            Some(
                scan.scope_id
                    .as_deref()
                    .filter(|s| !scan.running && state.storage.index(s).is_ok())
                    .ok_or("The approved scan is no longer available. Open the preview again.")?,
            )
        } else {
            None
        };
        Some(preview::take(
            &mut lock(&state.preview),
            Reviewed {
                id: preview_id
                    .as_deref()
                    .ok_or("Open the sharing preview before sending.")?,
                question: &question,
                conversation_id: conversation_id.as_deref(),
                past: &prior,
                sharing: sharing_of(names),
                storage_scope: scope,
            },
        )?)
    } else {
        None
    };
    // A follow-up reads its conversation again; a first question starts one.
    let (conversation_id, past, created) = {
        let history = lock(&state.history);
        match history.as_ref() {
            None => (None, Vec::new(), false),
            Some(store) => {
                let existing = match conversation_id {
                    Some(id) => store.get(&id).map_err(|e| e.to_string())?,
                    None => None,
                };
                match existing {
                    Some(c) => (Some(c.summary.id.clone()), past_turns(&c), false),
                    None => (store.create(&question, unix_ms()).ok(), Vec::new(), true),
                }
            }
        }
    };
    let past = prepared.as_ref().map_or(past, |p| p.past.clone());
    let id = state.next_id.fetch_add(1, Ordering::SeqCst);
    let running = Running {
        id,
        stop: Arc::new(AtomicBool::new(false)),
        answer: Arc::new(Mutex::new(None)),
        finished: Arc::new(AtomicBool::new(false)),
    };
    let (stop, answer, finished) = (
        running.stop.clone(),
        running.answer.clone(),
        running.finished.clone(),
    );
    // Without permission, the model gets an empty storage service: no folder is visible.
    let storage = if use_storage {
        state.storage.clone()
    } else {
        Arc::new(StorageService::new())
    };
    let history = state.history.clone();
    let saved_in = conversation_id.clone();
    thread::Builder::new()
        .name("investigation".into())
        .spawn(move || {
            let asked_at = unix_ms();
            // Local analysis, or the model with the person's choice of program names.
            let ask = Ask {
                question: &question,
                past: &past,
                cloud: cloud.then(|| sharing_of(names)),
                initial: prepared.as_ref().map(|p| &p.snapshot),
            };
            let outcome = investigation(&app, id, &ask, storage, &stop, &answer);
            // Every finished turn is kept, unless the conversation was deleted meanwhile.
            let conversation_id = match (&outcome, &saved_in) {
                (Ok(report), Some(conversation)) => {
                    let value = serde_json::to_value(report).unwrap_or_default();
                    lock(&history)
                        .as_mut()
                        .and_then(|store| {
                            store
                                .add_turn(conversation, &question, asked_at, &value)
                                .ok()
                                .flatten()
                        })
                        .map(|_| conversation.clone())
                }
                // A conversation whose first question could not even start keeps nothing.
                (Err(_), Some(conversation)) if created => {
                    if let Some(store) = lock(&history).as_ref() {
                        let _ = store.delete(conversation);
                    }
                    None
                }
                _ => None,
            };
            // Finished before the last event, so the next question can start right away.
            finished.store(true, Ordering::SeqCst);
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
    })
}

/// Runs one investigation, reporting each real step; returns the report, or why it could not
/// start (error code and message).
fn investigation(
    app: &AppHandle,
    id: u64,
    ask: &Ask<'_>,
    storage: Arc<StorageService>,
    stop: &Arc<AtomicBool>,
    answer: &Arc<Mutex<Option<mpsc::Sender<bool>>>>,
) -> Result<view::Report, (String, String)> {
    let emit = |step: Step| {
        let _ = app.emit(
            "inv:step",
            StepEvent {
                id,
                at_unix_ms: unix_ms(),
                step,
            },
        );
    };
    let mut collector = Collector::new();
    emit(Step::SnapshotStarted);
    let started = Instant::now();
    let initial = ask
        .initial
        .cloned()
        .unwrap_or_else(|| collector.snapshot(true, 10));
    emit(Step::SnapshotFinished {
        duration_ms: started.elapsed().as_millis() as u64,
        measured: initial
            .evidence
            .iter()
            .filter(|e| e.usable_value().is_some())
            .count(),
    });
    let question = ask.question;
    let report: Investigation = if let Some(sharing) = ask.cloud {
        let creds = Credentials::load();
        let model = match Config::from_lookup(|name| creds.get(name)).and_then(Nebius::new) {
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
        let (policy, prefs) = (Policy::from_env(), Preferences::default());
        let mut workstation = Workstation::new(&mut collector, storage, &policy, &prefs);
        workstation.set_sharing(sharing);
        let mut tools = ObservedTools::new(workstation, |tool: &str| {
            emit(Step::ToolStarted { tool: tool.into() })
        });
        let report = converse(
            &mut model,
            ask.past,
            question,
            &initial,
            &mut tools,
            |event| {
                emit(Step::ToolFinished {
                    tool: event.tool.clone(),
                    status: event.status,
                    duration_ms: event.duration_ms,
                })
            },
            |partial, request| ask_extension(app, id, partial, request, stop, answer),
        );
        let mut workstation = tools.into_inner();
        let jobs = workstation.take_jobs();
        for job in &jobs {
            job.cancel();
        }
        for job in jobs {
            let _ = job.wait();
        }
        report
    } else {
        local_report(&initial)
    };
    Ok(view::report(&report, question))
}

/// Blocks the investigation until the person answers in the window, stops, or closes it.
fn ask_extension(
    app: &AppHandle,
    id: u64,
    partial: &Investigation,
    request: &ExtensionRequest,
    stop: &AtomicBool,
    answer: &Mutex<Option<mpsc::Sender<bool>>>,
) -> bool {
    if stop.load(Ordering::SeqCst) {
        return false;
    }
    let (tx, rx) = mpsc::channel();
    *lock(answer) = Some(tx);
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
    accepted
}

#[tauri::command]
fn answer_extension(state: State<'_, AppState>, id: u64, accept: bool) -> Result<(), String> {
    let slot = lock(&state.investigation);
    let running = slot
        .as_ref()
        .filter(|r| r.id == id)
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
        app.dialog()
            .file()
            .set_title("Choose a folder to scan")
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
#[derive(Clone, Serialize)]
struct ScanFailed {
    scope_id: String,
    message: String,
}

fn storage_error(e: StorageError, shown: &str) -> String {
    match e {
        StorageError::NotADirectory => format!("{shown} is not a folder."),
        StorageError::ReparseRoot => {
            format!("{shown} is a link to another place. Choose the folder it points to instead.")
        }
        StorageError::UnsupportedVolume(what) => {
            format!("{shown} cannot be scanned: {what}.")
        }
        other => format!("Cannot scan {shown}: {other}."),
    }
}

#[tauri::command]
fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<ScanStarted, String> {
    let root = PathBuf::from(path.trim());
    let shown = root.display().to_string();
    if !root.is_absolute() {
        return Err("Choose a complete folder path, such as C:\\Users\\you\\Downloads.".into());
    }
    if !root.exists() {
        return Err(format!("{shown} does not exist."));
    }
    let mut slot = lock(&state.scan);
    if slot.running {
        return Err("A scan is already running.".into());
    }
    let info = state
        .storage
        .authorize(&root)
        .map_err(|e| storage_error(e, &shown))?;
    // Only the latest folder stays authorized and indexed; its memory is freed otherwise.
    if let Some(old) = slot.scope_id.replace(info.scope_id.clone()) {
        state.storage.revoke(&old);
    }
    slot.root = Some(info.root.clone());
    slot.running = true;
    drop(slot);
    let (storage, scan) = (state.storage.clone(), state.scan.clone());
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
                    Ok(view::storage(
                        &info.root, &summary, &overview, &dirs, &files,
                    ))
                });
            lock(&scan).running = false;
            let _ = match result {
                Ok(done) => app.emit("scan:done", done),
                Err(e) => app.emit(
                    "scan:failed",
                    ScanFailed {
                        scope_id: scope,
                        message: storage_error(e, &info.root),
                    },
                ),
            };
        })
        .map_err(|e| {
            lock(&state.scan).running = false;
            format!("Cannot start the scan: {e}")
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
) -> Result<view::Page, String> {
    let root = lock(&state.scan).root.clone().unwrap_or_default();
    state
        .storage
        .get_largest_items(&scope_id, kind, offset, PAGE.min(MAX_PAGE))
        .map(|p| view::page(&p, &root))
        .map_err(|e| storage_error(e, &root))
}

/// Opens Explorer on an item of the latest scan. The page names the item by reference only.
#[tauri::command]
fn reveal(state: State<'_, AppState>, scope_id: String, item_ref: String) -> Result<(), String> {
    let item = state
        .storage
        .index(&scope_id)
        .and_then(|index| index.item(&item_ref))
        .map_err(|_| "This item is no longer in the latest scan.".to_owned())?;
    let path = item.path.ok_or("This item has no local path.")?;
    pcagent_actions::show_in_explorer(Path::new(&path)).map_err(|e| e.detail)
}

#[tauri::command]
fn save_cloud_settings(api_key: Option<String>, model_id: String) -> Result<view::Cloud, String> {
    let model_id = model_id.trim().to_owned();
    // The same rule as the CLI: an explicit nvidia/ Nemotron ID, never a silent fallback.
    Config::from_lookup(|name| match name {
        "NEBIUS_API_KEY" => Some("checked-separately".into()),
        _ => Some(model_id.clone()),
    })
    .map_err(|e| e.to_string())?;
    let key = api_key
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty());
    let mut updates = vec![("NEBIUS_MODEL_ID", model_id.as_str())];
    if let Some(key) = &key {
        updates.push(("NEBIUS_API_KEY", key.as_str()));
    }
    let file = credentials::file_path().ok_or("The APPDATA folder is unknown.")?;
    credentials::save(&file, &updates)?;
    Ok(view::cloud(&Credentials::load()))
}

#[derive(Serialize)]
struct CloudCheck {
    price: Option<view::PriceView>,
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
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("measurements".into())
        .spawn(move || measure_thread(rx))
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
            preview: Mutex::new(None),
            preview_revision: AtomicU64::new(0),
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            dashboard,
            cloud_preview,
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
            test_cloud,
            conversations_list,
            conversation_get,
            conversation_delete,
            conversations_clear
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title(&format!("PC Agent {}", env!("CARGO_PKG_VERSION")));
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("PC Agent's window could not start (is Microsoft Edge WebView2 installed?)");
}
