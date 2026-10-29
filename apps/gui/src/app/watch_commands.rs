//! Explicit local C7 commands. Alert creation never starts a model turn.
use super::history_commands::history_unavailable;
use super::{lock, AppState};
use crate::failure::{self, Code, Failure};
use pcagent_actions::notifications::{Delivery, Event, Notification, Service};
use pcagent_shared::unix_ms;
use pcagent_store::{
    sampler::WatchHooks,
    watch_engine::WatchEngineStatus,
    watches::{Watch, WatchAlert, WatchAlertPage, WatchNotification, WatchRule, WatchSummary},
    Store, StoreError,
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};

type History = Arc<Mutex<Option<Store>>>;

#[derive(Clone)]
pub(super) struct Runtime {
    pub(super) notifications: Service,
    pub(super) activation: Arc<Mutex<Option<String>>>,
    failed: Arc<Mutex<VecDeque<String>>>,
    last_test: Arc<Mutex<Option<Instant>>>,
}

impl Runtime {
    pub(super) fn start(history: History, handle: Arc<OnceLock<AppHandle>>) -> Self {
        let activation = Arc::new(Mutex::new(None));
        let failed = Arc::new(Mutex::new(VecDeque::new()));
        let (pending, failures) = (activation.clone(), failed.clone());
        let notifications = Service::start(Arc::new(move |event| match event {
            Event::Activated(id) => {
                *lock(&pending) = Some(id.clone());
                if let Some(app) = handle.get() {
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.unminimize();
                        let _ = window.set_focus();
                    }
                    let _ = app.emit("watch:open", id);
                }
            }
            Event::Failed(id) => {
                let mut failed = lock(&failures);
                failed.push_back(id.clone());
                while failed.len() > 128 {
                    failed.pop_front();
                }
                if let Some(store) = lock(&history).as_ref() {
                    let _ = store.watch_notification_result(&id, WatchNotification::Failed);
                }
                if let Some(app) = handle.get() {
                    let _ = app.emit("watch:changed", ());
                }
            }
        }));
        Self {
            notifications,
            activation,
            failed,
            last_test: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn hooks(&self, history: History, handle: Arc<OnceLock<AppHandle>>) -> WatchHooks {
        let notifications = self.notifications.clone();
        let failures = self.failed.clone();
        WatchHooks {
            notify: Arc::new(move |alert, language| {
                let (title, body) = notification_text(alert, language);
                match notifications.show(Notification {
                    alert_id: alert.id.clone(),
                    watch_id: alert.watch_id.clone(),
                    title,
                    body,
                }) {
                    Delivery::Sent => WatchNotification::Sent,
                    Delivery::Disabled => WatchNotification::Disabled,
                    _ => WatchNotification::Failed,
                }
            }),
            opened: Arc::new(move |alert, unseen| {
                // Failed may race Show's successful return. After the sampler saves the result,
                // reconcile an earlier asynchronous Windows failure under the callback's lock.
                let mut failed = lock(&failures);
                if let Some(index) = failed.iter().position(|id| id == &alert.id) {
                    failed.remove(index);
                    if let Some(store) = lock(&history).as_ref() {
                        let _ =
                            store.watch_notification_result(&alert.id, WatchNotification::Failed);
                    }
                }
                if let Some(app) = handle.get() {
                    let _ = app.emit(
                        "watch:alert",
                        serde_json::json!({
                            "alert": alert, "unseen_alerts": unseen,
                        }),
                    );
                    let _ = app.emit("watch:changed", unseen);
                }
            }),
        }
    }
}

fn notification_text(alert: &WatchAlert, language: &str) -> (String, String) {
    let measure = match (&alert.rule, language == "fr") {
        (WatchRule::VolumeFreeBelow { .. }, true) => "Espace libre faible",
        (WatchRule::VolumeFreeBelow { .. }, false) => "Low free space",
        (WatchRule::GpuTemperatureAbove { .. }, true) => "Température GPU élevée",
        (WatchRule::GpuTemperatureAbove { .. }, false) => "High GPU temperature",
        (WatchRule::AppCpuAbove { .. } | WatchRule::AppMemoryAbove { .. }, true) => {
            "Activité d’une application"
        }
        (WatchRule::AppCpuAbove { .. } | WatchRule::AppMemoryAbove { .. }, false) => {
            "Application activity"
        }
        (_, true) => "Ressource au-dessus du seuil",
        (_, false) => "Resource above threshold",
    };
    let body = if language == "fr" {
        "Ouvrez Surveillances pour voir les mesures. Aucune donnée envoyée au cloud."
    } else {
        "Open Watches to inspect the measurements. No data was sent to the cloud."
    };
    (format!("PC Agent — {measure}"), body.into())
}

/// C7: while at least one rule is enabled, closing the window hides it in the notification area
/// so the sampler keeps watching; "Quit PC Agent" still stops everything.
pub(super) fn any_enabled(state: &AppState) -> bool {
    lock(&state.history)
        .as_ref()
        .and_then(|store| store.watches().ok())
        .is_some_and(|rules| rules.iter().any(|rule| rule.enabled))
}

fn read<T>(
    history: &History,
    f: impl FnOnce(&Store) -> Result<T, StoreError>,
) -> Result<T, Failure> {
    f(lock(history).as_ref().ok_or_else(history_unavailable)?).map_err(failure::history)
}

fn checked_id(id: &str) -> Result<(), Failure> {
    if id.is_empty() || id.len() > 64 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        Err(Failure::new(
            Code::Unknown,
            "Invalid watch or alert identifier.",
        ))
    } else {
        Ok(())
    }
}

