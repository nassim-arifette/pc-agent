//! Bounded local targets captured during inspections. Nothing in these maps is model authority.
use crate::proposals::PolicyProtection;
use pcagent_actions::{
    FileTarget, Guard, ProcessIdentity, ProcessTarget, ProtectionCheck, RecycleCleanupTarget,
    ResolvedTarget, StartupTarget, UserCleanupAdapter, UserCleanupKind, MAX_TARGETS,
};
use pcagent_inspect::{CoverageStatus, Inspection};
use pcagent_policy::{Attributes, CandidateInput, CleanupClass, Policy, Preferences};
use pcagent_shared::{Origin, ProcessInstance, Status};
use pcagent_storage::{ItemKind, ItemView};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

const FILETIME_EPOCH: u64 = 116_444_736_000_000_000;

struct CleanupCapture {
    entity_ref: String,
    target: RecycleCleanupTarget,
}

/// The index's scan-time object identity stays local; metadata equality alone cannot bind a file.
pub(crate) struct ScannedFile {
    pub item: ItemView,
    pub identity: Option<(u64, u128)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_actions::{FileIdentity, StartupApproval, StartupIdentity};
    use pcagent_inspect::Observation;
    use pcagent_policy::{NormalizedPath, PreservedApp};
    use pcagent_shared::Window;
    use std::cell::Cell;

    #[derive(Default)]
    struct FakeGuard {
        startup: Option<StartupIdentity>,
        file: Option<FileIdentity>,
        process: Option<ProcessIdentity>,
        startup_reads: Cell<usize>,
    }
    impl Guard for FakeGuard {
        fn observe_file(&self, _: &Path) -> Result<Option<FileIdentity>, Status> {
            Ok(self.file.clone())
        }
        fn observe_process(&self, _: u32) -> Result<Option<ProcessIdentity>, Status> {
            Ok(self.process.clone())
        }
        fn observe_user_startup(&self, _: &str) -> Result<Option<StartupIdentity>, Status> {
            self.startup_reads.set(self.startup_reads.get() + 1);
            Ok(self.startup.clone())
        }
    }

    fn inspection() -> Inspection {
        Inspection {
            status: Status::Ok,
            measured_at_unix_ms: 1,
            window: Window::instant(),
            origin: Origin::SyntheticFixture,
            observations: vec![],
            coverage: vec![],
            names: BTreeMap::new(),
        }
    }
    fn add(inspection: &mut Inspection, entity: &str, metric: &str, value: Value) {
        inspection.observations.push(Observation {
            observation_id: format!("measurement-{}", inspection.observations.len()),
            measured_at_unix_ms: 1,
            window: Window::instant(),
            entity_ref: entity.into(),
            metric: metric.into(),
            value,
            unit: "category".into(),
            status: CoverageStatus::Ok,
            source: "synthetic fixture".into(),
            origin: Origin::SyntheticFixture,
        });
    }
    fn startup(inspection: &mut Inspection, reference: &str, kind: &str, scope: &str) {
        inspection
            .names
            .insert(reference.into(), "PRIVATE-REGISTRY-NAME".into());
        add(inspection, reference, "startup_kind", json!(kind));
        add(inspection, reference, "startup_scope", json!(scope));
    }
    fn startup_guard() -> FakeGuard {
        FakeGuard {
            startup: Some(StartupIdentity {
                run_key_last_write_filetime: 1,
                approval: StartupApproval::Missing,
            }),
            ..Default::default()
        }
    }
    fn file_identity(directory: bool) -> FileIdentity {
        FileIdentity {
            volume_serial: 1,
            file_id: if directory { 1 } else { 2 },
            size_bytes: 3,
            modified_filetime: FILETIME_EPOCH + 10_000,
            is_directory: directory,
            is_reparse_point: false,
        }
    }
    fn cleanup(kind: UserCleanupKind) -> RecycleCleanupTarget {
        serde_json::from_value(json!({"kind":kind,
            "root": {"path":r"C:\Fixture\Cache", "identity":file_identity(true)},
            "directories":[], "files":[{"path":r"C:\Fixture\Cache\old.bin", "identity":file_identity(false)}],
            "bytes":3})).unwrap()
    }

