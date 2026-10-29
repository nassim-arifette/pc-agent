use super::*;
use crate::{
    answer, Args, CitationKind, PrivacyContext, Sharing, SnapshotTools, StorageContext, Toolbox,
};
use pcagent_shared::{Evidence, Metric, Origin, ProcessInstance, Snapshot, Status, Window};
use serde_json::{json, Value};

mod battery;
mod captures;
mod engine;
mod history;
mod storage;
mod watches;

fn capture() -> Snapshot {
    let mut snapshot = Snapshot::empty("private-selection-session");
    snapshot.id = "private-selection-snapshot".into();
    snapshot.origin = Origin::SyntheticFixture;
    snapshot.processes.push(ProcessInstance {
        entity_ref: "proc-a1b2c3d4".into(),
        pid: 777777,
        creation_time_filetime: 987654321,
        boot_id: Some("private-boot".into()),
        executable_name: Some(r"C:\Users\Alice\Programs\browser.exe".into()),
    });
    let window = Window {
        started_at_unix_ms: 1000,
        finished_at_unix_ms: 2000,
        duration_ms: 1000,
    };
    snapshot.evidence.push(Evidence::measured(
        Metric::ProcessWorkingSetBytes,
        "proc-a1b2c3d4",
        200_000_000,
        "fixture measured process",
        window.clone(),
    ));
    snapshot.evidence.push(Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        9300,
        "fixture GetSystemTimes",
        window,
    ));
    for evidence in &mut snapshot.evidence {
        evidence.origin = Origin::SyntheticFixture;
    }
    snapshot
}

fn privacy() -> PrivacyContext {
    PrivacyContext::from_identifiers(
        r"C:\Users\Alice",
        "Alice",
        "private-PC",
        &["fixture-secret-token"],
    )
}

fn frozen(source: &SelectionSource, target: &SelectionTarget) -> FrozenSelection {
    FrozenSelection::freeze(
        source,
        target,
        SelectionIntent::Explain,
        6000,
        Sharing::Anonymized,
        false,
        &privacy(),
    )
    .unwrap()
}

fn checked(text: &str, received: Value) -> bool {
    answer::review(&json!({"answer":text,"confidence":"low","category":"insufficient_evidence","action_refs":[],"follow_ups":[]}).to_string(), &answer::Context { received: &[received], references: &[("O0".into(), CitationKind::Observation, "selected-local".into())], plans: &[] }).is_ok()
}
