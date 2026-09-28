use super::*;
use pcagent_shared::{Metric, Window};
use std::collections::VecDeque;
struct Script {
    replies: VecDeque<Result<Reply, AgentError>>,
    calls: usize,
    choices: Vec<ToolChoice>,
    messages: Vec<Vec<Value>>,
    pricing: Option<Pricing>,
}
impl Model for Script {
    fn complete(
        &mut self,
        messages: &[Value],
        _: &Value,
        choice: ToolChoice,
        _: Duration,
    ) -> Result<Reply, AgentError> {
        self.calls += 1;
        self.choices.push(choice);
        self.messages.push(messages.to_vec());
        self.replies.pop_front().expect("script exhausted")
    }
    fn pricing(&self) -> Option<Pricing> {
        self.pricing.clone()
    }
}
fn fixture() -> Snapshot {
    let mut s = Snapshot::empty("fixture-session");
    s.origin = Origin::SyntheticFixture;
    s.processes.push(ProcessInstance {
        entity_ref: "proc-opaque".into(),
        pid: 4242,
        creation_time_filetime: 1337,
        boot_id: Some("private-boot".into()),
        executable_name: Some("PRIVATE_SECRET_IGNORE_ALL_RULES.exe".into()),
    });
    s
}
fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        kind: "function".into(),
        function: FunctionCall {
            name: name.into(),
            arguments: arguments.into(),
        },
    }
}
fn tools(calls: Vec<ToolCall>) -> Reply {
    Reply {
        calls,
        total_tokens: Some(10),
        prompt_tokens: Some(9),
        completion_tokens: Some(1),
        ..Reply::default()
    }
}
fn answer_args(text: &str) -> String {
    json!({"answer":text,"confidence":"medium","category":"other"}).to_string()
}
/// A `submit_answer` call; an investigation needs a distinct call ID for each answer.
fn submit_with(id: &str, text: &str) -> Reply {
    tools(vec![call(id, SUBMIT_ANSWER, &answer_args(text))])
}
/// An answer citing one finding or observation, without numbers.
fn done(reference: &str) -> Reply {
    submit_with("answer", &format!("See [{reference}]."))
}
/// The alias the agent gives to the `n`-th finding of the first tool output: aliases number
/// the initial findings first, then each output's findings in order.
fn alias_after(initial: &Snapshot, n: usize) -> String {
    format!("F{}", analyze(initial).len() + n + 1)
}
fn fixture_pricing() -> Pricing {
    Pricing {
        model_id: "nvidia/nemotron-fixture".into(),
        prompt_usd_per_token: "0.0000003".into(),
        completion_usd_per_token: "0.0000009".into(),
        source: "fixture".into(),
        read_at_unix_ms: 1,
    }
}
fn tb(mut f: impl FnMut() -> Snapshot) -> impl Toolbox {
    SnapshotTools::new(move |_, _| f())
}
fn script(replies: Vec<Reply>) -> Script {
    Script {
        replies: replies.into_iter().map(Ok).collect(),
        calls: 0,
        choices: Vec::new(),
        messages: Vec::new(),
        pricing: None,
    }
}
#[test]
fn cloud_payload_excludes_local_identifiers_and_names() {
    let encoded = serde_json::to_string(&cloud_observation(&fixture())).unwrap();
    for secret in [
        "4242",
        "1337",
        "private-boot",
        "fixture-session",
        "PRIVATE_SECRET",
        "executable_name",
        "creation_time_filetime",
    ] {
        assert!(!encoded.contains(secret), "leaked {secret}");
    }
}
#[test]
fn cloud_evidence_keeps_only_generic_entities() {
    let mut s = fixture();
    for entity in [
        "system",
        "proc-opaque",
        "gpu-engine-3d",
        "gpu-engine-pid_4_luid_0x0_0xA_phys_0",
        "gpu-engine-",
        r"C:\Users\PRIVATE",
        "gpu-nvidia-0",
        "gpu-nvidia-GeForce MX350",
        "gpu-nvidia-123",
    ] {
        s.evidence.push(Evidence::measured(
            Metric::GpuEngineBasisPoints,
            entity,
            1,
            "fixture",
            Window::instant(),
        ));
    }
    let kept: Vec<String> = cloud_observation(&s)
        .measurements
        .values
        .into_keys()
        .collect();
    assert_eq!(
        kept,
        ["gpu-engine-3d", "gpu-nvidia-0", "proc-opaque", "system"]
    );
}
#[test]
fn arbitrary_tools_and_arguments_are_rejected() {
    for c in [
        call("a", "run_shell", "{}"),
        call("b", "get_processes", "{\"path\":\"C:\\\\Users\"}"),
        call("e", "get_processes", "{\"limit\":11}"),
        call("c", "get_system_snapshot", "null"),
        call("d", "get_processes", "[]"),
    ] {
        assert_eq!(
            validate_call(&c, SnapshotTools::new(|_, _| fixture()).specs()).err(),
            Some(AgentError::InvalidTool)
        );
    }
}
#[test]
fn validates_all_calls_before_any_execution() {
    let initial = fixture();
    let mut executed = 0;
    let mut m = script(vec![
        tools(vec![
            call("a", "get_processes", "{}"),
            call("b", "delete_path", "{}"),
        ]),
        tools(vec![call("c", "get_processes", "{}")]),
        done("F1"),
    ]);
    let r = investigate(
        &mut m,
        "test",
        &initial,
        &mut tb(|| {
            executed += 1;
            usable_snapshot()
        }),
        |_| {},
    );
    // Only the retried valid call ran: a batch with an invalid call runs nothing.
    assert_eq!(executed, 1);
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.answer.unwrap().repairs, 1);
    let replies: Vec<&Value> = m.messages[1]
        .iter()
        .filter(|m| m["role"] == "tool")
        .collect();
    assert_eq!(replies.len(), 2);
    assert!(replies[0]["content"].as_str().unwrap().contains("not_run"));
    assert!(replies[1]["content"]
        .as_str()
        .unwrap()
        .contains("invalid_call"));
}
#[test]
fn invalid_calls_are_retried_a_bounded_number_of_times() {
    let bad = |i: usize| tools(vec![call(&format!("bad-{i}"), "delete_path", "{}")]);
    let mut m = script((0..=MAX_REPAIRS).map(bad).collect());
    let r = investigate(
        &mut m,
        "test",
        &fixture(),
        &mut tb(|| panic!("nothing runs")),
        |_| {},
    );
    assert_eq!(r.error, Some(AgentError::InvalidTool));
    assert_eq!(m.calls, MAX_REPAIRS + 1);
}
#[test]
fn happy_path_requires_a_real_dispatch_and_existing_finding() {
    let initial = fixture();
    let mut next = fixture();
    next.evidence.push(Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        500,
        "fixture",
        Window::instant(),
    ));
    let id = analyze(&next)[0].id.clone();
    let mut m = script(vec![
        tools(vec![call("a", "get_system_snapshot", "{}")]),
        done(&alias_after(&initial, 0)),
    ]);
    let mut events = 0;
    let r = investigate(&mut m, "test", &initial, &mut tb(|| next.clone()), |_| {
        events += 1
    });
    assert!(r.error.is_none());
    assert_eq!(r.selected.id, id);
    assert_eq!(
        r.answer.unwrap().text,
        format!("See [{}].", alias_after(&initial, 0))
    );
    assert_eq!(events, 1);
    assert_eq!(r.stats.inference_calls, 2);
    assert_eq!(r.stats.total_tokens, Some(20));
    // Every turn must call a tool; the answer itself is a tool call.
    assert_eq!(m.choices, [ToolChoice::Required, ToolChoice::Required]);
}
#[test]
fn answers_are_checked_and_repaired_before_acceptance() {
    let initial = fixture();
    let mut next = fixture();
    for (metric, value) in [
        (Metric::PhysicalTotalBytes, 17_179_869_184),
        (Metric::PhysicalAvailableBytes, 1_653_562_163),
    ] {
        next.evidence.push(Evidence::measured(
            metric,
            "system",
            value,
            "fixture",
            Window::instant(),
        ));
    }
    let mut m = script(vec![
        tools(vec![call("a", "get_system_snapshot", "{}")]),
        submit_with("first", "Il ne reste que 3 Go de mémoire [O1]."),
        submit_with(
            "second",
            "Il ne reste que 1,54 Go de mémoire disponible sur 16 Go [O1].",
        ),
    ]);
    let r = investigate(
        &mut m,
        "Pourquoi mon PC rame ?",
        &initial,
        &mut tb(|| next.clone()),
        |_| {},
    );
    let answer = r.answer.expect("accepted after one repair");
    assert_eq!(answer.repairs, 1);
    assert!(answer.unverified.is_empty(), "{:?}", answer.unverified);
    assert_eq!(answer.citations[0].reference, "O1");
    assert_eq!(
        r.observation_refs,
        [r.observations[0].observation_id.clone()]
    );
    let repair = m.messages[2].last().unwrap();
    assert_eq!(repair["role"], "tool");
    assert!(repair["content"]
        .as_str()
        .unwrap()
        .contains("unsupported_number"));
    // The model cites short references and never sees local observation IDs.
    let sent = serde_json::to_string(&m.messages[2]).unwrap();
    assert!(!sent.contains(&r.observations[0].observation_id));
    assert!(sent.contains(r#"\"citable_observation_ids\":[\"O1\"]"#));
}
#[test]
fn repairs_are_bounded_and_the_last_answer_is_shown_with_its_problems() {
    let mut replies = vec![tools(vec![call("a", "get_system_snapshot", "{}")])];
    replies.extend(
        (0..=MAX_REPAIRS).map(|i| submit_with(&format!("answer-{i}"), "Windows needs 32 GB [O1].")),
    );
    let mut m = script(replies);
    let r = investigate(&mut m, "test", &fixture(), &mut tb(usable_snapshot), |_| {});
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.execution_status, ExecutionStatus::Completed);
    let answer = r.answer.unwrap();
    assert_eq!(answer.repairs, MAX_REPAIRS);
    assert_eq!(answer.unverified.len(), 1);
    assert_eq!(answer.unverified[0].kind, IssueKind::UnsupportedNumber);
    assert_eq!(answer.unverified[0].excerpt, "32 GB");
}
#[test]
fn submit_answer_alone_and_only_as_a_tool_call() {
    let mut executed = 0;
    let mut m = script(vec![
        tools(vec![
            call("a", "get_system_snapshot", "{}"),
            call("b", SUBMIT_ANSWER, &answer_args("Too early.")),
        ]),
        Reply {
            content: Some("Here is my answer in plain text.".into()),
            ..Reply::default()
        },
        tools(vec![call("c", "get_system_snapshot", "{}")]),
        done("O1"),
    ]);
    let r = investigate(
        &mut m,
        "test",
        &fixture(),
        &mut tb(|| {
            executed += 1;
            usable_snapshot()
        }),
        |_| {},
    );
    assert_eq!(executed, 1);
    assert_eq!(r.answer.unwrap().repairs, 2);
    let alone = m.messages[1].last().unwrap();
    assert!(alone["content"].as_str().unwrap().contains("not_alone"));
    let plain = m.messages[2].last().unwrap();
    assert_eq!(plain["role"], "user");
    assert!(plain["content"].as_str().unwrap().contains("no_tool_call"));
}
#[test]
fn alternatives_are_existing_findings_ranked_by_the_model() {
    let initial = fixture();
    let mut next = fixture();
    next.evidence.push(Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        500,
        "fixture",
        Window::instant(),
    ));
    let ids: Vec<String> = analyze(&next).into_iter().map(|f| f.id).collect();
    let mut m = script(vec![
        tools(vec![call("a", "get_processes", "{\"limit\":2}")]),
        Reply {
            calls: vec![call(
                "answer",
                SUBMIT_ANSWER,
                &answer_args(&format!("[{}], then [F1].", alias_after(&initial, 0))),
            )],
            ..Reply::default()
        },
    ]);
    let r = investigate(&mut m, "test", &initial, &mut tb(|| next.clone()), |_| {});
    assert_eq!(r.execution_status, ExecutionStatus::Completed);
    assert_eq!(r.status, Status::Partial);
    assert_eq!(r.selected.id, ids[0]);
    assert_eq!(r.alternatives.len(), 1);
    assert_eq!(r.stats.total_tokens, None);
}
#[test]
fn a_direct_answer_without_tool_execution_is_not_live_validation() {
    let initial = fixture();
    let mut m = script(
        (0..=MAX_REPAIRS)
            .map(|i| submit_with(&format!("answer-{i}"), "See [F1]."))
            .collect(),
    );
    let r = investigate(
        &mut m,
        "test",
        &initial,
        &mut tb(|| panic!("unexpected call")),
        |_| {},
    );
    assert_eq!(r.error, Some(AgentError::InvalidFinding));
    assert!(r.answer.is_none());
    assert_eq!(m.calls, MAX_REPAIRS + 1);
}
#[test]
fn repeated_call_ids_are_not_replayed() {
    let initial = fixture();
    let reply = tools(vec![call("same", "get_processes", "{}")]);
    let mut m = script(vec![reply.clone(), reply]);
    let r = investigate(&mut m, "test", &initial, &mut tb(fixture), |_| {});
    assert_eq!(r.stats.tool_calls, 1);
    assert_eq!(r.error, Some(AgentError::DuplicateCall));
}
#[test]
fn rounds_and_tools_are_bounded() {
    let initial = fixture();
    let mut m = script(
        (0..MAX_ROUNDS)
            .map(|i| tools(vec![call(&format!("c{i}"), "get_system_snapshot", "{}")]))
            .collect(),
    );
    let r = investigate(&mut m, "test", &initial, &mut tb(fixture), |_| {});
    assert_eq!(m.calls, MAX_ROUNDS);
    assert_eq!(r.error, Some(AgentError::Budget));
    let mut m = script(vec![tools(
        (0..=MAX_TOOLS)
            .map(|i| call(&format!("c{i}"), "get_processes", "{}"))
            .collect(),
    )]);
    let r = investigate(
        &mut m,
        "test",
        &initial,
        &mut tb(|| panic!("over budget")),
        |_| {},
    );
    assert_eq!(r.stats.tool_calls, 0);
    assert_eq!(r.error, Some(AgentError::Budget));
}
#[test]
fn network_failure_preserves_evidence_without_fake_success() {
    let initial = fixture();
    let mut m = Script {
        replies: vec![Err(AgentError::Network)].into(),
        calls: 0,
        choices: Vec::new(),
        messages: Vec::new(),
        pricing: None,
    };
    let r = investigate(
        &mut m,
        "test",
        &initial,
        &mut tb(|| panic!("unexpected call")),
        |_| {},
    );
    assert_eq!(r.status, Status::Partial);
    assert_eq!(r.stats.total_tokens, None);
    assert_eq!(
        r.selected.classification,
        Classification::InsufficientEvidence
    );
}
#[test]
fn instructions_mention_proposals_only_when_the_tool_is_offered() {
    let plain = system_prompt(SnapshotTools::new(|_, _| fixture()).specs());
    assert!(plain.ends_with(NO_ACTIONS));
    assert!(!plain.contains("prepare_action_plan"));
    let with_plans = system_prompt(&[PREPARE_ACTION_PLAN]);
    assert!(with_plans.contains("call prepare_action_plan"));
    assert!(!with_plans.contains(NO_ACTIONS));
}
#[test]
fn invalid_question_never_reaches_transport() {
    let mut m = script(vec![]);
    let r = investigate(&mut m, "", &fixture(), &mut tb(fixture), |_| {});
    assert_eq!(m.calls, 0);
    assert_eq!(r.error, Some(AgentError::InvalidQuestion));
}

