use super::*;

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
    let repair: Value = serde_json::from_str(alone["content"].as_str().unwrap()).unwrap();
    let kinds: Vec<_> = repair["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"not_alone"));
    assert!(kinds.contains(&"no_inspection"));
    assert!(repair["next"].as_str().unwrap().contains("inspection tool"));
    let plain = m.messages[2].last().unwrap();
    assert_eq!(plain["role"], "user");
    assert!(plain["content"].as_str().unwrap().contains("no_tool_call"));
    assert!(plain["content"].as_str().unwrap().contains("no_inspection"));
}

#[test]
fn a_protocol_invalid_answer_is_never_shown_as_a_fallback() {
    for inspected in [false, true] {
        let mut replies = Vec::new();
        if inspected {
            replies.push(tools(vec![call("inspect", "get_system_snapshot", "{}")]));
        }
        for attempt in 0..=MAX_REPAIRS {
            replies.push(tools(vec![
                call(
                    &format!("{attempt}-a"),
                    SUBMIT_ANSWER,
                    &answer_args("See [O0]."),
                ),
                call(
                    &format!("{attempt}-b"),
                    SUBMIT_ANSWER,
                    &answer_args("See [O0]."),
                ),
            ]));
        }
        let mut model = script(replies);
        let result = investigate(
            &mut model,
            "test",
            &fixture(),
            &mut tb(usable_snapshot),
            |_| {},
        );
        assert!(result.answer.is_none());
        assert_eq!(result.error, Some(AgentError::InvalidFinding));
        assert_eq!(model.calls, MAX_REPAIRS + 1 + usize::from(inspected));
        for (index, choice) in model.choices.iter().enumerate() {
            assert_eq!(
                *choice,
                if inspected && index >= 2 {
                    ToolChoice::SubmitAnswer
                } else {
                    ToolChoice::Required
                }
            );
        }
    }
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
fn invalid_question_never_reaches_transport() {
    let mut m = script(vec![]);
    let r = investigate(&mut m, "", &fixture(), &mut tb(fixture), |_| {});
    assert_eq!(m.calls, 0);
    assert_eq!(r.error, Some(AgentError::InvalidQuestion));
}
