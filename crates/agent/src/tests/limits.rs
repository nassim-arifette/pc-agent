use super::*;

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