    #[test]
    fn startup_only_retains_observed_current_user_run_and_never_shares_the_name() {
        let mut data = inspection();
        startup(&mut data, "startup-user", "Run", "user_64");
        startup(&mut data, "startup-machine", "Run", "machine_64");
        startup(&mut data, "startup-once", "RunOnce", "user_64");
        startup(&mut data, "startup-folder", "startup_folder", "user_folder");
        startup(&mut data, "startup-view32", "Run", "user_32");
        add(&mut data, "startup-unnamed", "startup_kind", json!("Run"));
        add(
            &mut data,
            "startup-unnamed",
            "startup_scope",
            json!("user_64"),
        );
        let mut targets = ActionTargets::default();
        let guard = startup_guard();
        targets.retain_startup(&data, &guard, &Preferences::default());
        assert_eq!(targets.startup.len(), 1);
        assert_eq!(guard.startup_reads.get(), 1);
        assert!(targets.startup("startup-user").is_some());
        assert!(targets.startup("startup-machine").is_none());
        let cloud = json!({"observations":data.observations});
        let view = targets.view("get_startup_programs", &cloud, "");
        assert_eq!(view[0]["startup_ref"], "startup-user");
        assert!(!serde_json::to_string(&view).unwrap().contains("PRIVATE"));
        targets.restrict_inspection_projection("get_startup_programs", &json!({"observations":[]}));
        assert!(targets.startup("startup-user").is_none());
    }

    #[test]
    fn startup_maps_are_bounded_and_unknown_or_preserved_targets_remain_unavailable() {
        let mut data = inspection();
        for index in 0..=MAX_TARGETS {
            startup(&mut data, &format!("startup-{index:03}"), "Run", "user_64");
        }
        let mut targets = ActionTargets::default();
        targets.retain_startup(&data, &startup_guard(), &Preferences::default());
        assert_eq!(targets.startup.len(), MAX_TARGETS);
        targets.retain_startup(&data, &FakeGuard::default(), &Preferences::default());
        assert!(targets.startup.is_empty());
        let guard = startup_guard();
        let prefs = Preferences {
            preserved_apps: vec![PreservedApp {
                executable: "needed.exe".into(),
                needed_for: None,
            }],
            ..Preferences::default()
        };
        targets.retain_startup(&data, &guard, &prefs);
        assert!(targets.startup.is_empty());
        assert_eq!(guard.startup_reads.get(), 0);
        let mut disabled = [0; 12];
        disabled[0] = 3;
        let guard = FakeGuard {
            startup: Some(StartupIdentity {
                run_key_last_write_filetime: 1,
                approval: StartupApproval::Binary(disabled),
            }),
            ..Default::default()
        };
        targets.retain_startup(&data, &guard, &Preferences::default());
        assert!(targets.startup.is_empty());
    }

