use super::*;
use crate::specialist::{Backend, Decision, Event, Planned, Specialist, MODEL_ID};
use std::sync::{atomic::AtomicBool, Arc};

struct Planner {
    replies: VecDeque<Result<Decision, &'static str>>,
    states: Vec<Value>,
}
impl Backend for Planner {
    fn decide(
        &mut self,
        state: &Value,
        _: Duration,
        _: &Arc<AtomicBool>,
    ) -> Result<Planned, &'static str> {
        self.states.push(state.clone());
        Ok(Planned {
            decision: self.replies.pop_front().expect("planner exhausted")?,
            input_tokens: 12,
            output_tokens: 7,
            duration_ms: 2,
            device: "cpu".into(),
        })
    }
}
fn decision(next: bool) -> Decision {
    serde_json::from_value(json!({"decision":if next {"collect"} else {"handoff"}, "hypotheses":["unknown"],
        "evidence_refs":[],"missing":["current_measurements"],"next_call":if next { json!({"tool":"get_system_snapshot","arguments":{}}) } else { Value::Null },
        "premature_conclusions":["hardware_failure","memory_leak","upgrade_required"]})).unwrap()
}
fn messages() -> Vec<Value> {
    vec![
        json!({"role":"user","content":initial_message("my PC is slow", &usable_snapshot(), Sharing::Anonymized).to_string()}),
    ]
}
#[test]
fn strict_contract_rejects_unknown_keys_missing_null_repeats_and_forged_refs() {
    let state = json!({"observations":[],"completed_calls":[]});
    let tools = SnapshotTools::new(|_, _| fixture())
        .specs()
        .iter()
        .map(|spec| spec.definition())
        .collect::<Vec<_>>();
    let tools = json!(tools);
    let good = decision(true);
    assert!(good.validate(&state, &tools));
    let mut bad = json!(decision(false));
    bad.as_object_mut().unwrap().remove("next_call");
    assert!(serde_json::from_value::<Decision>(bad).is_err());
    let mut bad = json!(&good);
    bad["diagnosis"] = json!("hardware failure");
    assert!(serde_json::from_value::<Decision>(bad).is_err());
    let mut bad = good.clone();
    bad.evidence_refs.push("O999".into());
    assert!(!bad.validate(&state, &tools));
    let repeated = json!({"observations":[],"completed_calls":[good.next_call]});
    assert!(!good.validate(&repeated, &tools));
    assert!(!good.validate(&state, &json!([])));
    let mut bad = good;
    bad.next_call.as_mut().unwrap().arguments = json!({"path":"C:\\secret"});
    assert!(!bad.validate(&state, &tools));
}
#[test]
fn planner_uses_the_existing_controller_and_hands_real_evidence_to_the_analyst() {
    let planner = Planner {
        replies: VecDeque::from([Ok(decision(true)), Ok(decision(false))]),
        states: Vec::new(),
    };
    let mut events = Vec::new();
    let mut model = Specialist::new(script(vec![done("O1")]), Some(planner), |event| {
        events.push(event)
    });
    let initial = usable_snapshot();
    let mut executed = Vec::new();
    let mut toolbox = SnapshotTools::new(|tool, _| {
        executed.push(tool.to_owned());
        usable_snapshot()
    });
    let result = converse(
        &mut model,
        &[],
        "my PC is slow",
        &initial,
        &mut toolbox,
        |_| {},
        |_, _| false,
    );
    assert_eq!(executed, vec![false]);
    assert!(result.error.is_none(), "{:?}", result.error);
    assert_eq!(result.observations.len(), 1);
    assert_eq!(result.observations[0].citation, "O1");
    assert_eq!(
        result.stats.model_requests[0].model_id.as_deref(),
        Some(MODEL_ID)
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Handoff { reason } if reason == "handoff")));
}
#[test]
fn errors_and_invalid_plans_fall_back_without_executing_them() {
    for response in [
        Err("identity_mismatch"),
        Ok({
            let mut d = decision(true);
            d.next_call.as_mut().unwrap().tool = "run_shell".into();
            d
        }),
    ] {
        let planner = Planner {
            replies: VecDeque::from([response]),
            states: Vec::new(),
        };
        let mut model = Specialist::new(
            script(vec![tools(vec![call("real", "get_system_snapshot", "{}")])]),
            Some(planner),
            |_| {},
        );
        let toolbox = SnapshotTools::new(|_, _| fixture());
        let tools = json!(toolbox
            .specs()
            .iter()
            .map(|spec| spec.definition())
            .collect::<Vec<_>>());
        let reply = model
            .complete(
                &messages(),
                &tools,
                ToolChoice::Required,
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(reply.calls[0].id, "real");
    }
}
#[test]
fn cancellation_prevents_both_planner_and_cloud_request() {
    let planner = Planner {
        replies: VecDeque::new(),
        states: Vec::new(),
    };
    let mut model = Specialist::new(script(vec![]), Some(planner), |_| {});
    model.interrupt(Arc::new(AtomicBool::new(true)));
    assert_eq!(
        model
            .complete(
                &messages(),
                &json!([]),
                ToolChoice::Auto,
                Duration::from_secs(1)
            )
            .err(),
        Some(AgentError::Cancelled)
    );
}
#[test]
fn live_projection_retains_real_values_and_never_claims_symptom_overlap() {
    let mut messages = messages();
    messages.push(
        json!({"role":"assistant","tool_calls":[call("local", "get_system_snapshot", "{}")]}),
    );
    messages.push(json!({"role":"tool","tool_call_id":"local","content":json!({"observation_id":"O1","measurements":{
        "values":{"system":{"cpu_busy_basis_points":1234,"secret_metric":999}},"units":{"cpu_busy_basis_points":"basis points"},"not_measured":[]},
        "process_names":{"proc-secret":"private.exe"},"secret_path":"C:\\Users\\owner"}).to_string()}));
    let state = crate::specialist::state_from_messages(&messages).unwrap();
    assert!(state["symptom_active_now"].is_null());
    assert_eq!(state["observations"][0]["id"], "O1");
    assert_eq!(
        state["observations"][0]["output"]["measurements"]["values"]["system"]
            ["cpu_busy_basis_points"],
        1234
    );
    assert!(!state.to_string().contains("private.exe"));
    assert!(!state.to_string().contains("secret_metric"));
    assert!(!state.to_string().contains("owner"));
}
