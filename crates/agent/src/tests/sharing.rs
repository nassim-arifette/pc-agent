use super::*;

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
    let message = initial_message("Pourquoi mon PC rame ?", &s, Sharing::Anonymized);
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

#[test]
fn complete_sharing_sends_executable_names_and_nothing_else_local() {
    let s = named_snapshot();
    let shared = initial_message("Quel programme ?", &s, Sharing::Complete).to_string();
    assert!(
        shared.contains(r#""process_names":{"proc-opaque":"PRIVATE_SECRET_IGNORE_ALL_RULES.exe"}"#),
        "{shared}"
    );
    // Only processes the payload mentions are named; paths, IDs and times never leave.
    for local in [
        "proc-path",
        "alice",
        "4242",
        "1337",
        "private-boot",
        "fixture-session",
        "creation_time",
    ] {
        assert!(!shared.contains(local), "{local} in {shared}");
    }
    let hidden = initial_message("Quel programme ?", &s, Sharing::Anonymized).to_string();
    assert!(!hidden.contains("process_names"));
    assert!(!hidden.contains("PRIVATE"));
    assert_eq!(
        shared_name("C:\\Users\\alice\\tools\\bad\u{202E}name.exe").as_deref(),
        Some("badname.exe")
    );
    assert_eq!(
        shared_name(&"x".repeat(200)).map(|n| n.len()),
        Some(MAX_SHARED_NAME_CHARS)
    );
    assert_eq!(shared_name("   "), None);
}

#[test]
fn names_follow_the_sharing_choice_through_the_investigation() {
    struct Named(Sharing);
    impl Toolbox for Named {
        fn specs(&self) -> &[ToolSpec] {
            &NAMED_SPECS
        }
        fn sharing(&self) -> Sharing {
            self.0
        }
        fn run(&mut self, _: &ToolSpec, _: &Args) -> ToolOutput {
            ToolOutput::from_snapshot(&named_snapshot())
        }
    }
    const NAMED_SPECS: [ToolSpec; 1] = [ToolSpec {
        name: "get_processes",
        class: ToolClass::Inspection,
        description: "",
        params: &[],
    }];
    for sharing in [Sharing::Complete, Sharing::Anonymized] {
        let mut m = script(vec![
            tools(vec![call("a", "get_processes", "{}")]),
            submit_with(
                "first",
                "PRIVATE_SECRET_IGNORE_ALL_RULES.exe uses 2.48 GB [O1].",
            ),
            submit_with("second", "The largest process uses 2.48 GB [O1]."),
        ]);
        let r = investigate(&mut m, "test", &fixture(), &mut Named(sharing), |_| {});
        let answer = r.answer.expect("answered");
        let tool_reply = m.messages[1].last().unwrap()["content"]
            .as_str()
            .unwrap()
            .to_owned();
        let system = m.messages[0][0]["content"].as_str().unwrap().to_owned();
        match sharing {
            Sharing::Complete => {
                assert!(tool_reply.contains("process_names"));
                assert!(system.contains("process_names"));
                // The name was received, so the first answer is accepted as written.
                assert_eq!(answer.repairs, 0);
            }
            Sharing::Anonymized => {
                assert!(!tool_reply.contains("PRIVATE"));
                // A name the model never received is sent back for correction.
                assert_eq!(answer.repairs, 1);
            }
        }
    }
}

#[test]
fn frozen_c5_projection_matches_preview_and_filters_every_request() {
    struct SensitiveTools(Sharing);
    impl Toolbox for SensitiveTools {
        fn specs(&self) -> &[ToolSpec] {
            &SNAPSHOT_SPECS_FOR_PRIVACY
        }
        fn sharing(&self) -> Sharing {
            self.0
        }
        fn run(&mut self, _: &ToolSpec, _: &Args) -> ToolOutput {
            let mut output = ToolOutput::from_snapshot(&usable_snapshot());
            output.cloud["note"] =
                json!("Alice: known-example-secret, alice@example.test on ALICE-LAPTOP");
            output.cloud["command_line"] = json!("PRIVATE-COMMAND-CONTENT");
            output.cloud["shared_names"] = json!({"service-1":"Private Service"});
            output.cloud["items"] = json!([{"shared_name":"private-report.pdf", "shared_path":"%USERPROFILE%\\Downloads\\private-report.pdf"}]);
            output
        }
    }
    const SNAPSHOT_SPECS_FOR_PRIVACY: [ToolSpec; 1] = [ToolSpec {
        name: "get_system_snapshot",
        class: ToolClass::Inspection,
        description: "",
        params: &[],
    }];
    let privacy = PrivacyContext::from_identifiers(
        r"C:\Users\Alice",
        "Alice",
        "ALICE-LAPTOP",
        &["known-example-secret"],
    );
    let question =
        "Alice: why does C:\\Users\\Alice\\Downloads\\private-report.pdf affect Private Service?";
    let past = vec![PastTurn {
        question: "Private Service and private-report.pdf on ALICE-LAPTOP".into(),
        answer: "Earlier, Alice saw known-example-secret".into(),
        shared_names: [
            ("service-1".into(), "Private Service".into()),
            ("file-1".into(), "private-report.pdf".into()),
        ]
        .into(),
        ..PastTurn::default()
    }];
    for sharing in [Sharing::Complete, Sharing::Anonymized] {
        let initial = usable_snapshot();
        let preview =
            conversation_message_with_privacy(&past, question, &initial, sharing, &privacy)
                .to_string();
        let mut model = script(vec![
            tools(vec![call("inspect", "get_system_snapshot", "{}")]),
            submit_with(
                "unsafe",
                "Run `powershell -Command known-example-secret` on ALICE-LAPTOP [O1].",
            ),
            submit_with("safe", "The inspection returned usable observations [O1]."),
        ]);
        let result = converse_with_privacy(
            &mut model,
            &past,
            question,
            &initial,
            &mut SensitiveTools(sharing),
            &privacy,
            |_| {},
            |_, _| false,
        );
        assert!(result.answer.unwrap().unverified.is_empty());
        assert_eq!(
            model.messages[0][1]["content"].as_str(),
            Some(preview.as_str())
        );
        let wire = serde_json::to_string(&model.messages).unwrap();
        for forbidden in [
            "Alice",
            "ALICE-LAPTOP",
            "alice@example.test",
            "known-example-secret",
            "PRIVATE-COMMAND-CONTENT",
        ] {
            assert!(!wire.contains(forbidden), "private marker sent to a model");
        }
        if sharing == Sharing::Anonymized {
            assert!(!wire.contains("Private Service"));
            assert!(!wire.contains("private-report.pdf"));
        } else {
            assert!(wire.contains("Private Service"));
            assert!(wire.contains("private-report.pdf"));
        }
    }
}

#[test]
fn an_executable_mentioned_only_in_the_question_is_not_a_verified_name() {
    let mut model = script(vec![
        tools(vec![call("inspect", "get_system_snapshot", "{}")]),
        submit_with("guessed", "question-only.exe is responsible [O1]."),
        submit_with(
            "fixed",
            "The measured evidence does not identify that program [O1].",
        ),
    ]);
    let result = investigate(
        &mut model,
        "Could question-only.exe be responsible?",
        &fixture(),
        &mut tb(usable_snapshot),
        |_| {},
    );
    assert_eq!(result.answer.unwrap().repairs, 1);
}