#[test]
fn unsuccessful_inspection_and_context_do_not_unlock_a_conclusion() {
    struct Fixed {
        spec: [ToolSpec; 1],
        output: ToolOutput,
    }
    impl Toolbox for Fixed {
        fn specs(&self) -> &[ToolSpec] {
            &self.spec
        }
        fn run(&mut self, _: &ToolSpec, _: &Args) -> ToolOutput {
            self.output.clone()
        }
    }
    for (class, status, usable) in [
        (ToolClass::Inspection, Status::Error, false),
        (ToolClass::Inspection, Status::Partial, false),
        (ToolClass::Context, Status::Ok, true),
        (ToolClass::Proposal, Status::Ok, true),
    ] {
        let initial = fixture();
        let mut output = ToolOutput::from_snapshot(&initial);
        output.status = status;
        output.inspection_complete = usable;
        let mut toolbox = Fixed {
            spec: [ToolSpec {
                name: "inspect",
                class,
                description: "",
                params: &[],
            }],
            output,
        };
        let mut replies = vec![tools(vec![call("one", "inspect", "{}")])];
        replies.extend((0..=MAX_REPAIRS).map(|i| submit_with(&format!("answer-{i}"), "See [F1].")));
        let mut model = script(replies);
        let result = investigate(&mut model, "test", &initial, &mut toolbox, |_| {});
        assert_eq!(result.error, Some(AgentError::InvalidFinding));
        assert!(model.choices.iter().all(|c| *c == ToolChoice::Required));
        assert_eq!(result.observations.len(), 1);
    }
}

