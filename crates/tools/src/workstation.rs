//! Read-only investigation tools backed by the real collectors, storage index and policy.
//! Every cloud payload is built from an explicit allowlist: PIDs, executable names, full paths,
//! boot IDs and collector session IDs stay local.
use crate::action_targets::{ActionTargets, ScannedFile};
use crate::proposals::{
    self, ActionDesk, PendingPlan, PolicyProtection, Proposal, CLEANUP_TARGET_REF,
    PREPARE_ACTION_PLAN,
};
use pcagent_actions::{ActionKind, FileTarget, ResolvedTarget, SyntheticDemoCache};
use pcagent_agent::{Args, ToolOutput, ToolSpec, Toolbox, MAX_PROCESSES};
use pcagent_policy::{Attributes, CandidateInput, Policy, Preferences, SYNTHETIC_CACHE_ID};
use pcagent_shared::{Origin, Status};
use pcagent_storage::{ItemKind, ScanJob, ScanOptions, StorageError, StorageService};
use pcagent_telemetry::Collector;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Local toolbox over the real collectors. Scopes must already be authorized by the user.
pub struct Workstation<'a> {
    pub collector: &'a mut Collector,
    pub storage: Arc<StorageService>,
    pub policy: &'a Policy,
    pub prefs: &'a Preferences,
    pub scan_options: ScanOptions,
    jobs: Vec<ScanJob>,
    specs: Vec<ToolSpec>,
    actions: Option<ActionDesk>,
    action_targets: ActionTargets,
    observation_refs: Vec<String>,
    storage_context: pcagent_agent::StorageContext,
    sharing: pcagent_agent::Sharing,
    history: Option<crate::history_tools::HistoryTools>,
    allowed_scopes: Option<Vec<String>>,
    elevated: Vec<crate::elevated_tools::ApprovedInspection>,
    external: Option<crate::external_tools::ExternalTools>,
    /// The host's probes once it restricted the turn to what works; `None` declares every tool.
    probes: Option<pcagent_agent::capabilities::Probes>,
}
impl<'a> Workstation<'a> {
    pub fn new(
        collector: &'a mut Collector,
        storage: Arc<StorageService>,
        policy: &'a Policy,
        prefs: &'a Preferences,
    ) -> Self {
        Self {
            collector,
            storage,
            policy,
            prefs,
            scan_options: ScanOptions::default(),
            jobs: Vec::new(),
            specs: Self::tool_specs(),
            actions: None,
            action_targets: ActionTargets::default(),
            observation_refs: Vec::new(),
            storage_context: pcagent_agent::StorageContext::default(),
            sharing: pcagent_agent::Sharing::Anonymized,
            history: None,
            allowed_scopes: None,
            elevated: Vec::new(),
            external: None,
            probes: None,
        }
    }
    /// Declare only what works in this turn (`capabilities`): the platform, the optional flows
    /// this toolbox was given (history, approved scopes, web lookups, proposals) and the host's
    /// probes of this PC. Absent tools are not declared; the others may take a variant whose text
    /// names nothing absent. The order of this and the other setters does not matter.
    pub fn offer_available(&mut self, probes: pcagent_agent::capabilities::Probes) {
        self.probes = Some(probes);
        self.declare();
    }
    /// The capabilities this turn's tools follow, or `None` while every tool is declared.
    pub fn capabilities(&self) -> Option<pcagent_agent::capabilities::Capabilities> {
        self.capabilities_on(cfg!(windows))
    }
    fn capabilities_on(&self, windows: bool) -> Option<pcagent_agent::capabilities::Capabilities> {
        let probes = self.probes?;
        let flows = pcagent_agent::capabilities::Flows {
            history_sampler: self.history.is_some(),
            storage_scopes: self.storage.scopes().iter().any(|scope| {
                self.allowed_scopes
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&scope.scope_id))
            }),
            web_search: self.external.is_some(),
            action_proposals: self.actions.is_some(),
        };
        Some(pcagent_agent::capabilities::Capabilities::of_turn(
            windows, flows, probes,
        ))
    }
    /// The fixed catalog, then the enabled web lookups and proposals, restricted to this turn's
    /// capabilities once the host passed its probes.
    fn declare(&mut self) {
        let mut specs = Self::tool_specs();
        if self.external.is_some() {
            specs.extend(crate::external_tools::TOOLS);
        }
        if self.actions.is_some() {
            specs.push(PREPARE_ACTION_PLAN);
        }
        self.specs = match self.capabilities() {
            Some(capabilities) => pcagent_agent::capabilities::catalog(&specs, &capabilities),
            None => specs,
        };
    }
    /// Declares the typed web lookups (C6), only after the user's web search consent and with a
    /// key. Their queries are composed locally from this turn's measurements.
    pub fn enable_web_search(
        &mut self,
        searcher: Box<dyn crate::tavily::WebSearcher + Send>,
        country: Option<String>,
    ) {
        self.external = Some(crate::external_tools::ExternalTools::new(searcher, country));
        self.declare();
    }
    /// Tavily credits used by this turn's web lookups.
    pub fn web_credits(&self) -> f64 {
        self.external
            .as_ref()
            .map_or(0.0, |external| external.credits)
    }
    pub fn set_web_cache(&mut self, cache: pcagent_store::web_cache::WebCache) {
        if let Some(external) = self.external.take() {
            self.external = Some(external.with_cache(cache));
        }
    }
    /// Lets the model propose plans with `prepare_action_plan`. Nothing is approved or executed
    /// here; the caller reviews [`Workstation::take_actions`] locally.
    pub fn enable_action_proposals(&mut self, desk: ActionDesk) {
        self.actions = Some(desk);
        self.declare();
    }
    /// The action desk and the plans proposed during the investigation, for local review.
    pub fn take_actions(&mut self) -> Option<(ActionDesk, Vec<PendingPlan>)> {
        self.actions.take().map(|mut desk| {
            let pending = desk.take_pending();
            (desk, pending)
        })
    }
    /// Background scans started by the model, so the caller can wait for or cancel them.
    pub fn take_jobs(&mut self) -> Vec<ScanJob> {
        std::mem::take(&mut self.jobs)
    }
    /// Local opt-in settings; no tool can expand name sharing or start content comparison.
    /// Program names reach the model only in complete sharing, chosen by the user.
    pub fn set_sharing(&mut self, sharing: pcagent_agent::Sharing) {
        self.sharing = sharing;
    }
    pub fn tool_specs() -> Vec<ToolSpec> {
        pcagent_agent::catalog::inspection_tools()
    }
    pub fn set_history(
        &mut self,
        sampler: pcagent_store::sampler::Sampler,
        stop: Arc<std::sync::atomic::AtomicBool>,
    ) {
        self.history = Some(crate::history_tools::HistoryTools { sampler, stop });
        self.declare();
    }
    /// Host-owned results of locally approved helper jobs; tools cannot acquire elevation.
    pub fn set_elevated_evidence(
        &mut self,
        evidence: Vec<crate::elevated_tools::ApprovedInspection>,
    ) {
        self.elevated = evidence
            .into_iter()
            .filter(|result| result.available())
            .collect();
    }
    pub fn set_storage_context(&mut self, context: pcagent_agent::StorageContext) {
        self.storage_context = context;
    }
    /// Restrict a turn to the scopes approved in its exact local preview.
    pub fn set_allowed_scopes(&mut self, scopes: Vec<String>) {
        self.allowed_scopes = Some(scopes);
        self.declare();
    }
}

