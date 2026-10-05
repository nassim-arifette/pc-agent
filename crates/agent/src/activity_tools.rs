//! Process activity tool registry and explicit C5 cloud projection.
use crate::{Args, Param, ParamKind, PrivacyContext, Sharing, ToolClass, ToolOutput, ToolSpec};
use pcagent_shared::Evidence;
use pcagent_telemetry::{Collector, ProcessActivity, DEFAULT_ACTIVITY_WINDOW_SECONDS};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashSet};

pub const TOOLS: &[ToolSpec] = &[ToolSpec {
    name: "get_process_activity",
    class: ToolClass::Inspection,
    description: "Measure CPU, memory increase/decrease, I/O throughput and observed GPU activity over one fresh window. Return processes and application groups, ranked by CPU then I/O and memory, with service attribution and explicit coverage. Growth is an observation, not proof of a leak. No commands, paths or process IDs.",
    params: &[
        Param { name: "window_seconds", description: "Fresh two-sample measurement duration in seconds (default 3).", kind: ParamKind::Integer { min: 1, max: 30 }, required: false },
        Param { name: "limit", description: "Maximum returned processes and applications, after measuring all instances (default 10).", kind: ParamKind::Integer { min: 1, max: 50 }, required: false },
    ],
}];

pub fn run(
    spec: &ToolSpec,
    args: &Args,
    collector: &mut Collector,
    sharing: Sharing,
) -> Option<ToolOutput> {
    if spec.name != "get_process_activity" {
        return None;
    }
    let activity = collector.process_activity(
        args.integer("window_seconds")
            .unwrap_or(DEFAULT_ACTIVITY_WINDOW_SECONDS),
        args.integer("limit").unwrap_or(10) as usize,
    );
    Some(project(activity, sharing, &PrivacyContext::local()))
}

