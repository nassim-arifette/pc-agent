//! Synthetic transport tests: no network or saved calibration.
use super::*;
use crate::routing::{
    Calibration, CalibrationCase, CalibrationOrigin, EscalationReason, RoutedModel, RoutingMode,
    RoutingSettings,
};
use pcagent_shared::{Metric, Window};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const SUPER: &str = "nvidia/nemotron-super-fixture";
const ULTRA: &str = "nvidia/nemotron-ultra-fixture";
type Messages = Arc<Mutex<Vec<Vec<Value>>>>;
struct Scripted {
    id: &'static str,
    replies: VecDeque<Result<Reply, AgentError>>,
    messages: Messages,
    price: Pricing,
}
impl Model for Scripted {
    fn complete(
        &mut self,
        messages: &[Value],
        _: &Value,
        _: ToolChoice,
        _: Duration,
    ) -> Result<Reply, AgentError> {
        self.messages.lock().unwrap().push(messages.to_vec());
        self.replies
            .pop_front()
            .expect("unexpected inference request")
    }
    fn model_id(&self) -> Option<&str> {
        Some(self.id)
    }
    fn pricing(&self) -> Option<Pricing> {
        Some(self.price.clone())
    }
}
fn script(id: &'static str, replies: Vec<Result<Reply, AgentError>>) -> (Scripted, Messages) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let (prompt, completion) = if id == SUPER {
        ("0.0000003", "0.0000009")
    } else {
        ("0.000003", "0.000009")
    };
    (
        Scripted {
            id,
            replies: replies.into(),
            messages: messages.clone(),
            price: Pricing {
                model_id: id.into(),
                prompt_usd_per_token: prompt.into(),
                completion_usd_per_token: completion.into(),
                source: "synthetic fixture".into(),
                read_at_unix_ms: 1,
            },
        },
        messages,
    )
}
fn call(id: &str, tool: &str, arguments: Value) -> Reply {
    Reply {
        calls: vec![ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: tool.into(),
                arguments: arguments.to_string(),
            },
        }],
        prompt_tokens: Some(9),
        completion_tokens: Some(1),
        total_tokens: Some(10),
        ..Reply::default()
    }
}
fn inspect(id: &str) -> Reply {
    call(id, "get_system_snapshot", json!({}))
}
fn answer(id: &str, text: &str, confidence: &str) -> Reply {
    call(
        id,
        SUBMIT_ANSWER,
        json!({"answer":text,"confidence":confidence,"category":"other"}),
    )
}
fn snapshot() -> Snapshot {
    let mut snapshot = Snapshot::empty("synthetic-routing-fixture");
    snapshot.origin = Origin::SyntheticFixture;
    snapshot.evidence.push(Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        500,
        "synthetic fixture",
        Window::instant(),
    ));
    snapshot
}
fn routed(primary: Scripted, ultra: Scripted, reason: EscalationReason) -> RoutedModel<Scripted> {
    let settings = RoutingSettings {
        mode: RoutingMode::Automatic,
        super_model_id: Some(SUPER.into()),
        ultra_model_id: Some(ULTRA.into()),
    };
    // Live/human flags are deliberately fabricated only inside this in-memory unit fixture.
    // No fixture is saved, accepted by the eval harness or claimed to measure model quality.
    let calibration = Calibration {
        schema_version: crate::routing::CALIBRATION_SCHEMA_VERSION,
        runtime: crate::routing::CalibrationRuntime::current(),
        super_model_id: SUPER.into(),
        ultra_model_id: ULTRA.into(),
        reviewed_at_unix_ms: 1,
        maximum_extra_cost_usd: "0.1".into(),
        cases: (1..=3)
            .map(|i| CalibrationCase {
                capture_sha256: format!("{i:064x}"),
                primary_trace_sha256: format!("{:064x}", i + 10),
                ultra_trace_sha256: format!("{:064x}", i + 20),
                origin: CalibrationOrigin::Live,
                human_reviewed: true,
                trigger: reason,
                primary_requests: 2,
                primary_quality: 1,
                ultra_quality: 3,
                primary_critical_error: false,
                ultra_critical_error: false,
                primary_cost_usd: "0.01".into(),
                ultra_cost_usd: "0.02".into(),
                primary_duration_ms: 10,
                ultra_duration_ms: 20,
            })
            .collect(),
    };
    RoutedModel::new(primary, Some(ultra), &settings, Some(&calibration)).unwrap()
}
#[test]
fn escalation_preserves_observations_citations_and_prices_each_actual_model() {
    let (primary, _) = script(
        SUPER,
        vec![Ok(inspect("read")), Ok(answer("draft", "See [O1].", "low"))],
    );
    let (ultra, messages) = script(ULTRA, vec![Ok(answer("final", "See [O1].", "medium"))]);
    let mut model = routed(primary, ultra, EscalationReason::LowConfidence);
    let mut executions = 0;
    let report = investigate(
        &mut model,
        "Inspect",
        &snapshot(),
        &mut SnapshotTools::new(|_, _| {
            executions += 1;
            snapshot()
        }),
        |_| {},
    );
    assert!(report.error.is_none(), "{:?}", report.error);
    assert_eq!(report.answer.as_ref().unwrap().text, "See [O1].");
    assert_eq!(executions, 1);
    assert_eq!(report.stats.inference_calls, 3);
    assert_eq!(
        report
            .stats
            .model_requests
            .iter()
            .map(|r| r.model_id.as_deref())
            .collect::<Vec<_>>(),
        [Some(SUPER), Some(SUPER), Some(ULTRA)]
    );
    assert_eq!(report.stats.total_cost_usd.as_deref(), Some("0.0000432"));
    assert!(
        report.stats.cost.is_none(),
        "mixed prices cannot use the legacy single tariff"
    );
    let seen = messages.lock().unwrap();
    assert_eq!(seen.len(), 1);
    let tool_outputs: Vec<Value> = seen[0]
        .iter()
        .filter(|v| v["role"] == "tool")
        .filter_map(|v| serde_json::from_str(v["content"].as_str()?).ok())
        .collect();
    assert!(tool_outputs.iter().any(|v| v["observation_id"] == "O1"));
    assert_eq!(
        report
            .stats
            .routing
            .as_ref()
            .unwrap()
            .change
            .as_ref()
            .unwrap()
            .after_requests,
        2
    );
    assert!(model
        .try_escalate(EscalationReason::LowConfidence, 3)
        .is_none());
}
#[test]
fn failed_ultra_keeps_valid_primary_answer_but_cost_is_unknown_and_cancel_never_falls_back() {
    for error in [AgentError::Timeout, AgentError::Cancelled] {
        let (primary, _) = script(
            SUPER,
            vec![Ok(inspect("read")), Ok(answer("draft", "See [O1].", "low"))],
        );
        let (ultra, _) = script(ULTRA, vec![Err(error.clone())]);
        let mut model = routed(primary, ultra, EscalationReason::LowConfidence);
        let report = investigate(
            &mut model,
            "Inspect",
            &snapshot(),
            &mut SnapshotTools::new(|_, _| snapshot()),
            |_| {},
        );
        assert_eq!(report.stats.inference_calls, 3);
        assert_eq!(report.stats.model_requests.len(), 3);
        assert!(report.stats.model_requests[2].cost.is_none());
        assert!(report.stats.total_cost_usd.is_none());
        assert!(report.stats.prompt_tokens.is_none());
        assert_eq!(
            report.stats.routing.as_ref().unwrap().last_request_error,
            Some(error.clone())
        );
        if error == AgentError::Cancelled {
            assert_eq!(report.error, Some(AgentError::Cancelled));
            assert!(report.answer.is_none());
        } else {
            assert!(report.error.is_none());
            assert_eq!(report.answer.unwrap().confidence, Confidence::Low);
        }
    }
}
#[test]
fn escalation_does_not_reset_the_total_inference_or_tool_budget() {
    let (primary, _) = script(SUPER, vec![Ok(inspect("p1")), Ok(inspect("p2"))]);
    let (ultra, seen) = script(
        ULTRA,
        (0..MAX_ROUNDS - 2)
            .map(|i| Ok(inspect(&format!("u{i}"))))
            .collect(),
    );
    let mut model = routed(primary, ultra, EscalationReason::PrimaryRequestLimit);
    let report = investigate(
        &mut model,
        "Inspect",
        &snapshot(),
        &mut SnapshotTools::new(|_, _| snapshot()),
        |_| {},
    );
    assert_eq!(report.error, Some(AgentError::Budget));
    assert_eq!(report.stats.inference_calls, MAX_ROUNDS);
    assert_eq!(report.stats.tool_calls, MAX_ROUNDS);
    assert_eq!(seen.lock().unwrap().len(), MAX_ROUNDS - 2);
    assert_eq!(report.observations.len(), MAX_ROUNDS);
}
#[test]
fn verification_escalation_preserves_the_existing_repair_limit() {
    // A different wrong value each time, so the repairs are not cut short by a repeat.
    let bad = |i: usize| format!("La mémoire mesurée est de 99{i} Go [O1].");
    let mut replies = vec![Ok(inspect("read"))];
    replies.extend((0..=MAX_REPAIRS).map(|i| Ok(answer(&format!("p{i}"), &bad(i), "medium"))));
    let (primary, _) = script(SUPER, replies);
    let (ultra, seen) = script(ULTRA, vec![Ok(answer("u0", &bad(9), "medium"))]);
    let mut model = routed(primary, ultra, EscalationReason::VerificationFailure);
    let report = investigate(
        &mut model,
        "Inspect",
        &snapshot(),
        &mut SnapshotTools::new(|_, _| snapshot()),
        |_| {},
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(report.stats.inference_calls, MAX_REPAIRS + 3);
    assert_eq!(report.answer.unwrap().repairs, MAX_REPAIRS);
}
