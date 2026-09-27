use std::{path::Path, process::Command};
/// No Nebius settings: neither in the environment nor in a per-user credentials file.
fn command() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pcagent"));
    c.env_remove("NEBIUS_API_KEY")
        .env_remove("NEBIUS_MODEL_ID")
        .env("APPDATA", std::env::temp_dir().join("pcagent-no-appdata"));
    c
}
fn cli(args: &[&str]) -> std::process::Output {
    command().args(args).output().unwrap()
}
fn doctor_json(appdata: &Path, cwd: &Path) -> serde_json::Value {
    let out = command()
        .args(["doctor", "--json"])
        .env("APPDATA", appdata)
        .current_dir(cwd)
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}
#[test]
fn credentials_come_from_the_user_file_never_from_the_current_folder() {
    let root = std::env::temp_dir().join(format!("pcagent-creds-cli-{}", std::process::id()));
    let (appdata, folder) = (root.join("appdata"), root.join("someone-elses-folder"));
    std::fs::create_dir_all(appdata.join("pcagent")).unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    let settings = "NEBIUS_API_KEY=k\nNEBIUS_MODEL_ID=nvidia/nemotron-test\n";
    std::fs::write(folder.join(".env"), settings).unwrap();

    let ignored = doctor_json(&appdata, &folder);
    assert_eq!(ignored["nebius_key_configured"], false);
    assert_eq!(ignored["credentials_file_found"], false);

    std::fs::write(appdata.join("pcagent").join(".env"), settings).unwrap();
    let used = doctor_json(&appdata, &folder);
    assert_eq!(used["nebius_key_configured"], true);
    assert_eq!(used["nebius_model_configured"], true);
    assert_eq!(used["credentials_file_found"], true);
    assert_eq!(used["cloud_check"], "not_requested");
    std::fs::remove_dir_all(&root).unwrap();
}
#[test]
fn help_is_available_without_configuration() {
    let out = cli(&["--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("diagnose"));
}
#[test]
fn doctor_does_not_silently_contact_cloud() {
    let out = cli(&["doctor", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["cloud_check"], "not_requested");
    assert_eq!(value["nebius_key_configured"], false);
    assert_eq!(value["read_only"], true);
}
#[test]
fn default_diagnosis_works_without_an_api_key() {
    let out = cli(&["diagnose", "Why is my PC slow?", "--json"]);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["mode"], "local_rules");
    assert_eq!(value["stats"]["inference_calls"], 0);
    assert_eq!(value["stats"]["tool_calls"], 0);
    assert!(value["error"].is_null());
}
#[test]
fn explicit_cloud_without_key_fails_without_claiming_success() {
    let out = cli(&["diagnose", "test", "--allow-cloud"]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}
#[test]
fn preview_does_not_send_or_leak_process_metadata() {
    let out = cli(&["diagnose", "test", "--preview-cloud"]);
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["sent"], false);
    assert!(value["initial_observation"].get("processes").is_none());
    assert!(value["initial_observation"].get("session_id").is_none());
}
#[test]
fn storage_scan_is_read_only_and_reports_items() {
    let dir = std::env::temp_dir().join(format!("pcagent-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.bin"), vec![1u8; 4096]).unwrap();
    let out = cli(&["storage", dir.to_str().unwrap(), "--json", "--limit", "5"]);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["largest"]["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(std::fs::read(dir.join("a.bin")).unwrap().len(), 4096);
    std::fs::remove_dir_all(&dir).unwrap();
}
#[test]
fn preview_lists_tools_and_counts_scopes_without_paths() {
    let dir = std::env::temp_dir().join(format!("pcagent-preview-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = cli(&[
        "diagnose",
        "test",
        "--preview-cloud",
        "--scan-root",
        dir.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(!text.contains(&*dir.to_string_lossy()));
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["authorized_scopes"], 1);
    assert!(value["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t == "get_largest_items"));
    std::fs::remove_dir_all(&dir).unwrap();
}
#[test]
fn stored_cloud_prohibition_overrides_allow_cloud() {
    let file = std::env::temp_dir().join(format!("pcagent-prefs-{}.json", std::process::id()));
    std::fs::write(&file, r#"{"schema_version":1}"#).unwrap();
    let out = cli(&[
        "diagnose",
        "test",
        "--allow-cloud",
        "--preferences",
        file.to_str().unwrap(),
    ]);
    std::fs::remove_file(&file).unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cloud_allowed"));
}
#[test]
fn action_proposals_require_cloud_and_demo_cache_is_bounded() {
    for args in [
        &["diagnose", "test", "--propose-actions"][..],
        &["diagnose", "test", "--offline", "--propose-actions"],
        &["diagnose", "test", "--journal-dir", "x"],
    ] {
        let out = cli(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("requires"),
            "{args:?}"
        );
    }
    assert!(!cli(&["demo-cache", "--mb", "0"]).status.success());
    assert!(!cli(&["demo-cache", "--mb", "257"]).status.success());
}
