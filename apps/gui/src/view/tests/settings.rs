use super::*;

#[test]
fn app_info_lists_the_capabilities_of_this_pc_with_unknown_probes_as_null() {
    let probes = Probes {
        elevated_helper: true,
        nvidia_gpu: Some(false),
        battery: None,
        ntfs_volumes: Some(true),
    };
    let nowhere = std::env::temp_dir()
        .join(pcagent_shared::new_id("pcagent-no-settings"))
        .join(".env");
    let info = app_info(
        &Credentials::load_file(&nowhere),
        capabilities(false, probes),
    );
    let shown = serde_json::to_value(&info).unwrap();
    assert_eq!(
        shown["capabilities"],
        serde_json::json!({"windows": cfg!(windows), "history_sampler": false,
            "elevated_helper": true, "nvidia_gpu": false, "battery": null, "ntfs_volumes": true})
    );
    // The existing fields keep their shape.
    assert!(shown["version"].is_string() && shown["cloud"].is_object());
    assert_eq!(shown["models"].as_array().map(Vec::len), Some(MODELS.len()));
    // Every name is a capability the agent knows; per-turn flows are not listed.
    for name in info.capabilities.keys() {
        assert!(Capability::ALL.iter().any(|c| c.name() == *name), "{name}");
    }
    for per_turn in ["storage_scopes", "web_search", "action_proposals"] {
        assert!(!info.capabilities.contains_key(per_turn));
    }
    let fresh = capabilities(true, Probes::default());
    assert_eq!(fresh["history_sampler"], Some(true));
    assert_eq!(fresh["elevated_helper"], Some(false));
    assert!(fresh["nvidia_gpu"].is_none() && fresh["battery"].is_none());
}

#[test]
fn cloud_state_shows_a_key_hint_but_never_the_key() {
    let dir = std::env::temp_dir().join(format!("pcagent-gui-{}", std::process::id()));
    let file = dir.join(".env");
    pcagent_agent::credentials::save(
        &file,
        &[
            ("NEBIUS_API_KEY", "v1.secret-key-abcd"),
            ("NEBIUS_MODEL_ID", "nvidia/nemotron-3-super-120b-a12b"),
        ],
    )
    .unwrap();
    let creds = Credentials::load_file(&file);
    let c = cloud(&creds);
    assert!(c.ready && c.key_configured);
    assert_eq!(c.key_hint.as_deref(), Some("abcd"));
    assert_eq!(c.key_source, Some("file"));
    assert!(!serde_json::to_string(&c).unwrap().contains("secret"));
    std::fs::remove_dir_all(&dir).unwrap();
}
