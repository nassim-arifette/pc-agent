//! Shared host state and typed turn inputs/events; no command dispatch.
use crate::images::ImageSlot;
use crate::model_routing::{ModelRouting, RoutingReview};
use crate::preview::PreparedTurn;
use crate::selection::{PreparedSelection, SelectionRegistry};
use crate::session::Step;
use crate::settings::PrivacySettings;
use crate::{actions, elevation, view};
use pcagent_agent::capabilities::Probes;
use pcagent_agent::image_input::ImageInput;
use pcagent_agent::selection::{FrozenSelection, SelectionIntent, SelectionRef, SelectionTarget};
use pcagent_agent::{ExtensionRequest, PastTurn, PrivacyContext, Sharing, StorageContext};
use pcagent_shared::Snapshot;
use pcagent_storage::StorageService;
use pcagent_store::sampler::Sampler;
use pcagent_store::Store;
use pcagent_telemetry::ProgramNames;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{mpsc, Arc, Mutex};

/// Requests for the thread that owns the live collector (its native handles stay on it).
pub(super) enum Measure {
    Dashboard(mpsc::Sender<view::Dashboard>),
    Activity(mpsc::Sender<view::ActivityView>),
    Preview(mpsc::Sender<Snapshot>),
}

pub(super) struct Running {
    pub(super) id: u64,
    pub(super) turn_id: String,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) answer: Arc<Mutex<Option<mpsc::Sender<bool>>>>,
    pub(super) finished: Arc<AtomicBool>,
    /// What a reloaded window needs to reattach this turn (`current_turn`).
    pub(super) question: String,
    pub(super) conversation_id: Option<String>,
    pub(super) started_at_unix_ms: u64,
    /// The extension request waiting for an answer, as `turn:extension` sent it.
    pub(super) extension: Arc<Mutex<Option<Value>>>,
}

/// The running turn, so a reloaded window can show it, stop it and answer its extension request.
#[derive(Serialize)]
pub(super) struct CurrentTurn {
    pub(super) turn_id: String,
    pub(super) conversation_id: Option<String>,
    pub(super) question: String,
    pub(super) started_at_unix_ms: u64,
    pub(super) stopping: bool,
    pub(super) extension: Option<Value>,
}

#[derive(Default)]
pub(super) struct ScanSlot {
    pub(super) scope_id: Option<String>,
    pub(super) root: Option<String>,
    pub(super) running: bool,
}

pub(super) struct AppState {
    pub(super) measure: Mutex<mpsc::Sender<Measure>>,
    /// Shared by the Storage screen and investigations: a folder scanned there is authorized for
    /// the model (sizes only, names hidden) when the person allows it for an investigation.
    pub(super) storage: Arc<StorageService>,
    pub(super) scan: Arc<Mutex<ScanSlot>>,
    pub(super) investigation: Mutex<Option<Running>>,
    pub(super) next_id: AtomicU64,
    /// Local conversation history; `None` when its file cannot be opened.
    pub(super) history: Arc<Mutex<Option<Store>>>,
    pub(super) sampler: Option<Sampler>,
    pub(super) preview: Mutex<Option<PreparedTurn>>,
    pub(super) images: Mutex<ImageSlot>,
    pub(super) image_decoding: Arc<AtomicBool>,
    pub(super) preview_revision: AtomicU64,
    /// Changes of consent/model policy, independent of ordinary dashboard/preview refreshes.
    pub(super) selection_context_revision: AtomicU64,
    pub(super) privacy: Mutex<PrivacySettings>,
    pub(super) selections: Arc<Mutex<SelectionRegistry>>,
    pub(super) model_routing: Mutex<ModelRouting>,
    /// The last finished turn's action proposals; they run only after a click in this window.
    pub(super) actions: Arc<actions::Slot>,
    pub(super) elevation: Arc<elevation::Slot>,
    /// What this PC offers (`pcagent_agent::capabilities`), from the probes the window already
    /// runs: the helper next to the app at start, NVIDIA GPUs and NTFS volumes from the live
    /// measurements, the battery from the live battery read. Turns and `app_info` read it.
    pub(super) probes: Arc<Mutex<Probes>>,
    /// Readable program names for the window, read and cached on the threads that measure or
    /// investigate, never on the window's thread. They stay local (C5, note of 26 October).
    pub(super) program_names: Arc<ProgramNames>,
}

pub(super) struct SelectionStart {
    pub(super) reference: SelectionRef,
    pub(super) intent: SelectionIntent,
    pub(super) preview_id: String,
}