    #[test]
    fn cleanup_requires_the_inspected_category_exact_adapter_and_local_manifest() {
        let mut targets = ActionTargets::default();
        let policy = Policy::default();
        let prefs = Preferences::default();
        let mut data = inspection();
        targets.retain_cleanup(&data, &policy, &prefs, |_| {
            panic!("no inspection, no inventory")
        });
        assert!(targets.cleanup("user.temp").is_none());
        for (reference, category) in [
            ("cleanup-temp", "temporary_files"),
            ("cleanup-nvidia", "nvidia_shader_cache"),
            ("cleanup-directx", "directx_shader_cache"),
        ] {
            add(&mut data, reference, "cleanup_category", json!(category));
            add(&mut data, reference, "cleanup_level", json!("regenerable"));
        }
        targets.retain_cleanup(&data, &policy, &prefs, |kind| Ok(cleanup(kind)));
        assert_eq!(targets.cleanup.len(), 3);
        let view = targets.view(
            "get_cleanup_opportunities",
            &json!({"observations":data.observations}),
            "",
        );
        assert_eq!(view.len(), 3);
        assert!(view.iter().all(|v| v["immediate_reclaim_bytes"] == 0));
        let encoded = serde_json::to_string(&view).unwrap();
        assert!(
            !encoded.contains("Fixture")
                && !encoded.contains("old.bin")
                && !encoded.contains("file_id")
        );
        targets.retain_cleanup(&data, &policy, &prefs, |_| Err(Status::Partial));
        assert!(targets.cleanup.is_empty());
        targets.retain_cleanup(&data, &policy, &prefs, |_| {
            Ok(cleanup(UserCleanupKind::UserTemp))
        });
        assert_eq!(targets.cleanup.len(), 1);
        let prefs = Preferences {
            excluded_paths: vec![NormalizedPath::parse(r"C:\Fixture\Cache").unwrap()],
            ..Preferences::default()
        };
        targets.retain_cleanup(&data, &policy, &prefs, |kind| Ok(cleanup(kind)));
        assert!(targets.cleanup.is_empty());
        let prefs = Preferences {
            excluded_categories: vec![CleanupClass::Temporary, CleanupClass::RegenerableIdentified],
            ..Preferences::default()
        };
        targets.retain_cleanup(&data, &policy, &prefs, |_| {
            panic!("excluded category must not inventory")
        });
        assert!(targets.cleanup.is_empty());
    }

    #[test]
    fn normal_close_requires_the_same_observed_instance_and_honors_preserved_apps() {
        let process = ProcessInstance {
            entity_ref: "proc-fixture".into(),
            pid: 42,
            creation_time_filetime: 100,
            boot_id: Some("fixture-boot".into()),
            executable_name: Some("needed.exe".into()),
        };
        let mut targets = ActionTargets::default();
        let mut guard = FakeGuard {
            process: Some(ProcessIdentity {
                pid: 42,
                creation_time_filetime: 100,
                boot_id: process.boot_id.clone(),
            }),
            ..Default::default()
        };
        targets.retain_processes(
            std::slice::from_ref(&process),
            &guard,
            &Preferences::default(),
        );
        assert!(targets.process("proc-fixture").is_some());
        let encoded =
            serde_json::to_string(&targets.view("get_processes", &json!({}), "")).unwrap();
        assert!(
            !encoded.contains("needed.exe")
                && !encoded.contains("42")
                && !encoded.contains("fixture-boot")
        );
        let prefs = Preferences {
            preserved_apps: vec![PreservedApp {
                executable: "NEEDED.EXE".into(),
                needed_for: None,
            }],
            ..Preferences::default()
        };
        targets.retain_processes(std::slice::from_ref(&process), &guard, &prefs);
        assert!(targets.processes.is_empty());
        guard.process.as_mut().unwrap().creation_time_filetime += 1;
        targets.retain_processes(
            std::slice::from_ref(&process),
            &guard,
            &Preferences::default(),
        );
        assert!(targets.processes.is_empty());
        guard.process.as_mut().unwrap().creation_time_filetime = 100;
        let mut unknown_boot = process.clone();
        unknown_boot.boot_id = None;
        targets.retain_processes(&[unknown_boot], &guard, &Preferences::default());
        assert!(targets.processes.is_empty());
    }

