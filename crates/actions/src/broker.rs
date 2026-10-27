use crate::{
    approval::LocalSession,
    cleanup::{SyntheticDemoCache, SYNTHETIC_DEMO_CACHE_ID},
    is_native_absolute,
    journal::{Journal, JournalPhase, JournalRecord, PreState},
    plan::PlanParts,
    ActionError, ActionIntent, ActionKind, ActionPlan, ApprovalToken, FileIdentity, FileTarget,
    PlanItem, PlanState, Precondition, ProcessIdentity, ProcessTarget, Recovery,
    RecycleCleanupTarget, ResolvedTarget, SettingsPage, StartupIdentity, StartupTarget,
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
    fn observe_user_startup(&self, _value_name: &str) -> Result<Option<StartupIdentity>, Status> {
        Err(Status::Unsupported)
    }
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
    fn set_user_startup(
        &self,
        _permit: &Permit,
        _startup: &StartupTarget,
    ) -> Result<EffectOutcome, EffectError> {
        Err(EffectError::new(
            Status::Unsupported,
            "user startup changes are unsupported",
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    Protected,
    PermissionDenied,
    TargetMissing,
    TargetReplaced,
    ProcessReplaced,
    StartupEntryChanged,
    ReparsePoint,
    /// Another open of the file forbids the exclusive pin required before recycling.
    InUse,
    /// The file has more than one hard link; recycling one name would not remove the data.
    MultipleLinks,
    Unverifiable {
        status: Status,
    },
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
    Partial {
        status: Status,
        detail: String,
        files_removed: u64,
        bytes_removed: Option<u64>,
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
    /// A successful effect may still lack a durable completion record.
    pub journal_status: Status,
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
            ActionKind::RunCleanupAdapter if cfg!(windows) && self.synthetic_cache.is_some() => {
                Capability::Executable
            }
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
            if let ResolvedTarget::RecycleCleanup(cleanup) = target {
                for (index, file) in cleanup.split().enumerate() {
                    let identity = &file.files()[0].identity;
                    if !seen_files.insert((identity.volume_serial, identity.file_id)) {
                        return Err(ActionError::DuplicateTarget(target_ref.clone()));
                    }
                    items.push(PlanItem {
                        target_ref: format!("{}:file-{index:03}", file.kind().adapter_id()),
                        target: ResolvedTarget::RecycleCleanup(file),
                    });
                }
                continue;
            }
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
            ActionKind::RunCleanupAdapter
                if items
                    .iter()
                    .all(|item| matches!(item.target, ResolvedTarget::RecycleCleanup(_))) =>
            {
                (
                    Some(0),
                    Recovery::RecycleBinIfOperationSupported,
                    vec![
                        CleanupManifestUnchanged,
                        IdentityRevalidated,
                        NotProtected,
                        NotReparsePoint,
                        RecycleBinAvailable,
                    ],
                )
            }
            ActionKind::RunCleanupAdapter => (
                items.first().and_then(|i| match &i.target {
                    ResolvedTarget::CleanupAdapter(c) => c.bytes,
                    _ => None,
                }),
                Recovery::NoneSyntheticRegenerableData,
                vec![SyntheticCacheMarkerPresent],
            ),
            ActionKind::DisableUserStartup | ActionKind::RestoreUserStartup => (
                None,
                Recovery::RestorePreviousStartupApproval,
                vec![UserStartupEntryUnchanged, KnownStartupApprovalFormat],
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
            ActionKind::RunCleanupAdapter if target_ref == SYNTHETIC_DEMO_CACHE_ID => {
                let cache = self
                    .synthetic_cache
                    .ok_or_else(|| ActionError::UnknownAdapter(target_ref.into()))?;
                let manifest = cache
                    .manifest()
                    .map_err(|_| ActionError::UnknownAdapter(target_ref.into()))?;
                return Ok(ResolvedTarget::CleanupAdapter(manifest));
            }
            _ => {}
        }
        if kind == ActionKind::RunCleanupAdapter
            && crate::UserCleanupKind::from_ref(target_ref).is_none()
        {
            return Err(ActionError::UnknownAdapter(target_ref.into()));
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
            (
                ResolvedTarget::UserStartup(startup),
                ActionKind::DisableUserStartup | ActionKind::RestoreUserStartup,
            ) => {
                startup.validate(kind)?;
                self.check_startup(startup).err()
            }
            (ResolvedTarget::RecycleCleanup(cleanup), ActionKind::RunCleanupAdapter) => {
                if target_ref != cleanup.kind().adapter_id() {
                    return Err(mismatch());
                }
                self.check_cleanup(cleanup).err()
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
        if !is_native_absolute(&file.path) || self.protection.is_protected(&file.path) {
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
    fn check_startup(&self, startup: &StartupTarget) -> Result<StartupIdentity, Refusal> {
        match self.guard.observe_user_startup(startup.value_name()) {
            Ok(Some(observed)) if observed == *startup.identity() => Ok(observed),
            Ok(Some(_)) => Err(Refusal::StartupEntryChanged),
            Ok(None) => Err(Refusal::TargetMissing),
            Err(Status::PermissionDenied) => Err(Refusal::PermissionDenied),
            Err(status) => Err(Refusal::Unverifiable { status }),
        }
    }
    fn check_cleanup(&self, cleanup: &RecycleCleanupTarget) -> Result<(), Refusal> {
        cleanup.validate_directories(self.guard, self.protection)?;
        for file in cleanup.files() {
            self.check_file(file)?;
        }
        Ok(())
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
            ResolvedTarget::UserStartup(startup) => {
                self.check_startup(startup)?;
                Ok(PreState::UserStartup {
                    target: startup.clone(),
                })
            }
            ResolvedTarget::RecycleCleanup(cleanup) => {
                self.check_cleanup(cleanup)?;
                Ok(PreState::RecycleCleanup {
                    target: cleanup.clone(),
                })
            }
            ResolvedTarget::CleanupAdapter(c) => match self.synthetic_cache {
                Some(cache) if cache.root() == c.root => match cache.validate(c) {
                    Ok(()) => Ok(PreState::CleanupAdapter {
                        files: c.files,
                        bytes: c.bytes,
                    }),
                    Err(_) => Err(Refusal::AdapterPreconditionFailed),
                },
                _ => Err(Refusal::AdapterPreconditionFailed),
            },
        }
    }

    fn apply(
        &self,
        kind: ActionKind,
        target: &ResolvedTarget,
        permit: &Permit,
        cancel: &AtomicBool,
    ) -> ItemOutcome {
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
            (
                ActionKind::DisableUserStartup | ActionKind::RestoreUserStartup,
                ResolvedTarget::UserStartup(startup),
            ) => {
                if startup.validate(kind).is_err() {
                    return ItemOutcome::Refused {
                        reason: Refusal::StartupEntryChanged,
                    };
                }
                match self.effects.set_user_startup(permit, startup) {
                    Ok(EffectOutcome::Done(detail)) => {
                        return match self.guard.observe_user_startup(startup.value_name()) {
                            Ok(Some(observed)) if observed.run_key_last_write_filetime == startup.identity().run_key_last_write_filetime
                                && &observed.approval == startup.desired_approval() => ItemOutcome::Completed { detail, bytes_removed: None },
                            _ => ItemOutcome::Failed { status: Status::Error, detail: "startup approval could not be verified after the operation; inspect its journal before restoring".into() },
                        };
                    }
                    other => other,
                }
            }
            (ActionKind::RunCleanupAdapter, ResolvedTarget::RecycleCleanup(cleanup)) => {
                let Some(file) = cleanup
                    .files()
                    .first()
                    .filter(|_| cleanup.files().len() == 1)
                else {
                    return ItemOutcome::Refused {
                        reason: Refusal::AdapterPreconditionFailed,
                    };
                };
                return self.recycle(permit, file);
            }
            (ActionKind::RecycleSelectedFiles, ResolvedTarget::File(f)) => {
                return self.recycle(permit, f);
            }
            (ActionKind::RunCleanupAdapter, ResolvedTarget::CleanupAdapter(c)) => {
                return match self.synthetic_cache {
                    Some(cache) => cache.clean(c, cancel),
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
    fn recycle(&self, permit: &Permit, file: &FileTarget) -> ItemOutcome {
        match self.effects.recycle_file(permit, file) {
            Ok(EffectOutcome::Done(detail)) => match self.guard.observe_file(&file.path) {
                Ok(None) => ItemOutcome::Completed {
                    detail,
                    bytes_removed: Some(0),
                },
                Ok(Some(identity)) if !crate::same_object(&identity, &file.identity) => {
                    ItemOutcome::Completed {
                        detail,
                        bytes_removed: Some(0),
                    }
                }
                _ => ItemOutcome::Failed {
                    status: Status::Error,
                    detail:
                        "target disappearance could not be verified after the recycle operation"
                            .into(),
                },
            },
            Ok(EffectOutcome::Refused(reason)) => ItemOutcome::Refused { reason },
            Err(error) => ItemOutcome::Failed {
                status: error.status,
                detail: error.detail,
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
                            let outcome = self.apply(kind, &item.target, &permit, cancel);
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
        let cancelled = halted == Some(SkipReason::Cancelled) || cancel.load(Ordering::SeqCst);
        let partial_effect = results
            .iter()
            .any(|r| matches!(r.outcome, ItemOutcome::Partial { files_removed, .. } if files_removed > 0));
        let journal_status = if halted == Some(SkipReason::JournalUnavailable) {
            Status::Error
        } else {
            Status::Ok
        };
        let (overall, state) = if completed == results.len() && journal_status == Status::Ok {
            (OverallOutcome::Completed, PlanState::Completed)
        } else if completed > 0 || partial_effect {
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
            journal_status,
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

#[cfg(test)]
pub(crate) fn test_permit() -> Permit {
    Permit(())
}
