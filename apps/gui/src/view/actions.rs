//! Local action-plan review and approved-result presentation.
use super::{labels::readable_name, snake, Readable};
use pcagent_shared::ProcessInstance;
use serde::Serialize;
use serde_json::Value;

/// One target of a proposed action, as the person reviews it. Local paths may appear: this is
/// the local window, and the person must see exactly what would be touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetView {
    pub kind: String,
    pub label: String,
    /// For a process shown by its readable name: its executable name, the label's tooltip.
    pub executable_name: Option<String>,
    pub files: Option<u64>,
    pub bytes: Option<u64>,
    /// A process target's PID, so the window can name an unnamed process in its own language.
    pub pid: Option<u32>,
    /// A process target without an executable name: `label` holds the English fallback.
    pub unnamed: bool,
}
/// A proposal awaiting a decision in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanView {
    pub plan_id: String,
    pub kind: String,
    pub targets: Vec<TargetView>,
    pub recovery: String,
    pub checks: Vec<String>,
    /// `None` until the person declines or runs it; then `declined`, `running`, the outcome or
    /// `refused`.
    pub decision: Option<String>,
}
/// What an approved action did, with the measured before/after comparison.
#[derive(Debug, Clone, Serialize)]
pub struct ActionOutcome {
    pub plan_id: String,
    pub overall: String,
    pub items: Value,
    pub verification: Value,
}

/// A proposal as the person reviews it. A process target reads as the program it is ("Google
/// Chrome", its executable name as tooltip); its PID shows only when two targets would read the
/// same, or when no name is known at all.
pub fn plan(
    plan: &pcagent_actions::ActionPlan,
    decision: Option<&str>,
    readable: Readable<'_>,
) -> PlanView {
    use pcagent_actions::ResolvedTarget;
    let target = |kind: &str, label: String, files, bytes| TargetView {
        kind: kind.into(),
        label,
        executable_name: None,
        files,
        bytes,
        pid: None,
        unnamed: false,
    };
    // Each process target's readable and executable names, read once.
    let named: Vec<(Option<String>, Option<String>)> = plan
        .items()
        .iter()
        .map(|item| match &item.target {
            ResolvedTarget::Process(t) => {
                let instance = ProcessInstance {
                    entity_ref: String::new(),
                    pid: t.identity.pid,
                    creation_time_filetime: t.identity.creation_time_filetime,
                    boot_id: None,
                    executable_name: t.executable_name.clone(),
                };
                let executable = t
                    .executable_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned);
                (readable_name(&instance, readable), executable)
            }
            _ => (None, None),
        })
        .collect();
    let shown = |index: usize| {
        let (readable, executable) = &named[index];
        readable.as_deref().or(executable.as_deref())
    };
    let targets = plan
        .items()
        .iter()
        .enumerate()
        .map(|(index, item)| match &item.target {
            ResolvedTarget::File(f) => target("file", f.path.display().to_string(), Some(1), None),
            ResolvedTarget::SettingsPage { page } => {
                target("settings_page", page.uri().into(), None, None)
            }
            ResolvedTarget::CleanupAdapter(c) => target(
                "cleanup",
                c.root.display().to_string(),
                Some(c.files),
                c.bytes,
            ),
            ResolvedTarget::Process(t) => {
                let name = shown(index);
                let shared = name.is_some_and(|name| {
                    (0..named.len()).any(|other| {
                        other != index
                            && shown(other).is_some_and(|o| o.to_lowercase() == name.to_lowercase())
                    })
                });
                let label = match name {
                    Some(name) if !shared => name.to_owned(),
                    Some(name) => format!("{name} (PID {})", t.identity.pid),
                    None => format!("process (PID {})", t.identity.pid),
                };
                let (readable, executable) = &named[index];
                TargetView {
                    pid: Some(t.identity.pid),
                    unnamed: t.executable_name.is_none(),
                    executable_name: readable.as_ref().and(executable.clone()),
                    ..target("process", label, None, None)
                }
            }
            ResolvedTarget::UserStartup(t) => target("startup", t.value_name().into(), None, None),
            ResolvedTarget::RecycleCleanup(c) => target(
                "cleanup",
                c.root().path.display().to_string(),
                Some(c.files().len() as u64),
                c.bytes(),
            ),
        })
        .collect();
    PlanView {
        plan_id: plan.plan_id().to_owned(),
        kind: snake(&plan.action_kind()),
        targets,
        recovery: snake(&plan.recovery()),
        checks: plan.preconditions().iter().map(snake).collect(),
        decision: decision.map(str::to_owned),
    }
}

pub fn outcome(
    result: &pcagent_actions::ActionResult,
    verification: &pcagent_agent::ActionVerification,
) -> ActionOutcome {
    ActionOutcome {
        plan_id: result.plan_id.clone(),
        overall: snake(&result.overall),
        items: serde_json::to_value(&result.items).unwrap_or_default(),
        verification: serde_json::to_value(verification).unwrap_or_default(),
    }
}