#[test]
fn refreshed_measurements_supersede_older_findings() {
    let mut initial = fixture();
    let mut before = Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        9900,
        "fixture",
        Window::instant(),
    );
    before.window.finished_at_unix_ms = 100;
    before.window.started_at_unix_ms = 100;
    initial.evidence.push(before);
    let initial_findings = analyze(&initial);
    let position = initial_findings
        .iter()
        .position(|f| f.classification == Classification::HighCpuSample)
        .unwrap();
    let old = initial_findings[position].clone();
    let mut next = fixture();
    let mut after = Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        100,
        "fixture",
        Window::instant(),
    );
    after.window.started_at_unix_ms = 200;
    after.window.finished_at_unix_ms = 200;
    next.evidence.push(after);
    let mut replies = vec![tools(vec![call("fresh", "get_system_snapshot", "{}")])];
    replies.extend(
        (0..=MAX_REPAIRS)
            .map(|i| submit_with(&format!("answer-{i}"), &format!("See [F{}].", position + 1))),
    );
    let mut model = script(replies);
    let report = investigate(
        &mut model,
        "test",
        &initial,
        &mut tb(|| next.clone()),
        |_| {},
    );
    // A superseded finding still describes an earlier moment: it stays a valid source.
    let answer = report.answer.as_ref().expect("accepted");
    assert_eq!(answer.repairs, 0);
    assert!(answer.unverified.is_empty());
    assert_eq!(answer.citations[0].target, old.id);
    assert!(report.historical_findings.iter().any(|f| f.id == old.id));
    assert!(!report.findings.iter().any(|f| f.id == old.id));
    assert_eq!(report.evidence.len(), 2);
}

