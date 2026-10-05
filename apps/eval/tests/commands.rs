//! Portable command tests: no credentials, network, current-PC measurements or real user files.
use pcagent_agent::replay::Scenario;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

fn command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_pcagent-eval"));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    cmd.env_remove("NEBIUS_API_KEY")
        .env_remove("NEBIUS_MODEL_ID")
        .env(
            "APPDATA",
            std::env::temp_dir().join("pcagent-eval-no-credentials"),
        );
    cmd
}
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../eval/scenarios")
        .join(format!("{name}.json"))
}
fn parsed(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "Invalid output ({e}); stderr={}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
struct Temp(PathBuf);

fn assert_preserved(expected: &Value, actual: &Value, location: &str) {
    match expected {
        Value::Object(fields) => {
            for (key, value) in fields {
                assert_preserved(value, &actual[key], &format!("{location}.{key}"));
            }
        }
        Value::Array(items) => {
            let actual = actual.as_array().expect("preserved array");
            assert_eq!(items.len(), actual.len(), "{location}");
            for (index, (expected, actual)) in items.iter().zip(actual).enumerate() {
                assert_preserved(expected, actual, &format!("{location}[{index}]"));
            }
        }
        _ => assert_eq!(expected, actual, "{location}"),
    }
}
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-eval-test"));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn every_checked_in_fixture_replays_without_credentials_and_preserves_values() {
    for name in [
        "memory-pressure",
        "cpu-busy",
        "disk-latency",
        "no-contention",
        "storage-full",
    ] {
        let path = fixture(name);
        let scenario: Scenario = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        scenario.validate().unwrap();
        for evidence in &scenario.initial.evidence {
            assert_eq!(evidence.unit, evidence.metric.unit());
        }
        let out = command().arg("replay").arg(&path).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let result = parsed(&out);
        assert_eq!(result["mode"], "offline_replay");
        assert_eq!(result["scenario_origin"], "synthetic_fixture");
        assert_eq!(result["scenario_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(result["tool_registry_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(result["replay_complete"], true);
        assert_eq!(result["served_captures"], scenario.calls.len());
        assert_eq!(result["reused_calls"], 0);
        assert_eq!(result["replay_misses"], json!([]));
        assert_eq!(
            result["initial_message"]["initial_observation"]["origin"],
            "synthetic_fixture"
        );
        for (index, call) in scenario.calls.iter().enumerate() {
            for (key, value) in call.output.cloud.as_object().unwrap() {
                // Human-readable values may be added, but every captured raw field must remain.
                assert_preserved(
                    value,
                    &result["calls"][index]["output"][key],
                    &format!("{name}: {key}"),
                );
            }
        }
    }
}

#[test]
fn calibration_is_offline_and_refuses_synthetic_or_unreviewed_inputs_before_output() {
    let temp = Temp::new();
    let annotation_path = temp.file("annotations.json");
    let output = temp.file("calibration.json");
    let provenance = temp.file("provenance.json");
    let review = json!({"source":"human","quality_scale":"routing_human_0_to_4",
        "human_reviewed":true,"trigger":"low_confidence","primary_quality":1,
        "ultra_quality":3,"primary_critical_error":false,"ultra_critical_error":false,
        "rationale":"Synthetic CLI refusal fixture, not an actual human review."});
    let pair = json!({
        "scenario":fixture("cpu-busy"),"primary_result":"missing-primary.json",
        "ultra_result":"missing-ultra.json","scenario_sha256":"a".repeat(64),
        "tool_registry_sha256":"b".repeat(64),"question_sha256":"c".repeat(64),
        "sharing":"anonymized","primary_trace_sha256":"d".repeat(64),
        "ultra_trace_sha256":"e".repeat(64),"review":review
    });
    let mut annotations = json!({
        "schema_version":1,
        "super_model_id":"nvidia/nemotron-super-test",
        "ultra_model_id":"nvidia/nemotron-ultra-test",
        "reviewed_at_unix_ms":1,
        "maximum_extra_cost_usd":"0.01",
        "pairs":[pair]
    });
    for human_reviewed in [true, false] {
        annotations["pairs"][0]["review"]["human_reviewed"] = human_reviewed.into();
        fs::write(&annotation_path, serde_json::to_vec(&annotations).unwrap()).unwrap();
        let result = command()
            .arg("calibrate-nemotron")
            .arg("--annotations")
            .arg(&annotation_path)
            .arg("--out")
            .arg(&output)
            .arg("--provenance-out")
            .arg(&provenance)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        let reason = String::from_utf8_lossy(&result.stderr);
        assert!(
            reason.contains(if human_reviewed {
                "calibration_requires_a_real_frozen_capture"
            } else {
                "calibration_requires_explicit_human_labels_and_rationale"
            }),
            "{reason}"
        );
        assert!(!output.exists());
        assert!(!provenance.exists());
    }
    let result = command()
        .arg("calibrate-nemotron")
        .arg("--annotations")
        .arg(&annotation_path)
        .arg("--out")
        .arg(&output)
        .arg("--provenance-out")
        .arg(&provenance)
        .arg("--allow-cloud")
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert!(!output.exists());
    assert!(!provenance.exists());
}

#[test]
fn missing_capture_is_explicit_and_produces_a_nonzero_exit_with_a_report() {
    let dir = Temp::new();
    let calls = dir.file("calls.json");
    fs::write(
        &calls,
        r#"[{"tool":"get_processes","arguments":{"limit":3}}]"#,
    )
    .unwrap();
    let out = command()
        .arg("replay")
        .arg(fixture("memory-pressure"))
        .arg("--calls")
        .arg(calls)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let result = parsed(&out);
    assert_eq!(result["replay_complete"], false);
    assert_eq!(result["replay_misses"][0]["arguments"]["limit"], 3);
    assert_eq!(result["calls"][0]["output"]["status"], "unsupported");
    assert_eq!(result["calls"][0]["output"]["inspection_complete"], false);
}

#[test]
fn preview_is_offline_and_never_emits_local_scenario_metadata() {
    let dir = Temp::new();
    let mut value: Value =
        serde_json::from_slice(&fs::read(fixture("memory-pressure")).unwrap()).unwrap();
    value["notes"] = json!(["PRIVATE_GRADER_NOTE"]);
    value["initial"]["session_id"] = json!("PRIVATE_SESSION_ID");
    value["initial"]["processes"] = json!([{"entity_ref":"proc-private", "pid":424242, "creation_time_filetime":987654321,
        "boot_id":"PRIVATE_BOOT", "executable_name":"PRIVATE_PROGRAM.exe"}]);
    let input = dir.file("scenario.json");
    fs::write(&input, value.to_string()).unwrap();
    let out = command()
        .arg("replay")
        .arg(input)
        .args(["--preview-cloud", "--anonymize"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout.clone()).unwrap();
    for marker in [
        "PRIVATE_GRADER",
        "PRIVATE_SESSION",
        "PRIVATE_BOOT",
        "PRIVATE_PROGRAM",
        "424242",
        "987654321",
    ] {
        assert!(!text.contains(marker), "leaked {marker}");
    }
    assert_eq!(parsed(&out)["mode"], "preview");
}

#[test]
fn invalid_scenarios_and_call_lists_fail_before_model_configuration() {
    let dir = Temp::new();
    let mut value: Value =
        serde_json::from_slice(&fs::read(fixture("memory-pressure")).unwrap()).unwrap();
    value["schema_version"] = json!(999);
    let input = dir.file("bad.json");
    fs::write(&input, value.to_string()).unwrap();
    let out = command()
        .arg("replay")
        .arg(input)
        .arg("--allow-cloud")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("schema version"));
    let calls = dir.file("calls.json");
    fs::write(&calls, r#"[{"tool":"get_system_snapshot"},{"tool":"get_processes","arguments":{"path":"PRIVATE_INPUT"}}]"#).unwrap();
    let out = command()
        .arg("replay")
        .arg(fixture("memory-pressure"))
        .arg("--calls")
        .arg(calls)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("PRIVATE_INPUT"));
}

#[test]
fn extra_capture_calls_are_validated_before_any_collection_or_output_reservation() {
    let dir = Temp::new();
    let calls = dir.file("extra.json");
    let destination = dir.file("capture.json");
    fs::write(
        &calls,
        r#"[{"tool":"get_processes","arguments":{"PRIVATE_EXTRA_ARGUMENT":1}}]"#,
    )
    .unwrap();
    let output = command()
        .args(["capture", "--name", "test", "--allow-recording", "--out"])
        .arg(&destination)
        .arg("--extra-calls")
        .arg(calls)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!destination.exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_EXTRA_ARGUMENT"));
}

#[test]
fn result_files_never_overwrite_inputs_or_enter_a_git_tree() {
    let dir = Temp::new();
    let target = dir.file("output.json");
    fs::write(&target, "KEEP_ME").unwrap();
    let out = command()
        .arg("replay")
        .arg(fixture("memory-pressure"))
        .arg("--out")
        .arg(&target)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert_eq!(fs::read_to_string(&target).unwrap(), "KEEP_ME");
    fs::create_dir(dir.file(".git")).unwrap();
    let target = dir.file("new.json");
    let out = command()
        .arg("replay")
        .arg(fixture("memory-pressure"))
        .arg("--out")
        .arg(&target)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!target.exists());
    assert!(String::from_utf8_lossy(&out.stderr).contains("outside a Git"));
}

#[test]
fn opt_ins_are_required_and_failed_cloud_setup_removes_only_the_reservation() {
    let dir = Temp::new();
    let output = dir.file("scenario.json");
    let out = command()
        .args(["capture", "--name", "test", "--out"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--allow-recording"));
    assert!(!output.exists());
    let out = command()
        .args(["mcp", "--scenario"])
        .arg(fixture("memory-pressure"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--allow-cloud"));
    let out = command()
        .arg("replay")
        .arg(fixture("memory-pressure"))
        .args(["--allow-cloud", "--out"])
        .arg(&output)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("NEBIUS_API_KEY"));
    assert!(!output.exists());
}

#[test]
fn mcp_subprocess_uses_only_jsonrpc_stdout_and_never_collects_on_a_miss() {
    let mut child = command()
        .args(["mcp", "--allow-cloud", "--scenario"])
        .arg(fixture("memory-pressure"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let messages = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_initial_context","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_system_snapshot","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_processes","arguments":{"limit":3}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/list","params":{}}),
    ];
    {
        let mut input = child.stdin.take().unwrap();
        for message in messages {
            writeln!(input, "{message}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let replies: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(replies.len(), 5);
    let processes = replies[4]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "get_processes")
        .unwrap();
    let description = processes["description"].as_str().unwrap();
    assert!(description.contains("Frozen replay: only these exact argument objects were captured:"));
    assert!(description.contains("Omitted defaults differ from explicit defaults"));
    assert_eq!(
        replies[1]["result"]["structuredContent"]["initial_observation"]["observation_id"],
        "O0"
    );
    assert_eq!(replies[2]["result"]["isError"], false);
    assert_eq!(replies[3]["result"]["isError"], true);
    assert_eq!(
        replies[3]["result"]["structuredContent"]["status"],
        "unsupported"
    );
}

#[test]
fn mcp_recording_links_actual_calls_to_the_same_replay_identity_without_a_client_bill() {
    let dir = Temp::new();
    let trace = dir.file("mcp-session.json");
    let baseline = command()
        .arg("replay")
        .arg(fixture("memory-pressure"))
        .output()
        .unwrap();
    let baseline = parsed(&baseline);
    let mut child = command()
        .args(["mcp", "--allow-cloud", "--allow-recording", "--out"])
        .arg(&trace)
        .arg("--scenario")
        .arg(fixture("memory-pressure"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = child.stdin.take().unwrap();
        for request in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_system_snapshot","arguments":{}}}),
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_processes","arguments":{"limit":3}}}),
        ] {
            writeln!(input, "{request}").unwrap();
        }
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    for frame in String::from_utf8(output.stdout).unwrap().lines() {
        assert_eq!(
            serde_json::from_str::<Value>(frame).unwrap()["jsonrpc"],
            "2.0"
        );
    }
    let trace: Value = serde_json::from_slice(&fs::read(trace).unwrap()).unwrap();
    for key in ["scenario_sha256", "tool_registry_sha256", "scenario_origin"] {
        assert_eq!(trace[key], baseline[key]);
    }
    assert_eq!(trace["mode"], "mcp_replay_trace");
    assert_eq!(trace["calls"].as_array().unwrap().len(), 2);
    assert_eq!(trace["calls"][0]["tool"], "get_system_snapshot");
    assert_eq!(trace["replay_complete"], false);
    assert_eq!(trace["replay_misses"].as_array().unwrap().len(), 1);
    assert_eq!(trace["transport_complete"], true);
    for key in ["answer", "usage", "cost_usd"] {
        assert!(trace[key].is_null());
    }
}

#[test]
fn mcp_recording_requires_separate_local_recording_consent_and_never_truncates() {
    let dir = Temp::new();
    let trace = dir.file("mcp-session.json");
    let output = command()
        .args(["mcp", "--allow-cloud", "--out"])
        .arg(&trace)
        .arg("--scenario")
        .arg(fixture("memory-pressure"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(!trace.exists());
    fs::write(&trace, "KEEP_ME").unwrap();
    let output = command()
        .args(["mcp", "--allow-cloud", "--allow-recording", "--out"])
        .arg(&trace)
        .arg("--scenario")
        .arg(fixture("memory-pressure"))
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fs::read_to_string(trace).unwrap(), "KEEP_ME");
    assert!(output.stdout.is_empty());
}
