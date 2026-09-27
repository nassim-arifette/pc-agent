use crate::{
    approval::LocalSession,
    cleanup::{SyntheticDemoCache, SYNTHETIC_DEMO_CACHE_ID},
    is_windows_absolute,
    journal::{Journal, JournalPhase, JournalRecord, PreState},
    plan::PlanParts,
    ActionError, ActionIntent, ActionKind, ActionPlan, ApprovalToken, FileIdentity, FileTarget,
    PlanItem, PlanState, Precondition, ProcessIdentity, ProcessTarget, Recovery, ResolvedTarget,
    SettingsPage,
};
use pcagent_shared::{unix_ms, Status, SCHEMA_VERSION};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const DEFAULT_PLAN_TTL: Duration = Duration::from_secs(15 * 60);
pub const MAX_PLAN_TTL: Duration = Duration::from_secs(60 * 60);

/// Resolves opaque model-visible references to local targets (storage/telemetry/policy crates).
pub trait TargetResolver {
    fn resolve(&self, target_ref: &str) -> Result<ResolvedTarget, Status>;
}
/// Observes the current identity of a target. `Ok(None)` means it no longer exists.
pub trait Guard {
    fn observe_file(&self, path: &Path) -> Result<Option<FileIdentity>, Status>;
    fn observe_process(&self, pid: u32) -> Result<Option<ProcessIdentity>, Status>;
}
/// Protected locations and user exclusions (policy crate). `true` refuses the target.
pub trait ProtectionCheck {
    fn is_protected(&self, path: &Path) -> bool;
}