#[test]
fn healthy_measurements_do_not_select_an_unsupported_memory_cause() {
    let mut s = fixture();
    for (metric, entity, value) in [
        (Metric::PhysicalTotalBytes, "system", 16_000_000_000),
        (Metric::PhysicalAvailableBytes, "system", 12_000_000_000),
        (Metric::CpuBusyBasisPoints, "system", 500),
        (
            Metric::ProcessPrivateCommitBytes,
            "proc-opaque",
            1_000_000_000,
        ),
    ] {
        s.evidence.push(Evidence::measured(
            metric,
            entity,
            value,
            "fixture",
            Window::instant(),
        ));
    }
    assert_ne!(
        local_report(&s).selected.classification,
        Classification::LargestMemoryContributor
    );
}

fn usable_snapshot() -> Snapshot {
    let mut snapshot = fixture();
    snapshot.evidence.push(Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        500,
        "fixture",
        Window::instant(),
    ));
    snapshot
}

#[test]
fn extension_keeps_context_and_resumes_without_replaying_inspections() {
    let initial = usable_snapshot();
    let mut replies: Vec<_> = (0..MAX_ROUNDS)
        .map(|i| {
            tools(vec![call(
                &format!("step-{i}"),
                "get_system_snapshot",
                "{}",
            )])
        })
        .collect();
    replies.push(done("F1"));
    let mut model = script(replies);
    model.pricing = Some(fixture_pricing());
    let (mut prompts, mut executed) = (0, 0);
    let report = investigate_with_extension(
        &mut model,
        "continue",
        &initial,
        &mut tb(|| {
            executed += 1;
            initial.clone()
        }),
        |_| {},
        |partial, request| {
            prompts += 1;
            assert_eq!(request.reason, BudgetLimit::Rounds);
            assert_eq!(partial.execution_status, ExecutionStatus::BudgetExhausted);
            assert_eq!(partial.observations.len(), MAX_ROUNDS);
            assert_eq!(partial.stats.inference_calls, MAX_ROUNDS);
            // The consent prompt shows what the investigation has cost so far.
            assert_eq!(partial.stats.requests.len(), MAX_ROUNDS);
            assert!(partial.stats.cost.is_some());
            true
        },
    );
    assert_eq!(prompts, 1);
    assert_eq!(executed, MAX_ROUNDS);
    assert_eq!(report.execution_status, ExecutionStatus::Completed);
    assert_eq!(report.stats.extensions, 1);
    assert!(report.continuation.is_none());
    let last_context = model.messages.last().unwrap();
    assert_eq!(
        last_context.iter().filter(|m| m["role"] == "tool").count(),
        MAX_ROUNDS
    );
    assert_eq!(
        last_context.iter().filter(|m| m["role"] == "user").count(),
        1
    );
    assert_eq!(report.diagnostic_support, Support::Insufficient);
    assert_eq!(report.coverage.status, CoverageStatus::Partial);
    assert_eq!(report.status, Status::Partial);
}

