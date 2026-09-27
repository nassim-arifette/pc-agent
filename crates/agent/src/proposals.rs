//! Model-proposed action plans. The model may only ask for a plan to be *prepared*; approval and
//! execution happen afterwards in the trusted local UI with an [`pcagent_actions::ApprovalToken`].
use crate::tools::{Param, ParamKind, ToolSpec};
use pcagent_actions::{
    ActionIntent, ActionKind, ActionPlan, BaselineProtection, Broker, Effects, Guard, Journal,
    NativeEffects, NativeGuard, ProtectionCheck, ResolvedTarget, SyntheticDemoCache,
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
];
pub const SETTINGS_PAGES: &[&str] = &["storagesense", "appsfeatures", "startupapps"];

pub const PREPARE_ACTION_PLAN: ToolSpec = ToolSpec {
    name: "prepare_action_plan",
    description: "Propose one action for the user to review locally after the investigation. \
        Nothing runs now; you cannot approve or execute it. open_settings_page needs \
        settings_page. open_in_explorer needs scope_id + item_ref from get_largest_items or \
        get_cleanup_candidates. run_cleanup_adapter needs scope_id + the item_ref of a \
        get_cleanup_candidates entry (kind dirs) that has an adapter_id.",
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
        _ => None,
    }
}

/// Builds the plan from a locally resolved proposal and keeps it pending.
pub(crate) fn prepare(
    desk: &mut ActionDesk,
    proposal: Proposal,
    protection: &dyn ProtectionCheck,
) -> Result<pcagent_actions::PlanModelView, String> {
    // The same action on the same target is reviewed once, not once per proposal.
    if let Some(existing) = desk.pending.iter().find(|p| {
        p.plan.action_kind() == proposal.kind
            && p.plan
                .items()
                .iter()
                .any(|i| i.target_ref == proposal.target_ref)
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
        evidence_refs: Vec::new(),
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
