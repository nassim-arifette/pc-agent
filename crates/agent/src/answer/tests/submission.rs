use super::*;
use crate::answer::text::ascii_reference_hyphens;

#[test]
fn citations_must_exist_and_follow_the_short_form() {
    assert_eq!(kinds("See [O9]."), [IssueKind::UnknownCitation]);
    assert_eq!(kinds("See [O1, F7]."), [IssueKind::UnknownCitation]);
    let answer = check("Mémoire [F1] et processus [O1] ; voir [la note].").unwrap();
    assert_eq!(
        answer
            .citations
            .iter()
            .map(|c| c.target.as_str())
            .collect::<Vec<_>>(),
        ["finding-real", "obs-real"]
    );
}

#[test]
fn names_must_have_been_received_and_markup_is_refused() {
    assert_eq!(kinds("Close steam.exe [O1]."), [IssueKind::UnseenName]);
    assert_eq!(kinds("Run `sfc /scannow` [O1]."), [IssueKind::Command]);
    assert_eq!(
        kinds("Run `Remove-Item C:\\temp` [O1]."),
        [IssueKind::Command]
    );
    assert!(kinds("The key `Run` lists startup programs [O1].").is_empty());
    assert_eq!(kinds("```\ndel /s\n``` [O1]"), [IssueKind::Command]);
    assert_eq!(kinds("See [docs](https://x.test) [O1]."), [IssueKind::Link]);
    assert_eq!(kinds("<b>Low memory</b> [O1]."), [IssueKind::Html]);
    assert!(kinds("Latency stays < 3 ms [O0].").is_empty());
}

#[test]
fn schema_and_action_references_are_enforced() {
    let received = received();
    let references = refs();
    let plans = vec!["plan-1".to_owned()];
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &plans,
    };
    // Form is read leniently: an unknown confidence is medium, an extra field is ignored and
    // a list sent as one text has one item per line.
    let lenient = review(
        r#"{"answer":"x","confidence":"Sure","category":"Storage space","verdict":"ok","follow_ups":"- Et le disque D: ?\n2. Et la mémoire ?"}"#,
        &ctx,
    )
    .unwrap();
    assert_eq!(lenient.confidence, Confidence::Medium);
    assert_eq!(lenient.category, Category::StorageSpace);
    assert_eq!(lenient.follow_ups, ["Et le disque D: ?", "Et la mémoire ?"]);
    let missing = review(r#"{"answer":"x"}"#, &ctx).unwrap();
    assert_eq!(
        (missing.confidence, missing.category),
        (Confidence::Medium, Category::Other)
    );
    let malformed = review(r#"{"confidence":"low"}"#, &ctx);
    assert!(matches!(malformed, Err((ref i, None)) if i[0].kind == IssueKind::Malformed));
    let args = json!({"answer":"Free space [O1].","confidence":"low","category":"storage_space",
        "action_refs":["plan-1","plan-9"],
        "follow_ups":["  Et le disque D: ?  ","See https://x.test",""]})
    .to_string();
    // An unknown plan is dropped and marked; the answer is not sent back for it.
    let shown = review(&args, &ctx).unwrap();
    assert_eq!(shown.unverified[0].kind, IssueKind::UnknownAction);
    assert_eq!(shown.action_refs, ["plan-1"]);
    assert_eq!(shown.follow_ups, ["Et le disque D: ?"]);
    assert_eq!(shown.unverified.len(), 1);
}

#[test]
fn typographic_dashes_in_references_become_ascii_hyphens() {
    // Seen in a real answer: "firefox.exe (proc‑5580aed9)" with U+2011.
    assert_eq!(
        ascii_reference_hyphens("firefox.exe (proc\u{2011}5580aed9) utilisait 2,25 GiB"),
        "firefox.exe (proc-5580aed9) utilisait 2,25 GiB"
    );
    assert_eq!(
        ascii_reference_hyphens("file\u{2013}3 et dir\u{2010}7"),
        "file-3 et dir-7"
    );
    // Ordinary text keeps its dashes: no known prefix, or not followed by a reference.
    for text in [
        "Mesure \u{2013} 3 GiB",
        "approc\u{2011}12",
        "proc\u{2011} seul",
        "2\u{2212}3",
    ] {
        assert_eq!(ascii_reference_hyphens(text), text);
    }
}

#[test]
fn repair_requests_explain_each_problem() {
    let request = repair_request(
        &[Issue::new(IssueKind::UnsupportedNumber, "32 GB")],
        &refs(),
    );
    assert_eq!(request["accepted"], false);
    assert_eq!(request["problems"][0]["kind"], "unsupported_number");
    assert_eq!(request["problems"][0]["excerpt"], "32 GB");
    assert!(request["problems"][0]["fix"]
        .as_str()
        .unwrap()
        .contains("cite that measurement"));
    assert_eq!(request["valid_references"], json!(["O0", "O1", "F1"]));
    let request = repair_request(&[Issue::new(IssueKind::UnknownCitation, "[O9]")], &refs());
    assert_eq!(request["valid_references"], json!(["O0", "O1", "F1"]));
    // Examples name a reference that exists, listed once.
    let mut doubled = refs();
    doubled.push(("F1".into(), CitationKind::Finding, "finding-real".into()));
    let request = repair_request(
        &[Issue::new(IssueKind::MissingCitation, "11,75 GiB")],
        &doubled,
    );
    let fix = request["problems"][0]["fix"].as_str().unwrap();
    assert!(fix.contains("[O0]") && !fix.contains("[O2]"), "{fix}");
    assert_eq!(request["valid_references"], json!(["O0", "O1", "F1"]));
    let request = repair_request(
        &[Issue::new(IssueKind::HistoricalWording, "11,75 GiB")],
        &refs(),
    );
    assert!(request["problems"][0]["fix"]
        .as_str()
        .unwrap()
        .contains("in the past"));
    assert!(request["valid_references"].is_array());
    // A protocol problem asks for the next step, not for the same answer again.
    let request = repair_request(&[Issue::new(IssueKind::NoInspection, "")], &refs());
    assert!(!request["next"].as_str().unwrap().contains("again"));
    assert!(request.get("valid_references").is_none());
    let request = repair_request(&[Issue::new(IssueKind::Malformed, "")], &refs());
    let fix = request["problems"][0]["fix"].as_str().unwrap();
    for field in definition()["function"]["parameters"]["properties"]
        .as_object()
        .unwrap()
        .keys()
    {
        assert!(fix.contains(field), "{field}: {fix}");
    }
}

#[test]
fn a_malformed_submission_says_which_field_is_wrong() {
    let ctx = Context {
        received: &[],
        references: &refs(),
        plans: &[],
    };
    // A closed class and a declared field, never text the model wrote.
    let Err((issues, None)) = review(r#"{"confidence":"high","citations":[]}"#, &ctx) else {
        panic!("an answer without text is malformed");
    };
    assert_eq!(issues[0].kind, IssueKind::Malformed);
    assert_eq!(issues[0].excerpt, "missing_field answer");
    let Err((issues, None)) = review(r#"{"answer":"x","#, &ctx) else {
        panic!("broken JSON is malformed");
    };
    assert_eq!(issues[0].excerpt, "syntax unknown");
}
