//! Action proposals in the window. The model can only propose a plan with `prepare_action_plan`;
//! the plans of the last turn wait here. Nothing runs until the person clicks to approve one exact
//! plan in this window; the same broker, guard, journal and protection as the CLI then execute it,
//! with a measurement before and after.
use crate::failure::{self, Failure, Refusal};
use crate::view;
use pcagent_actions::{
    approve_locally, recover, ActionKind, Journal, LocalSession, ResolvedTarget,
};
use pcagent_agent::{verify_action, VERIFY_SAMPLES};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::Snapshot;
use pcagent_telemetry::Collector;
use pcagent_tools::{ActionDesk, PendingPlan, PolicyProtection};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Mutex},
    time::Duration,
};

/// How long an approval token stays valid: one click, run right away.
const APPROVAL_TTL: Duration = Duration::from_secs(60);
const SETTLE_AFTER_ACTION: Duration = Duration::from_secs(2);

/// The plans proposed during the last finished turn, and what the person decided for each.
pub struct ActionSlot {
    pub turn_id: String,
    /// What the person reviews, frozen when the turn finished.
    pub views: Vec<view::PlanView>,
    /// Plans not yet run; a plan leaves this list when it runs, so it can run only once.
    pub pending: Vec<PendingPlan>,
    pub decided: HashMap<String, String>,
}
impl ActionSlot {
    /// Built on the worker thread that prepared the plans: reading readable program names opens
    /// processes, which the window's own thread never does.
    fn new(turn_id: &str, pending: Vec<PendingPlan>, readable: view::Readable<'_>) -> Self {
        Self {
            turn_id: turn_id.to_owned(),
            views: pending
                .iter()
                .map(|p| view::plan(&p.plan, None, readable))
                .collect(),
            pending,
            decided: HashMap::new(),
        }
    }
}

#[derive(serde::Serialize)]
pub struct StartupRestoration {
    /// Local action group accepted by action_plans/action_run; not a conversation turn.
    pub turn_id: String,
    pub plan: view::PlanView,
}

pub type Slot = Mutex<Option<ActionSlot>>;

/// A local browser inspection can prepare an action without contacting the model.
pub fn prepare_browser_manager(
    slot: &Slot,
    instance: &pcagent_shared::ProcessInstance,
    readable: view::Readable<'_>,
) -> Result<StartupRestoration, Failure> {
    let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
    if slot.as_ref().is_some_and(|current| {
        current.decided.values().any(|v| v == "running")
            || current
                .pending
                .iter()
                .any(|p| !current.decided.contains_key(p.plan.plan_id()))
    }) {
        return Err(Failure::refused(
            Some(Refusal::Busy),
            "Review or dismiss the pending actions before preparing a browser manager.",
        ));
    }
    let desk = desk().ok_or_else(journal_unavailable)?;
    let (policy, prefs) = (Policy::from_env(), Preferences::default());
    let protection = PolicyProtection {
        policy: &policy,
        prefs: &prefs,
    };
    let pending = desk
        .prepare_browser_manager(instance, &protection)
        .map_err(|error| Failure::refused(Some(Refusal::Stale), error))?;
    let turn_id = format!("local-browser-{}", pending.plan.plan_id());
    let prepared = ActionSlot::new(&turn_id, vec![pending], readable);
    let plan = prepared.views[0].clone();
    *slot = Some(prepared);
    Ok(StartupRestoration { turn_id, plan })
}

/// Where the action journal lives, as for the CLI.
pub fn journal_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pcagent")
        .join("journal")
}

/// The desk offered to the model. Interrupted earlier actions are reported by the journal on
/// open; none is replayed.
pub fn desk() -> Option<ActionDesk> {
    let journal = Journal::open(journal_dir()).ok()?;
    let _ = recover(&journal);
    Some(ActionDesk::native(journal))
}

fn journal_unavailable() -> Failure {
    Failure::refused(
        Some(Refusal::Journal),
        "The action journal cannot be opened.",
    )
}

fn not_current(message: &str) -> Failure {
    Failure::refused(Some(Refusal::Stale), message)
}

/// Keeps a finished turn's proposals, replacing an older turn's: those can no longer be run.
pub fn keep(slot: &Slot, turn_id: &str, pending: Vec<PendingPlan>, readable: view::Readable<'_>) {
    let kept = ActionSlot::new(turn_id, pending, readable);
    *slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(kept);
}

/// Read a completed disable's local journal and prepare a fresh restoration for review. This
/// does not execute it, and refuses to replace the slot while another approved action is running.
pub fn prepare_startup_restore(
    slot: &Slot,
    source_plan_id: &str,
    item_index: usize,
    readable: view::Readable<'_>,
) -> Result<StartupRestoration, Failure> {
    let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
    if slot
        .as_ref()
        .is_some_and(|current| current.decided.values().any(|v| v == "running"))
    {
        return Err(Failure::refused(
            Some(Refusal::Busy),
            "An approved action is still running.",
        ));
    }
    let desk = desk().ok_or_else(journal_unavailable)?;
    let (policy, prefs) = (Policy::from_env(), Preferences::default());
    let protection = PolicyProtection {
        policy: &policy,
        prefs: &prefs,
    };
    let pending = desk
        .prepare_startup_restore(source_plan_id, item_index, &protection)
        .map_err(|error| Failure::refused(Some(Refusal::RestoreUnavailable), error))?;
    let turn_id = format!("local-restore-{}", pending.plan.plan_id());
    let restoration = ActionSlot::new(&turn_id, vec![pending], readable);
    let plan = restoration.views[0].clone();
    *slot = Some(restoration);
    Ok(StartupRestoration { turn_id, plan })
}