    #[test]
    fn recycling_binds_the_scanned_metadata_and_invalidates_changed_or_excluded_files() {
        let item: ItemView = serde_json::from_value(json!({"item_ref":"scan-fixture:1", "alias":"F1", "alias_path":"F1",
            "parent_ref":null, "kind":"files", "path":r"C:\Fixture\old.bin", "redacted_path":"[file]",
            "logical_bytes":3, "allocated_bytes":null, "allocation_unknown_files":1, "estimated_reclaim_bytes":null,
            "files":null, "link_count":1, "sparse":false, "compressed":false, "modified_unix_ms":1,
            "last_used":"unknown", "status":"ok"})).unwrap();
        let item = ScannedFile {
            item,
            identity: Some((1, 2)),
        };
        let mut targets = ActionTargets::default();
        let mut guard = FakeGuard {
            file: Some(file_identity(false)),
            ..Default::default()
        };
        let policy = Policy::default();
        targets.retain_files(
            "scope-fixture",
            std::slice::from_ref(&item),
            &guard,
            &policy,
            &Preferences::default(),
        );
        assert!(targets.file("scope-fixture", "scan-fixture:1").is_some());
        assert!(targets.file("scope-forged", "scan-fixture:1").is_none());
        guard.file.as_mut().unwrap().size_bytes += 1;
        targets.retain_files(
            "scope-fixture",
            std::slice::from_ref(&item),
            &guard,
            &policy,
            &Preferences::default(),
        );
        assert!(targets.files.is_empty());
        guard.file = Some(file_identity(false));
        let prefs = Preferences {
            excluded_paths: vec![NormalizedPath::parse(r"C:\Fixture").unwrap()],
            ..Preferences::default()
        };
        targets.retain_files(
            "scope-fixture",
            std::slice::from_ref(&item),
            &guard,
            &policy,
            &prefs,
        );
        assert!(targets.files.is_empty());
        let mut item = item;
        item.item.modified_unix_ms = None;
        targets.retain_files(
            "scope-fixture",
            &[item],
            &guard,
            &policy,
            &Preferences::default(),
        );
        assert!(targets.files.is_empty());
    }

    #[test]
    fn replacing_a_file_with_identical_metadata_never_rebinds_its_scan_reference() {
        let item: ItemView = serde_json::from_value(json!({"item_ref":"scan-fixture:1", "alias":"F1", "alias_path":"F1",
            "parent_ref":null, "kind":"files", "path":r"C:\Fixture\old.bin", "redacted_path":"[file]",
            "logical_bytes":3, "allocated_bytes":null, "allocation_unknown_files":1, "estimated_reclaim_bytes":null,
            "files":null, "link_count":1, "sparse":false, "compressed":false, "modified_unix_ms":1,
            "last_used":"unknown", "status":"ok"})).unwrap();
        let mut scanned = ScannedFile {
            item,
            identity: Some((1, 2)),
        };
        let policy = Policy::default();
        let prefs = Preferences::default();
        let mut targets = ActionTargets::default();
        let mut replacement = file_identity(false);
        replacement.file_id = 3; // Same size and modification time as the scanned object.
        let mut guard = FakeGuard {
            file: Some(replacement.clone()),
            ..Default::default()
        };
        // Replacement before the first page display must never issue a target.
        targets.retain_files(
            "scope-fixture",
            std::slice::from_ref(&scanned),
            &guard,
            &policy,
            &prefs,
        );
        assert!(targets.file("scope-fixture", "scan-fixture:1").is_none());
        guard.file = Some(file_identity(false));
        targets.retain_files(
            "scope-fixture",
            std::slice::from_ref(&scanned),
            &guard,
            &policy,
            &prefs,
        );
        assert!(targets.file("scope-fixture", "scan-fixture:1").is_some());
        // A refresh invalidates the old entry instead of binding its ref to the replacement.
        guard.file = Some(replacement);
        targets.retain_files(
            "scope-fixture",
            std::slice::from_ref(&scanned),
            &guard,
            &policy,
            &prefs,
        );
        assert!(targets.file("scope-fixture", "scan-fixture:1").is_none());
        guard.file = Some(file_identity(false));
        scanned.identity = Some((2, 2)); // A volume change is also a different object.
        targets.retain_files(
            "scope-fixture",
            std::slice::from_ref(&scanned),
            &guard,
            &policy,
            &prefs,
        );
        assert!(targets.files.is_empty());
        scanned.identity = None;
        targets.retain_files("scope-fixture", &[scanned], &guard, &policy, &prefs);
        assert!(targets.files.is_empty());
    }
}

