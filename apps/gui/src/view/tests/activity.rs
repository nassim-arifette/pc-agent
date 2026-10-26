use super::*;

#[test]
fn applications_keep_their_own_sums_members_and_services() {
    let window = serde_json::json!({"started_at_unix_ms":1000,"finished_at_unix_ms":3000,"duration_ms":2000});
    let ev = |metric: &str, entity: &str, value: Option<u64>| {
        serde_json::json!({"id":format!("{entity}-{metric}"),
        "metric":metric,"entity_ref":entity,"value":value,"unit":"","source":"fixture","window":window,
        "status": if value.is_some() {"ok"} else {"partial"},"origin":"synthetic_fixture","limitations":["not measured"]})
    };
    let instance = |r: &str, pid: u32, created: u64, exe: &str| serde_json::json!({"entity_ref":r,"pid":pid,"creation_time_filetime":created,"boot_id":null,"executable_name":exe});
    let process = |r: &str, pid: u32, created: u64, services: serde_json::Value| {
        serde_json::json!({
        "instance":instance(r, pid, created, "browser.exe"),
        "parent_ref":null,"application_ref":"app-1","role":"application","handles":1,"threads":1,"session":1,"services":services})
    };
    let raw: pcagent_telemetry::ProcessActivity = serde_json::from_value(serde_json::json!({
        "schema_version":1,"id":"a","session_id":"s","origin":"synthetic_fixture","status":"ok","window":window,
        "processes":[process("proc-1",10,5,serde_json::json!([])),
            process("proc-2",11,6,serde_json::json!([{"pid":11,"service_name":"Spooler","display_name":"Print Spooler","state":"running"}]))],
        "applications":[{"entity_ref":"app-1","name":"browser.exe","process_refs":["proc-1","proc-2","proc-0","proc-w"],"process_count":12,"members_truncated":false}],
        "evidence":[ev("process_cpu_basis_points","app-1",Some(940)),ev("process_working_set_bytes","app-1",Some(3 << 30)),
            ev("process_cpu_basis_points","proc-1",Some(700)),ev("process_gpu_basis_points","app-1",None)],
        "coverage":{"processes":{"status":"ok","enumerated":2,"readable":2,"permission_denied":0,"unavailable":0,"returned":2,"enumeration_truncated":false},
            "stable_instances":2,"new_instances":0,"exited_instances":0,"pseudo_processes":[],"services_status":"ok",
            "services_attributed":1,"services_unattributed":0,"gpu_status":"unsupported","gpu_processes":0,
            "gpu_instances_rejected":0,"gpu_instances_unattributed":0,"applications_total":5,"applications_returned":1},
        "limitations":[]
    })).unwrap();
    // The collector's registry also knows members ranked too low to be returned: the group's
    // earliest browser process, and an even earlier embedded runtime it started.
    let registry: Vec<ProcessInstance> = raw
        .processes
        .iter()
        .map(|p| p.instance.clone())
        .chain([
            serde_json::from_value(instance("proc-0", 9, 3, "browser.exe")).unwrap(),
            serde_json::from_value(instance("proc-w", 8, 2, "runtime.exe")).unwrap(),
        ])
        .collect();
    let lookup = |r: &str| registry.iter().find(|p| p.entity_ref == r).cloned();
    let names = |pid: u32, created: u64| match (pid, created) {
        (9, 3) => Some("Example Browser".to_owned()),
        (10, 5) => Some("  Example Browser ".to_owned()),
        (8, 2) => Some("Example Runtime".to_owned()),
        // A process that refused a query-only handle has no readable name.
        _ => None,
    };
    let view = activity(&raw, lookup, &names);
    let app = &view.applications[0];
    assert_eq!((app.name.as_str(), app.process_count), ("browser.exe", 12));
    // Named after its earliest browser process, never after the runtime it embeds.
    assert_eq!(app.readable_name.as_deref(), Some("Example Browser"));
    // The group's own sum, not its first member's value.
    assert_eq!(app.cpu.value, Some(940));
    assert_eq!(app.members.len(), 2);
    assert_eq!(app.members[0].cpu.value, Some(700));
    assert_eq!(
        (
            app.members[0].name.as_str(),
            app.members[0].readable_name.as_deref()
        ),
        ("browser.exe", Some("Example Browser"))
    );
    // Unknown: the page shows the executable name.
    assert_eq!(app.members[1].readable_name, None);
    assert_eq!(app.services[0].display_name, "Print Spooler");
    // Unknown GPU stays unknown, with its reason.
    assert_eq!(app.gpu.value, None);
    assert_eq!(app.gpu.note.as_deref(), Some("not measured"));
    assert_eq!(view.applications_total, 5);
    // Without readable names, or with one that only repeats the executable name, the page
    // keeps the executable name.
    let plain = activity(&raw, lookup, &no_readable_names);
    assert!(plain.applications[0].readable_name.is_none());
    assert!(plain.applications[0]
        .members
        .iter()
        .all(|m| m.readable_name.is_none()));
    let echo = activity(&raw, lookup, &|_, _| Some("BROWSER.EXE".to_owned()));
    assert!(echo.applications[0].readable_name.is_none());
    // A group whose members are all unknown to the registry stays unnamed.
    let unknown = activity(&raw, |_| None, &names);
    assert!(unknown.applications[0].readable_name.is_none());
    // Readable names stay in the view: the selection source is the raw activity.
    let shared = serde_json::to_string(&raw).unwrap();
    assert!(!shared.contains("Example Browser"));
    assert!(!app.unnamed && !app.members[0].unnamed);
    // Without a name the English fallback stays, flagged for the window to word itself.
    let mut raw = raw;
    raw.applications[0].name = None;
    raw.processes[0].instance.executable_name = Some("  ".into());
    let view = activity(&raw, lookup, &no_readable_names);
    let app = &view.applications[0];
    assert_eq!(app.name, "Unnamed application");
    assert!(app.unnamed && app.members[0].unnamed && !app.members[1].unnamed);
}