pub fn plans(slot: &Slot, turn_id: &str) -> Vec<view::PlanView> {
    let slot = slot.lock().unwrap_or_else(|p| p.into_inner());
    let Some(slot) = slot.as_ref().filter(|s| s.turn_id == turn_id) else {
        return Vec::new();
    };
    slot.views
        .iter()
        .map(|v| view::PlanView {
            decision: slot.decided.get(&v.plan_id).cloned(),
            ..v.clone()
        })
        .collect()
}

pub fn decline(slot: &Slot, turn_id: &str, plan_id: &str) -> Result<(), Failure> {
    let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
    let slot = slot
        .as_mut()
        .filter(|s| s.turn_id == turn_id)
        .ok_or_else(|| not_current("These proposals are no longer current."))?;
    if !slot.views.iter().any(|v| v.plan_id == plan_id) {
        return Err(not_current("Unknown proposal."));
    }
    slot.decided
        .entry(plan_id.to_owned())
        .or_insert_with(|| "declined".into());
    Ok(())
}

fn sample(collector: &mut Collector, cancel: &AtomicBool) -> Vec<Snapshot> {
    (0..VERIFY_SAMPLES)
        .take_while(|_| !cancel.load(std::sync::atomic::Ordering::SeqCst))
        .map(|_| collector.snapshot(false, 1))
        .collect()
}

fn free_bytes(root: &Path) -> Option<u64> {
    pcagent_storage::volume_space(root).free_bytes
}

/// Runs one plan of the current turn, once, after the person's click. The plan is taken out of
/// the slot first, so a second click cannot run it again.
pub fn run(slot: &Slot, turn_id: &str, plan_id: &str) -> Result<view::ActionOutcome, Failure> {
    let pending = {
        let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
        let slot = slot
            .as_mut()
            .filter(|s| s.turn_id == turn_id)
            .ok_or_else(|| not_current("These proposals are no longer current."))?;
        if slot.decided.contains_key(plan_id) {
            return Err(Failure::refused(
                Some(Refusal::AlreadyDecided),
                "This proposal was already decided.",
            ));
        }
        let index = slot
            .pending
            .iter()
            .position(|p| p.plan.plan_id() == plan_id)
            .ok_or_else(|| not_current("Unknown proposal."))?;
        slot.decided.insert(plan_id.to_owned(), "running".into());
        slot.pending.remove(index)
    };
    let outcome = execute(pending);
    let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(slot) = guard.as_mut().filter(|s| s.turn_id == turn_id) {
        let decision = match &outcome {
            Ok(o) => o.overall.clone(),
            Err(_) => "refused".into(),
        };
        slot.decided.insert(plan_id.to_owned(), decision);
    }
    outcome
}

fn execute(mut pending: PendingPlan) -> Result<view::ActionOutcome, Failure> {
    let desk = desk().ok_or_else(journal_unavailable)?;
    let (policy, prefs) = (Policy::from_env(), Preferences::default());
    let protection = PolicyProtection {
        policy: &policy,
        prefs: &prefs,
    };
    let session = LocalSession::new();
    let cancel = AtomicBool::new(false);
    let mut collector = Collector::new();
    let measure_system = pending.plan.action_kind() == ActionKind::RequestNormalClose;
    let volume_root = pending.plan.items().iter().find_map(|i| match &i.target {
        ResolvedTarget::CleanupAdapter(c) => Some(c.root.clone()),
        ResolvedTarget::RecycleCleanup(c) => Some(c.root().path.clone()),
        _ => None,
    });
    let before = if measure_system {
        sample(&mut collector, &cancel)
    } else {
        Vec::new()
    };
    let free_before = volume_root.as_deref().and_then(free_bytes);
    let token = approve_locally(&mut pending.plan, &session, APPROVAL_TTL)
        .map_err(|error| failure::action(&error))?;
    let mut plan = pending.plan.clone();
    let mut result = desk
        .broker(&pending, &protection)
        .execute(
            &mut plan,
            &token,
            &session,
            &cancel,
            before.last().map(|s| s.id.clone()),
        )
        .map_err(|error| failure::action(&error))?;
    if measure_system {
        std::thread::sleep(SETTLE_AFTER_ACTION);
    }
    let after = if measure_system {
        sample(&mut collector, &cancel)
    } else {
        Vec::new()
    };
    if let Some(last) = after.last() {
        result.record_after_measurement(last.id.clone());
    }
    let free_after = volume_root.as_deref().and_then(free_bytes);
    let verification = verify_action(
        &result,
        &before,
        &after,
        volume_root.map(|_| (free_before, free_after)),
    );
    Ok(view::outcome(&result, &verification))
}
