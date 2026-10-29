//! Capability registry: what each model tool needs, so that a turn offers only the tools that work
//! on this PC and in this turn. A host detects a small closed set of capabilities at turn start
//! with probes it already runs ([`Probes`]); [`REQUIREMENTS`] is the one table that maps every
//! tool to the capabilities it needs, and [`VARIANTS`] swaps in a constant variant of a tool whose
//! canonical text names something a missing capability takes away. Absent tools are not declared
//! to the model at all, and the system prompt follows the declared tools.
//!
//! Available means that the capability exists here and now, not that data already exists: the
//! elevated tool stays offered without any approved result, because its `needs_admin` reply is what
//! makes the app offer the check to the person.
//!
//! The global registry identity (`routing::CalibrationRuntime`) and replays keep the fixed full
//! catalog of `crate::catalog::inspection_tools`; only the per-turn toolbox is narrowed.
use crate::tools::ToolSpec;
use pcagent_shared::Snapshot;
use std::collections::BTreeSet;

/// What a tool can need. A closed set: a new capability changes this module, its table and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// The native Windows collectors; other systems only build with stubs that measure nothing.
    Windows,
    /// A persistent local sampler: stored history and one-second recordings.
    HistorySampler,
    /// At least one storage scope the person approved for this turn.
    StorageScopes,
    /// A host flow that runs locally approved helper tasks (the desktop app on Windows), with
    /// `pcagent-elevated.exe` installed next to it.
    ElevatedHelper,
    /// Web lookups (C6): cloud mode, the person's consent and a saved Tavily key.
    WebSearch,
    /// The action journal opened: the model may prepare plans for local review.
    ActionProposals,
    /// NVML lists at least one NVIDIA GPU.
    NvidiaGpu,
    /// Windows lists at least one system battery, readable or not.
    Battery,
    /// At least one fixed NTFS volume, the only kind the helper's MFT inventory reads.
    NtfsVolumes,
}
use Capability::*;

impl Capability {
    pub const ALL: [Capability; 9] = [
        Windows,
        HistorySampler,
        StorageScopes,
        ElevatedHelper,
        WebSearch,
        ActionProposals,
        NvidiaGpu,
        Battery,
        NtfsVolumes,
    ];
    /// The stable snake_case name the window and the CLI show.
    pub fn name(self) -> &'static str {
        match self {
            Windows => "windows",
            HistorySampler => "history_sampler",
            StorageScopes => "storage_scopes",
            ElevatedHelper => "elevated_helper",
            WebSearch => "web_search",
            ActionProposals => "action_proposals",
            NvidiaGpu => "nvidia_gpu",
            Battery => "battery",
            NtfsVolumes => "ntfs_volumes",
        }
    }
}

/// The capabilities of one turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities(BTreeSet<Capability>);
impl Capabilities {
    /// Nothing: only the tools that need no capability are offered.
    pub fn none() -> Self {
        Self::default()
    }
    /// Every capability: the whole catalog, in its offered variants.
    pub fn all() -> Self {
        Self(Capability::ALL.into_iter().collect())
    }
    pub fn with(mut self, capability: Capability, available: bool) -> Self {
        if available {
            self.0.insert(capability);
        } else {
            self.0.remove(&capability);
        }
        self
    }
    /// A probe result: one not measured yet (`None`) counts as present, see [`Probes`].
    pub fn with_probe(self, capability: Capability, probe: Option<bool>) -> Self {
        self.with(capability, probe != Some(false))
    }
    pub fn contains(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }
    /// A turn's capabilities: the platform, the optional flows of its toolbox and the host's
    /// probes of this PC.
    pub fn of_turn(windows: bool, flows: Flows, probes: Probes) -> Self {
        Self::none()
            .with(Windows, windows)
            .with(HistorySampler, flows.history_sampler)
            .with(StorageScopes, flows.storage_scopes)
            .with(ElevatedHelper, probes.elevated_helper)
            .with(WebSearch, flows.web_search)
            .with(ActionProposals, flows.action_proposals)
            .with_probe(NvidiaGpu, probes.nvidia_gpu)
            .with_probe(Battery, probes.battery)
            .with_probe(NtfsVolumes, probes.ntfs_volumes)
    }
}

/// The optional flows a toolbox was given for a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flows {
    pub history_sampler: bool,
    pub storage_scopes: bool,
    pub web_search: bool,
    pub action_proposals: bool,
}

