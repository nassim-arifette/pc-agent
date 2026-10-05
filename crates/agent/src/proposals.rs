//! Model-proposed action plans. The model may only ask for a plan to be *prepared*; approval and
//! execution happen afterwards in the trusted local UI with an [`pcagent_actions::ApprovalToken`].
use crate::tools::{Args, Param, ParamKind, ToolSpec};
use pcagent_actions::{
    ActionIntent, ActionKind, ActionPlan, BaselineProtection, Broker, Effects, Guard, Journal,
    NativeEffects, NativeGuard, ProtectionCheck, ResolvedTarget, StartupTarget, SyntheticDemoCache,
    TargetResolver, DEFAULT_PLAN_TTL, SYNTHETIC_DEMO_CACHE_ID,
};
use pcagent_policy::{NormalizedPath, Policy, Preferences};
use pcagent_shared::Status;
use std::{collections::HashMap, path::Path};

/// Plans a single investigation may leave for local review.
pub const MAX_PENDING_PLANS: usize = 3;

pub const PROPOSABLE_KINDS: &[&str] = &[
    "open_in_explorer",
    "open_settings_page",
    "run_cleanup_adapter",
    "disable_user_startup",
    "request_normal_close",
    "recycle_selected_files",
];
pub const SETTINGS_PAGES: &[&str] = &[
    "storagesense",
    "appsfeatures",
    "startupapps",
    "windowsupdate",
    "windowsupdate-optionalupdates",
    "powersleep",
    "batterysaver-usagedetails",
];
pub const USER_CLEANUP_ADAPTERS: &[&str] = &["user.temp", "nvidia.dxcache", "windows.d3dscache"];

pub const PREPARE_ACTION_PLAN: ToolSpec = ToolSpec {
    name: "prepare_action_plan",
    class: crate::tools::ToolClass::Proposal,
    description: "Propose one action for the user to review locally after the investigation. \
        Nothing runs now; you cannot approve or execute it. open_settings_page needs \
        settings_page. open_in_explorer needs scope_id + item_ref from get_largest_items or \
        get_cleanup_candidates. recycle_selected_files needs a file scope_id + item_ref listed \
        as proposable_actions by those tools. request_normal_close needs process_ref from \
        get_processes. disable_user_startup needs startup_ref from get_startup_programs. \
        run_cleanup_adapter needs adapter_id from get_cleanup_opportunities, or scope_id + \
        item_ref of a validated synthetic demo directory from get_cleanup_candidates. Use \
        only locally issued references; never send a path, command, PID or registry name.",
    params: &[
        Param {
            name: "action_kind",
            description: "Action to propose.",
            kind: ParamKind::OneOf(PROPOSABLE_KINDS),
            required: true,
        },
        Param {
            name: "scope_id",
            description: "Opaque storage scope of item_ref.",
            kind: ParamKind::OpaqueRef { prefix: "scope-" },
            required: false,
        },
        Param {
            name: "item_ref",
            description: "Opaque item reference from get_largest_items or get_cleanup_candidates.",
            kind: ParamKind::OpaqueRef { prefix: "scan-" },
            required: false,
        },
        Param {
            name: "settings_page",
            description: "Allowlisted Windows Settings page.",
            kind: ParamKind::OneOf(SETTINGS_PAGES),
            required: false,
        },
        Param {
            name: "startup_ref",
            description: "Opaque current-user Run target listed by get_startup_programs.",
            kind: ParamKind::OpaqueRef { prefix: "startup-" },
            required: false,
        },
        Param {
            name: "adapter_id",
            description: "Finite adapter listed as proposable by get_cleanup_opportunities.",
            kind: ParamKind::OneOf(USER_CLEANUP_ADAPTERS),
            required: false,
        },
        Param {
            name: "process_ref",
            description: "Opaque process instance listed as proposable by get_processes.",
            kind: ParamKind::OpaqueRef { prefix: "proc-" },
            required: false,
        },
    ],
};

