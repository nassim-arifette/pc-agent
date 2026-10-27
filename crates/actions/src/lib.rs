//! Local action broker (spec 11, 15.2, 17.4). The model may only propose an [`ActionIntent`];
//! this crate resolves it into an immutable [`ActionPlan`], requires a local single-use
//! [`ApprovalToken`], revalidates every target and executes a tiny fixed catalog.
//! Nothing here is a model tool.
#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

mod approval;
mod broker;
mod cancellation;
mod cleanup;
mod journal;
mod native;
mod os;
mod plan;
mod startup;
#[cfg(test)]
mod tests;
mod user_cleanup;
#[cfg(windows)]
use os::windows;

pub use approval::{
    approve_locally, ApprovalRejection, ApprovalToken, LocalSession, MAX_APPROVAL_TTL,
};
pub use broker::{
    ActionResult, Broker, Capability, EffectError, EffectOutcome, Effects, Guard, ItemOutcome,
    ItemResult, MeasurementHooks, OverallOutcome, Permit, ProtectionCheck, Refusal, SkipReason,
    TargetResolver, DEFAULT_PLAN_TTL, MAX_PLAN_TTL,
};
pub use cancellation::ActionCancellation;
pub use cleanup::{CacheInventory, SyntheticDemoCache, SYNTHETIC_DEMO_CACHE_ID};
pub use journal::{
    recover, InterruptedItem, Journal, JournalPhase, JournalRecord, PreState, RecoveryReport,
};
pub use native::{
    explorer_select_arg, is_windows_absolute, BaselineProtection, NativeEffects, NativeGuard,
};
#[cfg_attr(not(windows), allow(unused_imports))]
pub(crate) use native::{same_object, verify_recycled, RecycleEvidence};
pub use plan::{
    ActionIntent, ActionKind, ActionPlan, CleanupTarget, FileIdentity, FileTarget, PlanItem,
    PlanModelView, PlanState, Precondition, ProcessIdentity, ProcessTarget, Recovery,
    ResolvedTarget, SettingsPage, MAX_REF_LEN, MAX_TARGETS,
};
pub use startup::{StartupApproval, StartupIdentity, StartupTarget};
pub use user_cleanup::{
    RecycleCleanupTarget, UserCleanupAdapter, UserCleanupKind, MAX_USER_CLEANUP_ENTRIES,
    MAX_USER_CLEANUP_FILES, MIN_TEMP_AGE,
};
#[cfg(windows)]
pub use windows::{open_web_page, show_in_explorer};

/// A plain `https://` address with a host name: no user information, backslash, port other than
/// digits, space, quote, control character, comma or semicolon (Explorer's argument separators).
pub fn web_url_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    if url.len() > 2048
        || !url.bytes().all(|b| {
            b.is_ascii_graphic()
                && !matches!(b, b'"' | b'\\' | b'^' | b'`' | b'<' | b'>' | b',' | b';')
        })
    {
        return false;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    port.is_none_or(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && host.contains('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

#[cfg(test)]
mod web_url_tests {
    use super::web_url_allowed;
    #[test]
    fn only_plain_https_pages_can_be_opened() {
        assert!(web_url_allowed(
            "https://www.nvidia.com/en-us/drivers/details/237728/"
        ));
        assert!(web_url_allowed("https://learn.microsoft.com:443/a?b=c#d"));
        for url in [
            "http://nvidia.com/",
            "https://evil.com\\@nvidia.com/",
            "https://user@nvidia.com/",
            "https://nvidia.com/a b",
            "https://nvidia.com/\"x",
            "https://nvidia.com:80a/",
            "https://localhost/",
            "file:///C:/Windows/System32/calc.exe",
            "https://nvidia.com/,/root,C:\\Windows",
            // A comma would let Explorer read a second object to open.
            "https://nvidia.com/a,file:C:/Windows/System32/calc.exe",
            "https://nvidia.com/a;b",
        ] {
            assert!(!web_url_allowed(url), "{url}");
        }
    }
}

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
