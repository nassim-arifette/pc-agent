use crate::ActionError;
use pcagent_shared::{new_id, unix_ms, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, path::PathBuf};

pub const MAX_TARGETS: usize = 64;
pub const MAX_REF_LEN: usize = 128;
const MAX_INTENT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    OpenInExplorer,
    OpenSettingsPage,
    RequestNormalClose,
    RecycleSelectedFiles,
    RunCleanupAdapter,
}
impl ActionKind {
    fn max_targets(self) -> usize {
        match self {
            Self::RequestNormalClose | Self::RecycleSelectedFiles => MAX_TARGETS,
            _ => 1,
        }
    }
}

/// Untrusted proposal from the model. It carries opaque references only, never paths or PIDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionIntent {
    pub action_kind: ActionKind,
    pub target_refs: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}
impl ActionIntent {
    pub fn from_model_json(text: &str) -> Result<Self, ActionError> {
        if text.len() > MAX_INTENT_BYTES {
            return Err(ActionError::InvalidIntent("intent too large"));
        }
        let intent: Self = serde_json::from_str(text)
            .map_err(|_| ActionError::InvalidIntent("malformed intent"))?;
        intent.validate()?;
        Ok(intent)
    }
    pub(crate) fn validate(&self) -> Result<(), ActionError> {
        if self.target_refs.is_empty() || self.target_refs.len() > self.action_kind.max_targets() {
            return Err(ActionError::InvalidIntent(
                "target count outside the allowed range",
            ));
        }
        if self.evidence_refs.len() > MAX_TARGETS {
            return Err(ActionError::InvalidIntent("too many evidence references"));
        }
        if !self
            .target_refs
            .iter()
            .chain(&self.evidence_refs)
            .all(|r| valid_ref(r))
        {
            return Err(ActionError::InvalidIntent(
                "reference is not an opaque identifier",
            ));
        }
        let mut seen = HashSet::new();
        if !self.target_refs.iter().all(|r| seen.insert(r)) {
            return Err(ActionError::InvalidIntent("duplicate target reference"));
        }
        Ok(())
    }
}
pub(crate) fn valid_ref(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= MAX_REF_LEN
        && r.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

/// Fixed allowlist of `ms-settings:` pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsPage {
    StorageSense,
    AppsFeatures,
    StartupApps,
}
impl SettingsPage {
    pub fn from_ref(target_ref: &str) -> Option<Self> {
        match target_ref {
            "storagesense" => Some(Self::StorageSense),
            "appsfeatures" => Some(Self::AppsFeatures),
            "startupapps" => Some(Self::StartupApps),
            _ => None,
        }
    }
    pub fn uri(self) -> &'static str {
        match self {
            Self::StorageSense => "ms-settings:storagesense",
            Self::AppsFeatures => "ms-settings:appsfeatures",
            Self::StartupApps => "ms-settings:startupapps",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileIdentity {
    pub volume_serial: u64,
    #[serde(with = "hex_u128")]
    pub file_id: u128,
    pub size_bytes: u64,
    pub modified_filetime: u64,
    pub is_directory: bool,
    /// Also set when the opened object's final path differs from the requested path.
    pub is_reparse_point: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub creation_time_filetime: u64,
    pub boot_id: Option<String>,
}
impl ProcessIdentity {
    /// A missing boot identity never matches.
    pub fn same_instance(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.creation_time_filetime == other.creation_time_filetime
            && self.boot_id.is_some()
            && self.boot_id == other.boot_id
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTarget {
    pub path: PathBuf,
    pub identity: FileIdentity,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTarget {
    pub identity: ProcessIdentity,
    pub executable_name: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupTarget {
    pub adapter_id: String,
    pub root: PathBuf,
    pub files: u64,
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedTarget {
    File(FileTarget),
    Process(ProcessTarget),
    SettingsPage { page: SettingsPage },
    CleanupAdapter(CleanupTarget),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanItem {
    pub target_ref: String,
    pub target: ResolvedTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    NotApplicable,
    RecycleBinIfOperationSupported,
    UserCanRestartApplication,
    NoneSyntheticRegenerableData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Precondition {
    IdentityRevalidated,
    NotProtected,
    NotReparsePoint,
    SameProcessInstance,
    SettingsPageAllowlisted,
    SyntheticCacheMarkerPresent,
    RecycleBinAvailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Draft,
    Validated,
    AwaitingApproval,
    Approved,
    Executing,
    Verifying,
    Completed,
    Partial,
    Failed,
    Cancelled,
    Expired,
    Invalidated,
}
impl PlanState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Partial
                | Self::Failed
                | Self::Cancelled
                | Self::Expired
                | Self::Invalidated
        )
    }
    pub fn can_transition(self, to: Self) -> bool {
        use PlanState::*;
        matches!(
            (self, to),
            (Draft, Validated)
                | (Validated, AwaitingApproval)
                | (AwaitingApproval, Approved)
                | (Approved, AwaitingApproval)
                | (Approved, Executing)
                | (Executing, Verifying)
                | (Executing, Cancelled)
                | (Verifying, Completed | Partial | Failed | Cancelled)
                | (
                    Draft | Validated | AwaitingApproval | Approved,
                    Expired | Invalidated | Cancelled
                )
        )
    }
}

/// Immutable manifest. The serialized form is the local (trusted UI) view and contains resolved
/// paths and PIDs; use [`ActionPlan::model_view`] for anything that may reach the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPlan {
    schema_version: u32,
    plan_id: String,
    state: PlanState,
    action_kind: ActionKind,
    target_refs: Vec<String>,
    items: Vec<PlanItem>,
    evidence_refs: Vec<String>,
    estimated_immediate_reclaim_bytes: Option<u64>,
    recovery: Recovery,
    preconditions: Vec<Precondition>,
    requires_local_approval: bool,
    created_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    fingerprint: String,
}

#[derive(Serialize)]
struct Canonical<'a> {
    schema_version: u32,
    plan_id: &'a str,
    action_kind: ActionKind,
    target_refs: &'a [String],
    items: &'a [PlanItem],
    evidence_refs: &'a [String],
    estimated_immediate_reclaim_bytes: Option<u64>,
    recovery: Recovery,
    preconditions: &'a [Precondition],
    requires_local_approval: bool,
    created_at_unix_ms: u64,
    expires_at_unix_ms: u64,
}

/// Model-safe projection: no paths, PIDs, identities or fingerprint.
#[derive(Debug, Clone, Serialize)]
pub struct PlanModelView {
    pub schema_version: u32,
    pub plan_id: String,
    pub state: PlanState,
    pub action_kind: ActionKind,
    pub target_refs: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub estimated_immediate_reclaim_bytes: Option<u64>,
    pub recovery: Recovery,
    pub preconditions: Vec<Precondition>,
    pub requires_local_approval: bool,
    pub approval_granted: bool,
    pub expires_at_unix_ms: u64,
}

pub(crate) struct PlanParts {
    pub action_kind: ActionKind,
    pub items: Vec<PlanItem>,
    pub evidence_refs: Vec<String>,
    pub estimated_immediate_reclaim_bytes: Option<u64>,
    pub recovery: Recovery,
    pub preconditions: Vec<Precondition>,
    pub ttl_ms: u64,
}

impl ActionPlan {
    pub(crate) fn build(parts: PlanParts) -> Result<Self, ActionError> {
        let created = unix_ms();
        let mut plan = Self {
            schema_version: SCHEMA_VERSION,
            plan_id: new_id("plan"),
            state: PlanState::Draft,
            action_kind: parts.action_kind,
            target_refs: parts.items.iter().map(|i| i.target_ref.clone()).collect(),
            items: parts.items,
            evidence_refs: parts.evidence_refs,
            estimated_immediate_reclaim_bytes: parts.estimated_immediate_reclaim_bytes,
            recovery: parts.recovery,
            preconditions: parts.preconditions,
            requires_local_approval: true,
            created_at_unix_ms: created,
            expires_at_unix_ms: created.saturating_add(parts.ttl_ms),
            fingerprint: String::new(),
        };
        plan.fingerprint = plan.compute_fingerprint()?;
        plan.transition(PlanState::Validated)?;
        plan.transition(PlanState::AwaitingApproval)?;
        Ok(plan)
    }
    fn compute_fingerprint(&self) -> Result<String, ActionError> {
        let canonical = Canonical {
            schema_version: self.schema_version,
            plan_id: &self.plan_id,
            action_kind: self.action_kind,
            target_refs: &self.target_refs,
            items: &self.items,
            evidence_refs: &self.evidence_refs,
            estimated_immediate_reclaim_bytes: self.estimated_immediate_reclaim_bytes,
            recovery: self.recovery,
            preconditions: &self.preconditions,
            requires_local_approval: self.requires_local_approval,
            created_at_unix_ms: self.created_at_unix_ms,
            expires_at_unix_ms: self.expires_at_unix_ms,
        };
        let bytes = serde_json::to_vec(&canonical)
            .map_err(|_| ActionError::InvalidIntent("target is not representable"))?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
    /// Recomputes the fingerprint; any mismatch invalidates the plan.
    pub fn verify_fingerprint(&mut self) -> Result<(), ActionError> {
        let consistent = self.schema_version == SCHEMA_VERSION
            && self.requires_local_approval
            && self.target_refs.len() == self.items.len()
            && self
                .target_refs
                .iter()
                .zip(&self.items)
                .all(|(r, i)| *r == i.target_ref);
        if consistent && self.compute_fingerprint()? == self.fingerprint {
            return Ok(());
        }
        if self.state.can_transition(PlanState::Invalidated) {
            self.state = PlanState::Invalidated;
        }
        Err(ActionError::FingerprintMismatch)
    }
    pub fn transition(&mut self, to: PlanState) -> Result<(), ActionError> {
        if !self.state.can_transition(to) {
            return Err(ActionError::IllegalTransition {
                from: self.state,
                to,
            });
        }
        self.state = to;
        Ok(())
    }
    /// Expires the plan if its deadline passed. Returns `true` when expired.
    pub fn check_expiry(&mut self) -> bool {
        if unix_ms() < self.expires_at_unix_ms {
            return false;
        }
        if self.state.can_transition(PlanState::Expired) {
            self.state = PlanState::Expired;
        }
        true
    }
    /// A revision creates new content: the fingerprint changes and any prior approval is void.
    pub fn exclude_targets(&mut self, target_refs: &[&str]) -> Result<(), ActionError> {
        if !matches!(
            self.state,
            PlanState::Validated | PlanState::AwaitingApproval | PlanState::Approved
        ) {
            return Err(ActionError::IllegalTransition {
                from: self.state,
                to: PlanState::AwaitingApproval,
            });
        }
        self.verify_fingerprint()?;
        let remaining: Vec<PlanItem> = self
            .items
            .iter()
            .filter(|i| !target_refs.contains(&i.target_ref.as_str()))
            .cloned()
            .collect();
        if remaining.is_empty() {
            return Err(ActionError::InvalidIntent(
                "a plan needs at least one target",
            ));
        }
        self.target_refs = remaining.iter().map(|i| i.target_ref.clone()).collect();
        self.items = remaining;
        self.fingerprint = self.compute_fingerprint()?;
        if self.state != PlanState::AwaitingApproval {
            self.transition(PlanState::AwaitingApproval)?;
        }
        Ok(())
    }
    pub fn model_view(&self) -> PlanModelView {
        PlanModelView {
            schema_version: self.schema_version,
            plan_id: self.plan_id.clone(),
            state: self.state,
            action_kind: self.action_kind,
            target_refs: self.target_refs.clone(),
            evidence_refs: self.evidence_refs.clone(),
            estimated_immediate_reclaim_bytes: self.estimated_immediate_reclaim_bytes,
            recovery: self.recovery,
            preconditions: self.preconditions.clone(),
            requires_local_approval: self.requires_local_approval,
            approval_granted: self.state == PlanState::Approved,
            expires_at_unix_ms: self.expires_at_unix_ms,
        }
    }
    pub fn plan_id(&self) -> &str {
        &self.plan_id
    }
    pub fn state(&self) -> PlanState {
        self.state
    }
    pub fn action_kind(&self) -> ActionKind {
        self.action_kind
    }
    pub fn items(&self) -> &[PlanItem] {
        &self.items
    }
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
    pub fn estimated_immediate_reclaim_bytes(&self) -> Option<u64> {
        self.estimated_immediate_reclaim_bytes
    }
    pub fn recovery(&self) -> Recovery {
        self.recovery
    }
    pub fn preconditions(&self) -> &[Precondition] {
        &self.preconditions
    }
    pub fn created_at_unix_ms(&self) -> u64 {
        self.created_at_unix_ms
    }
    pub fn expires_at_unix_ms(&self) -> u64 {
        self.expires_at_unix_ms
    }
}

mod hex_u128 {
    use serde::{de::Error, Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{v:032x}"))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        let text = String::deserialize(d)?;
        if text.len() != 32 {
            return Err(D::Error::custom("file id must be 32 hex digits"));
        }
        u128::from_str_radix(&text, 16).map_err(D::Error::custom)
    }
}