/// Protected locations and user exclusions from the policy crate, plus the actions baseline.
pub struct PolicyProtection<'a> {
    pub policy: &'a Policy,
    pub prefs: &'a Preferences,
}
impl ProtectionCheck for PolicyProtection<'_> {
    fn is_protected(&self, path: &Path) -> bool {
        BaselineProtection.is_protected(path)
            || path
                .to_str()
                .and_then(|s| NormalizedPath::parse(s).ok())
                .is_none_or(|p| self.policy.check_target(&p, self.prefs).is_forbidden())
    }
}

/// Resolver over targets that were resolved locally when the plan was prepared.
#[derive(Default)]
pub struct FixedTargets(HashMap<String, ResolvedTarget>);
impl TargetResolver for FixedTargets {
    fn resolve(&self, target_ref: &str) -> Result<ResolvedTarget, Status> {
        self.0.get(target_ref).cloned().ok_or(Status::Stale)
    }
}

/// A plan awaiting local review, with everything the local UI needs to execute it.
pub struct PendingPlan {
    pub plan: ActionPlan,
    pub targets: FixedTargets,
    pub synthetic_cache: Option<SyntheticDemoCache>,
}

/// Local services used to prepare plans. Execution uses the same services from the local UI.
pub struct ActionDesk {
    pub journal: Journal,
    pub guard: Box<dyn Guard>,
    pub effects: Box<dyn Effects>,
    pub(crate) pending: Vec<PendingPlan>,
}
impl ActionDesk {
    pub fn native(journal: Journal) -> Self {
        Self::new(
            journal,
            Box::new(NativeGuard),
            Box::new(NativeEffects::default()),
        )
    }
    pub fn new(journal: Journal, guard: Box<dyn Guard>, effects: Box<dyn Effects>) -> Self {
        Self {
            journal,
            guard,
            effects,
            pending: Vec::new(),
        }
    }
    pub fn take_pending(&mut self) -> Vec<PendingPlan> {
        std::mem::take(&mut self.pending)
    }
    /// Local recovery entry point; deliberately absent from the model's proposal schema.
    pub fn prepare_startup_restore(
        &self,
        source_plan_id: &str,
        item_index: usize,
        protection: &dyn ProtectionCheck,
    ) -> Result<PendingPlan, String> {
        let target = StartupTarget::restore_from_journal(
            &self.journal,
            source_plan_id,
            item_index,
            self.guard.as_ref(),
        )
        .map_err(|error| error.to_string())?;
        let target_ref = "startup-restore".to_owned();
        let targets = FixedTargets(HashMap::from([(
            target_ref.clone(),
            ResolvedTarget::UserStartup(target),
        )]));
        let plan = self
            .broker_for(&targets, None, protection)
            .prepare_plan(&ActionIntent {
                action_kind: ActionKind::RestoreUserStartup,
                target_refs: vec![target_ref],
                evidence_refs: vec![source_plan_id.into()],
            })
            .map_err(|error| error.to_string())?;
        Ok(PendingPlan {
            plan,
            targets,
            synthetic_cache: None,
        })
    }
    /// Broker for one pending plan; only the local UI may call `execute` on it.
    pub fn broker<'a>(
        &'a self,
        pending: &'a PendingPlan,
        protection: &'a dyn ProtectionCheck,
    ) -> Broker<'a> {
        self.broker_for(
            &pending.targets,
            pending.synthetic_cache.as_ref(),
            protection,
        )
    }
    fn broker_for<'a>(
        &'a self,
        targets: &'a FixedTargets,
        synthetic_cache: Option<&'a SyntheticDemoCache>,
        protection: &'a dyn ProtectionCheck,
    ) -> Broker<'a> {
        Broker {
            resolver: targets,
            guard: self.guard.as_ref(),
            protection,
            effects: self.effects.as_ref(),
            journal: &self.journal,
            synthetic_cache,
            plan_ttl: DEFAULT_PLAN_TTL,
        }
    }
}

