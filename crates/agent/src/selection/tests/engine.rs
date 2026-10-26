use super::*;
use crate::PastTurn;

#[test]
fn the_public_selected_engine_sends_the_exact_preview_and_verifies_the_selected_tool_result() {
    use crate::{
        converse_with_selection, AgentError, FunctionCall, Model, Reply, ToolCall, ToolChoice,
        SUBMIT_ANSWER,
    };
    use std::{collections::VecDeque, time::Duration};
    struct Script {
        replies: VecDeque<Reply>,
        requests: Vec<Vec<Value>>,
        definitions: Vec<Value>,
        choices: Vec<ToolChoice>,
    }
    impl Model for Script {
        fn complete(
            &mut self,
            messages: &[Value],
            tools: &Value,
            choice: ToolChoice,
            _: Duration,
        ) -> Result<Reply, AgentError> {
            self.requests.push(messages.to_vec());
            self.definitions.push(tools.clone());
            self.choices.push(choice);
            Ok(self.replies.pop_front().expect("script exhausted"))
        }
    }
    let reply = |id: &str, name: &str, arguments: Value| Reply {
        calls: vec![ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.to_string(),
            },
        }],
        total_tokens: Some(10),
        prompt_tokens: Some(8),
        completion_tokens: Some(2),
        ..Reply::default()
    };
    let source = SelectionSource::Snapshot(capture());
    let selection = frozen(
        &source,
        &SelectionTarget::Resource {
            resource: Resource::Cpu,
        },
    );
    let past = vec![PastTurn {
        question: "What did we observe earlier?".into(),
        answer: "The earlier capture was incomplete.".into(),
        ..PastTurn::default()
    }];
    let expected =
        selection.preview_message(&past, "Explain the selected CPU capture.", &privacy());
    let mut model = Script {
        replies: VecDeque::from([
            reply("selected-read", GET_SELECTED_EVIDENCE.name, json!({})),
            reply(
                "answer",
                SUBMIT_ANSWER,
                json!({"answer":"The selected capture measured CPU usage of 93% [O1]. This describes the saved capture; the present state remains unknown.","confidence":"low","category":"cpu","action_refs":[],"follow_ups":[]}),
            ),
        ]),
        requests: vec![],
        definitions: vec![],
        choices: vec![],
    };
    let mut delegate =
        SnapshotTools::new(|_, _| panic!("Explain cannot reach the native collector"));
    let mut tools = SelectedTools::new(&mut delegate, &selection);
    let report = converse_with_selection(
        &mut model,
        &past,
        "Explain the selected CPU capture.",
        &selection,
        &mut tools,
        &privacy(),
        |_| {},
        |_, _| false,
    );
    let actual: Value =
        serde_json::from_str(model.requests[0][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(actual, expected);
    // The re-read lists O0 among the citable IDs, as the checker accepts it, and repeats no
    // finding the turn already holds.
    let tool_reply: Value = model.requests[1]
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .map(|message| serde_json::from_str(message["content"].as_str().unwrap()).unwrap())
        .unwrap();
    assert_eq!(tool_reply["citable_observation_ids"], json!(["O0", "O1"]));
    let ids = tool_reply["current_finding_ids"].as_array().unwrap();
    let unique: std::collections::BTreeSet<_> = ids.iter().filter_map(Value::as_str).collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    assert_eq!(model.definitions[0].as_array().unwrap().len(), 2);
    assert_eq!(report.stats.tool_calls, 1);
    assert_eq!(report.initial_observation.measured_at_unix_ms, 2000);
    assert_eq!(report.observations[0].measured_at_unix_ms, 2000);
    assert!(report
        .answer
        .as_ref()
        .is_some_and(|answer| answer.unverified.is_empty()));
    assert_eq!(
        report.observations[0].observation["measured_at_unix_ms"],
        2000
    );
    // The re-read returns what the first message already held: a short reply points at O0.
    assert_eq!(tool_reply["same_result_as"], "O0");
    assert_eq!(actual["citable_observation_ids"], json!(["O0"]));

    // Explain answers from O0 in one request: the selection is the turn's inspection.
    let mut model = Script {
        replies: VecDeque::from([reply(
            "answer",
            SUBMIT_ANSWER,
            json!({"answer":"The selected capture measured CPU usage of 93% [O0]. The present state remains unknown.","confidence":"low","category":"cpu"}),
        )]),
        requests: vec![],
        definitions: vec![],
        choices: vec![],
    };
    let mut delegate =
        SnapshotTools::new(|_, _| panic!("Explain cannot reach the native collector"));
    let mut tools = SelectedTools::new(&mut delegate, &selection);
    let report = converse_with_selection(
        &mut model,
        &[],
        "Explain the selected CPU capture.",
        &selection,
        &mut tools,
        &privacy(),
        |_| {},
        |_, _| false,
    );
    assert!(model.requests[0][0]["content"]
        .as_str()
        .unwrap()
        .contains("submit_answer directly"));
    assert_eq!(report.stats.inference_calls, 1);
    assert_eq!(report.stats.tool_calls, 0);
    let answer = report.answer.expect("answered from O0");
    assert_eq!(answer.repairs, 0);
    assert!(answer.unverified.is_empty(), "{:?}", answer.unverified);
    assert_eq!(answer.citations[0].reference, "O0");

    let mut model = Script {
        replies: VecDeque::from([
            reply("malformed", SUBMIT_ANSWER, json!({"answer":false})),
            reply(
                "corrected",
                SUBMIT_ANSWER,
                json!({"answer":"The selected capture measured CPU usage of 93% [O0]. The present state remains unknown.","confidence":"low","category":"cpu"}),
            ),
        ]),
        requests: vec![],
        definitions: vec![],
        choices: vec![],
    };
    let report = converse_with_selection(
        &mut model,
        &[],
        "Explain the selected CPU capture.",
        &selection,
        &mut tools,
        &privacy(),
        |_| {},
        |_, _| false,
    );
    assert_eq!(report.stats.inspection_calls, 0);
    assert_eq!(report.stats.tool_calls, 0);
    assert_eq!(report.answer.unwrap().repairs, 1);
    assert_eq!(
        model.choices,
        [ToolChoice::Required, ToolChoice::SubmitAnswer]
    );

    // In Explain, a right value written as current is marked, not sent back.
    let mut model = Script {
        replies: VecDeque::from([reply(
            "answer",
            SUBMIT_ANSWER,
            json!({"answer":"CPU usage is 93% [O0].","confidence":"low","category":"cpu"}),
        )]),
        requests: vec![],
        definitions: vec![],
        choices: vec![],
    };
    let mut delegate =
        SnapshotTools::new(|_, _| panic!("Explain cannot reach the native collector"));
    let mut tools = SelectedTools::new(&mut delegate, &selection);
    let report = converse_with_selection(
        &mut model,
        &[],
        "Explain the selected CPU capture.",
        &selection,
        &mut tools,
        &privacy(),
        |_| {},
        |_, _| false,
    );
    assert_eq!(report.stats.inference_calls, 1);
    let answer = report.answer.expect("accepted with a mark");
    assert_eq!(answer.repairs, 0);
    assert_eq!(answer.unverified.len(), 1);
    assert_eq!(
        answer.unverified[0].kind,
        answer::IssueKind::HistoricalWording
    );
}

#[test]
fn explain_reports_missing_coverage_and_repairs_an_invented_number_without_collecting() {
    use crate::{
        converse_with_selection, AgentError, FunctionCall, Model, Reply, ToolCall, ToolChoice,
        SUBMIT_ANSWER,
    };
    use std::{collections::VecDeque, time::Duration};
    struct Script(VecDeque<Reply>);
    impl Model for Script {
        fn complete(
            &mut self,
            _: &[Value],
            _: &Value,
            _: ToolChoice,
            _: Duration,
        ) -> Result<Reply, AgentError> {
            Ok(self.0.pop_front().expect("script exhausted"))
        }
    }
    let reply = |id: &str, name: &str, arguments: Value| Reply {
        calls: vec![ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.to_string(),
            },
        }],
        ..Reply::default()
    };
    let answer = |text: &str| json!({"answer":text,"confidence":"low","category":"insufficient_evidence","action_refs":[],"follow_ups":[]});
    let mut snapshot = capture();
    for evidence in &mut snapshot.evidence {
        evidence.value = None;
        evidence.status = Status::Unsupported;
    }
    let source = SelectionSource::Snapshot(snapshot);
    let selection = frozen(
        &source,
        &SelectionTarget::Resource {
            resource: Resource::Cpu,
        },
    );
    assert!(!selection.output().inspection_complete);
    let first = selection.preview_message(&[], "Explain unavailable CPU coverage.", &privacy());
    assert!(!checked(
        "During the selected capture, CPU usage was 93% [O0].",
        first
    ));
    let mut delegate =
        SnapshotTools::new(|_, _| panic!("Explain cannot start a collector for missing data"));
    // Unsupported capture coverage remains exact even when the local read succeeds.
    {
        let mut unsupported = selection.clone();
        unsupported.output.status = Status::Unsupported;
        unsupported.output.cloud["status"] = json!(Status::Unsupported);
        let mut tools = SelectedTools::new(&mut delegate, &unsupported);
        let read = tools.run(&GET_SELECTED_EVIDENCE, &Args::default());
        assert!(read.inspection_complete);
        assert_eq!(read.status, Status::Partial);
        assert_eq!(read.cloud, unsupported.output().cloud);
        assert!(read
            .evidence
            .iter()
            .all(|evidence| evidence.value.is_none() && evidence.status == Status::Unsupported));
    }
    let conclusion = "The selected capture did not provide usable CPU measurements [O1]. The evidence is insufficient to diagnose CPU load.";
    let mut model = Script(VecDeque::from([
        reply("selected-read", GET_SELECTED_EVIDENCE.name, json!({})),
        reply(
            "invented-answer",
            SUBMIT_ANSWER,
            answer("During the selected capture, CPU usage was 93% [O1]."),
        ),
        reply("coverage-answer", SUBMIT_ANSWER, answer(conclusion)),
    ]));
    let mut tools = SelectedTools::new(&mut delegate, &selection);
    let report = converse_with_selection(
        &mut model,
        &[],
        "Explain unavailable CPU coverage.",
        &selection,
        &mut tools,
        &privacy(),
        |_| {},
        |_, _| false,
    );
    assert_eq!(report.stats.inference_calls, 3);
    assert_eq!(report.stats.inspection_calls, 1);
    assert_eq!(report.stats.tool_calls, 1);
    let answer = report
        .answer
        .expect("missing coverage can receive an honest model explanation");
    assert_eq!(answer.text, conclusion);
    assert_eq!(answer.category, crate::Category::InsufficientEvidence);
    assert_eq!(answer.repairs, 1);
    assert!(answer.unverified.is_empty());
    assert!(model.0.is_empty());
}