/// These are never serialized. Pending plans own a separate immutable copy of each target.
#[derive(Default)]
pub(crate) struct ActionTargets {
    startup: BTreeMap<String, StartupTarget>,
    cleanup: BTreeMap<String, CleanupCapture>,
    processes: BTreeMap<String, ProcessTarget>,
    files: BTreeMap<(String, String), FileTarget>,
}

fn observation<'a>(inspection: &'a Inspection, entity: &str, metric: &str) -> Option<&'a Value> {
    inspection
        .observations
        .iter()
        .find(|o| o.entity_ref == entity && o.metric == metric && o.status == CoverageStatus::Ok)
        .map(|o| &o.value)
}

fn opaque(reference: &str, prefix: &str) -> bool {
    reference.starts_with(prefix)
        && reference.len() <= pcagent_actions::MAX_REF_LEN
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
}

fn executable_basename(name: &str) -> &str {
    name.rsplit(['\\', '/']).next().unwrap_or(name)
}

pub(crate) fn preserved_process(process: &ProcessTarget, prefs: &Preferences) -> bool {
    // Without an executable name we cannot prove that this is not a preserved application.
    process.executable_name.as_deref().is_none_or(|name| {
        prefs.preserved_apps.iter().any(|app| {
            executable_basename(name).eq_ignore_ascii_case(executable_basename(&app.executable))
        })
    })
}

impl ActionTargets {
    pub(crate) fn retain_inspection(
        &mut self,
        tool: &str,
        inspection: &Inspection,
        guard: &dyn Guard,
        policy: &Policy,
        prefs: &Preferences,
    ) {
        match tool {
            "get_startup_programs" => self.retain_startup(inspection, guard, prefs),
            "get_cleanup_opportunities" => self.retain_cleanup(inspection, policy, prefs, |kind| {
                UserCleanupAdapter::current_user(kind)?.capture(guard)
            }),
            _ => {}
        }
    }

    fn retain_startup(&mut self, inspection: &Inspection, guard: &dyn Guard, prefs: &Preferences) {
        self.startup.clear();
        // Run value names do not establish the executable behind an entry: no command is read.
        if !prefs.preserved_apps.is_empty() {
            return;
        }
        for (reference, name) in &inspection.names {
            if self.startup.len() >= MAX_TARGETS {
                break;
            }
            if opaque(reference, "startup-")
                && observation(inspection, reference, "startup_kind").and_then(Value::as_str)
                    == Some("Run")
                && observation(inspection, reference, "startup_scope").and_then(Value::as_str)
                    == Some("user_64")
            {
                if let Ok(target) = StartupTarget::disable(name, guard) {
                    self.startup.insert(reference.clone(), target);
                }
            }
        }
    }