#[test]
fn refusing_an_extension_preserves_a_partial_report_and_stops_requests() {
    let snapshot = usable_snapshot();
    let mut model = script(
        (0..MAX_ROUNDS)
            .map(|i| {
                tools(vec![call(
                    &format!("step-{i}"),
                    "get_system_snapshot",
                    "{}",
                )])
            })
            .collect(),
    );
    let mut prompts = 0;
    let report = investigate_with_extension(
        &mut model,
        "stop",
        &snapshot,
        &mut tb(|| snapshot.clone()),
        |_| {},
        |_, _| {
            prompts += 1;
            false
        },
    );
    assert_eq!(prompts, 1);
    assert_eq!(model.calls, MAX_ROUNDS);
    assert_eq!(report.stats.extensions, 0);
    assert_eq!(report.observations.len(), MAX_ROUNDS);
    assert_eq!(report.error, Some(AgentError::Budget));
    assert!(report.continuation.unwrap().available);
}

#[test]
fn extensions_have_a_hard_cap_and_do_not_reset_duplicate_call_protection() {
    let snapshot = usable_snapshot();
    let cap = MAX_ROUNDS * (1 + MAX_EXTENSIONS);
    let mut model = script(
        (0..cap)
            .map(|i| {
                tools(vec![call(
                    &format!("step-{i}"),
                    "get_system_snapshot",
                    "{}",
                )])
            })
            .collect(),
    );
    let mut prompts = 0;
    let report = investigate_with_extension(
        &mut model,
        "bound",
        &snapshot,
        &mut tb(|| snapshot.clone()),
        |_| {},
        |_, _| {
            prompts += 1;
            true
        },
    );
    assert_eq!(prompts, MAX_EXTENSIONS);
    assert_eq!(model.calls, cap);
    assert!(!report.continuation.unwrap().available);
    let mut replies: Vec<_> = (0..MAX_ROUNDS)
        .map(|i| {
            tools(vec![call(
                &format!("step-{i}"),
                "get_system_snapshot",
                "{}",
            )])
        })
        .collect();
    replies.push(tools(vec![call("step-0", "get_system_snapshot", "{}")]));
    let mut model = script(replies);
    let report = investigate_with_extension(
        &mut model,
        "no replay",
        &snapshot,
        &mut tb(|| snapshot.clone()),
        |_| {},
        |_, _| true,
    );
    assert_eq!(report.error, Some(AgentError::DuplicateCall));
    assert_eq!(report.stats.tool_calls, MAX_ROUNDS);
}

