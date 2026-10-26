use super::*;

#[test]
fn a_process_target_reads_as_its_program_with_a_pid_only_to_tell_two_apart() {
    let target = |reference: &str, pid: u32, exe: Option<&str>| {
        serde_json::json!({"target_ref":reference,"target":{"kind":"process",
            "identity":{"pid":pid,"creation_time_filetime":u64::from(pid),"boot_id":"boot"},
            "executable_name":exe}})
    };
    let proposal: pcagent_actions::ActionPlan = serde_json::from_value(serde_json::json!({
        "schema_version":1,"plan_id":"plan-1","state":"awaiting_approval",
        "action_kind":"request_normal_close","target_refs":["proc-a","proc-b","proc-c","proc-d"],
        "items":[target("proc-a",40,Some("browser.exe")),target("proc-b",41,Some("browser.exe")),
            target("proc-c",4242,Some("editor.exe")),target("proc-d",77,None)],
        "evidence_refs":[],"estimated_immediate_reclaim_bytes":null,
        "recovery":"user_can_restart_application","preconditions":["same_process_instance"],
        "requires_local_approval":true,"created_at_unix_ms":0,"expires_at_unix_ms":1,
        "fingerprint":"fixture"
    }))
    .unwrap();
    let shown = |view: &PlanView| -> Vec<(String, Option<String>)> {
        view.targets
            .iter()
            .map(|t| (t.label.clone(), t.executable_name.clone()))
            .collect()
    };
    let named = |label: &str, exe: Option<&str>| (label.to_owned(), exe.map(str::to_owned));
    assert_eq!(
        shown(&plan(&proposal, None, &names)),
        [
            named("Example Browser (PID 40)", Some("browser.exe")),
            named("Example Browser (PID 41)", Some("browser.exe")),
            named("Example Editor", Some("editor.exe")),
            named("process (PID 77)", None),
        ]
    );
    // Without readable names: executable names, with no tooltip repeating them.
    assert_eq!(
        shown(&plan(&proposal, None, &no_readable_names))[1..3],
        [
            named("browser.exe (PID 41)", None),
            named("editor.exe", None)
        ]
    );
}