    fn retain_cleanup(
        &mut self,
        inspection: &Inspection,
        policy: &Policy,
        prefs: &Preferences,
        mut capture: impl FnMut(UserCleanupKind) -> Result<RecycleCleanupTarget, Status>,
    ) {
        self.cleanup.clear();
        let protection = PolicyProtection { policy, prefs };
        for (kind, category, class) in [
            (
                UserCleanupKind::UserTemp,
                "temporary_files",
                CleanupClass::Temporary,
            ),
            (
                UserCleanupKind::NvidiaDxCache,
                "nvidia_shader_cache",
                CleanupClass::RegenerableIdentified,
            ),
            (
                UserCleanupKind::Direct3dShaderCache,
                "directx_shader_cache",
                CleanupClass::RegenerableIdentified,
            ),
        ] {
            if prefs.excluded_categories.contains(&class) {
                continue;
            }
            let Some(entity) = inspection
                .observations
                .iter()
                .find(|o| {
                    o.metric == "cleanup_category"
                        && o.status == CoverageStatus::Ok
                        && o.value.as_str() == Some(category)
                        && observation(inspection, &o.entity_ref, "cleanup_level")
                            .and_then(Value::as_str)
                            == Some("regenerable")
                })
                .map(|o| o.entity_ref.clone())
            else {
                continue;
            };
            let Ok(target) = capture(kind) else {
                continue;
            };
            if target.kind() != kind
                || target.files().is_empty()
                || target.files().len() > MAX_TARGETS
                || protection.is_protected(&target.root().path)
                || target
                    .files()
                    .iter()
                    .any(|file| protection.is_protected(&file.path))
                || std::iter::once(target.root())
                    .chain(target.files())
                    .any(|file| {
                        policy
                            .classify(
                                &CandidateInput {
                                    item_ref: kind.adapter_id().into(),
                                    path: file.path.to_string_lossy().into_owned(),
                                    is_dir: file.identity.is_directory,
                                    logical_bytes: file.identity.size_bytes,
                                    allocated_bytes: None,
                                    modified_unix_ms: file
                                        .identity
                                        .modified_filetime
                                        .checked_sub(FILETIME_EPOCH)
                                        .map(|v| v / 10_000),
                                    attributes: Attributes::default(),
                                    origin: Origin::Live,
                                    marker_verified: false,
                                },
                                prefs,
                            )
                            .verdict
                            .is_forbidden()
                    })
            {
                continue;
            }
            self.cleanup.insert(
                kind.adapter_id().into(),
                CleanupCapture {
                    entity_ref: entity,
                    target,
                },
            );
        }
    }

    pub(crate) fn retain_processes(
        &mut self,
        processes: &[ProcessInstance],
        guard: &dyn Guard,
        prefs: &Preferences,
    ) {
        self.processes.clear();
        for process in processes {
            if self.processes.len() >= MAX_TARGETS {
                break;
            }
            let target = ProcessTarget {
                identity: ProcessIdentity {
                    pid: process.pid,
                    creation_time_filetime: process.creation_time_filetime,
                    boot_id: process.boot_id.clone(),
                },
                executable_name: process.executable_name.clone(),
            };
            if !opaque(&process.entity_ref, "proc-") || preserved_process(&target, prefs) {
                continue;
            }
            if guard
                .observe_process(process.pid)
                .ok()
                .flatten()
                .is_some_and(|identity| target.identity.same_instance(&identity))
            {
                self.processes.insert(process.entity_ref.clone(), target);
            }
        }
    }