#[derive(Clone, Serialize)]
pub(super) struct StepEvent {
    pub(super) id: u64,
    pub(super) at_unix_ms: u64,
    pub(super) step: Step,
}
#[derive(Clone, Serialize)]
pub(super) struct ExtensionEvent {
    pub(super) id: u64,
    pub(super) request: ExtensionRequest,
    pub(super) usage: view::Usage,
}
#[derive(Clone, Serialize)]
pub(super) struct DoneEvent {
    pub(super) id: u64,
    pub(super) report: view::Report,
    /// The conversation the turn was saved in; `None` without a history file.
    pub(super) conversation_id: Option<String>,
}
#[derive(Serialize)]
pub(super) struct Started {
    pub(super) id: u64,
    pub(super) conversation_id: Option<String>,
    pub(super) turn_id: String,
}
/// The question of one turn, what the model reads again, and where it may go.
pub(super) struct Ask<'a> {
    pub(super) turn_id: &'a str,
    pub(super) question: &'a str,
    pub(super) past: &'a [PastTurn],
    /// `None` for a local analysis; otherwise the person's choice of program names.
    pub(super) cloud: Option<Sharing>,
    pub(super) initial: Option<&'a Snapshot>,
    pub(super) privacy: Option<&'a PrivacyContext>,
    pub(super) storage_context: Option<&'a StorageContext>,
    pub(super) scope_ids: Vec<String>,
    pub(super) selected: Option<&'a FrozenSelection>,
    pub(super) routing: Option<&'a RoutingReview>,
    pub(super) image: Option<&'a ImageInput>,
    pub(super) elevated_evidence: Vec<pcagent_tools::elevated_tools::ApprovedInspection>,
    /// The person consented to web lookups (C6); a key is still required.
    pub(super) web_search: bool,
    /// What this PC offered when the turn started: the model is offered only what works.
    pub(super) probes: Probes,
}

pub(super) enum ApprovedCapture {
    Snapshot(Box<PreparedTurn>),
    Selected(Box<PreparedSelection>),
}
impl ApprovedCapture {
    pub(super) fn image(&self) -> Option<&ImageInput> {
        match self {
            Self::Snapshot(prepared) => prepared.image.as_ref().map(|image| &image.image),
            Self::Selected(_) => None,
        }
    }
    pub(super) fn snapshot(&self) -> &Snapshot {
        match self {
            Self::Snapshot(prepared) => &prepared.snapshot,
            Self::Selected(prepared) => prepared.selection.snapshot(),
        }
    }
    pub(super) fn privacy(&self) -> &PrivacyContext {
        match self {
            Self::Snapshot(prepared) => &prepared.privacy,
            Self::Selected(prepared) => &prepared.privacy,
        }
    }
    pub(super) fn past(&self) -> &[PastTurn] {
        match self {
            Self::Snapshot(prepared) => &prepared.past,
            Self::Selected(prepared) => &prepared.past,
        }
    }
    pub(super) fn context(&self) -> &StorageContext {
        match self {
            Self::Snapshot(prepared) => &prepared.storage_context,
            Self::Selected(prepared) => prepared.selection.storage_context(),
        }
    }
    pub(super) fn scope(&self) -> Option<&str> {
        match self {
            Self::Snapshot(prepared) => prepared.storage_scope.as_deref(),
            Self::Selected(prepared) => match &prepared.reference.target {
                SelectionTarget::Storage { scope_id, .. } => Some(scope_id),
                _ => None,
            },
        }
    }
    pub(super) fn selected(&self) -> Option<&FrozenSelection> {
        match self {
            Self::Snapshot(_) => None,
            Self::Selected(prepared) => Some(&prepared.selection),
        }
    }
    pub(super) fn routing(&self) -> Option<&RoutingReview> {
        match self {
            Self::Snapshot(prepared) => prepared.routing.as_ref(),
            Self::Selected(prepared) => prepared.routing.as_ref(),
        }
    }
}
#[derive(Clone, Serialize)]
pub(super) struct FailedEvent {
    pub(super) id: u64,
    pub(super) code: String,
    pub(super) message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StartRequest {
    pub(super) question: String,
    pub(super) cloud: bool,
    pub(super) use_storage: bool,
    pub(super) names: bool,
    pub(super) conversation_id: Option<String>,
    pub(super) preview_id: Option<String>,
    #[serde(default)]
    pub(super) file_names: bool,
    pub(super) image_id: Option<String>,
}
