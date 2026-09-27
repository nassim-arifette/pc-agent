//! Live smoke test against Nebius. Ignored by default; run with real credentials:
//! `cargo test -p pcagent-agent --test live_nebius -- --ignored --nocapture`
use pcagent_agent::{investigate, Config, Nebius, SnapshotTools};
use pcagent_shared::{Origin, Snapshot};

#[test]
#[ignore = "needs NEBIUS_API_KEY and NEBIUS_MODEL_ID and network access"]
fn live_tool_loop_completes() {
    let config = Config::from_env().expect("NEBIUS_API_KEY and NEBIUS_MODEL_ID");
    let mut model = Nebius::new(config).unwrap();
    let snapshot = || {
        let mut s = Snapshot::empty("live-smoke");
        s.origin = Origin::SyntheticFixture;
        s
    };
    let mut tools = SnapshotTools::new(|_, _| snapshot());
    let report = investigate(
        &mut model,
        "My PC feels slow.",
        &snapshot(),
        &mut tools,
        |e| {
            eprintln!("tool event: {}", serde_json::to_string(e).unwrap());
        },
    );
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report.mode, "nebius");
    assert!(report.error.is_none(), "{:?}", report.error);
}