#[derive(Serialize)]
pub(super) struct Watches {
    rules: Vec<Watch>,
    engine: Option<WatchEngineStatus>,
    notification_status: Delivery,
    language: String,
    unseen_alerts: u64,
}

#[tauri::command]
pub(super) async fn watches_list(state: State<'_, AppState>) -> Result<Watches, Failure> {
    let (history, sampler, notifications) = (
        state.history.clone(),
        state.sampler.clone(),
        state.watches.notifications.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        let (rules, language, unseen_alerts) = read(&history, |s| {
            Ok((s.watches()?, s.watch_language()?, s.watch_unseen()?))
        })?;
        Ok(Watches {
            rules,
            language,
            unseen_alerts,
            engine: sampler.as_ref().and_then(|s| s.status().watches),
            notification_status: notifications.status(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_create(
    state: State<'_, AppState>,
    rule: WatchRule,
) -> Result<Watch, Failure> {
    let (history, sampler) = (
        state.history.clone(),
        state.sampler.clone().ok_or_else(history_unavailable)?,
    );
    tauri::async_runtime::spawn_blocking(move || {
        sampler
            .validate_watch_target(rule.clone())
            .map_err(failure::history)?;
        let watch = read(&history, |s| s.watch_create(rule, unix_ms()))?;
        // Persistence succeeded even if the evaluator stopped in the meantime. The list exposes
        // its actual state; a new owner also reloads the durable rules at its next lease renewal.
        let _ = sampler.watches_changed();
        Ok(watch)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_update(
    state: State<'_, AppState>,
    id: String,
    rule: WatchRule,
) -> Result<(), Failure> {
    checked_id(&id)?;
    let (history, sampler) = (
        state.history.clone(),
        state.sampler.clone().ok_or_else(history_unavailable)?,
    );
    tauri::async_runtime::spawn_blocking(move || {
        sampler
            .validate_watch_target(rule.clone())
            .map_err(failure::history)?;
        read(&history, |s| s.watch_update(&id, rule, unix_ms()))?;
        let _ = sampler.watches_changed();
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_set_enabled(
    state: State<'_, AppState>,
    id: String,
    enabled: bool,
) -> Result<(), Failure> {
    checked_id(&id)?;
    let (history, sampler) = (state.history.clone(), state.sampler.clone());
    tauri::async_runtime::spawn_blocking(move || {
        read(&history, |s| s.watch_set_enabled(&id, enabled, unix_ms()))?;
        if let Some(sampler) = sampler {
            let _ = sampler.watches_changed();
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_delete(
    state: State<'_, AppState>,
    id: String,
) -> Result<Delivery, Failure> {
    checked_id(&id)?;
    let selections = state.selections.clone();
    let (history, sampler, notifications) = (
        state.history.clone(),
        state.sampler.clone(),
        state.watches.notifications.clone(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        {
            let mut selections = lock(&selections);
            read(&history, |s| s.watch_delete(&id))?;
            selections.invalidate_watch(&id);
        }
        if let Some(sampler) = sampler {
            let _ = sampler.watches_changed();
        }
        Ok(notifications.remove_watch(&id))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub(super) struct AlertDetail {
    alert: WatchAlert,
    source_id: String,
}

#[tauri::command]
pub(super) async fn watch_alert_detail(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<AlertDetail>, Failure> {
    checked_id(&id)?;
    let (history, selections) = (state.history.clone(), state.selections.clone());
    let revision = lock(&selections).history_revision();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(alert) = read(&history, |s| s.watch_alert(&id))? else {
            return Ok(None);
        };
        let source_id = lock(&selections).publish_history(
            pcagent_agent::selection::SelectionSource::WatchAlert(alert.clone()),
            revision,
        )?;
        Ok(Some(AlertDetail { alert, source_id }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_alerts(
    state: State<'_, AppState>,
    from_unix_ms: u64,
    to_unix_ms: u64,
    limit: u64,
    offset: u64,
) -> Result<WatchAlertPage, Failure> {
    let history = state.history.clone();
    tauri::async_runtime::spawn_blocking(move || {
        read(&history, |s| {
            s.watch_alerts(from_unix_ms, to_unix_ms, limit, offset)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_alerts_seen(
    state: State<'_, AppState>,
    up_to_unix_ms: u64,
) -> Result<(), Failure> {
    let history = state.history.clone();
    tauri::async_runtime::spawn_blocking(move || {
        read(&history, |s| s.watch_alerts_seen(up_to_unix_ms))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_language_set(
    state: State<'_, AppState>,
    language: String,
) -> Result<(), Failure> {
    let history = state.history.clone();
    tauri::async_runtime::spawn_blocking(move || {
        read(&history, |s| s.watch_language_set(&language))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) async fn watch_notification_test(
    state: State<'_, AppState>,
) -> Result<Delivery, Failure> {
    let language = read(&state.history, Store::watch_language)?;
    {
        let mut last = lock(&state.watches.last_test);
        if last.is_some_and(|last| last.elapsed() < Duration::from_secs(10)) {
            return Err(Failure::new(
                Code::Unknown,
                "Wait ten seconds before testing another notification.",
            ));
        }
        *last = Some(Instant::now());
    }
    let notifications = state.watches.notifications.clone();
    tauri::async_runtime::spawn_blocking(move || {
        notifications.show(Notification {
            alert_id: "notification-test".into(),
            watch_id: "notification-test".into(),
            title: "PC Agent".into(),
            body: if language == "fr" {
                "Notification locale de test. Cliquez pour ouvrir Surveillances."
            } else {
                "Local test notification. Click to open Watches."
            }
            .into(),
        })
    })
    .await
    .map_err(|e| Failure::from(e.to_string()))
}

#[tauri::command]
pub(super) async fn watch_weekly_summary(
    state: State<'_, AppState>,
) -> Result<WatchSummary, Failure> {
    let history = state.history.clone();
    tauri::async_runtime::spawn_blocking(move || {
        read(&history, |s| s.watch_weekly_summary(unix_ms()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub(super) fn watch_activation_take(state: State<'_, AppState>) -> Option<String> {
    lock(&state.watches.activation).take()
}
