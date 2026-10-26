use super::*;
use pcagent_store::{HistoryQuery, HistorySeries, Store, StoredSample};

#[test]
fn events_use_measured_peaks_not_episode_boundaries_and_legacy_rows_cannot_certify_numbers() {
    let series = stored_events();
    assert!(!series.events.is_empty());
    let event_id = series.events[0].id.clone();
    let target = SelectionTarget::Event {
        event_id: event_id.clone(),
    };
    let source = SelectionSource::Events(series.clone());
    let selection = frozen(&source, &target);
    let message = selection.preview_message(&[], "Explain this recorded event.", &privacy());
    let peak = &message["initial_observation"]["events"][0]["peak_observation"];
    assert_eq!(peak["window"]["started_at_unix_ms"], 1000);
    assert_eq!(peak["window"]["duration_ms"], 1000);
    assert!(checked(
        "During the recorded event, CPU usage was 93% [O0].",
        message.clone()
    ));
    assert!(!checked("Currently CPU usage is 93% [O0].", message));
    let mut legacy = series;
    legacy.events[0].peak_observation = None;
    legacy.events[0].contributor_observation = None;
    let selection = frozen(&SelectionSource::Events(legacy), &target);
    let message = selection.preview_message(&[], "Explain this recorded event.", &privacy());
    assert!(message["initial_observation"]["events"][0]["peak_observation"].is_null());
    assert!(message["initial_observation"]["measured_at_unix_ms"].is_null());
    assert!(!message.to_string().contains(&event_id));
    assert!(!checked(
        "During the recorded event, CPU usage was 93% [O0].",
        message
    ));
}

#[test]
fn recording_reuses_stored_points_and_does_not_start_another_recording() {
    let mut store = Store::open_in_memory().unwrap();
    let recording = store.begin_recording(30, 1000).unwrap();
    store
        .append_sample(&StoredSample::from_snapshot(&capture(), 2000, 1000))
        .unwrap();
    store
        .finish_recording(&recording.id, 31000, false, 1)
        .unwrap();
    let history = store
        .metric_history(&HistoryQuery {
            metric: Metric::CpuBusyBasisPoints,
            entity_ref: "system".into(),
            from_unix_ms: 1000,
            to_unix_ms: 31000,
        })
        .unwrap();
    let source = SelectionSource::Recording(RecordingSource {
        recording: store.recording(&recording.id).unwrap().unwrap(),
        histories: vec![history],
        cpu_peaks: None,
        resource_peaks: None,
        battery: None,
    });
    let selection = frozen(
        &source,
        &SelectionTarget::Recording {
            recording_id: recording.id.clone(),
        },
    );
    let message = selection.preview_message(&[], "Explain the existing recording.", &privacy());
    assert_eq!(
        message["initial_observation"]["recording"]["started_at_unix_ms"],
        1000
    );
    assert_eq!(message["initial_observation"]["measured_at_unix_ms"], 2000);
    assert!(!message.to_string().contains(&recording.id));
    assert!(checked(
        "During the recording, CPU usage was 93% [O0].",
        message
    ));
    assert_eq!(store.recording(&recording.id).unwrap().unwrap().samples, 1);
}

fn stored_events() -> HistorySeries {
    let mut store = Store::open_in_memory().unwrap();
    let snapshot = capture();
    store
        .append_sample(&StoredSample::from_snapshot(&snapshot, 2000, 1000))
        .unwrap();
    store
        .metric_history(&HistoryQuery {
            metric: Metric::CpuBusyBasisPoints,
            entity_ref: "system".into(),
            from_unix_ms: 1,
            to_unix_ms: 4000,
        })
        .unwrap()
}