/// Native generated references contain exactly eight random hex digits. Reject composite
/// PID/creation-time references or names instead of allowing their spelling into JSON keys.
fn opaque(reference: &str, prefix: &str) -> bool {
    reference.strip_prefix(prefix).is_some_and(|rest| {
        rest.len() == 8
            && rest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

fn observation(index: usize, e: &Evidence) -> Value {
    json!({
        "observation_id":format!("measurement-{}", index + 1),
        "entity_ref":e.entity_ref, "metric":e.metric, "value":e.value,
        "unit":e.unit, "readable":e.usable_value().map(|value| e.metric.format_value(value)),
        "status":e.status, "source":e.source, "origin":e.origin,
        "measured_at_unix_ms":e.window.finished_at_unix_ms, "window":e.window,
    })
}

pub(crate) fn project(
    activity: ProcessActivity,
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> ToolOutput {
    let mut process_names = Map::new();
    let mut shared_names = Map::new();
    let mut known_names = BTreeMap::new();
    let mut allowed = HashSet::new();
    let mut processes = Vec::new();
    for process in &activity.processes {
        let entity = &process.instance.entity_ref;
        if !opaque(entity, "proc-") || !opaque(&process.application_ref, "app-") {
            continue;
        }
        allowed.insert(entity.as_str());
        if let Some(name) = process
            .instance
            .executable_name
            .as_deref()
            .and_then(|name| privacy.shared_name(name))
        {
            known_names.insert(entity.clone(), name.clone());
            process_names.insert(entity.clone(), Value::String(name));
        }
        let mut services = Vec::new();
        for (index, service) in process.services.iter().enumerate() {
            // Observation-scoped references depend only on an already opaque process ref and
            // an array position. SCM service names and numeric PIDs are never identity keys.
            let service_ref = format!("service-{}-{index}", &entity[5..]);
            if let Some(name) = privacy.shared_name(&service.service_name) {
                known_names.insert(service_ref.clone(), name.clone());
                shared_names.insert(service_ref.clone(), Value::String(name));
            }
            let display_ref = format!("{service_ref}-display");
            if let Some(name) = privacy.shared_name(&service.display_name) {
                known_names.insert(display_ref.clone(), name.clone());
                shared_names.insert(display_ref, Value::String(name));
            }
            let state = match service.state.as_str() {
                "stopped" | "start_pending" | "stop_pending" | "running" | "continue_pending"
                | "pause_pending" | "paused" => service.state.as_str(),
                _ => "unknown",
            };
            services.push(json!({"entity_ref":service_ref,"state":state}));
        }
        processes.push(json!({"entity_ref":entity,
            "parent_ref":process.parent_ref.as_deref().filter(|reference|opaque(reference,"proc-")),
            "application_ref":process.application_ref,"role":process.role,
            "handles":process.handles,"threads":process.threads,"services":services}));
    }
    let mut applications = Vec::new();
    for app in &activity.applications {
        if !opaque(&app.entity_ref, "app-") {
            continue;
        }
        allowed.insert(app.entity_ref.as_str());
        if let Some(name) = app
            .name
            .as_deref()
            .and_then(|name| privacy.shared_name(name))
        {
            known_names.insert(app.entity_ref.clone(), name.clone());
            shared_names.insert(app.entity_ref.clone(), Value::String(name));
        }
        let members: Vec<_> = app
            .process_refs
            .iter()
            .filter(|reference| opaque(reference, "proc-"))
            .collect();
        applications.push(json!({"entity_ref":app.entity_ref,"process_refs":members,
            "process_count":app.process_count,"members_truncated":app.members_truncated}));
    }
    let evidence: Vec<_> = activity
        .evidence
        .iter()
        .filter(|e| allowed.contains(e.entity_ref.as_str()))
        .collect();
    let observations: Vec<_> = evidence
        .iter()
        .enumerate()
        .map(|(index, e)| observation(index, e))
        .collect();
    let complete = evidence.iter().any(|e| e.usable_value().is_some());
    let mut cloud = json!({
        "status":activity.status,"origin":activity.origin,
        "measured_at_unix_ms":activity.window.finished_at_unix_ms,"window":activity.window,
        "processes":processes,"applications":applications,"observations":observations,
        "coverage":activity.coverage,"limitations":activity.limitations,
    });
    // Structured name dictionaries are the only place where product/service names can appear.
    // Removing them in a follow-up is sufficient to honor a new anonymized-sharing choice.
    crate::privacy::anonymize(&mut cloud, &known_names);
    if sharing == Sharing::Complete {
        if !process_names.is_empty() {
            cloud["process_names"] = Value::Object(process_names);
        }
        if !shared_names.is_empty() {
            cloud["shared_names"] = Value::Object(shared_names);
        }
    }
    privacy.sanitize_value(&mut cloud);
    ToolOutput {
        status: activity.status,
        inspection_complete: complete,
        cloud,
        evidence: activity.evidence,
        findings: Vec::new(),
        processes: activity.processes.into_iter().map(|p| p.instance).collect(),
        storage_items: Vec::new(),
        limitations: activity.limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{
        Metric, Origin, ProcessCoverage, ProcessInstance, Status, Window, SCHEMA_VERSION,
    };
    use pcagent_telemetry::{
        ActivityCoverage, ActivityProcess, ApplicationActivity, ProcessRole, ServiceProcess,
    };

    fn fixture() -> ProcessActivity {
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 4000,
            duration_ms: 3000,
        };
        let instance = ProcessInstance {
            entity_ref: "proc-a1b2c3d4".into(),
            pid: 48151,
            creation_time_filetime: 123456789,
            boot_id: Some("private-boot-identity".into()),
            executable_name: Some(r"C:\Users\Alice\Programs\browser.exe".into()),
        };
        let mut evidence = vec![
            Evidence::measured(
                Metric::ProcessCpuBasisPoints,
                &instance.entity_ref,
                2500,
                "fixture",
                window.clone(),
            ),
            Evidence::measured(
                Metric::ProcessIoReadBytesPerSecond,
                &instance.entity_ref,
                2 * 1024 * 1024,
                "fixture",
                window.clone(),
            ),
            Evidence::measured(
                Metric::ProcessWorkingSetIncreaseBytes,
                &instance.entity_ref,
                10 * 1024 * 1024,
                "fixture",
                window.clone(),
            ),
            Evidence::measured(
                Metric::ProcessWorkingSetDecreaseBytes,
                &instance.entity_ref,
                0,
                "fixture",
                window.clone(),
            ),
            Evidence::measured(
                Metric::ProcessCpuBasisPoints,
                "app-b1c2d3e4",
                2500,
                "fixture",
                window.clone(),
            ),
        ];
        for e in &mut evidence {
            e.origin = Origin::SyntheticFixture;
        }
        ProcessActivity {
            schema_version: SCHEMA_VERSION,
            id: "private-local-observation".into(),
            session_id: "private-session-identity".into(),
            origin: Origin::SyntheticFixture,
            status: Status::Ok,
            window,
            evidence,
            processes: vec![ActivityProcess {
                instance,
                parent_ref: Some("proc-11112222".into()),
                application_ref: "app-b1c2d3e4".into(),
                role: ProcessRole::Application,
                handles: 4,
                threads: 2,
                session: 48152,
                services: vec![ServiceProcess {
                    pid: 48151,
                    service_name: "ExampleService".into(),
                    display_name: "Alice Browser Service".into(),
                    state: "running".into(),
                }],
            }],
            applications: vec![ApplicationActivity {
                entity_ref: "app-b1c2d3e4".into(),
                name: Some("browser.exe".into()),
                process_refs: vec!["proc-a1b2c3d4".into()],
                process_count: 1,
                members_truncated: false,
            }],
            coverage: ActivityCoverage {
                processes: ProcessCoverage {
                    status: Status::Ok,
                    enumerated: 1,
                    readable: 1,
                    permission_denied: 0,
                    unavailable: 0,
                    returned: 1,
                    enumeration_truncated: false,
                },
                stable_instances: 1,
                new_instances: 0,
                exited_instances: 0,
                pseudo_processes: Vec::new(),
                services_status: Status::Ok,
                services_attributed: 1,
                services_unattributed: 0,
                gpu_status: Status::Unsupported,
                gpu_processes: 0,
                gpu_instances_rejected: 0,
                gpu_instances_unattributed: 0,
                applications_total: 1,
                applications_returned: 1,
            },
            limitations: vec![
                "Local fixture for Alice on PRIVATE-PC; API_KEY=unit-test-secret.".into(),
            ],
        }
    }
    fn privacy() -> PrivacyContext {
        PrivacyContext::from_identifiers(
            r"C:\Users\Alice",
            "Alice",
            "PRIVATE-PC",
            &["unit-test-secret"],
        )
    }

    #[test]
    fn closed_arguments_bound_windows_and_outputs() {
        let spec = &TOOLS[0];
        assert_eq!(
            spec.definition()["function"]["parameters"]["additionalProperties"],
            false
        );
        assert!(spec.validate("{}").is_ok());
        assert!(spec.validate(r#"{"window_seconds":30,"limit":50}"#).is_ok());
        for bad in [
            r#"{"window_seconds":0}"#,
            r#"{"window_seconds":31}"#,
            r#"{"window_seconds":1.5}"#,
            r#"{"limit":51}"#,
            r#"{"pid":123}"#,
            r#"{"command":"anything"}"#,
        ] {
            assert!(spec.validate(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn privacy_projection_keeps_all_identity_metadata_local() {
        for sharing in [Sharing::Complete, Sharing::Anonymized] {
            let output = project(fixture(), sharing, &privacy());
            let text = output.cloud.to_string();
            for forbidden in [
                "Alice",
                "PRIVATE-PC",
                "unit-test-secret",
                "48151",
                "48152",
                "123456789",
                "private-boot-identity",
                "private-session-identity",
                "private-local-observation",
                "C:\\Users",
                "creation_time_filetime",
                "\"pid\"",
                "\"session\"",
                "service_name",
                "display_name",
            ] {
                assert!(!text.contains(forbidden), "{forbidden} in {text}");
            }
            assert_eq!(output.processes[0].pid, 48151);
            if sharing == Sharing::Anonymized {
                assert!(!text.contains("browser.exe") && !text.contains("ExampleService"));
            } else {
                assert_eq!(
                    output.cloud["process_names"]["proc-a1b2c3d4"],
                    "browser.exe"
                );
                assert_eq!(output.cloud["shared_names"]["app-b1c2d3e4"], "browser.exe");
            }
        }
    }

    #[test]
    fn composite_pid_refs_are_rejected_and_never_serialized_as_cloud_keys() {
        let mut activity = fixture();
        activity.processes[0].instance.entity_ref = "proc-48151-123456789".into();
        activity.evidence[0].entity_ref = "proc-48151-123456789".into();
        activity.applications[0]
            .process_refs
            .push("C:\\Users\\Alice".into());
        let output = project(activity, Sharing::Complete, &privacy());
        let text = output.cloud.to_string();
        assert!(!text.contains("48151") && !text.contains("123456789") && !text.contains("Alice"));
        assert_eq!(output.cloud["processes"], json!([]));
    }

    #[test]
    fn activity_measurements_keep_units_windows_and_human_readable_values() {
        let output = project(fixture(), Sharing::Anonymized, &privacy());
        let record = &output.cloud["observations"][1];
        assert_eq!(record["unit"], "bytes_per_second");
        assert_eq!(record["readable"], "2.0 MiB/s");
        assert_eq!(record["window"]["duration_ms"], 3000);
        assert_eq!(record["source"], "fixture");
        assert!(record["observation_id"]
            .as_str()
            .unwrap()
            .starts_with("measurement-"));
    }

    #[test]
    fn c1_verifier_accepts_measured_process_rates_and_rejects_invented_growth() {
        let mut output = project(fixture(), Sharing::Anonymized, &privacy());
        output.cloud["observation_id"] = json!("O1");
        let received = [output.cloud];
        let references = [(
            "O1".into(),
            crate::CitationKind::Observation,
            "local-test".into(),
        )];
        let context = crate::answer::Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        let check = |text: &str| {
            crate::answer::review(
                &json!({"answer":text,"confidence":"high","category":"cpu"}).to_string(),
                &context,
            )
        };
        assert!(check("proc-a1b2c3d4 CPU is 25 % [O1].").is_ok());
        assert!(check("proc-a1b2c3d4 I/O read rate is 2 MiB/s [O1].").is_ok());
        assert!(check("proc-a1b2c3d4 memory increased by 10 MiB [O1].").is_ok());
        assert!(check("proc-a1b2c3d4 memory increased by 90 MiB [O1].").is_err());
    }
}