pub(crate) struct Proposal {
    pub kind: ActionKind,
    pub target_ref: String,
    pub target: Option<ResolvedTarget>,
    pub synthetic_cache: Option<SyntheticDemoCache>,
}

pub(crate) fn parse_kind(text: &str) -> Option<ActionKind> {
    match text {
        "open_in_explorer" => Some(ActionKind::OpenInExplorer),
        "open_settings_page" => Some(ActionKind::OpenSettingsPage),
        "run_cleanup_adapter" => Some(ActionKind::RunCleanupAdapter),
        "disable_user_startup" => Some(ActionKind::DisableUserStartup),
        "request_normal_close" => Some(ActionKind::RequestNormalClose),
        "recycle_selected_files" => Some(ActionKind::RecycleSelectedFiles),
        _ => None,
    }
}

pub(crate) fn validate_arguments(kind: ActionKind, args: &Args) -> Result<(), String> {
    let allowed: &[&str] = match kind {
        ActionKind::OpenSettingsPage => &["settings_page"],
        ActionKind::OpenInExplorer | ActionKind::RecycleSelectedFiles => &["scope_id", "item_ref"],
        ActionKind::RequestNormalClose => &["process_ref"],
        ActionKind::DisableUserStartup => &["startup_ref"],
        ActionKind::RunCleanupAdapter if args.text("adapter_id").is_some() => &["adapter_id"],
        ActionKind::RunCleanupAdapter => &["scope_id", "item_ref"],
        _ => return Err("unsupported action_kind".into()),
    };
    if args.to_value().as_object().is_none_or(|fields| {
        fields
            .keys()
            .any(|key| key != "action_kind" && !allowed.contains(&key.as_str()))
    }) {
        return Err("arguments do not match the requested action_kind".into());
    }
    if allowed.iter().any(|field| args.text(field).is_none()) {
        return Err("required local target reference is missing".into());
    }
    Ok(())
}

/// Builds the plan from a locally resolved proposal and keeps it pending.
pub(crate) fn prepare(
    desk: &mut ActionDesk,
    proposal: Proposal,
    protection: &dyn ProtectionCheck,
    evidence_refs: Vec<String>,
) -> Result<pcagent_actions::PlanModelView, String> {
    // The same action on the same target is reviewed once, not once per proposal.
    if let Some(existing) = desk.pending.iter().find(|p| {
        p.plan.action_kind() == proposal.kind
            && (p.targets.0.contains_key(&proposal.target_ref)
                || p.plan
                    .items()
                    .iter()
                    .any(|i| i.target_ref == proposal.target_ref))
            && proposal
                .target
                .as_ref()
                .is_none_or(|target| p.targets.0.get(&proposal.target_ref) == Some(target))
    }) {
        return Ok(existing.plan.model_view());
    }
    if desk.pending.len() >= MAX_PENDING_PLANS {
        return Err(format!(
            "at most {MAX_PENDING_PLANS} plans per investigation"
        ));
    }
    let mut targets = FixedTargets::default();
    if let Some(t) = proposal.target {
        targets.0.insert(proposal.target_ref.clone(), t);
    }
    let intent = ActionIntent {
        action_kind: proposal.kind,
        target_refs: vec![proposal.target_ref],
        evidence_refs,
    };
    let plan = desk
        .broker_for(&targets, proposal.synthetic_cache.as_ref(), protection)
        .prepare_plan(&intent)
        .map_err(|e| e.to_string())?;
    let view = plan.model_view();
    desk.pending.push(PendingPlan {
        plan,
        targets,
        synthetic_cache: proposal.synthetic_cache,
    });
    Ok(view)
}