/// What a host measured about this PC before the turn, with probes it already runs. A hardware
/// probe that has not run yet (`None`) counts as present: the tool reports a missing device
/// itself, while hiding a working tool would cost an answer. The platform and the optional flows
/// of a turn (history, approved scopes, web lookups, proposals) are read from the toolbox itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Probes {
    /// The host runs locally approved helper tasks and `pcagent-elevated.exe` is installed.
    pub elevated_helper: bool,
    pub nvidia_gpu: Option<bool>,
    pub battery: Option<bool>,
    pub ntfs_volumes: Option<bool>,
}
impl Probes {
    /// A host without the desktop app's helper flow (CLI, evaluation): NVIDIA GPUs as NVML
    /// listed them in the turn's fresh snapshot; the battery and NTFS volumes are not probed.
    pub fn without_helper(initial: &Snapshot) -> Self {
        Self {
            elevated_helper: false,
            nvidia_gpu: Some(!initial.gpus.is_empty()),
            battery: None,
            ntfs_volumes: None,
        }
    }
}

/// Every tool the agent can offer, with the capabilities it needs. `submit_answer` needs nothing
/// and is always declared by the controller.
pub const REQUIREMENTS: &[(&str, &[Capability])] = &[
    // Live collectors (pcagent-telemetry, pcagent-inspect) measure on Windows only.
    ("get_system_snapshot", &[Windows]),
    ("get_processes", &[Windows]),
    ("get_process_details", &[Windows]),
    ("get_process_tree", &[Windows]),
    ("get_process_activity", &[Windows]),
    ("get_startup_programs", &[Windows]),
    ("get_services", &[Windows]),
    ("get_recent_events", &[Windows]),
    ("get_system_state", &[Windows]),
    ("get_power_and_thermal", &[Windows]),
    ("get_installed_apps", &[Windows]),
    ("get_hardware_inventory", &[Windows]),
    ("get_drivers", &[Windows]),
    ("get_cleanup_opportunities", &[Windows]),
    ("query_system", &[Windows]),
    // Storage scans run on every system, but only on scopes approved for this turn.
    ("list_storage_scopes", &[StorageScopes]),
    ("get_storage_overview", &[StorageScopes]),
    ("start_storage_scan", &[StorageScopes]),
    ("get_largest_items", &[StorageScopes]),
    ("get_cleanup_candidates", &[StorageScopes]),
    // Local context.
    ("get_user_preferences", &[]),
    ("get_selected_evidence", &[]),
    // The persistent sampler stores Windows measurements.
    ("get_metric_history", &[Windows, HistorySampler]),
    ("get_activity_events", &[Windows, HistorySampler]),
    ("get_watch_alerts", &[Windows, HistorySampler]),
    ("record_activity", &[Windows, HistorySampler]),
    // Results of helper tasks the person approves in the desktop app.
    (
        "get_approved_elevated_inspection",
        &[Windows, ElevatedHelper],
    ),
    // Proposals act on Windows targets: Settings pages, Explorer, Run entries, processes.
    ("prepare_action_plan", &[Windows, ActionProposals]),
    ("propose_elevated_check", &[Windows, ElevatedHelper]),
    // Web lookups compose their queries from Windows inspections of this turn.
    ("lookup_driver_notes", &[Windows, WebSearch]),
    ("lookup_known_issue", &[Windows, WebSearch]),
    ("lookup_upgrade_price", &[Windows, WebSearch]),
    ("lookup_pc_specs", &[Windows, WebSearch]),
];

/// Helper tasks `get_approved_elevated_inspection` offers, with what each needs beyond the tool
/// itself. Performance traces use their separate guardian, not the deep-analysis batch.
pub const ELEVATED_TASKS: &[(&str, &[Capability])] = &[
    ("system_caches", &[]),
    ("mft_inventory", &[NtfsVolumes]),
    ("shadow_storage", &[]),
    ("disk_reliability", &[]),
    ("process_images", &[]),
    ("battery_energy", &[Battery]),
];

/// A constant variant of a tool, offered when every capability in `missing` is absent. Its
/// description and enums name only what remains; its name, class and argument names are those of
/// the canonical tool, so a call runs the same code.
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub missing: &'static [Capability],
    pub spec: ToolSpec,
}

