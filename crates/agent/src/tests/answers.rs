use super::*;

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
    assert_eq!(r.stats.model_requests.len(), 3);
    assert!(r
        .stats
        .model_requests
        .iter()
        .all(|m| m.duration_ms < 60_000));
    assert_eq!(r.stats.repair_requests.len(), 1);
    assert_eq!(r.stats.repair_requests[0].request, 2);
    assert_eq!(
        r.stats.repair_requests[0].issues[0].kind,
        IssueKind::UnsupportedNumber
    );
    assert!(!r.stats.repair_requests[0].output_limit);
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
    assert_eq!(
        m.choices,
        [
            ToolChoice::Required,
            ToolChoice::Required,
            ToolChoice::SubmitAnswer
        ]
    );
}

#[test]
fn plain_text_after_inspection_does_not_start_named_repair() {
    let mut model = script(vec![
        tools(vec![call("inspect", "get_system_snapshot", "{}")]),
        Reply {
            content: Some("A plain answer is not a structured submission.".into()),
            ..Reply::default()
        },
        tools(vec![call("malformed", SUBMIT_ANSWER, "{")]),
        done("O1"),
    ]);
    let result = investigate(
        &mut model,
        "test",
        &fixture(),
        &mut tb(usable_snapshot),
        |_| {},
    );
    assert_eq!(result.answer.unwrap().repairs, 2);
    let kinds: Vec<_> = result
        .stats
        .repair_requests
        .iter()
        .map(|r| {
            (
                r.request,
                r.issues.iter().map(|i| i.kind).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            (2, vec![IssueKind::NoToolCall]),
            (3, vec![IssueKind::Malformed])
        ]
    );
    assert_eq!(
        model.choices,
        [
            ToolChoice::Required,
            ToolChoice::Required,
            ToolChoice::Required,
            ToolChoice::SubmitAnswer,
        ]
    );
}

#[test]
fn named_repair_preserves_validation_and_never_runs_other_tools() {
    for unexpected in [
        Reply {
            content: Some("Plain text is still not a submission.".into()),
            ..Reply::default()
        },
        Reply {
            rejected: Some(AgentError::OutputLimit),
            ..Reply::default()
        },
        tools(vec![call("extra", "get_system_snapshot", "{}")]),
        tools(vec![
            call("extra", "get_system_snapshot", "{}"),
            call("mixed", SUBMIT_ANSWER, &answer_args("See [O1].")),
        ]),
        tools(vec![
            call("duplicate-a", SUBMIT_ANSWER, &answer_args("See [O1].")),
            call("duplicate-b", SUBMIT_ANSWER, &answer_args("See [O1].")),
        ]),
        submit_with("bad-citation", "See [O9]."),
        submit_with("bad-command", "Run `chkdsk /f` [O1]."),
    ] {
        let mut executed = 0;
        let mut model = script(vec![
            tools(vec![call("inspect", "get_system_snapshot", "{}")]),
            tools(vec![call("malformed", SUBMIT_ANSWER, "{")]),
            unexpected,
            done("O1"),
        ]);
        let result = investigate(
            &mut model,
            "test",
            &fixture(),
            &mut tb(|| {
                executed += 1;
                usable_snapshot()
            }),
            |_| {},
        );
        assert_eq!(executed, 1);
        assert_eq!(result.stats.inspection_calls, 1);
        assert_eq!(result.answer.unwrap().repairs, 2);
        assert_eq!(
            model.choices,
            [
                ToolChoice::Required,
                ToolChoice::Required,
                ToolChoice::SubmitAnswer,
                ToolChoice::SubmitAnswer,
            ]
        );
    }
}

#[test]
fn named_repair_does_not_leak_into_the_next_investigation() {
    let mut model = script(vec![
        tools(vec![call("inspect", "get_system_snapshot", "{}")]),
        tools(vec![call("malformed", SUBMIT_ANSWER, "{")]),
        done("O1"),
        tools(vec![call("inspect", "get_system_snapshot", "{}")]),
        done("O1"),
    ]);
    for _ in 0..2 {
        let result = investigate(
            &mut model,
            "test",
            &fixture(),
            &mut tb(usable_snapshot),
            |_| {},
        );
        assert!(result.error.is_none());
        assert!(result.answer.is_some());
    }
    assert_eq!(
        model.choices,
        [
            ToolChoice::Required,
            ToolChoice::Required,
            ToolChoice::SubmitAnswer,
            ToolChoice::Required,
            ToolChoice::Required,
        ]
    );
}

#[test]
fn repairs_are_bounded_and_the_last_answer_is_shown_with_its_problems() {
    let mut replies = vec![tools(vec![call("a", "get_system_snapshot", "{}")])];
    // A different wrong value each time: the model keeps trying, up to the repair limit.
    replies.extend((0..=MAX_REPAIRS).map(|i| {
        submit_with(
            &format!("answer-{i}"),
            &format!("Windows needs 3{i} GB [O1]."),
        )
    }));
    let mut m = script(replies);
    let r = investigate(&mut m, "test", &fixture(), &mut tb(usable_snapshot), |_| {});
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(r.execution_status, ExecutionStatus::Completed);
    let answer = r.answer.unwrap();
    assert_eq!(answer.repairs, MAX_REPAIRS);
    assert_eq!(answer.unverified.len(), 1);
    assert_eq!(answer.unverified[0].kind, IssueKind::UnsupportedNumber);
    assert_eq!(answer.unverified[0].excerpt, format!("3{MAX_REPAIRS} GB"));
}

#[test]
fn the_same_problems_twice_end_the_repairs_early() {
    let mut m = script(vec![
        tools(vec![call("a", "get_system_snapshot", "{}")]),
        submit_with("first", "Windows needs 32 GB [O1]."),
        submit_with("second", "Windows needs 32 GB [O1]."),
        done("O1"),
    ]);
    let r = investigate(&mut m, "test", &fixture(), &mut tb(usable_snapshot), |_| {});
    assert_eq!(m.calls, 3, "the unused reply is never requested");
    assert_eq!(r.stats.inference_calls, 3);
    let answer = r.answer.unwrap();
    assert_eq!(answer.repairs, 1);
    assert_eq!(answer.unverified[0].excerpt, "32 GB");
}

#[test]
fn the_best_attempt_is_shown_not_the_latest() {
    let better = "Windows needs 32 GB [O1].";
    let worse = "Windows needs 32 GB and 48 GB.";
    let mut m = script(vec![
        tools(vec![call("a", "get_system_snapshot", "{}")]),
        submit_with("first", better),
        submit_with("second", worse),
        submit_with("third", worse),
    ]);
    let r = investigate(&mut m, "test", &fixture(), &mut tb(usable_snapshot), |_| {});
    assert_eq!(r.stats.inference_calls, 4);
    let answer = r.answer.unwrap();
    assert_eq!(answer.text, better);
    assert_eq!(answer.repairs, 2);
    assert_eq!(answer.unverified.len(), 1);
}

#[test]
fn a_second_cut_off_in_a_row_ends_the_turn() {
    let cut_off = || Reply {
        rejected: Some(AgentError::OutputLimit),
        ..Reply::default()
    };
    let snapshot_call = || tools(vec![call("a", "get_system_snapshot", "{}")]);
    let mut m = script(vec![snapshot_call(), cut_off(), cut_off(), done("O1")]);
    let r = investigate(&mut m, "test", &fixture(), &mut tb(usable_snapshot), |_| {});
    assert_eq!(r.error, Some(AgentError::OutputLimit));
    assert_eq!(m.calls, 3);
    let retry = m.messages[2].last().unwrap();
    assert_eq!(retry["role"], "user");
    assert!(retry["content"]
        .as_str()
        .unwrap()
        .contains("at most about 1,500 characters"));
    // A reply in between resets the count; before any inspection the retry asks for tools.
    let mut m = script(vec![cut_off(), snapshot_call(), cut_off(), done("O1")]);
    let r = investigate(&mut m, "test", &fixture(), &mut tb(usable_snapshot), |_| {});
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(m.calls, 4);
    assert!(m.messages[1].last().unwrap()["content"]
        .as_str()
        .unwrap()
        .contains("do not write the answer yet"));
}

#[test]
fn cancellation_during_answer_repair_does_not_publish_the_existing_fallback() {
    for billed_reply in [false, true] {
        let mut m = script(vec![
            tools(vec![call("inspect", "get_system_snapshot", "{}")]),
            submit_with("unverified", "Windows needs 32 GB [O1]."),
        ]);
        let cancelled = if billed_reply {
            let mut reply = submit_with("late-answer", "The inspection is complete [O1].");
            reply.rejected = Some(AgentError::Cancelled);
            Ok(reply)
        } else {
            Err(AgentError::Cancelled)
        };
        m.replies.push_back(cancelled);
        let report = investigate(&mut m, "test", &fixture(), &mut tb(usable_snapshot), |_| {});
        assert_eq!(report.error, Some(AgentError::Cancelled));
        assert_eq!(report.execution_status, ExecutionStatus::Failed);
        assert!(
            report.answer.is_none(),
            "a stopped repair cannot publish its fallback"
        );
        assert_eq!(m.calls, 3);
        assert!(m.messages[2].last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("unsupported_number"));
        if billed_reply {
            assert_eq!(report.stats.prompt_tokens, Some(27));
            assert_eq!(report.stats.completion_tokens, Some(3));
        }
    }
}

#[test]
fn protocol_and_text_problems_are_repaired_together_without_running_other_calls() {
    let mut executed = 0;
    let mut model = script(vec![
        tools(vec![
            call("a", SUBMIT_ANSWER, &answer_args("Run `chkdsk C: /f /r`.")),
            call("b", "get_system_snapshot", "{}"),
        ]),
        tools(vec![call("c", "get_system_snapshot", "{}")]),
        done("O1"),
    ]);
    let result = investigate(
        &mut model,
        "test",
        &fixture(),
        &mut tb(|| {
            executed += 1;
            usable_snapshot()
        }),
        |_| {},
    );
    let repair: Value = serde_json::from_str(
        model.messages[1]
            .iter()
            .find(|m| m["tool_call_id"] == "a")
            .unwrap()["content"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let kinds: Vec<_> = repair["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap())
        .collect();
    for expected in ["not_alone", "no_inspection", "command"] {
        assert!(kinds.contains(&expected), "{repair}");
    }
    assert_eq!(executed, 1);
    assert_eq!(result.answer.unwrap().repairs, 1);
    assert!(model.choices.iter().all(|c| *c == ToolChoice::Required));
}

#[test]
fn a_measured_number_without_its_citation_is_shown_marked_not_sent_back() {
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
        submit_with(
            "uncited",
            "Il ne reste que 1,54 Go de mémoire disponible sur 16 Go. La mémoire est presque pleine [O1].",
        ),
    ]);
    let r = investigate(
        &mut m,
        "Pourquoi mon PC rame ?",
        &initial,
        &mut tb(|| next.clone()),
        |_| {},
    );
    let answer = r.answer.expect("accepted without a repair");
    assert_eq!(answer.repairs, 0);
    assert_eq!(m.calls, 2);
    assert!(r.stats.repair_requests.is_empty());
    assert_eq!(answer.unverified.len(), 1);
    assert_eq!(answer.unverified[0].kind, IssueKind::UncitedNumber);
    assert!(answer.unverified[0]
        .excerpt
        .starts_with("Il ne reste que 1,54 Go"));
}

#[test]
fn an_unmeasured_number_without_its_citation_is_still_sent_back() {
    let mut m = script(vec![
        tools(vec![call("a", "get_system_snapshot", "{}")]),
        submit_with("invented", "Il ne reste que 3 Go de mémoire disponible."),
        done("O1"),
    ]);
    let r = investigate(
        &mut m,
        "Pourquoi mon PC rame ?",
        &fixture(),
        &mut tb(usable_snapshot),
        |_| {},
    );
    let answer = r.answer.expect("accepted after one repair");
    assert_eq!(answer.repairs, 1);
    assert_eq!(r.stats.repair_requests.len(), 1);
    assert_eq!(
        r.stats.repair_requests[0].issues[0].kind,
        IssueKind::MissingCitation
    );
}