pub(crate) const CLEANUP_TARGET_REF: &str = SYNTHETIC_DEMO_CACHE_ID;

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_actions::SettingsPage;

    #[test]
    fn local_journal_restoration_builds_a_separate_approved_plan_without_model_authority() {
        use pcagent_actions::{
            approve_locally, Capability, EffectError, EffectOutcome, FileIdentity, FileTarget,
            LocalSession, OverallOutcome, Permit, PlanState, ProcessIdentity, ProcessTarget,
            StartupApproval, StartupIdentity,
        };
        use std::sync::{Arc, Mutex};
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        #[derive(Clone)]
        struct Fake(Arc<Mutex<(StartupIdentity, usize)>>);
        impl Guard for Fake {
            fn observe_file(&self, _: &Path) -> Result<Option<FileIdentity>, Status> {
                Ok(None)
            }
            fn observe_process(&self, _: u32) -> Result<Option<ProcessIdentity>, Status> {
                Ok(None)
            }
            fn observe_user_startup(&self, name: &str) -> Result<Option<StartupIdentity>, Status> {
                assert_eq!(name, "Fixture App");
                Ok(Some(self.0.lock().unwrap().0.clone()))
            }
        }
        impl Effects for Fake {
            fn capability(&self, kind: ActionKind) -> Capability {
                if matches!(
                    kind,
                    ActionKind::DisableUserStartup | ActionKind::RestoreUserStartup
                ) {
                    Capability::Executable
                } else {
                    Capability::Unsupported
                }
            }
            fn open_in_explorer(
                &self,
                _: &Permit,
                _: &FileTarget,
            ) -> Result<EffectOutcome, EffectError> {
                unreachable!()
            }
            fn open_settings_page(
                &self,
                _: &Permit,
                _: SettingsPage,
            ) -> Result<EffectOutcome, EffectError> {
                unreachable!()
            }
            fn request_normal_close(
                &self,
                _: &Permit,
                _: &ProcessTarget,
            ) -> Result<EffectOutcome, EffectError> {
                unreachable!()
            }
            fn recycle_file(
                &self,
                _: &Permit,
                _: &FileTarget,
            ) -> Result<EffectOutcome, EffectError> {
                unreachable!()
            }
            fn set_user_startup(
                &self,
                _: &Permit,
                target: &StartupTarget,
            ) -> Result<EffectOutcome, EffectError> {
                let mut state = self.0.lock().unwrap();
                state.0.approval = target.desired_approval().clone();
                state.1 += 1;
                Ok(EffectOutcome::Done(None))
            }
        }
        let path = std::env::temp_dir().join(format!(
            "pcagent-restore-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        let fake = Fake(Arc::new(Mutex::new((
            StartupIdentity {
                run_key_last_write_filetime: 1,
                approval: StartupApproval::Missing,
            },
            0,
        ))));
        let desk = ActionDesk::new(
            Journal::open(&path).unwrap(),
            Box::new(fake.clone()),
            Box::new(fake.clone()),
        );
        let targets = FixedTargets(HashMap::from([(
            "startup-fixture".into(),
            ResolvedTarget::UserStartup(StartupTarget::disable("Fixture App", &fake).unwrap()),
        )]));
        let protection = BaselineProtection;
        let plan = desk
            .broker_for(&targets, None, &protection)
            .prepare_plan(&ActionIntent {
                action_kind: ActionKind::DisableUserStartup,
                target_refs: vec!["startup-fixture".into()],
                evidence_refs: vec![],
            })
            .unwrap();
        let mut pending = PendingPlan {
            plan,
            targets,
            synthetic_cache: None,
        };
        let source_plan_id = pending.plan.plan_id().to_owned();
        let session = LocalSession::new();
        let token = approve_locally(&mut pending.plan, &session, Duration::from_secs(60)).unwrap();
        let mut executable = pending.plan.clone();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            desk.broker(&pending, &protection)
                .execute(&mut executable, &token, &session, &cancel, None)
                .unwrap()
                .overall,
            OverallOutcome::Completed
        );
        let mut restore = desk
            .prepare_startup_restore(&source_plan_id, 0, &protection)
            .unwrap();
        assert_eq!(restore.plan.state(), PlanState::AwaitingApproval);
        assert_eq!(restore.plan.action_kind(), ActionKind::RestoreUserStartup);
        assert_ne!(restore.plan.plan_id(), source_plan_id);
        assert_eq!(fake.0.lock().unwrap().1, 1);
        assert!(!serde_json::to_string(&restore.plan.model_view())
            .unwrap()
            .contains("Fixture App"));
        assert!(!PROPOSABLE_KINDS.contains(&"restore_user_startup"));
        let token = approve_locally(&mut restore.plan, &session, Duration::from_secs(60)).unwrap();
        let mut executable = restore.plan.clone();
        assert_eq!(
            desk.broker(&restore, &protection)
                .execute(&mut executable, &token, &session, &cancel, None)
                .unwrap()
                .overall,
            OverallOutcome::Completed
        );
        assert_eq!(fake.0.lock().unwrap().0.approval, StartupApproval::Missing);
        assert_eq!(fake.0.lock().unwrap().1, 2);
        assert!(desk
            .prepare_startup_restore(&source_plan_id, 0, &protection)
            .is_err());
        drop(desk);
        assert_eq!(
            path.canonicalize().unwrap().parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn settings_and_adapter_vocabularies_match_the_broker() {
        for page in SETTINGS_PAGES {
            assert!(SettingsPage::from_ref(page).is_some());
        }
        for adapter in USER_CLEANUP_ADAPTERS {
            assert!(pcagent_actions::UserCleanupKind::from_ref(adapter).is_some());
        }
        for kind in PROPOSABLE_KINDS {
            assert!(parse_kind(kind).is_some());
        }
    }

    #[test]
    fn each_action_requires_its_own_closed_target_arguments() {
        for valid in [
            r#"{"action_kind":"disable_user_startup","startup_ref":"startup-fixture"}"#,
            r#"{"action_kind":"run_cleanup_adapter","adapter_id":"user.temp"}"#,
            r#"{"action_kind":"request_normal_close","process_ref":"proc-fixture"}"#,
            r#"{"action_kind":"recycle_selected_files","scope_id":"scope-fixture","item_ref":"scan-fixture:1"}"#,
            r#"{"action_kind":"open_settings_page","settings_page":"windowsupdate-optionalupdates"}"#,
            r#"{"action_kind":"open_settings_page","settings_page":"batterysaver-usagedetails"}"#,
        ] {
            let args = PREPARE_ACTION_PLAN.validate(valid).unwrap();
            validate_arguments(
                parse_kind(args.text("action_kind").unwrap()).unwrap(),
                &args,
            )
            .unwrap();
        }
        for invalid in [
            r#"{"action_kind":"disable_user_startup","startup_ref":"startup-fixture","settings_page":"startupapps"}"#,
            r#"{"action_kind":"run_cleanup_adapter","adapter_id":"user.temp","scope_id":"scope-fixture","item_ref":"scan-fixture:1"}"#,
            r#"{"action_kind":"request_normal_close"}"#,
        ] {
            let args = PREPARE_ACTION_PLAN.validate(invalid).unwrap();
            assert!(validate_arguments(
                parse_kind(args.text("action_kind").unwrap()).unwrap(),
                &args
            )
            .is_err());
        }
        for invalid in [
            r#"{"action_kind":"disable_user_startup","value_name":"PRIVATE-NAME"}"#,
            r#"{"action_kind":"run_cleanup_adapter","adapter_id":"arbitrary.cache"}"#,
            r#"{"action_kind":"recycle_selected_files","path":"C:\\PRIVATE\\file"}"#,
            r#"{"action_kind":"request_normal_close","pid":42}"#,
            r#"{"action_kind":"open_settings_page","settings_page":"windowsupdate","command":"PAYLOAD"}"#,
        ] {
            assert!(PREPARE_ACTION_PLAN.validate(invalid).is_err());
        }
    }
}