/// Witness that the broker revalidated an item under a consumed approval. Only this crate can
/// construct it, so [`Effects`] methods cannot be invoked from outside the broker.
pub struct Permit(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Executable,
    RecommendationOnly(&'static str),
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectOutcome {
    Done(Option<String>),
    Refused(Refusal),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectError {
    pub status: Status,
    pub detail: String,
}
impl EffectError {
    pub fn new(status: Status, detail: &str) -> Self {
        Self {
            status,
            detail: detail.into(),
        }
    }
}

/// Side effects of the P0 catalog. Implementations must not elevate, force termination, invoke a
/// shell interpreter or fall back to permanent deletion.
pub trait Effects {
    fn capability(&self, kind: ActionKind) -> Capability;
    fn open_in_explorer(
        &self,
        permit: &Permit,
        file: &FileTarget,
    ) -> Result<EffectOutcome, EffectError>;
    fn open_settings_page(
        &self,
        permit: &Permit,
        page: SettingsPage,
    ) -> Result<EffectOutcome, EffectError>;
    fn request_normal_close(
        &self,
        permit: &Permit,
        process: &ProcessTarget,
    ) -> Result<EffectOutcome, EffectError>;
    fn recycle_file(
        &self,
        permit: &Permit,
        file: &FileTarget,
    ) -> Result<EffectOutcome, EffectError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Protected,
    PermissionDenied,
    TargetMissing,
    TargetReplaced,
    ProcessReplaced,
    ReparsePoint,
    Unverifiable { status: Status },
    NoTopLevelWindow,
    StillRunningAfterCloseRequest,
    AdapterPreconditionFailed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    Cancelled,
    JournalUnavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ItemOutcome {
    Completed {
        detail: Option<String>,
        bytes_removed: Option<u64>,
    },
    Refused {
        reason: Refusal,
    },
    Failed {
        status: Status,
        detail: String,
    },
    Skipped {
        reason: SkipReason,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemResult {
    pub item_index: usize,
    pub target_ref: String,
    pub outcome: ItemOutcome,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallOutcome {
    Completed,
    Partial,
    Failed,
    Cancelled,
}
/// Before/after hooks for the diagnostics crate. Only references and timestamps are recorded here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeasurementHooks {
    pub before_ref: Option<String>,
    pub operation_started_at_unix_ms: u64,
    pub operation_finished_at_unix_ms: u64,
    pub after_ref: Option<String>,
    pub after_recorded_at_unix_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub schema_version: u32,
    pub plan_id: String,
    pub fingerprint: String,
    pub action_kind: ActionKind,
    pub overall: OverallOutcome,
    pub items: Vec<ItemResult>,
    pub measurement: MeasurementHooks,
}
impl ActionResult {
    pub fn record_after_measurement(&mut self, reference: impl Into<String>) {
        self.measurement.after_ref = Some(reference.into());
        self.measurement.after_recorded_at_unix_ms = Some(unix_ms());
    }
}

/// Local action broker. Holds the only paths from an approved plan to [`Effects`].
pub struct Broker<'a> {
    pub resolver: &'a dyn TargetResolver,
    pub guard: &'a dyn Guard,
    pub protection: &'a dyn ProtectionCheck,
    pub effects: &'a dyn Effects,
    pub journal: &'a Journal,
    pub synthetic_cache: Option<&'a SyntheticDemoCache>,
    pub plan_ttl: Duration,
}

impl Broker<'_> {
    pub fn capability(&self, kind: ActionKind) -> Capability {
        match kind {
            ActionKind::RunCleanupAdapter if self.synthetic_cache.is_some() => {
                Capability::Executable
            }
            ActionKind::RunCleanupAdapter => Capability::Unsupported,
            _ => self.effects.capability(kind),
        }
    }
    fn require_executable(&self, kind: ActionKind) -> Result<(), ActionError> {
        match self.capability(kind) {
            Capability::Executable => Ok(()),
            Capability::RecommendationOnly(reason) => {
                Err(ActionError::NotExecutable { kind, reason })
            }
            Capability::Unsupported => Err(ActionError::NotExecutable {
                kind,
                reason: "unsupported on this platform or configuration",
            }),
        }
    }

    /// Intent → validated manifest awaiting local approval.
    pub fn prepare_plan(&self, intent: &ActionIntent) -> Result<ActionPlan, ActionError> {
        intent.validate()?;
        let kind = intent.action_kind;
        self.require_executable(kind)?;
        let mut items = Vec::with_capacity(intent.target_refs.len());
        let mut seen_files = HashSet::new();
        for target_ref in &intent.target_refs {
            let target = self.resolve(kind, target_ref)?;
            if let ResolvedTarget::File(f) = &target {
                if !seen_files.insert((f.identity.volume_serial, f.identity.file_id)) {
                    return Err(ActionError::DuplicateTarget(target_ref.clone()));
                }
            }
            items.push(PlanItem {
                target_ref: target_ref.clone(),
                target,
            });
        }
        use Precondition::*;
        let (estimate, recovery, preconditions) = match kind {
            ActionKind::OpenInExplorer => (
                None,
                Recovery::NotApplicable,
                vec![IdentityRevalidated, NotProtected, NotReparsePoint],
            ),
            ActionKind::OpenSettingsPage => {
                (None, Recovery::NotApplicable, vec![SettingsPageAllowlisted])
            }
            ActionKind::RequestNormalClose => (
                None,
                Recovery::UserCanRestartApplication,
                vec![IdentityRevalidated, SameProcessInstance],
            ),
            ActionKind::RecycleSelectedFiles => (
                Some(0),
                Recovery::RecycleBinIfOperationSupported,
                vec![
                    IdentityRevalidated,
                    NotProtected,
                    NotReparsePoint,
                    RecycleBinAvailable,
                ],
            ),
            ActionKind::RunCleanupAdapter => (
                items.first().and_then(|i| match &i.target {
                    ResolvedTarget::CleanupAdapter(c) => c.bytes,
                    _ => None,
                }),
                Recovery::NoneSyntheticRegenerableData,
                vec![SyntheticCacheMarkerPresent],
            ),
        };
        ActionPlan::build(PlanParts {
            action_kind: kind,
            items,
            evidence_refs: intent.evidence_refs.clone(),
            estimated_immediate_reclaim_bytes: estimate,
            recovery,
            preconditions,
            ttl_ms: u64::try_from(self.plan_ttl.min(MAX_PLAN_TTL).as_millis()).unwrap_or(0),
        })
    }

    fn resolve(&self, kind: ActionKind, target_ref: &str) -> Result<ResolvedTarget, ActionError> {
        let mismatch = || ActionError::TargetKindMismatch(target_ref.into());
        match kind {
            ActionKind::OpenSettingsPage => {
                return SettingsPage::from_ref(target_ref)
                    .map(|page| ResolvedTarget::SettingsPage { page })
                    .ok_or_else(|| ActionError::SettingsPageNotAllowed(target_ref.into()));
            }
            ActionKind::RunCleanupAdapter => {
                let cache = self
                    .synthetic_cache
                    .filter(|_| target_ref == SYNTHETIC_DEMO_CACHE_ID)
                    .ok_or_else(|| ActionError::UnknownAdapter(target_ref.into()))?;
                let inventory = cache
                    .inventory()
                    .map_err(|_| ActionError::UnknownAdapter(target_ref.into()))?;
                return Ok(ResolvedTarget::CleanupAdapter(crate::CleanupTarget {
                    adapter_id: SYNTHETIC_DEMO_CACHE_ID.into(),
                    root: cache.root().to_path_buf(),
                    files: inventory.files,
                    bytes: inventory.bytes,
                }));
            }
            _ => {}
        }
        let target = self
            .resolver
            .resolve(target_ref)
            .map_err(|status| ActionError::Resolve {
                target_ref: target_ref.into(),
                status,
            })?;
        let refusal = match (&target, kind) {
            (ResolvedTarget::File(f), ActionKind::OpenInExplorer)
            | (ResolvedTarget::File(f), ActionKind::RecycleSelectedFiles) => {
                if kind == ActionKind::RecycleSelectedFiles && f.identity.is_directory {
                    return Err(mismatch());
                }
                self.check_file(f).err()
            }
            (ResolvedTarget::Process(p), ActionKind::RequestNormalClose) => {
                self.check_process(p).err()
            }
            _ => return Err(mismatch()),
        };
        match refusal {
            None => Ok(target),
            Some(Refusal::Protected) => Err(ActionError::Protected(target_ref.into())),
            Some(Refusal::ReparsePoint) => Err(ActionError::ReparsePoint(target_ref.into())),
            Some(Refusal::PermissionDenied) => Err(ActionError::Resolve {
                target_ref: target_ref.into(),
                status: Status::PermissionDenied,
            }),
            Some(Refusal::Unverifiable { status }) => Err(ActionError::Resolve {
                target_ref: target_ref.into(),
                status,
            }),
            Some(_) => Err(ActionError::IdentityChanged(target_ref.into())),
        }
    }

    fn check_file(&self, file: &FileTarget) -> Result<FileIdentity, Refusal> {
        if !is_windows_absolute(&file.path) || self.protection.is_protected(&file.path) {
            return Err(Refusal::Protected);
        }
        if file.identity.is_reparse_point {
            return Err(Refusal::ReparsePoint);
        }
        let observed = match self.guard.observe_file(&file.path) {
            Ok(Some(identity)) => identity,
            Ok(None) => return Err(Refusal::TargetMissing),
            Err(Status::PermissionDenied) => return Err(Refusal::PermissionDenied),
            Err(status) => return Err(Refusal::Unverifiable { status }),
        };
        if observed.is_reparse_point {
            return Err(Refusal::ReparsePoint);
        }
        if observed != file.identity {
            return Err(Refusal::TargetReplaced);
        }
        Ok(observed)
    }
    fn check_process(&self, process: &ProcessTarget) -> Result<ProcessIdentity, Refusal> {
        match self.guard.observe_process(process.identity.pid) {
            Ok(Some(observed)) if process.identity.same_instance(&observed) => Ok(observed),
            Ok(Some(_)) => Err(Refusal::ProcessReplaced),
            Ok(None) => Err(Refusal::TargetMissing),
            Err(Status::PermissionDenied) => Err(Refusal::PermissionDenied),
            Err(status) => Err(Refusal::Unverifiable { status }),
        }
    }
    fn revalidate(&self, target: &ResolvedTarget) -> Result<PreState, Refusal> {
        match target {
            ResolvedTarget::File(f) => self.check_file(f).map(|identity| PreState::File {
                path: f.path.clone(),
                identity,
            }),
            ResolvedTarget::Process(p) => self
                .check_process(p)
                .map(|identity| PreState::Process { identity }),
            ResolvedTarget::SettingsPage { page } => Ok(PreState::SettingsPage { page: *page }),
            ResolvedTarget::CleanupAdapter(c) => match self.synthetic_cache {
                Some(cache) if cache.root() == c.root => match cache.inventory() {
                    Ok(inv) => Ok(PreState::CleanupAdapter {
                        files: inv.files,
                        bytes: inv.bytes,
                    }),
                    Err(_) => Err(Refusal::AdapterPreconditionFailed),
                },
                _ => Err(Refusal::AdapterPreconditionFailed),
            },
        }
    }

    fn apply(&self, kind: ActionKind, target: &ResolvedTarget, permit: &Permit) -> ItemOutcome {
        let effect = match (kind, target) {
            (ActionKind::OpenInExplorer, ResolvedTarget::File(f)) => {
                self.effects.open_in_explorer(permit, f)
            }
            (ActionKind::OpenSettingsPage, ResolvedTarget::SettingsPage { page }) => {
                self.effects.open_settings_page(permit, *page)
            }
            (ActionKind::RequestNormalClose, ResolvedTarget::Process(p)) => {
                self.effects.request_normal_close(permit, p)
            }
            (ActionKind::RecycleSelectedFiles, ResolvedTarget::File(f)) => {
                match self.effects.recycle_file(permit, f) {
                    Ok(EffectOutcome::Done(detail)) => {
                        return match self.guard.observe_file(&f.path) {
                            Ok(Some(id)) if id == f.identity => ItemOutcome::Failed {
                                status: Status::Error,
                                detail: "target still present after the recycle operation".into(),
                            },
                            _ => ItemOutcome::Completed {
                                detail,
                                bytes_removed: Some(0),
                            },
                        };
                    }
                    other => other,
                }
            }
            (ActionKind::RunCleanupAdapter, ResolvedTarget::CleanupAdapter(_)) => {
                return match self.synthetic_cache.map(SyntheticDemoCache::clean) {
                    Some(Ok(removed)) => ItemOutcome::Completed {
                        detail: Some(format!("{} synthetic files removed", removed.files)),
                        bytes_removed: removed.bytes,
                    },
                    Some(Err(e)) => ItemOutcome::Failed {
                        status: Status::Error,
                        detail: format!("synthetic cache cleanup failed: {}", e.kind()),
                    },
                    None => ItemOutcome::Refused {
                        reason: Refusal::AdapterPreconditionFailed,
                    },
                };
            }
            _ => Err(EffectError::new(Status::Error, "target kind mismatch")),
        };
        match effect {
            Ok(EffectOutcome::Done(detail)) => ItemOutcome::Completed {
                detail,
                bytes_removed: None,
            },
            Ok(EffectOutcome::Refused(reason)) => ItemOutcome::Refused { reason },
            Err(e) => ItemOutcome::Failed {
                status: e.status,
                detail: e.detail,
            },
        }
    }

    /// Executes an approved plan item by item. The token is consumed on first use, even if
    /// execution is then refused. Items are journaled before and after each effect.
    pub fn execute(
        &self,
        plan: &mut ActionPlan,
        token: &ApprovalToken,
        session: &LocalSession,
        cancel: &AtomicBool,
        before_measurement_ref: Option<String>,
    ) -> Result<ActionResult, ActionError> {
        plan.verify_fingerprint()?;
        let kind = plan.action_kind();
        self.require_executable(kind)?;
        let consumed = session.consume(token, plan);
        if plan.check_expiry() {
            return Err(ActionError::Expired);
        }
        consumed.map_err(ActionError::ApprovalRejected)?;
        plan.transition(PlanState::Executing)?;
        let started = unix_ms();
        let permit = Permit(());
        let record = |index: usize, item: &PlanItem, phase, pre_state, outcome| JournalRecord {
            schema_version: SCHEMA_VERSION,
            plan_id: plan.plan_id().into(),
            fingerprint: plan.fingerprint().into(),
            item_index: index,
            target_ref: item.target_ref.clone(),
            phase,
            pre_state,
            outcome,
            at_unix_ms: unix_ms(),
        };
        let mut results = Vec::with_capacity(plan.items().len());
        let mut halted = None;
        for (index, item) in plan.items().iter().enumerate() {
            if halted.is_none() && cancel.load(Ordering::SeqCst) {
                halted = Some(SkipReason::Cancelled);
            }
            let outcome = if let Some(reason) = halted {
                ItemOutcome::Skipped { reason }
            } else {
                match self.revalidate(&item.target) {
                    Err(reason) => {
                        let outcome = ItemOutcome::Refused { reason };
                        let rec = record(
                            index,
                            item,
                            JournalPhase::Finished,
                            None,
                            Some(outcome.clone()),
                        );
                        if self.journal.append(&rec).is_err() {
                            halted = Some(SkipReason::JournalUnavailable);
                        }
                        outcome
                    }
                    Ok(pre) => {
                        let rec = record(index, item, JournalPhase::Started, Some(pre), None);
                        if self.journal.append(&rec).is_err() {
                            halted = Some(SkipReason::JournalUnavailable);
                            ItemOutcome::Skipped {
                                reason: SkipReason::JournalUnavailable,
                            }
                        } else {
                            let outcome = self.apply(kind, &item.target, &permit);
                            let rec = record(
                                index,
                                item,
                                JournalPhase::Finished,
                                None,
                                Some(outcome.clone()),
                            );
                            if self.journal.append(&rec).is_err() {
                                halted = Some(SkipReason::JournalUnavailable);
                            }
                            outcome
                        }
                    }
                }
            };
            results.push(ItemResult {
                item_index: index,
                target_ref: item.target_ref.clone(),
                outcome,
            });
        }
        let finished = unix_ms();
        plan.transition(PlanState::Verifying)?;
        let completed = results
            .iter()
            .filter(|r| matches!(r.outcome, ItemOutcome::Completed { .. }))
            .count();
        let cancelled = halted == Some(SkipReason::Cancelled);
        let (overall, state) = if completed == results.len() {
            (OverallOutcome::Completed, PlanState::Completed)
        } else if completed > 0 {
            (OverallOutcome::Partial, PlanState::Partial)
        } else if cancelled {
            (OverallOutcome::Cancelled, PlanState::Cancelled)
        } else {
            (OverallOutcome::Failed, PlanState::Failed)
        };
        plan.transition(state)?;
        Ok(ActionResult {
            schema_version: SCHEMA_VERSION,
            plan_id: plan.plan_id().into(),
            fingerprint: plan.fingerprint().into(),
            action_kind: kind,
            overall,
            items: results,
            measurement: MeasurementHooks {
                before_ref: before_measurement_ref,
                operation_started_at_unix_ms: started,
                operation_finished_at_unix_ms: finished,
                after_ref: None,
                after_recorded_at_unix_ms: None,
            },
        })
    }
}

#[cfg(all(test, not(windows)))]
pub(crate) fn test_permit() -> Permit {
    Permit(())
}
