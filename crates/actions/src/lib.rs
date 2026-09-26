//! Local action broker (spec 11, 15.2, 17.4). The model may only propose an [`ActionIntent`];
//! this crate resolves it into an immutable [`ActionPlan`], requires a local single-use
//! [`ApprovalToken`], revalidates every target and executes a tiny fixed catalog.
//! Nothing here is a model tool.
#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

mod approval;
mod broker;
mod cleanup;
mod journal;
mod native;
mod plan;
#[cfg(test)]
mod tests;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

pub use approval::{
    approve_locally, ApprovalRejection, ApprovalToken, LocalSession, MAX_APPROVAL_TTL,
};
pub use broker::{
    ActionResult, Broker, Capability, EffectError, EffectOutcome, Effects, Guard, ItemOutcome,
    ItemResult, MeasurementHooks, OverallOutcome, Permit, ProtectionCheck, Refusal, SkipReason,
    TargetResolver, DEFAULT_PLAN_TTL, MAX_PLAN_TTL,
};
pub use cleanup::{CacheInventory, SyntheticDemoCache, SYNTHETIC_DEMO_CACHE_ID};
pub use journal::{
    recover, InterruptedItem, Journal, JournalPhase, JournalRecord, PreState, RecoveryReport,
};
pub use native::{
    explorer_select_arg, is_windows_absolute, BaselineProtection, NativeEffects, NativeGuard,
};
pub use plan::{
    ActionIntent, ActionKind, ActionPlan, CleanupTarget, FileIdentity, FileTarget, PlanItem,
    PlanModelView, PlanState, Precondition, ProcessIdentity, ProcessTarget, Recovery,
    ResolvedTarget, SettingsPage, MAX_REF_LEN, MAX_TARGETS,
};

use pcagent_shared::Status;

#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("invalid intent: {0}")]
    InvalidIntent(&'static str),
    #[error("target {target_ref} could not be resolved ({status:?})")]
    Resolve { target_ref: String, status: Status },
    #[error("target {0} does not match the action kind")]
    TargetKindMismatch(String),
    #[error("target {0} is protected")]
    Protected(String),
    #[error("target {0} is a link, reparse point or redirected path")]
    ReparsePoint(String),
    #[error("target {0} changed since it was resolved")]
    IdentityChanged(String),
    #[error("target {0} is duplicated")]
    DuplicateTarget(String),
    #[error("settings page {0} is not allowlisted")]
    SettingsPageNotAllowed(String),
    #[error("cleanup adapter {0} is not available")]
    UnknownAdapter(String),
    #[error("{kind:?} is not executable: {reason}")]
    NotExecutable {
        kind: ActionKind,
        reason: &'static str,
    },
    #[error("illegal plan transition {from:?} -> {to:?}")]
    IllegalTransition { from: PlanState, to: PlanState },
    #[error("plan expired")]
    Expired,
    #[error("plan content does not match its fingerprint")]
    FingerprintMismatch,
    #[error("approval rejected: {0:?}")]
    ApprovalRejected(ApprovalRejection),
    #[error("journal: {0}")]
    Journal(#[from] std::io::Error),
}
