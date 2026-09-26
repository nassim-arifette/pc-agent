//! Read-only investigation tools backed by the real collectors, storage index and policy.
//! Every cloud payload is built from an explicit allowlist: PIDs, executable names, full paths,
//! boot IDs and collector session IDs stay local.
use crate::proposals::{
    self, ActionDesk, PendingPlan, PolicyProtection, Proposal, CLEANUP_TARGET_REF,
    PREPARE_ACTION_PLAN,
};
use crate::tools::{Args, Param, ParamKind, ToolOutput, ToolSpec, Toolbox, MAX_PROCESSES};
use pcagent_actions::{ActionKind, FileTarget, ResolvedTarget, SyntheticDemoCache};
use pcagent_policy::{Attributes, CandidateInput, Policy, Preferences, SYNTHETIC_CACHE_ID};
use pcagent_shared::{Metric, Origin, Status};
use pcagent_storage::{ItemKind, ScanJob, ScanOptions, StorageError, StorageService};
use pcagent_telemetry::Collector;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const SCOPE: Param = Param {
    name: "scope_id",
    description: "Opaque storage scope ID from list_storage_scopes.",
    kind: ParamKind::OpaqueRef { prefix: "scope-" },
    required: true,
};
const ITEM_KIND: Param = Param {
    name: "kind",
    description: "Rank files or directories.",
    kind: ParamKind::OneOf(&["files", "dirs"]),
    required: true,
};
const PAGE_LIMIT: Param = Param {
    name: "limit",
    description: "Page size.",
    kind: ParamKind::Integer { min: 1, max: 50 },
    required: true,
};
pub const HISTORY_METRICS: &[&str] = &[
    "physical_available_bytes",
    "system_commit_bytes",
    "cpu_busy_basis_points",
];

pub const WORKSTATION_TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "get_system_snapshot",
        description: "Collect fresh system memory and aggregate CPU observations. No changes to the computer.",
        params: &[],
    },
    ToolSpec {
        name: "get_processes",
        description: "Collect readable process memory, CPU and I/O contributors as opaque session-scoped identities. No paths, names or commands.",
        params: &[Param {
            name: "limit",
            description: "Maximum process instances to return (default 10).",
            kind: ParamKind::Integer { min: 1, max: MAX_PROCESSES },
            required: false,
        }],
    },
    ToolSpec {
        name: "get_process_details",
        description: "Latest evidence for one process instance by opaque entity_ref; reports exited or reused PIDs explicitly.",
        params: &[Param {
            name: "entity_ref",
            description: "Opaque process reference returned by get_processes.",
            kind: ParamKind::OpaqueRef { prefix: "proc-" },
            required: true,
        }],
    },
    ToolSpec {
        name: "get_process_tree",
        description: "Observed parent/child relations between opaque process references, validated by creation time. Observation only.",
        params: &[],
    },
    ToolSpec {
        name: "get_metric_history",
        description: "System samples recorded during this session over a trailing window; gaps are reported, never interpolated.",
        params: &[
            Param {
                name: "metric",
                description: "System metric.",
                kind: ParamKind::OneOf(HISTORY_METRICS),
                required: true,
            },
            Param {
                name: "window_seconds",
                description: "Trailing window.",
                kind: ParamKind::Integer { min: 1, max: 3_600 },
                required: true,
            },
        ],
    },
    ToolSpec {
        name: "get_hardware_inventory",
        description: "Windows version, architecture, logical processors, installed memory, volume capacity and GPU adapters. No serial numbers.",
        params: &[],
    },
    ToolSpec {
        name: "list_storage_scopes",
        description: "Storage roots the user authorized locally for this session, as opaque scope IDs.",
        params: &[],
    },
    ToolSpec {
        name: "get_storage_overview",
        description: "Volume space and the latest completed scan totals and coverage for one authorized scope.",
        params: &[SCOPE],
    },
    ToolSpec {
        name: "start_storage_scan",
        description: "Start a read-only metadata scan of an authorized scope in the background. Never reads file contents or follows links.",
        params: &[SCOPE],
    },
    ToolSpec {
        name: "get_largest_items",
        description: "Page of the largest files or directories from the latest completed scan, with redacted relative paths.",
        params: &[
            SCOPE,
            ITEM_KIND,
            PAGE_LIMIT,
            Param {
                name: "offset",
                description: "Pagination offset.",
                kind: ParamKind::Integer { min: 0, max: 100_000 },
                required: false,
            },
        ],
    },
    ToolSpec {
        name: "get_cleanup_candidates",
        description: "Local policy classification of the largest scanned items (protected, personal, regenerable...). Not an authorization to delete.",
        params: &[SCOPE, ITEM_KIND, PAGE_LIMIT],
    },
    ToolSpec {
        name: "get_user_preferences",
        description: "User exclusions, preserved application references and permissions as opaque references.",
        params: &[],
    },
];

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
            specs: WORKSTATION_TOOLS.to_vec(),
            actions: None,
        }
    }
    /// Lets the model propose plans with `prepare_action_plan`. Nothing is approved or executed
    /// here; the caller reviews [`Workstation::take_actions`] locally.
    pub fn enable_action_proposals(&mut self, desk: ActionDesk) {
        if self.actions.is_none() {
            self.specs.push(PREPARE_ACTION_PLAN);
        }
        self.actions = Some(desk);
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
}