#[test]
fn tool_budget_extension_requires_consent_before_executing_the_pending_batch() {
    let snapshot = usable_snapshot();
    for accept in [false, true] {
        let mut model = script(vec![
            tools(
                (0..MAX_TOOLS)
                    .map(|i| call(&format!("step-{i}"), "get_system_snapshot", "{}"))
                    .collect(),
            ),
            tools(vec![call("pending", "get_system_snapshot", "{}")]),
            done("F1"),
        ]);
        let report = investigate_with_extension(
            &mut model,
            "more tools",
            &snapshot,
            &mut tb(|| snapshot.clone()),
            |_| {},
            |partial, request| {
                assert_eq!(partial.stats.tool_calls, MAX_TOOLS);
                assert_eq!(request.reason, BudgetLimit::Tools);
                accept
            },
        );
        assert_eq!(report.stats.tool_calls, MAX_TOOLS + usize::from(accept));
        assert_eq!(
            report.execution_status,
            if accept {
                ExecutionStatus::Completed
            } else {
                ExecutionStatus::BudgetExhausted
            }
        );
    }
}

#[test]
fn conclusion_cites_hardware_storage_and_history_without_inventing_a_diagnosis() {
    struct ContextTools;
    impl Toolbox for ContextTools {
        fn specs(&self) -> &[ToolSpec] {
            WORKSTATION_TOOLS
        }
        fn run(&mut self, spec: &ToolSpec, _: &Args) -> ToolOutput {
            let cloud = match spec.name {
                "get_hardware_inventory" => {
                    json!({"observation_id":"obs-hardware", "logical_processors":{"value":8,"status":"ok"},"installed_memory_bytes":{"value":8589934592_u64,"status":"ok"}})
                }
                "get_storage_overview" => {
                    json!({"observation_id":"obs-storage","scope_id":"scope-fixture","volume":{"free_bytes":4096,"total_bytes":8192},"scanned":{"logical_bytes":1024},"scan":{"coverage":{"files":3}}})
                }
                "get_metric_history" => {
                    json!({"observation_id":"obs-history","metric":"cpu_busy_basis_points","observed_interval_ms":2000,"points":[{"value":500,"status":"ok"},{"value":900,"status":"ok"},{"value":10000,"status":"stale"}],"gaps":[{"reason":"not_recorded"}]})
                }
                _ => panic!("unexpected inspection"),
            };
            ToolOutput {
                status: Status::Partial,
                inspection_complete: true,
                cloud,
                evidence: vec![],
                findings: vec![],
                processes: vec![],
                storage_items: vec![],
                limitations: vec![],
            }
        }
    }
    let refs = ["obs-hardware", "obs-storage", "obs-history"];
    let mut model = script(vec![
        tools(vec![
            call("hardware", "get_hardware_inventory", "{}"),
            call(
                "storage",
                "get_storage_overview",
                r#"{"scope_id":"scope-fixture"}"#,
            ),
            call(
                "history",
                "get_metric_history",
                r#"{"metric":"cpu_busy_basis_points","window_seconds":60}"#,
            ),
        ]),
        submit_with(
            "answer",
            "Le PC dispose de 8 Gio de mémoire [O1]. Le volume a 4 096 octets libres [O2]. \
             Le processeur a varié entre 5 % et 9 % [O3].",
        ),
    ]);
    let report = investigate(
        &mut model,
        "what was observed",
        &fixture(),
        &mut ContextTools,
        |_| {},
    );
    assert_eq!(report.execution_status, ExecutionStatus::Completed);
    assert_eq!(report.observation_refs, refs);
    // Memory, free space and the CPU range all come from the three tool results.
    assert!(report.answer.as_ref().unwrap().unverified.is_empty());
    assert_eq!(report.diagnostic_support, Support::Insufficient);
    assert!(report.observations[0]
        .facts
        .iter()
        .any(|f| f.contains("8.00 GiB")));
    assert!(report.observations[1]
        .facts
        .iter()
        .any(|f| f.contains("3 files")));
    assert!(report.observations[2]
        .facts
        .iter()
        .any(|f| f.contains("2 usable samples") && f.contains("5.0% to 9.0%")));
    let reply: Value = serde_json::from_str(
        model
            .messages
            .last()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m["role"] == "tool")
            .unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(reply["citable_observation_ids"], json!(["O1", "O2", "O3"]));
    assert_eq!(reply["observation_id"], "O3");
}

