use super::*;

#[test]
fn a_local_report_localizes_process_references_and_has_no_usage() {
    let mut s = fixture();
    s.evidence.push(Evidence::measured(
        Metric::ProcessPrivateCommitBytes,
        "proc-00000001",
        5 << 30,
        "fixture",
        Window::instant(),
    ));
    let inv = pcagent_agent::local_report(&s);
    let r = report(&inv, "Why is my PC slow?", &names);
    assert_eq!(r.mode, "local_rules");
    assert!(r.usage.is_none());
    assert_eq!(r.question, "Why is my PC slow?");
    let contributor = inv
        .findings
        .iter()
        .find(|f| f.classification == Classification::LargestMemoryContributor)
        .expect("the fixture has a largest memory contributor");
    // Named as the window names it, without a PID: no other cited process reads the same.
    for (readable, shown) in [
        (&names as Readable<'_>, "Example Editor"),
        (&no_readable_names as Readable<'_>, "editor.exe"),
    ] {
        let text =
            serde_json::to_string(&finding(contributor, &inv.local_processes, readable)).unwrap();
        assert!(!text.contains("proc-00000001"), "{text}");
        assert!(text.contains(shown) && !text.contains("PID"), "{text}");
    }
    // A model that wrote the name and the reference does not get the name twice.
    assert_eq!(
        localize(
            "editor.exe (proc-00000001) uses 5 GiB; proc-00000001 grew.",
            &inv.local_processes,
            &names
        ),
        "Example Editor uses 5 GiB; Example Editor grew."
    );
    assert_eq!(
        localize(
            "Editor.EXE (proc-00000001) uses 5 GiB.",
            &inv.local_processes,
            &no_readable_names
        ),
        "editor.exe uses 5 GiB."
    );
}

#[test]
fn an_answer_names_programs_locally_and_its_context_keeps_the_model_text() {
    use pcagent_agent::{Answer, Citation, Issue};
    let mut inv = pcagent_agent::local_report(&fixture());
    inv.local_processes.extend([
        process("proc-0000000a", 40, "browser.exe"),
        process("proc-0000000b", 41, "browser.exe"),
    ]);
    let mut source = observed("get_processes", serde_json::json!({}));
    source.observation_id = "obs-1".into();
    source.facts = vec![
        "proc-0000000a used 2.00 GiB.".into(),
        "proc-0000000b used 1.00 GiB.".into(),
    ];
    inv.observations.push(source);
    inv.observation_refs.push("obs-1".into());
    let text = "Close browser.exe (proc-0000000a) to free 2 GiB [O1]; proc-00000001 can stay.";
    inv.answer = Some(Answer {
        text: text.into(),
        confidence: Confidence::Medium,
        category: Category::Memory,
        citations: vec![Citation {
            reference: "O1".into(),
            kind: CitationKind::Observation,
            target: "obs-1".into(),
        }],
        action_refs: Vec::new(),
        follow_ups: vec!["Why does proc-0000000a grow?".into()],
        unverified: vec![Issue {
            kind: IssueKind::UnsupportedNumber,
            excerpt: "proc-0000000a to free 2 GiB".into(),
        }],
        repairs: 0,
    });
    let r = report(&inv, "Why is my PC slow?", &names);
    let answer = r.answer.as_ref().unwrap();
    // One browser process in the text: no PID there, though its source cites two. The text
    // is exactly what its streamed draft showed.
    assert_eq!(
        answer.text,
        "Close Example Browser to free 2 GiB [O1]; Example Editor can stay."
    );
    assert_eq!(answer.text, localize(text, &inv.local_processes, &names));
    assert_eq!(
        answer.unverified[0].excerpt,
        "Example Browser to free 2 GiB"
    );
    // The source reads as one block: its two browser processes are told apart.
    assert_eq!(
        answer.citations[0].facts,
        [
            "Example Browser (PID 40) used 2.00 GiB.",
            "Example Browser (PID 41) used 1.00 GiB."
        ]
    );
    assert_eq!(r.observations[0].facts, answer.citations[0].facts);
    // A follow-up is sent as the next question when clicked: it keeps the model's reference.
    assert_eq!(answer.follow_ups, ["Why does proc-0000000a grow?"]);
    // The next turns' context keeps the model's own text: no readable name leaves the PC.
    assert_eq!(r.context.answer, text);
    let context = serde_json::to_string(&r.context).unwrap();
    assert!(!context.contains("Example"), "{context}");
}

#[test]
fn findings_and_their_citations_carry_phrases_beside_their_english_text() {
    use pcagent_agent::{Answer, Category, Citation, Confidence};
    let mut s = fixture();
    s.evidence.push(Evidence::measured(
        Metric::ProcessPrivateCommitBytes,
        "proc-00000001",
        5 << 30,
        "fixture",
        Window::instant(),
    ));
    let mut inv = pcagent_agent::local_report(&s);
    let contributor = inv
        .findings
        .iter()
        .find(|f| f.classification == Classification::LargestMemoryContributor)
        .expect("the fixture has a largest memory contributor")
        .clone();
    // Every sentence the window shows has its phrase, in the same order as the texts.
    let view = finding(&contributor, &inv.local_processes, &no_readable_names);
    assert_eq!(
        view.summary_phrase.as_ref().map(|p| p.key),
        Some("summary.largest_memory_contributor")
    );
    assert_eq!(
        view.facts,
        // One process cited: no PID, as every text shown is labelled (#70).
        ["Process editor.exe had 5.00 GiB private commit."]
    );
    assert_eq!(
        view.fact_phrases,
        [Some(
            Phrase::new("fact.process_private_commit")
                .with(
                    "process",
                    Quantity::Process {
                        name: Some("editor.exe".into()),
                        pid: 4242
                    }
                )
                .with("size", Quantity::Bytes(5 << 30))
        )]
    );
    assert_eq!(
        view.next_step_phrase.as_ref().map(|p| p.key),
        Some("next.instance_growth")
    );
    assert_eq!(view.limitation_phrases.len(), view.limitations.len());
    assert!(
        view.limitation_phrases.iter().all(Option::is_some),
        "{:?}",
        view.limitations
    );
    // An unnamed process is named by the window, never by the English fallback.
    let mut unnamed = inv.local_processes.clone();
    unnamed[0].executable_name = None;
    assert_eq!(
        finding(&contributor, &unnamed, &no_readable_names).fact_phrases[0]
            .as_ref()
            .map(|p| p.values["process"].clone()),
        Some(Quantity::Process {
            name: None,
            pid: 4242
        })
    );
    // Cited by an AI answer: the classification names the source, its summary comes first.
    inv.answer = Some(Answer {
        text: "editor.exe holds the most [F1].".into(),
        confidence: Confidence::Medium,
        category: Category::Memory,
        citations: vec![Citation {
            reference: "F1".into(),
            kind: CitationKind::Finding,
            target: contributor.id.clone(),
        }],
        action_refs: Vec::new(),
        follow_ups: Vec::new(),
        unverified: Vec::new(),
        repairs: 0,
    });
    let r = report(&inv, "q", &no_readable_names);
    let cited = &r.answer.as_ref().unwrap().citations[0];
    assert_eq!(cited.title, "Largest memory contributor");
    assert_eq!(
        cited.classification,
        Some(Classification::LargestMemoryContributor)
    );
    assert_eq!(cited.facts.len(), 2);
    assert_eq!(cited.fact_phrases.len(), 2);
    assert_eq!(
        cited.fact_phrases[0].as_ref().map(|p| p.key),
        Some("summary.largest_memory_contributor")
    );
    assert_eq!(cited.fact_phrases[1], view.fact_phrases[0]);
    let json = serde_json::to_value(cited).unwrap();
    assert_eq!(json["classification"], "largest_memory_contributor");
    assert_eq!(json["fact_phrases"][1]["values"]["size"]["unit"], "bytes");
}

#[test]
fn turn_limitations_have_a_phrase_when_the_window_knows_their_wording() {
    let mut inv = pcagent_agent::local_report(&fixture());
    inv.limitations = vec![
        "Short sample: no sustained contention, memory leak or causal diagnosis can be established.".into(),
        "Process list is ranked by read I/O rate; unknown values sort last.".into(),
        "Process list is ranked by bogus; unknown values sort last.".into(),
        "Initial observation thresholds are prototype triggers, not validated diagnostic rules.".into(),
        LOCAL_EVIDENCE_NOTE.into(),
        "A limitation the window has no wording for.".into(),
    ];
    let keys: Vec<Option<&str>> = report(&inv, "q", &no_readable_names)
        .limitation_phrases
        .iter()
        .map(|p| p.as_ref().map(|p| p.key))
        .collect();
    assert_eq!(
        keys,
        [
            Some("note.short_sample"),
            Some("note.ranked_io_read"),
            None,
            Some("limit.thresholds"),
            Some("note.local_evidence"),
            None,
        ]
    );
}

fn observed(tool: &str, cloud: Value) -> pcagent_agent::Observation {
    pcagent_agent::Observation {
        observation_id: format!("obs-{tool}"),
        citation: String::new(),
        tool: tool.into(),
        measured_at_unix_ms: 0,
        status: pcagent_shared::Status::Unsupported,
        citable: false,
        facts: Vec::new(),
        fact_phrases: Vec::new(),
        limitations: Vec::new(),
        observation: cloud,
        storage_items: Vec::new(),
    }
}

#[test]
fn admin_hints_come_only_from_needs_admin_coverage_of_offered_checks() {
    let mut inv = pcagent_agent::local_report(&fixture());
    assert!(report(&inv, "q", &no_readable_names).admin_hints.is_empty());
    inv.observations.push(observed(
        "get_approved_elevated_inspection",
        serde_json::json!({"task":"disk_reliability","coverage":[{"status":"needs_admin"}]}),
    ));
    inv.observations.push(observed(
        "get_approved_elevated_inspection",
        serde_json::json!({"task":"performance_trace","coverage":[{"status":"needs_admin"}]}),
    ));
    inv.observations.push(observed(
        "get_approved_elevated_inspection",
        serde_json::json!({"task":"battery_energy","coverage":[{"status":"ok"}]}),
    ));
    inv.observations.push(observed(
        "get_cleanup_opportunities",
        serde_json::json!({"coverage":[
            {"scope":"previous_windows","status":"needs_admin"},
            {"scope":"temporary_files","status":"ok"}]}),
    ));
    inv.observations.push(observed(
        "get_cleanup_opportunities",
        serde_json::json!({"coverage":[{"scope":"hibernation_file","status":"needs_admin"}]}),
    ));
    inv.observations.push(observed(
        "get_system_state",
        serde_json::json!({"coverage":[{"scope":"x","status":"needs_admin"}]}),
    ));
    assert_eq!(
        report(&inv, "q", &no_readable_names).admin_hints,
        vec!["system_caches".to_owned(), "disk_reliability".to_owned()]
    );
}