fn output(status: Status, cloud: Value) -> ToolOutput {
    ToolOutput {
        status,
        cloud,
        evidence: Vec::new(),
        findings: Vec::new(),
        processes: Vec::new(),
        limitations: Vec::new(),
    }
}
fn storage_error(e: StorageError) -> ToolOutput {
    output(Status::Error, json!({ "error": e }))
}
fn to_value(v: &impl Serialize) -> Value {
    serde_json::to_value(v).expect("serializable tool output")
}
/// Keeps only the listed top-level keys of a serialized value.
fn allow(v: &impl Serialize, keys: &[&str]) -> Value {
    let Value::Object(mut map) = to_value(v) else {
        return Value::Null;
    };
    Value::Object(
        keys.iter()
            .filter_map(|k| map.remove(*k).map(|v| ((*k).into(), v)))
            .collect::<Map<_, _>>(),
    )
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
        let mut out = output(
            d.status,
            allow(
                &d,
                &[
                    "entity_ref",
                    "status",
                    "state",
                    "evidence",
                    "observed_at_unix_ms",
                    "age_ms",
                    "limitations",
                ],
            ),
        );
        out.evidence = d.evidence;
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
        output(
            t.status,
            json!({"status": t.status, "origin": t.origin, "window": t.window, "nodes": t.nodes.len(),
                "edges": edges, "truncated": t.truncated, "limitations": t.limitations}),
        )
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
        let proposal = match kind {
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
                    Err(status) => return Err(format!("item identity unavailable ({status:?})")),
                };
                Proposal {
                    kind,
                    target_ref: item_ref,
                    target: Some(ResolvedTarget::File(FileTarget { path, identity })),
                    synthetic_cache: None,
                }
            }
            ActionKind::RunCleanupAdapter => {
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
                    marker_verified: item_kind == ItemKind::Dirs && self.has_adapter_marker(&path),
                };
                let c = self.policy.classify(&input, self.prefs);
                if c.adapter_id != Some(SYNTHETIC_CACHE_ID) || !c.preselectable {
                    return Err("item is not a validated cleanup adapter target".into());
                }
                let cache =
                    SyntheticDemoCache::open(&path).map_err(|_| "adapter marker check failed")?;
                Proposal {
                    kind,
                    target_ref: CLEANUP_TARGET_REF.into(),
                    target: None,
                    synthetic_cache: Some(cache),
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
        let view = proposals::prepare(desk, proposal, &protection)?;
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
    fn metric_history(&self, args: &Args) -> ToolOutput {
        let Some(metric) = args
            .text("metric")
            .and_then(|m| serde_json::from_value::<Metric>(Value::String(m.into())).ok())
        else {
            return output(Status::Error, json!({"error": "unknown metric"}));
        };
        let window = Duration::from_secs(args.integer("window_seconds").unwrap_or(60));
        let s = self.collector.metric_history(metric, window);
        output(
            s.status,
            allow(
                &s,
                &[
                    "metric",
                    "unit",
                    "origin",
                    "status",
                    "requested_window_ms",
                    "observed_interval_ms",
                    "retention",
                    "points",
                    "gaps",
                    "limitations",
                ],
            ),
        )
    }
    fn inventory(&self) -> ToolOutput {
        let i = self.collector.hardware_inventory();
        let volumes = i.volumes.value.as_ref().map(|v| {
            v.iter()
                .map(|v| {
                    json!({"kind": v.kind, "total_bytes": v.total_bytes, "free_bytes": v.free_bytes,
                    "available_to_caller_bytes": v.available_to_caller_bytes, "status": v.status})
                })
                .collect::<Vec<_>>()
        });
        output(
            i.status,
            json!({"status": i.status, "origin": i.origin, "os_version": i.os_version,
                "architecture": i.architecture, "logical_processors": i.logical_processors,
                "installed_memory_bytes": i.installed_memory_bytes,
                "volumes": {"value": volumes, "status": i.volumes.status, "source": i.volumes.source},
                "gpu_adapters": i.gpu_adapters, "limitations": i.limitations}),
        )
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
        output(view.status, to_value(&view))
    }
}

