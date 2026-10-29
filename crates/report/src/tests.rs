use super::*;
use pcagent_diagnostics::Support;

fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("pcagent-report-{name}-{}", uuid::Uuid::new_v4()))
}

fn finding(classification: Classification) -> Finding {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "id": "finding-1",
        "classification": classification,
        "support": Support::Plausible,
        "support_reason": "Derived from one short sample.",
        "summary": "Synthetic summary.",
        "facts": [{ "text": "Synthetic fact.", "evidence_refs": [] }],
        "hypothesis": null,
        "counter_evidence": [],
        "missing": [],
        "evidence_refs": [],
        "next_step": "Synthetic next step.",
        "limitations": [],
    }))
    .unwrap()
}

fn no_history() -> (HistoryContext, Vec<AlertSummary>, u64) {
    (
        HistoryContext {
            status: HistoryStatus::Missing,
            latest_sample_unix_ms: None,
            events_last_day: None,
        },
        Vec::new(),
        0,
    )
}

fn alert(id: &str) -> AlertSummary {
    AlertSummary {
        id: id.into(),
        target: "resource:Cpu".into(),
        started_at_unix_ms: 1_793_260_800_000,
        ended_at_unix_ms: None,
        metric: "cpu_busy_basis_points".into(),
        extreme_value: 9_500,
        unit: "basis_points".into(),
        lower_bound: true,
        close_reason: None,
    }
}

const OCT_29: u64 = 1_793_232_000_000; // 2026-10-29 00:00 UTC

#[test]
fn dates_are_utc_calendar_days() {
    assert_eq!(date_stamp(0), "1970-01-01");
    assert_eq!(date_stamp(OCT_29), "2026-10-29");
    assert_eq!(date_stamp(OCT_29 - 1), "2026-10-28");
    assert_eq!(date_stamp(951_782_400_000), "2000-02-29");
}

#[test]
fn only_real_findings_and_alerts_count_as_points_to_review() {
    let quiet = assemble(
        vec![
            finding(Classification::NoContentionObserved),
            finding(Classification::InsufficientEvidence),
        ],
        no_history(),
        OCT_29,
        true,
    );
    assert_eq!(quiet.points_to_review, 0);
    let (history, _, _) = no_history();
    let busy = assemble(
        vec![finding(Classification::LowAvailableMemory)],
        (history, vec![alert("a")], 1),
        OCT_29,
        false,
    );
    assert_eq!(busy.points_to_review, 2);
}

#[test]
fn markdown_is_bilingual_and_states_its_coverage() {
    let quiet = assemble(
        vec![finding(Classification::NoContentionObserved)],
        no_history(),
        OCT_29,
        true,
    );
    let en = markdown(&quiet, Language::En);
    assert!(en.starts_with("# PC Agent report, 2026-10-29"));
    assert!(en.contains("Nothing needs attention"));
    assert!(en.contains("Local history: none yet"));
    assert!(en.contains("Nothing was sent anywhere."));
    let fr = markdown(&quiet, Language::Fr);
    assert!(fr.starts_with("# Rapport PC Agent, 2026-10-29"));
    assert!(fr.contains("Rien ne demande votre attention"));
    assert!(fr.contains("Rien n’a été envoyé."));
}

#[test]
fn markdown_lists_findings_and_bounded_alerts() {
    let (history, _, _) = no_history();
    let report = assemble(
        vec![finding(Classification::LowAvailableMemory)],
        (history, vec![alert("a")], 3),
        OCT_29,
        false,
    );
    let en = markdown(&report, Language::En);
    assert!(en.contains("2 point(s) to review."));
    assert!(en.contains("## Low available memory"));
    assert!(en.contains("- Synthetic fact."));
    assert!(en.contains("Next step: Synthetic next step."));
    assert!(en.contains("resource:Cpu"));
    assert!(en.contains("(lower bound)"));
    assert!(en.contains("still open"));
    assert!(en.contains("- 2 more alert(s)"));
    assert!(!en.contains("Written by the daily scheduled task"));
    let fr = markdown(&report, Language::Fr);
    assert!(fr.contains("Étape suivante : Synthetic next step."));
    assert!(fr.contains("borne basse"));
}

#[test]
fn an_empty_history_is_missing_not_invented() {
    let store = Store::open_in_memory().unwrap();
    let (history, alerts, total) = read_history(&store, OCT_29);
    assert_eq!(history.status, HistoryStatus::Missing);
    assert_eq!(history.latest_sample_unix_ms, None);
    assert!(alerts.is_empty());
    assert_eq!(total, 0);
}

#[test]
fn collect_never_creates_a_missing_history_file() {
    let dir = temp_dir("collect");
    let database = dir.join("pcagent.sqlite");
    let report = collect(Some(&database), OCT_29, true);
    assert_eq!(report.history.status, HistoryStatus::Missing);
    assert!(!database.exists());
    assert!(!report.findings.is_empty());
}

#[test]
fn write_replaces_the_day_rotates_and_reads_back_the_newest() {
    let dir = temp_dir("write");
    fs::create_dir_all(&dir).unwrap();
    for day in 1..=31 {
        for ext in ["json", "md"] {
            fs::write(dir.join(format!("rapport-2026-09-{day:02}.{ext}")), "{}").unwrap();
        }
    }
    fs::write(dir.join("notes.txt"), "keep").unwrap();
    fs::write(dir.join("rapport-draft.json"), "keep").unwrap();
    let report = assemble(
        vec![finding(Classification::HighCpuSample)],
        no_history(),
        OCT_29,
        true,
    );
    write(&dir, &report, Language::Fr).unwrap();
    write(&dir, &report, Language::Fr).unwrap();
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    // 30 days kept (29 old ones + today), plus the two unrelated files; no temporary file left.
    assert_eq!(names.len(), 62, "{names:?}");
    assert!(!names.iter().any(|n| n.ends_with(".tmp")));
    assert!(!names.contains(&"rapport-2026-09-02.md".to_owned()));
    assert!(names.contains(&"rapport-2026-09-03.md".to_owned()));
    assert!(names.contains(&"notes.txt".to_owned()));
    assert!(names.contains(&"rapport-draft.json".to_owned()));
    let read = latest(&dir).unwrap().unwrap();
    assert_eq!(read.date, "2026-10-29");
    assert_eq!(read.points_to_review, 1);
    assert!(fs::read_to_string(dir.join("rapport-2026-10-29.md"))
        .unwrap()
        .starts_with("# Rapport PC Agent"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_damaged_newest_report_falls_back_to_the_previous_one() {
    let dir = temp_dir("latest");
    assert!(latest(&dir).unwrap().is_none());
    let report = assemble(Vec::new(), no_history(), OCT_29 - DAY_MS, false);
    write(&dir, &report, Language::En).unwrap();
    fs::write(dir.join("rapport-2026-10-29.json"), "{ not json").unwrap();
    assert_eq!(latest(&dir).unwrap().unwrap().date, "2026-10-28");
    // The Markdown path follows the file name even when the content claims another date.
    let mut forged = report.clone();
    forged.date = "../../elsewhere".into();
    fs::write(
        dir.join("rapport-2026-10-30.json"),
        serde_json::to_vec(&forged).unwrap(),
    )
    .unwrap();
    let (path, read) = latest_file(&dir).unwrap().unwrap();
    assert_eq!(read.date, "../../elsewhere");
    assert_eq!(path, dir.join("rapport-2026-10-30.md"));
    fs::remove_dir_all(dir).unwrap();
}
