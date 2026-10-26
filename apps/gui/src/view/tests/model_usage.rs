use super::*;
use pcagent_agent::routing::{
    Calibration, CalibrationCase, CalibrationOrigin, CalibrationRuntime, EscalationReason,
    RoutedModel, RoutingMode, RoutingSettings, CALIBRATION_SCHEMA_VERSION,
};
use pcagent_agent::{FunctionCall, Model, Reply, SnapshotTools, ToolCall, ToolChoice};
use pcagent_shared::Origin;
use std::collections::VecDeque;

const SUPER: &str = "nvidia/nemotron-super-fixture";
const ULTRA: &str = "nvidia/nemotron-ultra-fixture";
type Replies = Vec<Result<Reply, AgentError>>;

struct Scripted {
    id: &'static str,
    price: Pricing,
    replies: VecDeque<Result<Reply, AgentError>>,
}
impl Model for Scripted {
    fn complete(
        &mut self,
        _: &[Value],
        _: &Value,
        _: ToolChoice,
        _: Duration,
    ) -> Result<Reply, AgentError> {
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
fn scripted(id: &'static str, replies: Replies) -> Scripted {
    // Ultra's synthetic price is ten times Super's.
    let (prompt, completion) = if id == SUPER {
        ("0.0000003", "0.0000009")
    } else {
        ("0.000003", "0.000009")
    };
    Scripted {
        id,
        price: Pricing {
            model_id: id.into(),
            prompt_usd_per_token: prompt.into(),
            completion_usd_per_token: completion.into(),
            source: "synthetic fixture".into(),
            read_at_unix_ms: 1_000,
        },
        replies: replies.into(),
    }
}
fn call(name: &str, arguments: Value, prompt: u64, completion: u64) -> Reply {
    Reply {
        calls: vec![ToolCall {
            id: format!("call-{prompt}"),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.to_string(),
            },
        }],
        prompt_tokens: Some(prompt),
        completion_tokens: Some(completion),
        total_tokens: Some(prompt + completion),
        ..Reply::default()
    }
}
fn inspect() -> Result<Reply, AgentError> {
    Ok(call(
        "get_system_snapshot",
        serde_json::json!({}),
        4_700,
        80,
    ))
}
fn answer(confidence: &str, prompt: u64, completion: u64) -> Result<Reply, AgentError> {
    let arguments = serde_json::json!({
        "answer": "See [O1].", "confidence": confidence, "category": "other"
    });
    Ok(call(
        pcagent_agent::SUBMIT_ANSWER,
        arguments,
        prompt,
        completion,
    ))
}
fn snapshot() -> Snapshot {
    let mut s = Snapshot::empty("synthetic-model-usage");
    s.origin = Origin::SyntheticFixture;
    s.evidence.push(Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        500,
        "synthetic fixture",
        Window::instant(),
    ));
    s
}
fn run(model: &mut impl Model) -> Report {
    let inv = pcagent_agent::investigate(
        model,
        "Inspect",
        &snapshot(),
        &mut SnapshotTools::new(|_, _| snapshot()),
        |_| {},
    );
    report(&inv, "Inspect", &no_readable_names)
}
/// Super's low-confidence answer hands the question to Ultra.
fn escalated(ultra: Replies) -> Report {
    let settings = RoutingSettings {
        mode: RoutingMode::Automatic,
        super_model_id: Some(SUPER.into()),
        ultra_model_id: Some(ULTRA.into()),
    };
    let calibration = Calibration {
        schema_version: CALIBRATION_SCHEMA_VERSION,
        runtime: CalibrationRuntime::current(),
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
                trigger: EscalationReason::LowConfidence,
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
    let primary = scripted(SUPER, vec![inspect(), answer("low", 5_000, 20)]);
    let mut model = RoutedModel::new(
        primary,
        Some(scripted(ULTRA, ultra)),
        &settings,
        Some(&calibration),
    )
    .unwrap();
    run(&mut model)
}

#[test]
fn one_model_gets_one_priced_line_equal_to_the_legacy_cost() {
    let mut model = scripted(SUPER, vec![inspect(), answer("medium", 5_000, 20)]);
    let usage = run(&mut model).usage.unwrap();
    assert_eq!(usage.models.len(), 1);
    let only = &usage.models[0];
    assert_eq!(only.model_id.as_deref(), Some(SUPER));
    assert_eq!(only.requests, 2);
    assert_eq!(
        (only.prompt_tokens, only.completion_tokens),
        (Some(9_700), Some(100))
    );
    assert_eq!(only.unpriced_requests, 0);
    // 9,700 tokens in at $0.30 and 100 out at $0.90 per million: exactly $0.003.
    let cost = only.cost.clone().unwrap();
    assert_eq!(
        (cost.usd.as_str(), cost.exact_usd.as_str()),
        ("0.0030", "0.003")
    );
    let price = only.price.as_ref().unwrap();
    assert_eq!(price.model_id, SUPER);
    assert_eq!(price.input_per_million.as_deref(), Some("0.30"));
    assert_eq!(price.output_per_million.as_deref(), Some("0.90"));
    assert_eq!(price.read_at_unix_ms, 1_000);
    assert_eq!(usage.total_cost, Some(cost.clone()));
    assert_eq!(usage.total_cost_usd.as_deref(), Some("0.003"));
    assert_eq!(usage.cost.unwrap().exact_usd, cost.exact_usd);
    assert!(usage.priced_cost_lower_bound.is_none());
}

#[test]
fn an_escalation_keeps_each_models_share_and_a_total_only_from_both_prices() {
    let r = escalated(vec![answer("medium", 6_000, 50)]);
    assert_eq!(r.answer.as_ref().unwrap().confidence, Confidence::Medium);
    let usage = r.usage.unwrap();
    let change = usage.routing.as_ref().unwrap().change.clone().unwrap();
    assert_eq!(
        (change.from_model_id.as_str(), change.to_model_id.as_str()),
        (SUPER, ULTRA)
    );
    assert_eq!(change.after_requests, 2);
    let ids: Vec<_> = usage.models.iter().map(|m| m.model_id.as_deref()).collect();
    assert_eq!(ids, [Some(SUPER), Some(ULTRA)]);
    let [first, second] = &usage.models[..] else {
        panic!("two models")
    };
    assert_eq!((first.requests, second.requests), (2, 1));
    assert_eq!(
        (second.prompt_tokens, second.completion_tokens),
        (Some(6_000), Some(50))
    );
    assert_eq!(first.cost.as_ref().unwrap().exact_usd, "0.003");
    // 6,000 in at $3 and 50 out at $9 per million: $0.01845, shown rounded half-up.
    let ultra = second.cost.clone().unwrap();
    assert_eq!(
        (ultra.usd.as_str(), ultra.exact_usd.as_str()),
        ("0.0185", "0.01845")
    );
    assert_eq!(
        second.price.as_ref().unwrap().input_per_million.as_deref(),
        Some("3.00")
    );
    let total = usage.total_cost.clone().unwrap();
    assert_eq!(
        (total.usd.as_str(), total.exact_usd.as_str()),
        ("0.0215", "0.02145")
    );
    assert_eq!(usage.total_cost_usd.as_deref(), Some("0.02145"));
    // Never both models' tokens at the last model's price.
    assert!(usage.cost.is_none() && usage.cost_lower_bound.is_none());
    assert!(usage.priced_cost_lower_bound.is_none());
}

#[test]
fn an_unpriced_request_leaves_its_model_without_cost_and_the_turn_without_total() {
    let r = escalated(vec![Err(AgentError::Timeout)]);
    // Ultra's only request failed: the answer kept is Super's.
    assert_eq!(r.answer.as_ref().unwrap().confidence, Confidence::Low);
    let usage = r.usage.unwrap();
    let routing = usage.routing.as_ref().unwrap();
    assert_eq!(routing.last_request_error, Some(AgentError::Timeout));
    let ultra = &usage.models[1];
    assert_eq!(ultra.model_id.as_deref(), Some(ULTRA));
    assert_eq!((ultra.requests, ultra.unpriced_requests), (1, 1));
    assert!(ultra.prompt_tokens.is_none() && ultra.completion_tokens.is_none());
    assert!(ultra.cost.is_none() && ultra.price.is_none());
    assert!(usage.models[0].cost.is_some());
    assert!(usage.total_cost.is_none() && usage.total_cost_usd.is_none());
    // At least Super's priced requests, never presented as the total.
    let lower = usage.priced_cost_lower_bound.unwrap();
    assert_eq!(
        (lower.usd.as_str(), lower.exact_usd.as_str()),
        ("0.0030", "0.003")
    );
}

#[test]
fn exact_amounts_parse_strictly_and_round_half_up() {
    assert_eq!(pico("0.0223698"), Some(22_369_800_000));
    assert_eq!(pico("2"), Some(2 * PICO_PER_USD));
    assert_eq!(pico("0.000000000001"), Some(1));
    for bad in [
        "",
        ".3",
        "3.",
        "-1",
        "1e-7",
        "0,3",
        " 1",
        "0.0000000000001",
        "NaN",
    ] {
        assert_eq!(pico(bad), None, "{bad:?}");
    }
    let shown = |usd: &str| amount(pico(usd).unwrap());
    assert_eq!(shown("0.0223698").usd, "0.0224");
    assert_eq!(shown("0.00005").usd, "0.0001");
    assert_eq!(shown("0.0000432").usd, "0.0000432");
    assert_eq!(shown("0").usd, "0.0000");
    assert_eq!(shown("12.5").usd, "12.5000");
    assert_eq!(shown("0.0000432").exact_usd, "0.0000432");
}
