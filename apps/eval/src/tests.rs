use super::*;
use pcagent_agent::{
    AgentError, FunctionCall, Pricing, Reply, SnapshotTools, ToolCall, ToolChoice,
};
use pcagent_shared::{Evidence, Metric, Origin, Window};
use std::time::Duration;

struct Script {
    request: usize,
    invented: bool,
}
impl Model for Script {
    fn complete(
        &mut self,
        messages: &[Value],
        _: &Value,
        _: ToolChoice,
        _: Duration,
    ) -> std::result::Result<Reply, AgentError> {
        // Capture and replay must reach the real controller, including source citations and
        // its repair messages. There is no provider transport in this test.
        assert!(messages.len() >= 2);
        let (name, arguments) = if self.request == 0 {
            ("get_system_snapshot", json!({}))
        } else {
            (
                "submit_answer",
                json!({
                    "answer": if self.invented { "CPU usage is 63 % [O1]." } else { "CPU usage is 5 % [O1]." },
                    "confidence": "medium", "category": "cpu", "action_refs": [], "follow_ups": []
                }),
            )
        };
        self.request += 1;
        Ok(Reply {
            calls: vec![ToolCall {
                id: format!("call-{}", self.request),
                kind: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: arguments.to_string(),
                },
            }],
            prompt_tokens: Some(100),
            completion_tokens: Some(10),
            reasoning_tokens: Some(0),
            total_tokens: Some(110),
            ..Reply::default()
        })
    }
    fn pricing(&self) -> Option<Pricing> {
        Some(Pricing {
            model_id: "synthetic-model".into(),
            prompt_usd_per_token: "0.000001".into(),
            completion_usd_per_token: "0.000002".into(),
            source: "synthetic test prices".into(),
            read_at_unix_ms: 1,
        })
    }
}
fn snapshot() -> Snapshot {
    let mut s = Snapshot::empty("test");
    s.origin = Origin::SyntheticFixture;
    s.evidence.push(Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        500,
        "fixture",
        Window::instant(),
    ));
    s.evidence[0].origin = Origin::SyntheticFixture;
    s
}

#[test]
fn captured_investigation_replays_with_citations_usage_cost_and_no_collectors() {
    let mut scenario = Scenario::new(
        "test".into(),
        Some("How busy is the CPU?".into()),
        snapshot(),
    );
    let mut recorder = Recorder::new(SnapshotTools::new(|_, _| snapshot()));
    let mut scripted = Script {
        request: 0,
        invented: false,
    };
    let (run, ok) = investigate(
        &mut scripted,
        "synthetic-model".into(),
        scenario.question.as_deref().unwrap(),
        &scenario.initial,
        &mut recorder,
    );
    assert!(ok);
    assert_eq!(run.report["answer"]["unverified"], json!([]));
    assert_eq!(run.report["stats"]["inference_calls"], 2);
    assert_eq!(run.report["stats"]["cost"]["usd"], "0.00024");
    scenario.run = Some(run);
    scenario.calls = recorder.into_parts().1;
    let scenario: Scenario =
        serde_json::from_str(&serde_json::to_string(&scenario).unwrap()).unwrap();
    let mut replay = ReplayTools::new(&scenario, Sharing::Anonymized).unwrap();
    let mut scripted = Script {
        request: 0,
        invented: false,
    };
    let (run, ok) = investigate(
        &mut scripted,
        "synthetic-model".into(),
        scenario.question.as_deref().unwrap(),
        &scenario.initial,
        &mut replay,
    );
    assert!(ok);
    assert!(replay.misses.is_empty());
    let original = &scenario.run.unwrap().report;
    assert_eq!(run.report["answer"]["text"], original["answer"]["text"]);
    assert_eq!(run.report["answer"]["citations"][0]["reference"], "O1");
    assert_eq!(run.report["stats"]["cost"], original["stats"]["cost"]);
}

#[test]
fn persistent_invented_numbers_remain_flagged_in_the_recorded_report() {
    let mut tools = SnapshotTools::new(|_, _| snapshot());
    let mut scripted = Script {
        request: 0,
        invented: true,
    };
    let (run, success) = investigate(
        &mut scripted,
        "synthetic-model".into(),
        "How busy is the CPU?",
        &snapshot(),
        &mut tools,
    );
    assert!(!success);
    assert!(!run.report["answer"]["unverified"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(run.report["answer"]["repairs"], pcagent_agent::MAX_REPAIRS);
    assert!(run.report["stats"]["cost"].is_object());
    assert_eq!(
        run.report["stats"]["requests"].as_array().unwrap().len(),
        scripted.request
    );
}