    pub(crate) fn retain_files(
        &mut self,
        scope: &str,
        items: &[ScannedFile],
        guard: &dyn Guard,
        policy: &Policy,
        prefs: &Preferences,
    ) {
        let protection = PolicyProtection { policy, prefs };
        for scanned in items {
            let item = &scanned.item;
            let key = (scope.to_owned(), item.item_ref.clone());
            // Refreshing a page never leaves a replaced or unreadable previous target usable.
            self.files.remove(&key);
            if self.files.len() >= MAX_TARGETS
                || item.kind != ItemKind::Files
                || item.status != Status::Ok
            {
                continue;
            }
            let Some(path) = item.path.as_deref() else {
                continue;
            };
            let path = Path::new(path.strip_prefix(r"\\?\").unwrap_or(path));
            let Ok(Some(identity)) = guard.observe_file(path) else {
                continue;
            };
            if protection.is_protected(path)
                || scanned.identity != Some((identity.volume_serial, identity.file_id))
                || identity.is_directory
                || identity.is_reparse_point
                || identity.size_bytes != item.logical_bytes
                || item.modified_unix_ms.is_none()
                || identity
                    .modified_filetime
                    .checked_sub(FILETIME_EPOCH)
                    .map(|v| v / 10_000)
                    != item.modified_unix_ms
            {
                continue;
            }
            let classified = policy.classify(
                &CandidateInput {
                    item_ref: item.item_ref.clone(),
                    path: path.to_string_lossy().into_owned(),
                    is_dir: false,
                    logical_bytes: item.logical_bytes,
                    allocated_bytes: item.allocated_bytes,
                    modified_unix_ms: item.modified_unix_ms,
                    attributes: Attributes::default(),
                    origin: Origin::Live,
                    marker_verified: false,
                },
                prefs,
            );
            if classified.verdict.is_forbidden() {
                continue;
            }
            self.files.insert(
                key,
                FileTarget {
                    path: path.into(),
                    identity,
                },
            );
        }
    }

    pub(crate) fn startup(&self, reference: &str) -> Option<ResolvedTarget> {
        self.startup
            .get(reference)
            .cloned()
            .map(ResolvedTarget::UserStartup)
    }
    pub(crate) fn cleanup(&self, reference: &str) -> Option<ResolvedTarget> {
        self.cleanup
            .get(reference)
            .map(|entry| ResolvedTarget::RecycleCleanup(entry.target.clone()))
    }
    pub(crate) fn process(&self, reference: &str) -> Option<ResolvedTarget> {
        self.processes
            .get(reference)
            .cloned()
            .map(ResolvedTarget::Process)
    }
    pub(crate) fn file(&self, scope: &str, reference: &str) -> Option<ResolvedTarget> {
        self.files
            .get(&(scope.into(), reference.into()))
            .cloned()
            .map(ResolvedTarget::File)
    }

    /// Explicit public facts only. Local names, commands, paths and native identities never appear.
    pub(crate) fn view(&self, tool: &str, cloud: &Value, scope: &str) -> Vec<Value> {
        let mut view = Vec::new();
        let visible = |reference: &str| {
            cloud["observations"].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["entity_ref"].as_str() == Some(reference))
            })
        };
        match tool {
            "get_startup_programs" => {
                for reference in self.startup.keys().filter(|r| visible(r)) {
                    view.push(
                        json!({"action_kind":"disable_user_startup", "startup_ref":reference}),
                    );
                }
            }
            "get_cleanup_opportunities" => {
                for (reference, entry) in
                    self.cleanup.iter().filter(|(_, e)| visible(&e.entity_ref))
                {
                    view.push(
                        json!({"action_kind":"run_cleanup_adapter", "adapter_id":reference,
                    "entity_ref":entry.entity_ref, "selected_files":entry.target.files().len(),
                    "selected_logical_bytes":entry.target.bytes(), "immediate_reclaim_bytes":0}),
                    );
                }
            }
            "get_processes" => {
                for reference in self.processes.keys() {
                    view.push(
                        json!({"action_kind":"request_normal_close", "process_ref":reference}),
                    );
                    if self.processes[reference]
                        .executable_name
                        .as_deref()
                        .and_then(pcagent_actions::BrowserManager::from_executable)
                        .is_some()
                    {
                        view.push(json!({"action_kind":"open_browser_process_manager",
                            "process_ref":reference}));
                    }
                }
            }
            "get_largest_items" | "get_cleanup_candidates" => {
                for ((target_scope, reference), _) in
                    self.files.iter().filter(|((s, _), _)| s == scope)
                {
                    let visible_item = cloud["items"].as_array().is_some_and(|rows| {
                        rows.iter()
                            .any(|row| row["item_ref"].as_str() == Some(reference))
                    }) || cloud["item_context"]["items"].as_array().is_some_and(
                        |rows| {
                            rows.iter()
                                .any(|row| row["item_ref"].as_str() == Some(reference))
                        },
                    );
                    if visible_item {
                        view.push(json!({"action_kind":"recycle_selected_files", "scope_id":target_scope, "item_ref":reference}));
                    }
                }
            }
            _ => {}
        }
        view
    }

    pub(crate) fn restrict_inspection_projection(&mut self, tool: &str, cloud: &Value) {
        let visible = |reference: &str| {
            cloud["observations"].as_array().is_some_and(|rows| {
                rows.iter()
                    .any(|row| row["entity_ref"].as_str() == Some(reference))
            })
        };
        match tool {
            "get_startup_programs" => self.startup.retain(|reference, _| visible(reference)),
            "get_cleanup_opportunities" => {
                self.cleanup.retain(|_, entry| visible(&entry.entity_ref))
            }
            _ => {}
        }
    }
}
