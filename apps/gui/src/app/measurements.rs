//! Collector-owning background threads and live measurement requests.
use super::selection_commands::retain_capture;
use super::state::Measure;
use super::{lock, AppState};
use crate::failure::{Code, Failure};
use crate::selection::SelectionRegistry;
use crate::view;
use pcagent_agent::capabilities::Probes;
use pcagent_agent::selection::SelectionSource;
use pcagent_shared::Snapshot;
use pcagent_telemetry::local_browser::{self, BrowserWindowTarget, LocalBrowserWindows};
use pcagent_telemetry::{Collector, ProgramNames};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::State;

/// Free space changes slowly; the inventory is read again at most this often.
pub(super) const VOLUMES_MAX_AGE: Duration = Duration::from_secs(30);

/// Reads the battery on its own thread, off the measurement thread, each time it is asked.
/// The read runs on `pcagent_inspect::live_battery`'s bounded worker, never on the shared
/// inspection worker used by agent tools and recordings.
pub(super) fn battery_thread(
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

pub(super) fn measure_thread(
    requests: mpsc::Receiver<Measure>,
    selections: Arc<Mutex<SelectionRegistry>>,
    battery: Arc<Mutex<view::LiveBattery>>,
    poke: mpsc::SyncSender<()>,
    probes: Arc<Mutex<Probes>>,
    names: Arc<ProgramNames>,
) {
    let mut collector = Collector::new();
    let mut volumes: Option<(Instant, Vec<view::VolumeView>)> = None;
    let mut browser_targets: HashMap<String, Vec<BrowserWindowTarget>> = HashMap::new();
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
                let activity = collector.process_activity(2, usize::MAX);
                browser_targets = activity
                    .applications
                    .iter()
                    .filter_map(|app| {
                        let targets: Vec<_> = activity
                            .processes
                            .iter()
                            .filter(|p| p.application_ref == app.entity_ref)
                            .filter_map(|p| {
                                p.browser.family.map(|family| BrowserWindowTarget {
                                    instance: p.instance.clone(),
                                    family,
                                })
                            })
                            .collect();
                        (!targets.is_empty()).then(|| {
                            (
                                view::application_key(app, &|reference| {
                                    collector.instance(reference).cloned()
                                }),
                                targets,
                            )
                        })
                    })
                    .collect();
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
            Measure::BrowserTargets {
                application_ref,
                reply,
            } => {
                let _ = reply.send(
                    browser_targets
                        .get(&application_ref)
                        .cloned()
                        .unwrap_or_default(),
                );
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

pub(super) async fn ask_measure<T: Send + 'static>(
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
pub(super) async fn dashboard(state: State<'_, AppState>) -> Result<view::Dashboard, Failure> {
    ask_measure(&state, Measure::Dashboard).await
}

/// Application groups, their members and services over a short fresh window (local only).
#[tauri::command]
pub(super) async fn process_activity(
    state: State<'_, AppState>,
) -> Result<view::ActivityView, Failure> {
    ask_measure(&state, Measure::Activity).await
}

/// Local-only endpoint: titles never enter the shared collector, selections, recordings or turns.
#[tauri::command]
pub(super) async fn local_browser_windows(
    state: State<'_, AppState>,
    application_ref: String,
    include_tabs: bool,
) -> Result<LocalBrowserWindows, Failure> {
    let targets = ask_measure(&state, |reply| Measure::BrowserTargets {
        application_ref,
        reply,
    })
    .await?;
    if targets.is_empty() {
        return Ok(LocalBrowserWindows::unavailable(
            pcagent_shared::Status::Stale,
        ));
    }
    tauri::async_runtime::spawn_blocking(move || local_browser::read(&targets, include_tabs))
        .await
        .map_err(|_| Failure::new(Code::LiveUnavailable, "Local browser inspection stopped."))
}
