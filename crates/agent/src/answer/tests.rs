use super::*;
use serde_json::json;

mod battery;
mod grounding;
mod measurements;
mod submission;

fn received() -> Vec<Value> {
    vec![
        json!({"question":"Pourquoi mon PC rame ? J'ai 8 Go de RAM.",
            "initial_observation":{"observation_id":"O0","measurements":{
                "values":{"system":{"physical_total_bytes":17_179_869_184_u64,
                    "physical_available_bytes":1_653_562_163_u64,
                    "cpu_busy_basis_points":1_740,
                    "disk_read_latency_microseconds":1_300},
                    "proc-1a2b3c4d":{"process_working_set_bytes":2_480_000_000_u64}},
                "units":{"physical_total_bytes":"bytes","physical_available_bytes":"bytes",
                    "cpu_busy_basis_points":"basis_points_of_total_cpu",
                    "disk_read_latency_microseconds":"microseconds",
                    "process_working_set_bytes":"bytes"}}},
            "process_names":{"proc-1a2b3c4d":"chrome.exe"},
            "findings":[{"id":"F1","facts":["Hard-fault page reads: 12.4 pages/s."]}]}),
        json!({"observation_id":"O1","metric":"cpu_busy_basis_points",
            "points":[{"value":500,"status":"ok"},{"value":900,"status":"ok"}],
            "observed_interval_ms":2000,
            "installed_memory_bytes":{"value":17_179_869_184_u64,"status":"ok"},
            "volume":{"free_bytes":64_000_000_000_u64,"total_bytes":512_000_000_000_u64},
            "items":[{"name":"chrome.exe"}]}),
    ]
}
fn refs() -> Vec<(String, CitationKind, String)> {
    vec![
        (
            "O0".into(),
            CitationKind::Observation,
            "initial-real".into(),
        ),
        ("O1".into(), CitationKind::Observation, "obs-real".into()),
        ("F1".into(), CitationKind::Finding, "finding-real".into()),
    ]
}
fn submit(text: &str) -> String {
    json!({"answer":text,"confidence":"medium","category":"memory"}).to_string()
}
fn check(text: &str) -> Result<Answer, Rejection> {
    let received = received();
    let references = refs();
    let plans = vec!["plan-1".to_owned()];
    review(
        &submit(text),
        &Context {
            received: &received,
            references: &references,
            plans: &plans,
        },
    )
}
fn kinds(text: &str) -> Vec<IssueKind> {
    match check(text) {
        Ok(_) => Vec::new(),
        Err((issues, _)) => issues.into_iter().map(|i| i.kind).collect(),
    }
}