/// Tried in order: the first row of a tool whose `missing` capabilities are all absent applies;
/// without one, the canonical tool is offered.
pub const VARIANTS: &[Variant] = &[
    Variant {
        missing: &[Battery, NtfsVolumes],
        spec: crate::catalog::elevated::PROPOSE_CORE_TASKS,
    },
    Variant {
        missing: &[Battery],
        spec: crate::catalog::elevated::PROPOSE_WITHOUT_BATTERY,
    },
    Variant {
        missing: &[NtfsVolumes],
        spec: crate::catalog::elevated::PROPOSE_WITHOUT_NTFS,
    },
    Variant {
        missing: &[NvidiaGpu],
        spec: crate::catalog::SNAPSHOT_WITHOUT_NVIDIA,
    },
    Variant {
        missing: &[Battery],
        spec: crate::catalog::system::POWER_WITHOUT_BATTERY,
    },
    Variant {
        missing: &[Battery, NtfsVolumes],
        spec: crate::catalog::elevated::CORE_TASKS,
    },
    Variant {
        missing: &[Battery],
        spec: crate::catalog::elevated::TASKS_WITHOUT_BATTERY,
    },
    Variant {
        missing: &[NtfsVolumes],
        spec: crate::catalog::elevated::TASKS_WITHOUT_NTFS,
    },
    Variant {
        missing: &[StorageScopes],
        spec: crate::catalog::proposals::PREPARE_WITHOUT_STORAGE,
    },
];

/// The capabilities `tool` needs; `None` for a tool the table does not list.
pub fn requirements(tool: &str) -> Option<&'static [Capability]> {
    REQUIREMENTS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, needs)| *needs)
}

/// What `capabilities` lacks for `tool`: empty when it can be offered, `None` for a tool the
/// table does not list.
pub fn missing(tool: &str, capabilities: &Capabilities) -> Option<Vec<Capability>> {
    requirements(tool).map(|needs| {
        needs
            .iter()
            .copied()
            .filter(|need| !capabilities.contains(*need))
            .collect()
    })
}

/// The spec offered for `spec` this turn: `None` when one of its needs is missing (a tool the
/// table does not list is never offered), else its variant for the missing capabilities.
pub fn offered(spec: &ToolSpec, capabilities: &Capabilities) -> Option<ToolSpec> {
    if !missing(spec.name, capabilities)?.is_empty() {
        return None;
    }
    Some(
        VARIANTS
            .iter()
            .find(|variant| {
                variant.spec.name == spec.name
                    && variant.missing.iter().all(|c| !capabilities.contains(*c))
            })
            .map_or(*spec, |variant| variant.spec),
    )
}

/// The tools of `specs` offered this turn, in their order.
pub fn catalog(specs: &[ToolSpec], capabilities: &Capabilities) -> Vec<ToolSpec> {
    specs
        .iter()
        .filter_map(|spec| offered(spec, capabilities))
        .collect()
}

