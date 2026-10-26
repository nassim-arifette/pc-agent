use super::*;

#[test]
fn grounding_a_value_from_another_observation_does_not_support_a_claim() {
    let received = vec![
        json!({"observation_id":"O1", "measurements":{
            "values":{"proc-a":{"process_working_set_bytes":2_000_000_000_u64}},
            "units":{"process_working_set_bytes":"bytes"}}}),
        json!({"observation_id":"O2", "measurements":{
            "values":{"proc-b":{"process_working_set_bytes":500_000_000}},
            "units":{"process_working_set_bytes":"bytes"}}}),
    ];
    let references = vec![
        ("O1".into(), CitationKind::Observation, "first".into()),
        ("O2".into(), CitationKind::Observation, "second".into()),
    ];
    let result = review(
        &submit("proc-b utilise 2 Go [O2]."),
        &Context {
            received: &received,
            references: &references,
            plans: &[],
        },
    );
    assert!(
        matches!(result, Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
        "a citation cannot borrow a value from a different observation: {result:?}"
    );
}

#[test]
fn grounding_named_processes_and_metrics_cannot_borrow_other_values() {
    let received = vec![json!({"observation_id":"O1", "measurements":{
        "values":{"proc-a":{"process_working_set_bytes":2_000_000_000_u64},
            "proc-b":{"process_working_set_bytes":500_000_000},
            "system":{"cpu_busy_basis_points":1_700},
            "gpu-nvidia-0":{"gpu_utilization_basis_points":7_200}},
        "units":{"process_working_set_bytes":"bytes", "cpu_busy_basis_points":"basis_points", "gpu_utilization_basis_points":"basis_points"}},
        "process_names":{"proc-a":"alpha.exe", "proc-b":"beta.exe"}})];
    let references = refs();
    let context = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in ["beta.exe uses 2 GB [O1].", "Le CPU est occupé à 72 % [O1]."] {
        assert!(
            matches!(review(&submit(text), &context), Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
            "{text}"
        );
    }
    for text in [
        "beta.exe uses 500 MB [O1].",
        "Le CPU est occupé à 17 % [O1].",
        "Le GPU est occupé à 72 % [O1].",
    ] {
        assert!(review(&submit(text), &context).is_ok(), "{text}");
    }
}

#[test]
fn grounding_finding_facts_keep_their_process_and_metric() {
    let received = vec![json!({"findings":[{"id":"F1", "facts":[
        "CPU busy: 17 %.", "GPU utilization: 72 %.",
        "alpha.exe used 2 GB.", "beta.exe used 500 MB."
    ]}], "process_names":{"proc-a":"alpha.exe", "proc-b":"beta.exe"}})];
    let references = refs();
    let context = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in ["Le CPU est à 72 % [F1].", "beta.exe uses 2 GB [F1]."] {
        assert!(
            matches!(review(&submit(text), &context), Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
            "{text}"
        );
    }
    for text in ["Le CPU est à 17 % [F1].", "beta.exe uses 500 MB [F1]."] {
        assert!(review(&submit(text), &context).is_ok(), "{text}");
    }
}

#[test]
fn grounding_question_text_is_not_a_measurement() {
    let mut received = received();
    received[0]["question"] = json!("Est-ce que le PC utilise 99 Go ?");
    let references = refs();
    let result = review(
        &submit("Le PC utilise 99 Go [O1]."),
        &Context {
            received: &received,
            references: &references,
            plans: &[],
        },
    );
    assert!(
        matches!(result, Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
        "a question cannot establish a measured value: {result:?}"
    );
}

#[test]
fn grounding_a_citation_in_another_sentence_is_insufficient() {
    assert!(
        kinds("La mémoire est presque pleine [O1]. Le CPU est à 17,4 %.")
            .contains(&IssueKind::MissingCitation)
    );
}

#[test]
fn earlier_values_cited_as_current_ask_for_past_wording() {
    let received = vec![json!({"historical":true,
        "initial_observation":{"observation_id":"O0","historical":true,
            "values":{"system":{"physical_memory_total":12_616_466_432_u64}},
            "units":{"physical_memory_total":"bytes"}}})];
    let ctx = Context {
        received: &received,
        references: &refs(),
        plans: &[],
    };
    let kinds = |text: &str| -> Vec<IssueKind> {
        let args = json!({"answer":text,"confidence":"high","category":"memory"});
        match review(&args.to_string(), &ctx) {
            Ok(_) => Vec::new(),
            Err((issues, _)) => issues.into_iter().map(|i| i.kind).collect(),
        }
    };
    assert_eq!(
        kinds("La mémoire physique totale est de 11,75 Gio [O0]."),
        [IssueKind::HistoricalWording]
    );
    assert_eq!(
        kinds("La mémoire physique totale est de 99 Gio [O0]."),
        [IssueKind::UnsupportedNumber]
    );
    assert!(
        kinds("Lors de la mesure, la mémoire physique totale était de 11,75 Gio [O0].").is_empty()
    );
    assert!(kinds("Les mesures montrent 11,75 Gio de mémoire physique [O0].").is_empty());
    assert!(kinds("The measured total memory was 11.75 GiB [O0].").is_empty());
    assert_eq!(
        kinds("La mémoire physique totale était de 11,75 Gio et reste toujours la même [O0]."),
        [IssueKind::HistoricalWording]
    );
}

#[test]
fn web_prices_versions_and_capacities_must_be_in_the_cited_snippet() {
    let received = vec![
        json!({"observation_id":"O1","measurements":{"values":{"system":{
            "physical_memory_total_bytes":17_179_869_184_u64}},
            "units":{"physical_memory_total_bytes":"bytes"}}}),
        json!({"origin":"web","sources":[
            {"source_id":"W1","domain":"ldlc.com","title":"Kingston 16 Go DDR5",
             "snippet":"Kingston FURY Impact 16 Go DDR5 SO-DIMM 5600 MHz : 54,95 € TTC"},
            {"source_id":"W2","domain":"nvidia.com","title":"GeForce driver",
             "snippet":"Version 566.36 (32.0.15.6636) fixes stuttering. Supports up to 4 displays."}]}),
    ];
    let mut references = refs();
    for id in ["W1", "W2"] {
        references.push((id.into(), CitationKind::Web, format!("obs-web#{id}")));
    }
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "Une barrette de 16 Go DDR5 coûte environ 54,95 € [W1].",
        "Le pilote 566.36 corrige les saccades [W2].",
        "La version 32.0.15.6636 est la plus récente [W2].",
        // Lowercasing İ changes its length: positions must come from the text itself.
        "İİİ : une barrette de 16 Go DDR5 coûte 54,95 € [W1].",
    ] {
        let answer = review(&submit(text), &ctx);
        assert!(answer.is_ok(), "{text}: {answer:?}");
        assert_eq!(answer.unwrap().citations[0].kind, CitationKind::Web);
    }
    for (text, kind) in [
        // A price absent from the snippet.
        (
            "Une barrette coûte 39 € [W1].",
            IssueKind::UnsupportedNumber,
        ),
        // A price is never a measurement of this PC.
        (
            "Une barrette coûte 54,95 € [O1].",
            IssueKind::MissingCitation,
        ),
        // A version the source does not give.
        (
            "Le pilote 571.96 corrige ce problème [W2].",
            IssueKind::UnsupportedNumber,
        ),
        // A PC measurement never comes from the web.
        (
            "Ce PC a 16 GiB de mémoire [W2].",
            IssueKind::UnsupportedNumber,
        ),
    ] {
        let Err((issues, _)) = review(&submit(text), &ctx) else {
            panic!("accepted: {text}");
        };
        assert!(issues.iter().any(|i| i.kind == kind), "{text}: {issues:?}");
    }
}

#[test]
fn one_instance_of_a_program_named_once_is_any_of_its_processes() {
    let received = vec![json!({"observation_id":"O1",
        "process_names":{"proc-a":"firefox.exe","proc-b":"firefox.exe","proc-c":"chrome.exe"},
        "measurements":{"values":{
            "proc-a":{"process_working_set_bytes":2_373_000_000_u64},
            "proc-b":{"process_working_set_bytes":540_000_000_u64},
            "proc-c":{"process_working_set_bytes":900_000_000_u64}},
        "units":{"process_working_set_bytes":"bytes"}}})];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "Une instance de Firefox utilisait 2,21 GiB [O1].",
        "Une instance de firefox.exe utilisait 540 MB [O1].",
    ] {
        assert!(review(&submit(text), &ctx).is_ok(), "{text}");
    }
    for text in [
        // No instance holds this value.
        "Une instance de Firefox utilisait 9 GiB [O1].",
        // Different programs keep strict attribution: chrome.exe never borrows firefox.exe's.
        "chrome.exe et firefox.exe utilisaient 2,21 GiB [O1].",
    ] {
        assert!(review(&submit(text), &ctx).is_err(), "{text}");
    }
}

#[test]
fn application_name_uses_its_aggregate_and_explicit_process_refs_take_priority() {
    let received = vec![json!({"observation_id":"O1",
        "process_names":{"proc-a":"browser.exe", "proc-b":"browser.exe"},
        "shared_names":{"app-a":"browser.exe"},
        "applications":[{"entity_ref":"app-a", "process_refs":["proc-a","proc-b"]}],
        "measurements":{"values":{"proc-a":{"process_working_set_bytes":1_000_000_000_u64},
            "proc-b":{"process_working_set_bytes":2_000_000_000_u64},
            "app-a":{"process_working_set_bytes":3_000_000_000_u64}},
            "units":{"process_working_set_bytes":"bytes"}}})];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "browser.exe uses 3 GB [O1].",
        "browser.exe process proc-a uses 1 GB [O1].",
    ] {
        assert!(
            review(&submit(text), &ctx).is_ok(),
            "aggregate scoping rejected: {text}"
        );
    }
    assert!(review(&submit("browser.exe uses 1 GB [O1]."), &ctx).is_err());
}