#[test]
fn only_usable_inspections_and_current_findings_can_be_cited() {
    let mut report = local_report(&fixture());
    let spec = ToolSpec {
        name: "inspect",
        class: ToolClass::Inspection,
        description: "",
        params: &[],
    };
    for (class, status, usable) in [
        (ToolClass::Inspection, Status::Ok, true),
        (ToolClass::Context, Status::Ok, true),
        (ToolClass::Proposal, Status::Ok, true),
        (ToolClass::Inspection, Status::Error, true),
        (ToolClass::Inspection, Status::Partial, false),
    ] {
        let mut output = ToolOutput::from_snapshot(&usable_snapshot());
        output.status = status;
        output.inspection_complete = usable;
        report.push_observation(Observation::record(
            &ToolSpec { class, ..spec },
            &mut output,
        ));
    }
    let references = report.references(&mut FindingAliases::default());
    let observations: Vec<&str> = references
        .iter()
        .filter(|r| r.1 == CitationKind::Observation)
        .map(|r| r.0.as_str())
        .collect();
    assert_eq!(observations, ["O1"]);
    assert_eq!(
        references
            .iter()
            .filter(|r| r.1 == CitationKind::Finding)
            .count(),
        report.findings.len()
    );
}

#[test]
fn execution_completion_is_independent_of_missing_sensors_and_diagnostic_support() {
    let missing = local_report(&fixture());
    assert_eq!(missing.execution_status, ExecutionStatus::Completed);
    assert_eq!(missing.coverage.status, CoverageStatus::Unavailable);
    assert_eq!(missing.diagnostic_support, Support::Insufficient);
    let sparse = local_report(&usable_snapshot());
    assert_eq!(sparse.execution_status, ExecutionStatus::Completed);
    assert_eq!(sparse.coverage.status, CoverageStatus::Partial);
    assert!(sparse
        .coverage
        .domains
        .iter()
        .any(|d| d.domain == Domain::Hardware && d.status == CoverageStatus::NotCollected));
    assert_eq!(sparse.status, Status::Partial);
}

