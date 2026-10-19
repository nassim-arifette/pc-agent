//! Live smoke test against Nebius. Ignored by default; run with real credentials:
//! `cargo test -p pcagent-agent --test live_nebius -- --ignored --nocapture`
use pcagent_agent::{investigate, Config, Nebius, SnapshotTools};
use pcagent_shared::{Origin, Snapshot};

/// Exercises the repair transport deterministically: inspection data and the duplicate first
/// submission are synthetic; only subsequent named repair requests reach the paid provider.
#[test]
#[ignore = "uses saved Nebius credentials and makes paid repair requests with synthetic data"]
fn live_final_answer_repair_uses_named_tool() {
    use pcagent_agent::{
        credentials::Credentials, AgentError, FunctionCall, Model, Reply, ToolCall, ToolChoice,
    };
    use pcagent_shared::{Evidence, Metric, Window};
    use serde_json::{json, Value};
    use std::{collections::VecDeque, time::Duration, time::Instant};

    struct RepairProbe {
        seed: VecDeque<Reply>,
        real: Nebius,
        real_requests: usize,
    }
    impl Model for RepairProbe {
        fn complete(
            &mut self,
            messages: &[Value],
            tools: &Value,
            choice: ToolChoice,
            timeout: Duration,
        ) -> Result<Reply, AgentError> {
            if let Some(reply) = self.seed.pop_front() {
                assert_eq!(choice, ToolChoice::Required);
                return Ok(reply);
            }
            assert_eq!(choice, ToolChoice::SubmitAnswer);
            self.real_requests += 1;
            let started = Instant::now();
            let reply = self.real.complete(messages, tools, choice, timeout)?;
            eprintln!(
                "real_request={} choice={choice:?} seconds={:.1} calls={} final_calls={} input={:?} output={:?} rejected={:?}",
                self.real_requests,
                started.elapsed().as_secs_f64(),
                reply.calls.len(),
                reply.calls.iter().filter(|c| c.function.name == "submit_answer").count(),
                reply.prompt_tokens,
                reply.completion_tokens,
                reply.rejected,
            );
            Ok(reply)
        }
    }
    let call = |id: &str, name: &str, arguments: Value| ToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall {
            name: name.into(),
            arguments: arguments.to_string(),
        },
    };
    let answer = json!({
        "answer":"Ce jeu de données mesure 8 Gio de mémoire totale [O1].",
        "confidence":"medium","category":"memory","action_refs":[],"follow_ups":[]
    });
    let credentials = Credentials::load();
    let config = Config::from_lookup(|key| credentials.get(key)).expect("Nebius configuration");
    let mut model = RepairProbe {
        seed: VecDeque::from([
            Reply {
                calls: vec![call("synthetic-read", "get_system_snapshot", json!({}))],
                ..Reply::default()
            },
            Reply {
                calls: vec![
                    call("synthetic-final-a", "submit_answer", answer.clone()),
                    call("synthetic-final-b", "submit_answer", answer),
                ],
                ..Reply::default()
            },
        ]),
        real: Nebius::new(config).unwrap(),
        real_requests: 0,
    };
    let mut snapshot = Snapshot::empty("synthetic-repair-probe");
    snapshot.origin = Origin::SyntheticFixture;
    snapshot.evidence.push(Evidence::measured(
        Metric::PhysicalTotalBytes,
        "system",
        8_589_934_592,
        "synthetic-repair-probe",
        Window::instant(),
    ));
    let mut tools = SnapshotTools::new(|_, _| snapshot.clone());
    let report = investigate(
        &mut model,
        "Décrivez uniquement la mémoire totale mesurée dans ce jeu de données synthétique, en une phrase française avec sa source. Ce n'est pas un diagnostic d'un PC réel.",
        &snapshot,
        &mut tools,
        |_| {},
    );
    assert!(model.real_requests > 0);
    assert_eq!(report.stats.inspection_calls, 1);
    assert_eq!(report.stats.tool_calls, 1);
    assert!(report.error.is_none(), "{:?}", report.error);
    assert!(report
        .answer
        .expect("a corrected answer")
        .unverified
        .is_empty());
}

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
