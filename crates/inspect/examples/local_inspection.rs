//! Explicit local smoke trace; no network or model requests. Writes names-free JSON only.
use pcagent_inspect::{inspect, Query};
use serde_json::json;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("Supply a local output JSON path.")?;
    let only = std::env::args().nth(2);
    let queries = [
        ("startup", Query::Startup),
        ("services", Query::Services { limit: 500 }),
        ("events", Query::Events { days: 7, limit: 50 }),
        ("system", Query::SystemState),
        ("power", Query::Power),
        ("apps", Query::Apps { limit: 500 }),
        ("hardware", Query::Hardware),
        ("cleanup", Query::Cleanup),
        (
            "query_system",
            Query::Allowlisted {
                name: "processor",
                limit: 10,
            },
        ),
    ];
    let mut traces = Vec::new();
    for (tool, query) in queries {
        if only.as_deref().is_some_and(|filter| filter != tool) {
            continue;
        }
        let started = std::time::Instant::now();
        let result = inspect(query);
        println!(
            "{tool}: {:?}, {} observations, {} coverage entries, {} ms",
            result.status,
            result.observations.len(),
            result.coverage.len(),
            started.elapsed().as_millis()
        );
        traces.push(json!({"tool":tool,"inspection":result}));
        std::fs::write(&path, serde_json::to_vec_pretty(&traces)?)?;
    }
    Ok(())
}