fn output(status: Status, cloud: Value) -> ToolOutput {
    ToolOutput {
        status,
        inspection_complete: status == Status::Ok,
        cloud,
        evidence: Vec::new(),
        findings: Vec::new(),
        processes: Vec::new(),
        storage_items: Vec::new(),
        limitations: Vec::new(),
    }
}
fn storage_error(e: StorageError) -> ToolOutput {
    output(Status::Error, json!({ "error": e }))
}
fn to_value(v: &impl Serialize) -> Value {
    serde_json::to_value(v).expect("serializable tool output")
}
fn kind(args: &Args) -> ItemKind {
    if args.text("kind") == Some("dirs") {
        ItemKind::Dirs
    } else {
        ItemKind::Files
    }
}
fn usize_arg(args: &Args, name: &str, default: usize) -> usize {
    args.integer(name)
        .and_then(|v| usize::try_from(v).ok())
        .unwrap_or(default)
}

impl Workstation<'_> {
    fn process_details(&mut self, entity_ref: &str) -> ToolOutput {
        let d = self.collector.process_details(entity_ref);
        let values = pcagent_agent::measurements(
            d.evidence
                .iter()
                .filter(|e| pcagent_agent::cloud_entity_allowed(&e.entity_ref)),
        );
        let mut out = output(
            d.status,
            json!({"entity_ref": d.entity_ref, "status": d.status, "state": d.state,
                "measurements": values, "observed_at_unix_ms": d.observed_at_unix_ms,
                "age_ms": d.age_ms, "limitations": d.limitations}),
        );
        out.evidence = d.evidence;
        out.inspection_complete =
            out.evidence.iter().any(|e| e.usable_value().is_some()) && d.status != Status::Stale;
        out
    }
    fn process_tree(&mut self) -> ToolOutput {
        let t = self.collector.process_tree();
        let edges: Vec<Value> = t
            .edges
            .iter()
            .filter(|e| e.child_ref.is_some())
            .map(|e| json!({"parent_ref": e.parent_ref, "child_ref": e.child_ref, "relation": e.relation}))
            .collect();
        let mut out = output(
            t.status,
            json!({"status": t.status, "origin": t.origin, "window": t.window, "nodes": t.nodes.len(),
                "edges": edges, "truncated": t.truncated, "limitations": t.limitations}),
        );
        out.inspection_complete = !t.nodes.is_empty();
        out
    }
    fn local_item(&self, args: &Args) -> Result<(PathBuf, ItemKind, String), String> {
        let (Some(scope), Some(item_ref)) = (args.text("scope_id"), args.text("item_ref")) else {
            return Err("scope_id and item_ref are required for this action".into());
        };
        let index = self.storage.index(scope).map_err(|e| e.to_string())?;
        let item = index.item(item_ref).map_err(|e| e.to_string())?;
        let path = index.local_path(item_ref).ok_or("unknown item")?;
        Ok((path, item.kind, item_ref.into()))
    }
    fn propose(&mut self, args: &Args) -> Result<Value, String> {
        let Some(desk) = self.actions.as_ref() else {
            return Err("action proposals are disabled".into());
        };
        let kind = args
            .text("action_kind")
            .and_then(proposals::parse_kind)
            .ok_or("unsupported action_kind")?;
        proposals::validate_arguments(kind, args)?;
        let proposal =
            match kind {
                ActionKind::OpenSettingsPage => Proposal {
                    kind,
                    target_ref: args
                        .text("settings_page")
                        .ok_or("settings_page is required")?
                        .into(),
                    target: None,
                    synthetic_cache: None,
                },
                ActionKind::OpenInExplorer => {
                    let (path, _, item_ref) = self.local_item(args)?;
                    let identity = match desk.guard.observe_file(&path) {
                        Ok(Some(identity)) => identity,
                        Ok(None) => return Err("item no longer exists".into()),
                        Err(status) => {
                            return Err(format!("item identity unavailable ({status:?})"))
                        }
                    };
                    Proposal {
                        kind,
                        target_ref: item_ref,
                        target: Some(ResolvedTarget::File(FileTarget { path, identity })),
                        synthetic_cache: None,
                    }
                }
                ActionKind::RunCleanupAdapter => {
                    if let Some(adapter) = args.text("adapter_id") {
                        Proposal {
                            kind,
                            target_ref: adapter.into(),
                            target: Some(self.action_targets.cleanup(adapter).ok_or(
                                "inspect a supported cleanup adapter before proposing it",
                            )?),
                            synthetic_cache: None,
                        }
                    } else {
                        if self.prefs.permissions.reversible_only {
                            return Err("user preferences allow reversible actions only".into());
                        }
                        let (path, item_kind, item_ref) = self.local_item(args)?;
                        let input = CandidateInput {
                            item_ref,
                            path: path.to_string_lossy().into_owned(),
                            is_dir: item_kind == ItemKind::Dirs,
                            logical_bytes: 0,
                            allocated_bytes: None,
                            modified_unix_ms: None,
                            attributes: Attributes::default(),
                            origin: Origin::Live,
                            marker_verified: item_kind == ItemKind::Dirs
                                && self.has_adapter_marker(&path),
                        };
                        let c = self.policy.classify(&input, self.prefs);
                        if c.adapter_id != Some(SYNTHETIC_CACHE_ID) || !c.preselectable {
                            return Err("item is not a validated cleanup adapter target".into());
                        }
                        let cache = SyntheticDemoCache::open(&path)
                            .map_err(|_| "adapter marker check failed")?;
                        Proposal {
                            kind,
                            target_ref: CLEANUP_TARGET_REF.into(),
                            target: None,
                            synthetic_cache: Some(cache),
                        }
                    }
                }
                ActionKind::DisableUserStartup => {
                    let reference = args.text("startup_ref").ok_or("startup_ref is required")?;
                    Proposal {
                        kind,
                        target_ref: reference.into(),
                        target: Some(self.action_targets.startup(reference).ok_or(
                            "inspect a supported current-user Run entry before proposing it",
                        )?),
                        synthetic_cache: None,
                    }
                }
                ActionKind::RequestNormalClose | ActionKind::OpenBrowserProcessManager => {
                    let reference = args.text("process_ref").ok_or("process_ref is required")?;
                    Proposal {
                        kind,
                        target_ref: reference.into(),
                        target: Some(self.action_targets.process(reference).ok_or(
                            "inspect a verifiable, non-preserved process before proposing it",
                        )?),
                        synthetic_cache: None,
                    }
                }
                ActionKind::RecycleSelectedFiles => {
                    let (scope, item_ref) = (
                        args.text("scope_id").ok_or("scope_id is required")?,
                        args.text("item_ref").ok_or("item_ref is required")?,
                    );
                    // A reference from a replaced scan cannot select an old local path.
                    self.local_item(args)?;
                    Proposal {
                        kind,
                        target_ref: item_ref.into(),
                        target: Some(
                            self.action_targets.file(scope, item_ref).ok_or(
                                "inspect a verifiable file before proposing its recycling",
                            )?,
                        ),
                        synthetic_cache: None,
                    }
                }
                _ => return Err("unsupported action_kind".into()),
            };
        let protection = PolicyProtection {
            policy: self.policy,
            prefs: self.prefs,
        };
        let desk = self
            .actions
            .as_mut()
            .ok_or("action proposals are disabled")?;
        if self.observation_refs.is_empty() {
            return Err("inspect the workstation before proposing an action".into());
        }
        let view = proposals::prepare(desk, proposal, &protection, self.observation_refs.clone())?;
        Ok(json!({
            "plan": view,
            "note": "Prepared for local review only. The user decides after the investigation; \
                do not claim it was applied."
        }))
    }
    fn has_adapter_marker(&self, dir: &Path) -> bool {
        self.policy
            .adapters
            .markers()
            .any(|m| fs::symlink_metadata(dir.join(m)).is_ok_and(|md| md.is_file()))
    }
    fn cleanup_candidates(&self, scope: &str, args: &Args) -> ToolOutput {
        let page =
            match self
                .storage
                .get_largest_items(scope, kind(args), 0, usize_arg(args, "limit", 20))
            {
                Ok(p) => p,
                Err(e) => return storage_error(e),
            };
        let inputs: Vec<CandidateInput> = page
            .items
            .iter()
            .filter_map(|i| {
                Some(CandidateInput {
                    item_ref: i.item_ref.clone(),
                    path: i.path.clone()?,
                    is_dir: i.kind == ItemKind::Dirs,
                    logical_bytes: i.logical_bytes,
                    allocated_bytes: i.allocated_bytes,
                    modified_unix_ms: i.modified_unix_ms,
                    attributes: Attributes::default(),
                    origin: Origin::Live,
                    marker_verified: i.kind == ItemKind::Dirs
                        && i.path
                            .as_deref()
                            .is_some_and(|p| self.has_adapter_marker(Path::new(p))),
                })
            })
            .collect();
        let mut view = self.policy.cleanup_candidates(&inputs, self.prefs);
        if page.scan_status != Status::Ok && view.status == Status::Ok {
            view.status = Status::Partial;
            view.limitations
                .push("The underlying scan is incomplete; larger items may be missing.");
        }
        let mut out = output(view.status, to_value(&view));
        let (context, local) = self
            .storage_context
            .project_page(&page, self.policy, self.prefs);
        out.cloud["item_context"] = context;
        out.storage_items = local;
        out.inspection_complete = view.status == Status::Ok || !inputs.is_empty();
        out
    }
}

