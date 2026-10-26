//! Exact sharing previews and their bounded, reviewed inputs.
use super::history_commands::{read_past, read_past_from};
use super::measurements::ask_measure;
use super::settings_commands::configured_model_id;
use super::state::Measure;
use super::{lock, AppState};
use crate::failure::{Code, Failure};
use crate::images::ImageView;
use crate::model_routing::RoutingView;
use crate::preview::PreparedTurn;
use crate::settings::PrivacyMode;
use crate::{failure, images};
use pcagent_agent::credentials::Credentials;
use pcagent_agent::image_input::MODEL_ID as IMAGE_MODEL_ID;
use pcagent_agent::routing::RoutingSettings;
use pcagent_agent::Sharing;
use pcagent_nebius::{Config, Nebius};
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::State;

#[derive(Serialize)]
pub(super) struct Preview {
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
    image: Option<ImageView>,
}

/// The person's choice in the interface: program names or opaque references (contract C5).
pub(super) fn sharing_of(names: bool) -> Sharing {
    if names {
        Sharing::Complete
    } else {
        Sharing::Anonymized
    }
}

/// Freeze the first data message, including historical context, until its matching approval.
#[tauri::command]
pub(super) async fn cloud_preview(
    state: State<'_, AppState>,
    question: String,
    names: bool,
    conversation_id: Option<String>,
    file_names: Option<bool>,
    image_id: Option<String>,
) -> Result<Preview, Failure> {
    let revision = state.preview_revision.fetch_add(1, Ordering::SeqCst) + 1;
    let question = checked_question(&question)?;
    let sharing = sharing_of(names);
    if image_id.is_some()
        && (sharing != Sharing::Complete || lock(&state.privacy).mode != PrivacyMode::Complete)
    {
        return Err(Failure::new(
            Code::ImageSharingRequired,
            "Images require Complete sharing.",
        ));
    }
    let image = image_id
        .as_deref()
        .map(|id| lock(&state.images).bind(id, conversation_id.as_deref()))
        .transpose()?;
    if image.is_some() {
        tauri::async_runtime::spawn_blocking(|| {
            let credentials = Credentials::load();
            let config = Config::from_lookup(|name| credentials.get(name))?;
            Nebius::new(config.for_model(IMAGE_MODEL_ID)?)?
                .check_image_catalog(Duration::from_secs(10))
        })
        .await
        .map_err(|_| {
            Failure::new(
                Code::ImageModelUnavailable,
                "The vision model could not be checked.",
            )
        })?
        .map_err(|_| {
            Failure::new(
                Code::ImageModelUnavailable,
                "The approved vision model is unavailable for this account.",
            )
        })?;
    }
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
    prepared.image = image;
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
    let image_view = prepared.image.as_ref().map(|image| image.view());
    let bytes = prepared.image.as_ref().map_or(text.len(), |image| {
        image.image.message_content(&message).to_string().len()
    });
    // The collector wait above cannot retain a mutex guard. Re-read under the history
    // lock and keep it through publication, rejecting a deleted or changed conversation.
    let history = lock(&state.history);
    if read_past_from(&history, prepared.conversation_id.as_deref())? != prepared.past {
        return Err(Failure::new(
            Code::PreviewChanged,
            "Conversation history changed. Open the sharing preview again.",
        ));
    }
    let images = lock(&state.images);
    if prepared
        .image
        .as_ref()
        .is_some_and(|image| !images.matches(image))
    {
        return Err(images::changed());
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
        bytes,
        measured,
        findings: message["findings"].as_array().map_or(0, Vec::len),
        names: message["process_names"].as_object().map_or(0, |n| n.len()),
        storage_folder,
        file_name_sharing,
        message,
        model_routing: if image_view.is_some() {
            RoutingView {
                settings: RoutingSettings::default(),
                revision: routing.revision,
                automatic_available: false,
                calibration_reviewed_at_unix_ms: None,
                calibrated_rules: Vec::new(),
                limitation: None,
            }
        } else {
            lock(&state.model_routing).view()
        },
        configured_model_id: if image_view.is_some() {
            Some(IMAGE_MODEL_ID.into())
        } else {
            routing.configured_model_id
        },
        image: image_view,
    })
}

pub(super) fn checked_question(question: &str) -> Result<String, Failure> {
    let question = question.trim();
    failure::check_question(question)?;
    Ok(question.to_owned())
}

pub(super) fn preview_required() -> Failure {
    Failure::new(
        Code::PreviewRequired,
        "Open the sharing preview before sending.",
    )
}
