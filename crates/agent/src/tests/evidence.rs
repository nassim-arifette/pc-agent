use super::*;

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