impl Toolbox for Workstation<'_> {
    fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }
    fn sharing(&self) -> pcagent_agent::Sharing {
        self.sharing
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        let scope = args.text("scope_id").unwrap_or_default();
        if !scope.is_empty()
            && self
                .allowed_scopes
                .as_ref()
                .is_some_and(|scopes| !scopes.iter().any(|s| s == scope))
        {
            return output(
                Status::PermissionDenied,
                json!({"scope_id":scope,"error":"Scope was not approved for this turn."}),
            );
        }
        if let Some(out) = self
            .external
            .as_mut()
            .and_then(|external| external.run(spec, args))
        {
            return out;
        }
        let targets = &mut self.action_targets;
        let desk = self.actions.as_ref();
        let policy = self.policy;
        let prefs = self.prefs;
        let external = &mut self.external;
        let extension =
            crate::system_tools::run_with_inspection(spec, args, self.sharing, |inspection| {
                // Local facts for web lookups: names stay here, only composed queries leave.
                if let Some(external) = external {
                    external.retain(spec.name, inspection);
                }
                if let Some(desk) = desk {
                    targets.retain_inspection(
                        spec.name,
                        inspection,
                        desk.guard.as_ref(),
                        policy,
                        prefs,
                    );
                }
            })
            .or_else(|| crate::history_tools::run(spec, args, self.history.as_ref(), self.sharing))
            .or_else(|| crate::activity_tools::run(spec, args, self.collector, self.sharing));
        let extension = extension.or_else(|| {
            crate::elevated_tools::run(
                spec,
                args,
                &self.elevated,
                self.sharing,
                &pcagent_agent::PrivacyContext::local(),
            )
        });
        let mut out = if let Some(out) = extension {
            out
        } else {
            match spec.name {
                "get_system_snapshot" => {
                    ToolOutput::from_snapshot(&self.collector.snapshot(false, 1))
                }
                "get_processes" => {
                    let limit = usize_arg(args, "limit", MAX_PROCESSES as usize);
                    let sort = args
                        .text("sort_by")
                        .and_then(pcagent_telemetry::ProcessSort::parse)
                        .unwrap_or_default();
                    ToolOutput::from_snapshot(&self.collector.snapshot_sorted(true, limit, sort))
                }
                "get_process_details" => {
                    self.process_details(args.text("entity_ref").unwrap_or_default())
                }
                "get_process_tree" => self.process_tree(),

                "list_storage_scopes" => {
                    let scopes: Vec<Value> = self
                    .storage
                    .scopes()
                    .iter()
                    .filter(|s| self.allowed_scopes.as_ref().is_none_or(|allowed| allowed.contains(&s.scope_id)))
                    .map(|s| json!({ "scope_id": s.scope_id, "scan_available": self.storage.index(&s.scope_id).is_ok() }))
                    .collect();
                    output(Status::Ok, json!({ "scopes": scopes }))
                }
                "get_storage_overview" => match self.storage.get_storage_overview(scope) {
                    Ok(o) => {
                        let o = o.without_paths().redact_names();
                        let status = o.scan.as_ref().map_or(Status::Partial, |s| s.status);
                        let mut cloud = to_value(&o);
                        if let Some(map) = cloud.as_object_mut() {
                            map.remove("redacted_root");
                        }
                        let mut out = output(status, cloud);
                        out.inspection_complete = o.scan.is_some() || o.volume.free_bytes.is_some();
                        out
                    }
                    Err(e) => storage_error(e),
                },
                "start_storage_scan" => {
                    match self.storage.start_storage_scan(scope, self.scan_options) {
                        Ok(job) => {
                            self.jobs.push(job);
                            output(
                                Status::Ok,
                                json!({"scope_id": scope, "started": true,
                        "note": "Poll get_storage_overview; results appear when the scan completes."}),
                            )
                        }
                        Err(e) => storage_error(e),
                    }
                }
                "get_largest_items" => {
                    let (offset, limit) =
                        (usize_arg(args, "offset", 0), usize_arg(args, "limit", 20));
                    match self
                        .storage
                        .get_largest_items(scope, kind(args), offset, limit)
                    {
                        Ok(p) => {
                            let (cloud, local) =
                                self.storage_context
                                    .project_page(&p, self.policy, self.prefs);
                            let mut out = output(p.scan_status, cloud);
                            out.storage_items = local;
                            out.inspection_complete =
                                p.scan_status == Status::Ok || !p.items.is_empty();
                            out
                        }
                        Err(e) => storage_error(e),
                    }
                }
                "get_cleanup_candidates" => self.cleanup_candidates(scope, args),
                "prepare_action_plan" => match self.propose(args) {
                    Ok(v) => output(Status::Ok, v),
                    Err(e) => output(Status::Error, json!({ "error": e })),
                },
                "get_user_preferences" => output(
                    Status::Ok,
                    to_value(&self.policy.user_preferences_view(self.prefs)),
                ),
                _ => output(Status::Error, json!({"error": "unregistered tool"})),
            }
        };
        // Preserve the target even on errors, so one failed scope cannot disappear from coverage.
        if !scope.is_empty() {
            out.cloud["scope_id"] = scope.into();
        }
        if let Some(desk) = self.actions.as_ref() {
            self.action_targets
                .restrict_inspection_projection(spec.name, &out.cloud);
            if spec.name == "get_processes" {
                self.action_targets.retain_processes(
                    &out.processes,
                    desk.guard.as_ref(),
                    self.prefs,
                );
            }
            if matches!(spec.name, "get_largest_items" | "get_cleanup_candidates") {
                let items: Vec<_> = self
                    .storage
                    .index(scope)
                    .ok()
                    .map(|index| {
                        out.storage_items
                            .iter()
                            .filter_map(|item| {
                                Some(ScannedFile {
                                    item: index.item(&item.item_ref).ok()?,
                                    identity: index.local_identity(&item.item_ref),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.action_targets.retain_files(
                    scope,
                    &items,
                    desk.guard.as_ref(),
                    self.policy,
                    self.prefs,
                );
            }
            if matches!(
                spec.name,
                "get_startup_programs"
                    | "get_cleanup_opportunities"
                    | "get_processes"
                    | "get_largest_items"
                    | "get_cleanup_candidates"
            ) {
                out.cloud["proposable_actions"] = self
                    .action_targets
                    .view(spec.name, &out.cloud, scope)
                    .into();
            }
            if spec.name == "get_startup_programs" && !self.prefs.preserved_apps.is_empty() {
                out.cloud["action_limitations"] = json!(["Startup entries cannot be mapped to preserved applications without reading commands; disable proposals remain unavailable."]);
            }
        }
        if matches!(spec.class, pcagent_agent::ToolClass::Inspection) && out.inspection_complete {
            let reference = pcagent_shared::new_id("obs");
            out.cloud["observation_id"] = reference.clone().into();
            if self.observation_refs.len() >= pcagent_actions::MAX_TARGETS {
                self.observation_refs.remove(0);
            }
            self.observation_refs.push(reference);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::catalog::{HISTORY_METRICS, WORKSTATION_TOOLS};
    use pcagent_shared::Metric;
    use std::fs;

    fn run(ws: &mut Workstation<'_>, name: &str, args: &str) -> ToolOutput {
        let spec = *ws.specs().iter().find(|s| s.name == name).unwrap();
        let args = spec.validate(args).unwrap();
        ws.run(&spec, &args)
    }
    #[test]
    fn every_tool_is_dispatched_and_definitions_are_unique() {
        let specs = Workstation::tool_specs();
        let mut names: Vec<_> = specs.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), specs.len());
        for name in [
            "get_process_activity",
            "get_startup_programs",
            "get_services",
            "get_recent_events",
            "get_system_state",
            "get_power_and_thermal",
            "get_installed_apps",
            "get_hardware_inventory",
            "get_cleanup_opportunities",
            "query_system",
            "record_activity",
            "get_metric_history",
            "get_activity_events",
        ] {
            let spec = specs
                .iter()
                .find(|spec| spec.name == name)
                .expect("registered phase-one tool");
            assert_eq!(
                spec.definition()["function"]["parameters"]["additionalProperties"],
                false
            );
        }
        let (mut c, policy, prefs) = (Collector::new(), Policy::default(), Preferences::default());
        let mut ws = Workstation::new(&mut c, Arc::new(StorageService::new()), &policy, &prefs);
        for spec in WORKSTATION_TOOLS {
            let out = ws.run(spec, &Args::default());
            assert!(
                out.cloud.get("error") != Some(&json!("unregistered tool")),
                "{}",
                spec.name
            );
        }
    }
    #[test]
    fn history_metrics_parse_as_shared_metrics() {
        for m in HISTORY_METRICS.iter().chain(crate::history_tools::METRICS) {
            assert!(serde_json::from_value::<Metric>(json!(m)).is_ok(), "{m}");
        }
    }
    #[test]
    fn a_turn_cannot_read_another_previously_authorized_scope() {
        let dir = std::env::temp_dir().join(pcagent_shared::new_id("scope-selection"));
        fs::create_dir_all(dir.join("first")).unwrap();
        fs::create_dir_all(dir.join("second")).unwrap();
        let storage = Arc::new(StorageService::new());
        let first = storage.authorize(&dir.join("first")).unwrap().scope_id;
        let second = storage.authorize(&dir.join("second")).unwrap().scope_id;
        let (mut collector, policy, prefs) =
            (Collector::new(), Policy::default(), Preferences::default());
        let mut tools = Workstation::new(&mut collector, storage, &policy, &prefs);
        tools.set_allowed_scopes(vec![first.clone()]);
        let listed = run(&mut tools, "list_storage_scopes", "{}");
        assert_eq!(listed.cloud["scopes"].as_array().unwrap().len(), 1);
        assert_eq!(listed.cloud["scopes"][0]["scope_id"], first);
        let rejected = run(
            &mut tools,
            "get_storage_overview",
            &json!({"scope_id":second}).to_string(),
        );
        assert_eq!(rejected.status, Status::PermissionDenied);
        assert!(tools.observation_refs.is_empty());
        drop(tools);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn storage_tools_keep_full_paths_and_file_names_of_the_root_local() {
        let dir = std::env::temp_dir().join(format!("pcagent-ws-{}", std::process::id()));
        let secret = dir.join("PRIVATE_ROOT_MARKER");
        fs::create_dir_all(secret.join("node_modules")).unwrap();
        fs::write(
            secret.join("node_modules").join("big.bin"),
            vec![0u8; 64 * 1024],
        )
        .unwrap();
        fs::write(secret.join("CV_PRIVATE_NAME.pdf"), vec![0u8; 128 * 1024]).unwrap();
        let storage = Arc::new(StorageService::new());
        let scope = storage.authorize(&secret).unwrap().scope_id;
        storage
            .scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})
            .unwrap();
        let (mut c, policy, prefs) = (Collector::new(), Policy::default(), Preferences::default());
        let mut ws = Workstation::new(&mut c, storage, &policy, &prefs);
        let scope_args = json!({"scope_id": scope}).to_string();
        let page_args = json!({"scope_id": scope, "kind": "files", "limit": 5}).to_string();
        let outputs = [
            run(&mut ws, "list_storage_scopes", "{}"),
            run(&mut ws, "get_storage_overview", &scope_args),
            run(&mut ws, "get_largest_items", &page_args),
            run(&mut ws, "get_cleanup_candidates", &page_args),
        ];
        for out in &outputs {
            let encoded = out.cloud.to_string();
            assert!(!encoded.contains("PRIVATE_ROOT_MARKER"), "{encoded}");
            assert!(!encoded.contains("CV_PRIVATE_NAME"), "{encoded}");
            assert!(!encoded.contains(&*dir.to_string_lossy()), "{encoded}");
        }
        let paths: Vec<&str> = outputs[2].cloud["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["redacted_path"].as_str())
            .collect();
        assert_eq!(paths.len(), 2);
        assert!(paths
            .iter()
            .all(|p| p.starts_with("<scope>/") && p.contains("file-")));
        assert_ne!(paths[0], paths[1]);
        assert_eq!(
            outputs[3].cloud["candidates"].as_array().map(Vec::len),
            Some(2)
        );
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn unauthorized_scopes_are_errors_not_empty_results() {
        let (mut c, policy, prefs) = (Collector::new(), Policy::default(), Preferences::default());
        let mut ws = Workstation::new(&mut c, Arc::new(StorageService::new()), &policy, &prefs);
        let out = run(
            &mut ws,
            "get_largest_items",
            r#"{"scope_id":"scope-unknown","kind":"dirs","limit":3}"#,
        );
        assert_eq!(out.status, Status::Error);
        let out = run(
            &mut ws,
            "start_storage_scan",
            r#"{"scope_id":"scope-unknown"}"#,
        );
        assert_eq!(out.status, Status::Error);
        assert!(ws.take_jobs().is_empty());
    }
    mod proposals {
        use super::*;
        use pcagent_actions::{
            approve_locally, Capability, EffectError, EffectOutcome, Effects, FileIdentity, Guard,
            Journal, LocalSession, OverallOutcome, Permit, PlanState, ProcessIdentity,
            ProcessTarget, SettingsPage, MAX_APPROVAL_TTL,
        };
        use std::{cell::Cell, path::Path, rc::Rc, sync::atomic::AtomicBool};

        struct FakeGuard;
        impl Guard for FakeGuard {
            fn observe_file(&self, _: &Path) -> Result<Option<FileIdentity>, Status> {
                Ok(Some(FileIdentity {
                    volume_serial: 1,
                    file_id: 2,
                    size_bytes: 3,
                    modified_filetime: 4,
                    is_directory: false,
                    is_reparse_point: false,
                }))
            }
            fn observe_process(&self, _: u32) -> Result<Option<ProcessIdentity>, Status> {
                Ok(Some(ProcessIdentity {
                    pid: 42,
                    creation_time_filetime: 100,
                    boot_id: Some("fixture-boot".into()),
                }))
            }
            fn observe_user_startup(
                &self,
                _: &str,
            ) -> Result<Option<pcagent_actions::StartupIdentity>, Status> {
                Ok(Some(pcagent_actions::StartupIdentity {
                    run_key_last_write_filetime: 1,
                    approval: pcagent_actions::StartupApproval::Missing,
                }))
            }
        }
        struct FakeEffects(Rc<Cell<u32>>);
        impl Effects for FakeEffects {
            fn capability(&self, _: ActionKind) -> Capability {
                Capability::Executable
            }
            fn open_in_explorer(
                &self,
                _: &Permit,
                _: &FileTarget,
            ) -> Result<EffectOutcome, EffectError> {
                self.0.set(self.0.get() + 1);
                Ok(EffectOutcome::Done(None))
            }
            fn open_settings_page(
                &self,
                _: &Permit,
                _: SettingsPage,
            ) -> Result<EffectOutcome, EffectError> {
                self.0.set(self.0.get() + 1);
                Ok(EffectOutcome::Done(None))
            }
            fn request_normal_close(
                &self,
                _: &Permit,
                _: &ProcessTarget,
            ) -> Result<EffectOutcome, EffectError> {
                unreachable!("not proposable")
            }
            fn recycle_file(
                &self,
                _: &Permit,
                _: &FileTarget,
            ) -> Result<EffectOutcome, EffectError> {
                unreachable!("not proposable")
            }
        }
        fn desk(tag: &str, calls: &Rc<Cell<u32>>) -> ActionDesk {
            let dir =
                std::env::temp_dir().join(format!("pcagent-journal-{tag}-{}", std::process::id()));
            ActionDesk::new(
                Journal::open(dir).unwrap(),
                Box::new(FakeGuard),
                Box::new(FakeEffects(calls.clone())),
            )
        }

        #[test]
        fn proposal_tool_is_opt_in_and_never_executes() {
            let (mut c, policy, prefs) =
                (Collector::new(), Policy::default(), Preferences::default());
            let mut ws = Workstation::new(&mut c, Arc::new(StorageService::new()), &policy, &prefs);
            assert!(!ws.specs().iter().any(|s| s.name == "prepare_action_plan"));
            assert!(ws
                .specs()
                .iter()
                .all(|s| !s.name.starts_with("execute_") && !s.name.starts_with("approve_")));
            let calls = Rc::new(Cell::new(0));
            ws.enable_action_proposals(desk("settings", &calls));
            ws.enable_action_proposals(desk("settings", &calls));
            let args = r#"{"action_kind":"open_settings_page","settings_page":"storagesense"}"#;
            assert_eq!(
                run(&mut ws, "prepare_action_plan", args).status,
                Status::Error
            );
            // A locally collected observation, independent of native telemetry in portable tests.
            ws.observation_refs.push("obs-fixture".into());
            assert_eq!(
                ws.specs()
                    .iter()
                    .filter(|s| s.name == "prepare_action_plan")
                    .count(),
                1
            );
            let out = run(
                &mut ws,
                "prepare_action_plan",
                r#"{"action_kind":"open_settings_page","settings_page":"storagesense"}"#,
            );
            assert_eq!(out.status, Status::Ok, "{}", out.cloud);
            assert_eq!(out.cloud["plan"]["state"], "awaiting_approval");
            assert_eq!(out.cloud["plan"]["approval_granted"], false);
            assert_eq!(out.cloud["plan"]["evidence_refs"], json!(["obs-fixture"]));
            assert!(out.cloud["plan"].get("fingerprint").is_none());
            assert_eq!(calls.get(), 0);

            let (desk, mut pending) = ws.take_actions().unwrap();
            assert_eq!(pending.len(), 1);
            let p = &mut pending[0];
            let session = LocalSession::new();
            let token = approve_locally(&mut p.plan, &session, MAX_APPROVAL_TTL).unwrap();
            let mut plan = p.plan.clone();
            let protection = PolicyProtection {
                policy: &policy,
                prefs: &prefs,
            };
            let result = desk
                .broker(p, &protection)
                .execute(&mut plan, &token, &session, &AtomicBool::new(false), None)
                .unwrap();
            assert_eq!(result.overall, OverallOutcome::Completed);
            assert_eq!(plan.state(), PlanState::Completed);
            assert_eq!(calls.get(), 1);
            assert!(desk
                .broker(p, &protection)
                .execute(&mut plan, &token, &session, &AtomicBool::new(false), None)
                .is_err());
            assert_eq!(calls.get(), 1);
        }

        #[test]
        fn a_turn_declares_only_what_its_flows_and_probes_support_in_any_order() {
            use pcagent_agent::capabilities::{Capability, Probes};
            let declared = |ws: &Workstation<'_>, name: &str| {
                ws.specs().iter().filter(|s| s.name == name).count()
            };
            let dir = std::env::temp_dir().join(pcagent_shared::new_id("capability-scope"));
            fs::create_dir_all(&dir).unwrap();
            let storage = Arc::new(StorageService::new());
            let scope = storage.authorize(&dir).unwrap().scope_id;
            let (mut collector, policy, prefs) =
                (Collector::new(), Policy::default(), Preferences::default());
            let mut ws = Workstation::new(&mut collector, storage, &policy, &prefs);
            // Without probes every tool stays declared, as replays and captures expect.
            assert_eq!(ws.specs().len(), Workstation::tool_specs().len());
            assert!(ws.capabilities().is_none());
            let calls = Rc::new(Cell::new(0));
            // Proposals before the probes, scopes after: the order does not matter.
            ws.enable_action_proposals(desk("capabilities", &calls));
            ws.offer_available(Probes {
                elevated_helper: true,
                battery: Some(false),
                ..Probes::default()
            });
            ws.set_allowed_scopes(Vec::new());
            let flows = ws.capabilities_on(true).unwrap();
            assert!(flows.contains(Capability::ActionProposals));
            assert!(flows.contains(Capability::ElevatedHelper));
            assert!(
                flows.contains(Capability::NvidiaGpu),
                "not probed counts as present"
            );
            for absent in [
                Capability::StorageScopes,
                Capability::HistorySampler,
                Capability::WebSearch,
                Capability::Battery,
            ] {
                assert!(!flows.contains(absent), "{absent:?}");
            }
            assert_eq!(declared(&ws, "list_storage_scopes"), 0);
            ws.set_allowed_scopes(vec![scope.clone()]);
            assert!(ws
                .capabilities_on(true)
                .unwrap()
                .contains(Capability::StorageScopes));
            assert_eq!(declared(&ws, "list_storage_scopes"), 1);
            assert_eq!(declared(&ws, "get_user_preferences"), 1);
            // Windows-only tools follow the platform; history tools need a sampler too.
            let windows = usize::from(cfg!(windows));
            for name in [
                "get_system_snapshot",
                "get_process_activity",
                "get_approved_elevated_inspection",
                "prepare_action_plan",
            ] {
                assert_eq!(declared(&ws, name), windows, "{name}");
            }
            assert_eq!(declared(&ws, "get_metric_history"), 0);
            if cfg!(windows) {
                let elevated = ws
                    .specs()
                    .iter()
                    .find(|s| s.name == "get_approved_elevated_inspection")
                    .unwrap();
                assert!(!elevated.description.contains("battery_energy"));
            }
            // A tool that is not declared cannot be called: the controller validates the call
            // against the declared specs only.
            let call = pcagent_agent::ToolCall {
                id: "undeclared".into(),
                kind: "function".into(),
                function: pcagent_agent::FunctionCall {
                    name: "get_metric_history".into(),
                    arguments: r#"{"metric":"cpu_busy_basis_points","window_seconds":60}"#.into(),
                },
            };
            assert!(pcagent_agent::validate_call(&call, ws.specs()).is_err());
            assert_eq!(calls.get(), 0);
            drop(ws);
            fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn identical_proposals_are_reviewed_once_and_plans_are_bounded() {
            use crate::proposals::{prepare, Proposal};
            let (policy, prefs) = (Policy::default(), Preferences::default());
            let protection = PolicyProtection {
                policy: &policy,
                prefs: &prefs,
            };
            let calls = Rc::new(Cell::new(0));
            let mut desk = desk("dedup", &calls);
            let settings = |page: &str| Proposal {
                kind: ActionKind::OpenSettingsPage,
                target_ref: page.into(),
                target: None,
                synthetic_cache: None,
            };
            let first = prepare(
                &mut desk,
                settings("startupapps"),
                &protection,
                vec!["obs-fixture".into()],
            )
            .unwrap();
            let again = prepare(
                &mut desk,
                settings("startupapps"),
                &protection,
                vec!["obs-fixture".into()],
            )
            .unwrap();
            assert_eq!(first.plan_id, again.plan_id);
            assert_eq!(desk.pending.len(), 1);
            for page in ["storagesense", "appsfeatures"] {
                prepare(
                    &mut desk,
                    settings(page),
                    &protection,
                    vec!["obs-fixture".into()],
                )
                .unwrap();
            }
            let over = prepare(
                &mut desk,
                settings("another"),
                &protection,
                vec!["obs-fixture".into()],
            )
            .unwrap_err();
            assert!(over.contains("at most"), "{over}");
            assert_eq!(calls.get(), 0);
        }

        #[test]
        fn proposals_are_bounded_validated_and_fail_closed() {
            let dir = std::env::temp_dir().join(format!("pcagent-prop-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("PRIVATE_NAME.bin"), vec![0u8; 4096]).unwrap();
            let storage = Arc::new(StorageService::new());
            let scope = storage.authorize(&dir).unwrap().scope_id;
            storage
                .scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})
                .unwrap();
            let item = storage
                .get_largest_items(&scope, ItemKind::Files, 0, 1)
                .unwrap()
                .items[0]
                .item_ref
                .clone();
            let (mut c, policy, prefs) =
                (Collector::new(), Policy::default(), Preferences::default());
            let mut ws = Workstation::new(&mut c, storage, &policy, &prefs);
            let calls = Rc::new(Cell::new(0));
            ws.enable_action_proposals(desk("bounds", &calls));
            run(
                &mut ws,
                "get_largest_items",
                &json!({"scope_id":scope,"kind":"files","limit":1}).to_string(),
            );

            let args = json!({"action_kind":"open_in_explorer","scope_id":scope,"item_ref":item})
                .to_string();
            let out = run(&mut ws, "prepare_action_plan", &args);
            assert_eq!(
                out.status,
                Status::Error,
                "non-Windows paths must be refused as protected"
            );
            assert!(
                !out.cloud.to_string().contains("PRIVATE_NAME"),
                "{}",
                out.cloud
            );
            let args =
                json!({"action_kind":"run_cleanup_adapter","scope_id":scope,"item_ref":item})
                    .to_string();
            assert_eq!(
                run(&mut ws, "prepare_action_plan", &args).status,
                Status::Error
            );
            for missing in [
                r#"{"action_kind":"open_settings_page"}"#,
                r#"{"action_kind":"open_in_explorer"}"#,
            ] {
                assert_eq!(
                    run(&mut ws, "prepare_action_plan", missing).status,
                    Status::Error
                );
            }
            for page in pcagent_agent::catalog::proposals::SETTINGS_PAGES
                .iter()
                .take(crate::MAX_PENDING_PLANS)
            {
                let args =
                    json!({"action_kind":"open_settings_page","settings_page":page}).to_string();
                assert_eq!(
                    run(&mut ws, "prepare_action_plan", &args).status,
                    Status::Ok
                );
            }
            let again = r#"{"action_kind":"open_settings_page","settings_page":"startupapps"}"#;
            assert_eq!(
                run(&mut ws, "prepare_action_plan", again).status,
                Status::Ok
            );
            assert_eq!(ws.take_actions().unwrap().1.len(), crate::MAX_PENDING_PLANS);
            assert_eq!(calls.get(), 0);
            fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        fn new_settings_pages_are_pending_and_targets_require_prior_inspection() {
            for page in [
                "windowsupdate",
                "windowsupdate-optionalupdates",
                "powersleep",
                "batterysaver-usagedetails",
            ] {
                let (mut collector, policy, prefs) =
                    (Collector::new(), Policy::default(), Preferences::default());
                let mut ws = Workstation::new(
                    &mut collector,
                    Arc::new(StorageService::new()),
                    &policy,
                    &prefs,
                );
                let calls = Rc::new(Cell::new(0));
                ws.enable_action_proposals(desk(page, &calls));
                ws.observation_refs.push("obs-fixture".into());
                let out = run(
                    &mut ws,
                    "prepare_action_plan",
                    &json!({"action_kind":"open_settings_page", "settings_page":page}).to_string(),
                );
                assert_eq!(out.status, Status::Ok);
                assert_eq!(out.cloud["plan"]["state"], "awaiting_approval");
                assert_eq!(calls.get(), 0);
                for unavailable in [
                    r#"{"action_kind":"disable_user_startup","startup_ref":"startup-forged"}"#,
                    r#"{"action_kind":"run_cleanup_adapter","adapter_id":"user.temp"}"#,
                    r#"{"action_kind":"request_normal_close","process_ref":"proc-forged"}"#,
                    r#"{"action_kind":"recycle_selected_files","scope_id":"scope-forged","item_ref":"scan-forged:1"}"#,
                ] {
                    assert_eq!(
                        run(&mut ws, "prepare_action_plan", unavailable).status,
                        Status::Error
                    );
                }
                assert_eq!(ws.take_actions().unwrap().1.len(), 1);
                assert_eq!(calls.get(), 0);
            }
        }

        #[test]
        fn startup_and_normal_close_plans_use_only_captured_targets_and_never_execute() {
            use pcagent_inspect::{CoverageStatus, Inspection, Observation};
            use pcagent_shared::{ProcessInstance, Window};
            let (mut collector, policy, prefs) =
                (Collector::new(), Policy::default(), Preferences::default());
            let mut ws = Workstation::new(
                &mut collector,
                Arc::new(StorageService::new()),
                &policy,
                &prefs,
            );
            let calls = Rc::new(Cell::new(0));
            ws.enable_action_proposals(desk("captured-startup-and-close", &calls));
            ws.observation_refs.push("obs-fixture".into());
            let inspection = Inspection {
                status: Status::Ok,
                measured_at_unix_ms: 1,
                window: Window::instant(),
                origin: Origin::SyntheticFixture,
                observations: [("startup_kind", "Run"), ("startup_scope", "user_64")]
                    .into_iter()
                    .map(|(metric, value)| Observation {
                        observation_id: format!("measurement-{metric}"),
                        measured_at_unix_ms: 1,
                        window: Window::instant(),
                        entity_ref: "startup-fixture".into(),
                        metric: metric.into(),
                        value: value.into(),
                        unit: "category".into(),
                        status: CoverageStatus::Ok,
                        source: "synthetic fixture".into(),
                        origin: Origin::SyntheticFixture,
                    })
                    .collect(),
                coverage: vec![],
                names: [("startup-fixture".into(), "PRIVATE-STARTUP-NAME".into())].into(),
            };
            ws.action_targets.retain_inspection(
                "get_startup_programs",
                &inspection,
                &FakeGuard,
                &policy,
                &prefs,
            );
            ws.action_targets.retain_processes(
                &[ProcessInstance {
                    entity_ref: "proc-fixture".into(),
                    pid: 42,
                    creation_time_filetime: 100,
                    boot_id: Some("fixture-boot".into()),
                    executable_name: Some("PRIVATE-PROCESS.exe".into()),
                }],
                &FakeGuard,
                &prefs,
            );
            for args in [
                r#"{"action_kind":"disable_user_startup","startup_ref":"startup-fixture"}"#,
                r#"{"action_kind":"request_normal_close","process_ref":"proc-fixture"}"#,
            ] {
                let out = run(&mut ws, "prepare_action_plan", args);
                assert_eq!(out.status, Status::Ok, "{}", out.cloud);
                assert_eq!(out.cloud["plan"]["state"], "awaiting_approval");
                assert_eq!(out.cloud["plan"]["approval_granted"], false);
                let text = out.cloud.to_string();
                for forbidden in [
                    "PRIVATE",
                    "fixture-boot",
                    "creation_time",
                    "run_key_last_write",
                    "pid",
                ] {
                    assert!(!text.contains(forbidden), "{forbidden}");
                }
            }
            assert_eq!(calls.get(), 0);
            assert_eq!(ws.take_actions().unwrap().1.len(), 2);
        }
    }
}
