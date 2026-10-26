use super::*;

#[test]
fn grounding_an_earlier_answer_cannot_certify_a_current_value() {
    let past = [
        PastTurn {
            question: "Pourquoi mon PC rame ?".into(),
            answer: "La mémoire est presque pleine : Chrome utilise 2,3 Go [O2] [F1].".into(),
            ..PastTurn::default()
        },
        PastTurn {
            question: "Et Chrome ?".into(),
            answer: "Voir [la note] et [O1, F2].".into(),
            ..PastTurn::default()
        },
    ];
    let mut m = script(vec![
        tools(vec![call("a", "get_system_snapshot", "{}")]),
        // "2,3 Go" only appears in an earlier answer, not in any measured evidence.
        submit_with(
            "answer",
            "Tout à l'heure, Chrome utilisait 2,3 Go ; c'est toujours visible [O1].",
        ),
        submit_with(
            "corrected",
            "La mesure actuelle ne confirme pas cette valeur [O1].",
        ),
    ]);
    let r = converse(
        &mut m,
        &past,
        "Et maintenant ?",
        &fixture(),
        &mut tb(usable_snapshot),
        |_| {},
        |_, _| false,
    );
    let answer = r.answer.expect("answered");
    assert_eq!(
        answer.repairs, 1,
        "an old answer is not a measurement: {:?}",
        answer.unverified
    );
    let first = &m.messages[0];
    let context: Value = serde_json::from_str(first[1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        context["previous_turns"][0]["question"],
        "Pourquoi mon PC rame ?"
    );
    assert_eq!(
        context["previous_turns"][0]["answer"],
        "La mémoire est presque pleine : Chrome utilise 2,3 Go."
    );
    assert_eq!(context["previous_turns"][1]["answer"], "Voir [la note] et.");
    assert_eq!(context["question"], "Et maintenant ?");
    assert!(first[0]["content"]
        .as_str()
        .unwrap()
        .contains("previous_turns"));
}

#[test]
fn grounding_historical_sources_survive_but_do_not_certify_current_claims() {
    let mut first_model = script(vec![
        tools(vec![call("first", "get_system_snapshot", "{}")]),
        submit_with("answer", "Measured process memory: 2.48 GB [O1]."),
    ]);
    let first = investigate(
        &mut first_model,
        "Memory?",
        &fixture(),
        &mut tb(named_snapshot),
        |_| {},
    );
    let past = vec![PastTurn::from_report("Memory?", &first)];
    assert_eq!(past[0].observations.len(), 1);
    let mut model = script(vec![
        tools(vec![call("current", "get_system_snapshot", "{}")]),
        submit_with("wrong", "Current process memory: 2.48 GB [O10000]."),
        submit_with("fixed", "Earlier measurement: 2.48 GB [O10000]."),
    ]);
    let result = converse(
        &mut model,
        &past,
        "And now?",
        &fixture(),
        &mut tb(usable_snapshot),
        |_| {},
        |_, _| false,
    );
    let answer = result.answer.as_ref().unwrap();
    assert_eq!(answer.repairs, 1);
    assert!(answer.unverified.is_empty());
    let context: Value =
        serde_json::from_str(model.messages[0][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(
        context["previous_turns"][0]["observations"][0]["historical"],
        true
    );
    assert!(context["previous_turns"][0]["observations"][0]["measured_at_unix_ms"].is_u64());
    assert_eq!(
        context["previous_turns"][0]["answer"],
        "Measured process memory: 2.48 GB [O10000]."
    );
    let next = PastTurn::from_report("And now?", &result);
    assert_eq!(
        next.observations[0].measured_at_unix_ms,
        past[0].observations[0].measured_at_unix_ms
    );
}

#[test]
fn anonymized_follow_ups_remove_previously_shared_program_names() {
    let past = vec![PastTurn {
        question: "Et SecretBrowser.exe ?".into(),
        answer: "SecretBrowser est actif [O1]. SECRETbrowser.exe.".into(),
        program_names: [("proc-old".into(), "SecretBrowser.exe".into())].into(),
        ..PastTurn::default()
    }];
    let message = conversation_message(&past, "Et maintenant ?", &fixture(), Sharing::Anonymized);
    assert!(!message.to_string().to_lowercase().contains("secretbrowser"));
    let complete = conversation_message(&past, "Et maintenant ?", &fixture(), Sharing::Complete);
    assert!(complete.to_string().contains("SecretBrowser"));
}

#[test]
fn only_the_most_recent_turns_that_fit_are_read_again() {
    let long = "x".repeat(MAX_PAST_CHARS / 2);
    let past: Vec<PastTurn> = (0..4)
        .map(|i| PastTurn {
            question: format!("q{i}"),
            answer: long.clone(),
            ..PastTurn::default()
        })
        .collect();
    let kept = conversation::recent(&past);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].question, "q3");
    assert!(conversation::recent(&[]).is_empty());
}