/// Every tool any host can declare, canonical.
#[cfg(test)]
pub(crate) fn registry() -> Vec<ToolSpec> {
    let mut specs = crate::catalog::inspection_tools();
    specs.extend(crate::catalog::external::TOOLS);
    specs.extend([
        crate::PREPARE_ACTION_PLAN,
        crate::selection::GET_SELECTED_EVIDENCE,
    ]);
    specs
}
/// Whether `text` names `tool` as a whole word (`get_processes` is not `get_process_tree`).
#[cfg(test)]
pub(crate) fn names_tool(text: &str, tool: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(tool).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(word)
            && !text[at + tool.len()..].chars().next().is_some_and(word)
    })
}
/// Every combination of the capabilities.
#[cfg(test)]
pub(crate) fn every_combination() -> impl Iterator<Item = Capabilities> {
    (0u32..1 << Capability::ALL.len()).map(|bits| {
        Capability::ALL
            .iter()
            .enumerate()
            .fold(Capabilities::none(), |set, (i, c)| {
                set.with(*c, bits & (1 << i) != 0)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ParamKind;
    use crate::{catalog::external, PREPARE_ACTION_PLAN, SUBMIT_ANSWER};
    fn names(specs: &[ToolSpec]) -> Vec<&'static str> {
        specs.iter().map(|s| s.name).collect()
    }
    fn offered_names(capabilities: &Capabilities) -> Vec<&'static str> {
        names(&catalog(&registry(), capabilities))
    }
    fn enum_values(spec: &ToolSpec, param: &str) -> Vec<&'static str> {
        spec.params
            .iter()
            .find(|p| p.name == param)
            .map(|p| match p.kind {
                ParamKind::OneOf(values) | ParamKind::UniqueChoices(values) => values.to_vec(),
                _ => panic!("{param} is not an enum"),
            })
            .unwrap_or_default()
    }
    const WINDOWS_ONLY: &[&str] = &[
        "get_system_snapshot",
        "get_processes",
        "get_process_details",
        "get_process_tree",
        "get_process_activity",
        "get_startup_programs",
        "get_services",
        "get_recent_events",
        "get_system_state",
        "get_power_and_thermal",
        "get_installed_apps",
        "get_hardware_inventory",
        "get_drivers",
        "get_cleanup_opportunities",
        "query_system",
        "get_metric_history",
        "get_activity_events",
        "get_watch_alerts",
        "record_activity",
        "get_approved_elevated_inspection",
        "propose_elevated_check",
        "prepare_action_plan",
        "lookup_driver_notes",
        "lookup_known_issue",
        "lookup_upgrade_price",
        "lookup_pc_specs",
    ];
    const HISTORY: &[&str] = &[
        "get_metric_history",
        "get_activity_events",
        "get_watch_alerts",
        "record_activity",
    ];
    const SCOPES: &[&str] = &[
        "list_storage_scopes",
        "get_storage_overview",
        "start_storage_scan",
        "get_largest_items",
        "get_cleanup_candidates",
    ];

    #[test]
    fn every_tool_has_one_row_and_every_row_and_variant_a_tool() {
        let registry = registry();
        for spec in &registry {
            let rows = REQUIREMENTS.iter().filter(|(n, _)| *n == spec.name).count();
            assert_eq!(rows, 1, "{}", spec.name);
        }
        assert_eq!(REQUIREMENTS.len(), registry.len());
        for variant in VARIANTS {
            let canonical = registry
                .iter()
                .find(|s| s.name == variant.spec.name)
                .expect("a variant of a registered tool");
            assert_eq!(
                std::mem::discriminant(&canonical.class),
                std::mem::discriminant(&variant.spec.class)
            );
            assert_ne!(canonical.definition(), variant.spec.definition());
            for param in variant.spec.params {
                let original = canonical
                    .params
                    .iter()
                    .find(|p| p.name == param.name)
                    .expect("a variant keeps the canonical argument names");
                assert_eq!(param.required, original.required, "{}", param.name);
                if let ParamKind::OneOf(values) | ParamKind::UniqueChoices(values) = param.kind {
                    let all = enum_values(canonical, param.name);
                    assert!(values.iter().all(|v| all.contains(v)), "{}", param.name);
                }
            }
        }
        let tasks = enum_values(&crate::catalog::elevated::GET_APPROVED_INSPECTION, "task");
        assert!(ELEVATED_TASKS.iter().all(|(task, _)| tasks.contains(task)));
    }

    #[test]
    fn other_systems_get_no_windows_only_tool() {
        for capabilities in every_combination().filter(|c| !c.contains(Windows)) {
            let offered = offered_names(&capabilities);
            assert!(
                offered.iter().all(|name| !WINDOWS_ONLY.contains(name)),
                "{offered:?}"
            );
            assert!(offered.contains(&"get_user_preferences"));
        }
        let elsewhere = Capabilities::all().with(Windows, false);
        let mut expected = vec!["get_user_preferences", "get_selected_evidence"];
        expected.extend(SCOPES);
        let mut offered = offered_names(&elsewhere);
        offered.sort_unstable();
        expected.sort_unstable();
        assert_eq!(offered, expected);
    }

    #[test]
    fn missing_flows_remove_exactly_their_tools() {
        let all = offered_names(&Capabilities::all());
        assert_eq!(all, names(&registry()));
        for (capability, removed) in [
            (HistorySampler, HISTORY),
            (StorageScopes, SCOPES),
            (
                ElevatedHelper,
                &["get_approved_elevated_inspection", "propose_elevated_check"][..],
            ),
            (ActionProposals, &["prepare_action_plan"][..]),
            (
                WebSearch,
                &[
                    "lookup_driver_notes",
                    "lookup_known_issue",
                    "lookup_upgrade_price",
                    "lookup_pc_specs",
                ][..],
            ),
        ] {
            let offered = offered_names(&Capabilities::all().with(capability, false));
            let expected: Vec<_> = all
                .iter()
                .copied()
                .filter(|name| !removed.contains(name))
                .collect();
            assert_eq!(offered, expected, "{capability:?}");
            assert_eq!(
                missing(removed[0], &Capabilities::all().with(capability, false)),
                Some(vec![capability])
            );
        }
        assert_eq!(missing("run_shell", &Capabilities::all()), None);
        let unknown = ToolSpec {
            name: "run_shell",
            ..crate::selection::GET_SELECTED_EVIDENCE
        };
        assert!(offered(&unknown, &Capabilities::all()).is_none());
    }

    #[test]
    fn web_lookups_and_proposals_keep_their_canonical_definitions() {
        let offered = catalog(&registry(), &Capabilities::all());
        for canonical in external::TOOLS.iter().chain([&PREPARE_ACTION_PLAN]) {
            let spec = offered.iter().find(|s| s.name == canonical.name).unwrap();
            assert_eq!(spec.definition(), canonical.definition());
        }
        // The only change with every capability: tasks no host offers are not declared.
        for spec in &offered {
            let canonical = registry()
                .into_iter()
                .find(|s| s.name == spec.name)
                .unwrap();
            if spec.name == "get_approved_elevated_inspection" {
                let tasks = enum_values(spec, "task");
                assert_eq!(
                    tasks,
                    ELEVATED_TASKS.iter().map(|(t, _)| *t).collect::<Vec<_>>()
                );
            } else {
                assert_eq!(spec.definition(), canonical.definition(), "{}", spec.name);
            }
        }
    }

    #[test]
    fn proposals_without_scopes_offer_no_storage_action() {
        let spec = offered(
            &PREPARE_ACTION_PLAN,
            &Capabilities::all().with(StorageScopes, false),
        )
        .unwrap();
        assert_eq!(
            enum_values(&spec, "action_kind"),
            [
                "open_settings_page",
                "run_cleanup_adapter",
                "disable_user_startup",
                "request_normal_close",
                "open_browser_process_manager"
            ]
        );
        assert!(spec
            .params
            .iter()
            .all(|p| !matches!(p.name, "scope_id" | "item_ref")));
        assert!(spec
            .validate(r#"{"action_kind":"open_in_explorer"}"#)
            .is_err());
        assert!(spec
            .validate(r#"{"action_kind":"open_settings_page","settings_page":"storagesense"}"#)
            .is_ok());
    }

    #[test]
    fn hardware_probes_change_texts_not_tools() {
        let all = Capabilities::all();
        let pick = |capabilities: &Capabilities, name: &str| {
            catalog(&registry(), capabilities)
                .into_iter()
                .find(|s| s.name == name)
                .unwrap()
        };
        for capability in [NvidiaGpu, Battery, NtfsVolumes] {
            assert_eq!(
                offered_names(&all.clone().with(capability, false)),
                offered_names(&all)
            );
        }
        let snapshot = pick(&all.clone().with(NvidiaGpu, false), "get_system_snapshot");
        assert!(!snapshot.description.contains("when NVML is available"));
        assert!(snapshot.description.contains("no GPU temperature"));
        assert!(pick(&all, "get_system_snapshot")
            .description
            .contains("when NVML is available"));
        let power = pick(&all.clone().with(Battery, false), "get_power_and_thermal");
        assert!(!power.description.contains("battery design"));
        for (missing, expected) in [
            (
                vec![],
                vec![
                    "system_caches",
                    "mft_inventory",
                    "shadow_storage",
                    "disk_reliability",
                    "process_images",
                    "battery_energy",
                ],
            ),
            (
                vec![Battery],
                vec![
                    "system_caches",
                    "mft_inventory",
                    "shadow_storage",
                    "disk_reliability",
                    "process_images",
                ],
            ),
            (
                vec![NtfsVolumes],
                vec![
                    "system_caches",
                    "shadow_storage",
                    "disk_reliability",
                    "process_images",
                    "battery_energy",
                ],
            ),
            (
                vec![Battery, NtfsVolumes],
                vec![
                    "system_caches",
                    "shadow_storage",
                    "disk_reliability",
                    "process_images",
                ],
            ),
        ] {
            let capabilities = missing
                .iter()
                .fold(all.clone(), |set, c| set.with(*c, false));
            let spec = pick(&capabilities, "get_approved_elevated_inspection");
            assert_eq!(enum_values(&spec, "task"), expected, "{missing:?}");
            let proposal = pick(&capabilities, "propose_elevated_check");
            assert_eq!(enum_values(&proposal, "tasks"), expected, "{missing:?}");
            // The description lists exactly the offered tasks, as the enum does.
            for (task, _) in ELEVATED_TASKS {
                assert_eq!(
                    names_tool(spec.description, task),
                    expected.contains(task),
                    "{task}"
                );
            }
            for never in ["performance_trace", "deep_analysis"] {
                assert!(!names_tool(spec.description, never));
            }
            // Each listed task keeps its canonical explanation.
            let canonical = crate::catalog::elevated::GET_APPROVED_INSPECTION.description;
            for clause in spec
                .description
                .split_once("Tasks: ")
                .unwrap()
                .1
                .split_once(". Call it")
                .unwrap()
                .0
                .split("; ")
            {
                assert!(canonical.contains(clause), "{clause}");
            }
            assert!(spec
                .description
                .ends_with(canonical.split_once(". Call it").unwrap().1));
        }
    }

    #[test]
    fn unknown_probes_count_as_present() {
        let measured = Capabilities::none()
            .with_probe(NvidiaGpu, None)
            .with_probe(Battery, Some(true))
            .with_probe(NtfsVolumes, Some(false));
        assert!(measured.contains(NvidiaGpu) && measured.contains(Battery));
        assert!(!measured.contains(NtfsVolumes));
        let mut snapshot = Snapshot::empty("probe-fixture");
        assert_eq!(Probes::without_helper(&snapshot).nvidia_gpu, Some(false));
        snapshot.gpus.push(pcagent_shared::GpuDevice {
            entity_ref: "gpu-nvidia-0".into(),
            name: None,
            source: "NVML".into(),
        });
        let probes = Probes::without_helper(&snapshot);
        assert_eq!(probes.nvidia_gpu, Some(true));
        assert!(!probes.elevated_helper && probes.battery.is_none());
    }

    #[test]
    fn the_global_registry_identity_ignores_per_turn_filtering() {
        use crate::routing::{registry_sha256, CalibrationRuntime};
        let mut canonical = crate::catalog::inspection_tools();
        canonical.extend([PREPARE_ACTION_PLAN, crate::selection::GET_SELECTED_EVIDENCE]);
        let global = CalibrationRuntime::current().registered_tool_registry_sha256;
        assert_eq!(
            global,
            registry_sha256(&canonical, crate::answer::definition())
        );
        // Turns restricted in any way keep that identity: hosts narrow their own toolbox only.
        for probes in [
            Probes::default(),
            Probes {
                elevated_helper: true,
                nvidia_gpu: Some(false),
                battery: Some(false),
                ntfs_volumes: Some(false),
            },
        ] {
            let declared = catalog(
                &canonical,
                &Capabilities::of_turn(true, Flows::default(), probes),
            );
            assert_ne!(declared.len(), canonical.len());
            assert_ne!(
                registry_sha256(&declared, crate::answer::definition_for(false)),
                global
            );
            assert_eq!(
                CalibrationRuntime::current().registered_tool_registry_sha256,
                global
            );
        }
        // The canonical definitions this module offers as is are those of the identity.
        for spec in catalog(&canonical, &Capabilities::all()) {
            if spec.name != "get_approved_elevated_inspection" {
                let original = canonical.iter().find(|s| s.name == spec.name).unwrap();
                assert_eq!(spec.definition(), original.definition());
            }
        }
    }

    #[test]
    fn declared_texts_name_only_declared_tools() {
        let mut all_names = names(&registry());
        all_names.push(SUBMIT_ANSWER);
        for capabilities in every_combination() {
            let offered = catalog(&registry(), &capabilities);
            let declared = names(&offered);
            for spec in &offered {
                let mut texts = vec![spec.description];
                texts.extend(spec.params.iter().map(|p| p.description));
                for text in texts {
                    for absent in all_names
                        .iter()
                        .filter(|name| !declared.contains(name) && **name != SUBMIT_ANSWER)
                    {
                        assert!(
                            !names_tool(text, absent),
                            "{} names {absent} without it ({capabilities:?})",
                            spec.name
                        );
                    }
                }
            }
        }
    }
}