#[test]
fn cost_needs_complete_usage_and_a_verified_catalog_price() {
    let pricing = fixture_pricing();
    let initial = fixture();
    let next = usable_snapshot();
    let id = analyze(&next)[0].id.clone();
    let run = |replies: Vec<Reply>, pricing: Option<Pricing>| {
        let mut m = script(replies);
        m.pricing = pricing;
        investigate(&mut m, "test", &initial, &mut tb(|| next.clone()), |_| {})
    };
    let snapshot_call = || tools(vec![call("a", "get_system_snapshot", "{}")]);
    let chosen = alias_after(&initial, 0);
    let r = run(vec![snapshot_call(), done(&chosen)], Some(pricing.clone()));
    assert_eq!(r.selected.id, id);
    assert_eq!(r.stats.prompt_tokens, Some(18));
    assert_eq!(r.stats.completion_tokens, Some(2));
    assert_eq!(r.stats.requests.len(), 2);
    // 18 input tokens at $0.30 and 2 output tokens at $0.90 per million.
    assert_eq!(
        r.stats.cost.as_ref().map(|c| c.usd.as_str()),
        Some("0.0000072")
    );
    assert_eq!(r.stats.cost.unwrap().pricing, pricing);

    assert!(run(vec![snapshot_call(), done(&chosen)], None)
        .stats
        .cost
        .is_none());
    let unreported = Reply {
        calls: vec![call(
            "answer",
            SUBMIT_ANSWER,
            &answer_args(&format!("See [{chosen}].")),
        )],
        ..Reply::default()
    };
    let r = run(vec![snapshot_call(), unreported], Some(pricing.clone()));
    assert_eq!(r.stats.prompt_tokens, None);
    assert!(r.stats.cost.is_none());

    // A reply cut off at the output limit is billed: it ends the investigation, but its
    // tokens are counted and priced.
    let cut_off = Reply {
        prompt_tokens: Some(9),
        completion_tokens: Some(u64::from(MAX_OUTPUT_TOKENS)),
        rejected: Some(AgentError::OutputLimit),
        ..Reply::default()
    };
    // A reply cut off at the output limit is billed: its tokens are counted and priced, and the
    // request is asked again a bounded number of times.
    let r = run(
        vec![cut_off.clone(), snapshot_call(), done(&chosen)],
        Some(pricing.clone()),
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.stats.inference_calls, 3);
    assert_eq!(
        r.stats.completion_tokens,
        Some(u64::from(MAX_OUTPUT_TOKENS) + 2)
    );
    let r = run(vec![cut_off; MAX_REPAIRS + 1], Some(pricing));
    assert_eq!(r.status, Status::Partial);
    assert_eq!(r.error, Some(AgentError::OutputLimit));
    assert_eq!(r.stats.inference_calls, MAX_REPAIRS + 1);
    assert_eq!(r.stats.tool_calls, 0);
    assert_eq!(
        r.stats.completion_tokens,
        Some(u64::from(MAX_OUTPUT_TOKENS) * (MAX_REPAIRS as u64 + 1))
    );
    assert!(r.stats.cost.is_some());
}
#[test]
fn first_message_is_compact_and_uses_finding_aliases() {
    let mut s = fixture();
    for i in 0..10 {
        let entity = format!("proc-{i:08x}");
        s.evidence.push(Evidence::measured(
            Metric::ProcessWorkingSetBytes,
            &entity,
            1 << 30,
            "Win32.PROCESS_MEMORY_COUNTERS_EX.WorkingSetSize",
            Window::instant(),
        ));
        s.evidence.push(Evidence::measured(
            Metric::ProcessPrivateCommitBytes,
            &entity,
            2 << 30,
            "Win32.PROCESS_MEMORY_COUNTERS_EX.PrivateUsage",
            Window::instant(),
        ));
        for metric in [
            Metric::ProcessCpuBasisPoints,
            Metric::ProcessIoReadBytes,
            Metric::ProcessIoWriteBytes,
            Metric::ProcessIoOtherBytes,
        ] {
            s.evidence.push(Evidence::missing(
                metric,
                &entity,
                "Win32.GetProcessTimes",
                Status::Partial,
                "First observation of this instance; a rate needs two observations.",
            ));
        }
    }
    let message = initial_message("Pourquoi mon PC rame ?", &s);
    let text = message.to_string();
    for local in [
        "snapshot-",
        "\"ev-",
        "\"source\"",
        "evidence_refs",
        "4242",
        "PRIVATE",
    ] {
        assert!(!text.contains(local), "{local} in {text}");
    }
    let count = analyze(&s).len();
    let expected: Vec<String> = (1..=count).map(|n| format!("F{n}")).collect();
    assert_eq!(message["current_finding_ids"], json!(expected));
    assert_eq!(message["findings"][0]["id"], "F1");
    let gaps = &message["initial_observation"]["measurements"]["not_measured"];
    assert_eq!(gaps.as_array().map(Vec::len), Some(4));
    assert_eq!(gaps[0]["entities"], 10);
    // One full record per evidence item is what the model used to receive.
    let records = serde_json::to_string(&s.evidence).unwrap().len();
    assert!(
        text.len() * 4 < records,
        "{} vs {records} bytes",
        text.len()
    );
}