impl Toolbox for Workstation<'_> {
    fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        let scope = args.text("scope_id").unwrap_or_default();
        match spec.name {
            "get_system_snapshot" => ToolOutput::from_snapshot(&self.collector.snapshot(false, 1)),
            "get_processes" => {
                let limit = usize_arg(args, "limit", MAX_PROCESSES as usize);
                ToolOutput::from_snapshot(&self.collector.snapshot(true, limit))
            }
            "get_process_details" => {
                self.process_details(args.text("entity_ref").unwrap_or_default())
            }
            "get_process_tree" => self.process_tree(),
            "get_metric_history" => self.metric_history(args),
            "get_hardware_inventory" => self.inventory(),
            "list_storage_scopes" => {
                let scopes: Vec<Value> = self
                    .storage
                    .scopes()
                    .iter()
                    .map(|s| json!({ "scope_id": s.scope_id }))
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
                    output(status, cloud)
                }
                Err(e) => storage_error(e),
            },
            "start_storage_scan" => match self.storage.start_storage_scan(scope, self.scan_options)
            {
                Ok(job) => {
                    self.jobs.push(job);
                    output(
                        Status::Ok,
                        json!({"scope_id": scope, "started": true,
                        "note": "Poll get_storage_overview; results appear when the scan completes."}),
                    )
                }
                Err(e) => storage_error(e),
            },
            "get_largest_items" => {
                let (offset, limit) = (usize_arg(args, "offset", 0), usize_arg(args, "limit", 20));
                match self
                    .storage
                    .get_largest_items(scope, kind(args), offset, limit)
                {
                    Ok(p) => {
                        let p = p.without_paths().redact_names();
                        output(p.scan_status, to_value(&p))
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(ws: &mut Workstation<'_>, name: &str, args: &str) -> ToolOutput {
        let spec = *ws.specs().iter().find(|s| s.name == name).unwrap();
        let args = spec.validate(args).unwrap();
        ws.run(&spec, &args)
    }
    #[test]
    fn every_tool_is_dispatched_and_definitions_are_unique() {
        let mut names: Vec<_> = WORKSTATION_TOOLS.iter().map(|s| s.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), WORKSTATION_TOOLS.len());
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
        for m in HISTORY_METRICS {
            assert!(serde_json::from_value::<Metric>(json!(m)).is_ok(), "{m}");
        }
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
        assert_eq!(paths, ["<scope>/*.pdf", "<scope>/node_modules/*.bin"]);
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
                Err(Status::Unsupported)
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
                .all(|s| !s.name.contains("execute") && !s.name.contains("approve")));
            let calls = Rc::new(Cell::new(0));
            ws.enable_action_proposals(desk("settings", &calls));
            ws.enable_action_proposals(desk("settings", &calls));
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
            let settings = r#"{"action_kind":"open_settings_page","settings_page":"startupapps"}"#;
            for _ in 0..crate::MAX_PENDING_PLANS {
                assert_eq!(
                    run(&mut ws, "prepare_action_plan", settings).status,
                    Status::Ok
                );
            }
            assert_eq!(
                run(&mut ws, "prepare_action_plan", settings).status,
                Status::Error
            );
            assert_eq!(ws.take_actions().unwrap().1.len(), crate::MAX_PENDING_PLANS);
            assert_eq!(calls.get(), 0);
            fs::remove_dir_all(&dir).unwrap();
        }
    }
}
