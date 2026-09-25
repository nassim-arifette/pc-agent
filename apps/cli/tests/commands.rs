use std::process::Command;
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pcagent")).args(args)
        .env_remove("NEBIUS_API_KEY").env_remove("NEBIUS_MODEL_ID").output().unwrap()
}
#[test]
fn help_is_available_without_configuration() {
    let out = cli(&["--help"]); assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("diagnose"));
}
#[test]
fn doctor_does_not_silently_contact_cloud() {
    let out = cli(&["doctor","--json"]);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["cloud_check"],"not_requested");
    assert_eq!(value["nebius_key_configured"],false);
    assert_eq!(value["read_only"],true);
}
#[test]
fn default_diagnosis_works_without_an_api_key() {
    let out = cli(&["diagnose","Why is my PC slow?","--json"]);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["mode"],"local_rules");
    assert_eq!(value["stats"]["inference_calls"],0);
    assert_eq!(value["stats"]["tool_calls"],0);
    assert!(value["error"].is_null());
}
#[test]
fn explicit_cloud_without_key_fails_without_claiming_success() {
    let out = cli(&["diagnose","test","--allow-cloud"]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}
#[test]
fn preview_does_not_send_or_leak_process_metadata() {
    let out = cli(&["diagnose","test","--preview-cloud"]);
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["sent"],false);
    assert!(value["initial_observation"].get("processes").is_none());
    assert!(value["initial_observation"].get("session_id").is_none());
}
