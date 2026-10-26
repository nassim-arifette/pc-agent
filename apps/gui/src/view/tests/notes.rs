use super::*;

/// The notes and missing-value reasons the window words itself are their producers' texts.
#[test]
fn notes_and_missing_reasons_match_their_producers() {
    let sources = [
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telemetry/src/windows/mod.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telemetry/src/windows/services.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telemetry/src/activity.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telemetry/src/counters.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telemetry/src/gpu.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/store/src/samples/history.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/agent/src/projections/history.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/tools/src/history_tools.rs"
        )),
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app.rs")),
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app/state.rs")),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/history_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/measurements.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/action_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/selection_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/preview_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/settings_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/turn_commands.rs"
        )),
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/app/worker.rs")),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/storage_commands.rs"
        )),
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/app/elevation_commands.rs"
        )),
    ]
    .concat();
    for (key, text) in NOTES {
        // The local Explain note is used by name where app.rs adds it.
        let text = if key == "note.local_evidence" {
            "view::LOCAL_EVIDENCE_NOTE"
        } else {
            text
        };
        assert!(sources.contains(text), "{key}: {text}");
    }
    assert!(sources.contains(&format!("\"{}{{}}{}\"", RANKED.0, RANKED.1)));
    for (code, _) in RANKED_KEYS {
        assert!(
            pcagent_telemetry::ProcessSort::parse(code).is_some(),
            "{code}"
        );
    }
    for (text, code) in MISSING_REASONS {
        assert!(sources.contains(text), "{code}: {text}");
    }
    let keys: BTreeSet<&str> = NOTES.iter().map(|(key, _)| *key).collect();
    assert_eq!(keys.len(), NOTES.len());
}
