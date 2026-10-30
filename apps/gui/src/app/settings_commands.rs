//! Local consent, provider settings, routing policy and validated web links.
use super::{lock, AppState};
use crate::failure::{Code, Failure};
use crate::model_routing::RoutingView;
use crate::settings::PrivacySettings;
use crate::{failure, settings, view};
use pcagent_agent::credentials;
use pcagent_agent::credentials::Credentials;
use pcagent_agent::routing::RoutingSettings;
use pcagent_nebius::{Config, Nebius};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::State;

#[tauri::command]
pub(super) async fn specialist_get(check: bool) -> Result<crate::specialist::View, String> {
    tauri::async_runtime::spawn_blocking(move || crate::specialist::view(check))
        .await
        .map_err(|_| "Specialist status is unavailable.".into())
}

#[tauri::command]
pub(super) fn specialist_set(enabled: bool) -> Result<crate::specialist::View, String> {
    crate::specialist::set_enabled(enabled).map_err(str::to_owned)?;
    Ok(crate::specialist::view(false))
}

#[tauri::command]
pub(super) fn startup_get() -> Result<bool, Failure> {
    pcagent_inspect::autostart::get().map_err(failure::startup)
}

#[tauri::command]
pub(super) fn startup_set(enabled: bool) -> Result<bool, Failure> {
    let executable = std::env::current_exe().map_err(failure::startup)?;
    pcagent_inspect::autostart::set(enabled, &executable).map_err(failure::startup)?;
    startup_get()
}

pub(super) fn configured_model_id() -> Option<String> {
    Credentials::load()
        .get("NEBIUS_MODEL_ID")
        .filter(|id| pcagent_agent::validate_model_id(id).is_ok())
}

#[tauri::command]
pub(super) fn model_routing_get(state: State<'_, AppState>) -> RoutingView {
    let mut routing = lock(&state.model_routing);
    // A campaign reviewed while the window is open makes Auto available without a restart.
    routing.refresh();
    routing.view()
}

#[tauri::command]
pub(super) fn model_routing_set(
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
pub(super) fn app_info(state: State<'_, AppState>) -> view::AppInfo {
    let probes = *lock(&state.probes);
    view::app_info(
        &Credentials::load(),
        view::capabilities(state.sampler.is_some(), probes),
    )
}

#[tauri::command]
pub(super) fn privacy_get(state: State<'_, AppState>) -> PrivacySettings {
    lock(&state.privacy).clone()
}

#[tauri::command]
pub(super) fn privacy_set(
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

#[tauri::command]
pub(super) fn save_cloud_settings(
    state: State<'_, AppState>,
    api_key: Option<String>,
    model_id: String,
    reasoning: Option<pcagent_nebius::Reasoning>,
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
pub(super) fn save_search_key(api_key: Option<String>) -> Result<view::Cloud, Failure> {
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
pub(super) fn open_web_source(
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
    let host = pcagent_tools::tavily::domain(url).ok_or("Only https web pages can be opened.")?;
    if !pcagent_tools::external_tools::searchable(&host) {
        return Err("This address is not one of the sites web lookups search.".into());
    }
    pcagent_actions::open_web_page(url).map_err(|e| Failure::new(Code::OpenFailed, e.detail))
}

#[derive(Serialize)]
pub(super) struct CloudCheck {
    price: Option<view::PriceView>,
}

/// A local setup click opens only the official console, never a caller-provided address.
#[tauri::command]
pub(super) fn open_nebius_console() -> Result<(), String> {
    pcagent_actions::open_web_page("https://tokenfactory.nebius.com/")
        .map_err(|_| "The browser could not be opened.".to_owned())
}

/// Lists the saved account's catalog; the page receives metadata, never the local key.
#[tauri::command]
pub(super) async fn cloud_models() -> Result<pcagent_nebius::ModelCatalog, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let credentials = Credentials::load();
        Nebius::account_catalog(|name| credentials.get(name), Duration::from_secs(10))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Explicit catalog check with the saved key; sends no telemetry.
#[tauri::command]
pub(super) async fn test_cloud() -> Result<CloudCheck, String> {
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
