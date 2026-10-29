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

#[test]
fn one_chart_source_selects_every_event_without_a_256_target_cutoff() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .append_sample(&StoredSample::from_snapshot(&capture(), 2000, 1000))
        .unwrap();
    let mut chart = store.history_chart(1, 4000, 300).unwrap();
    let event = chart.events[0].clone();
    chart.events = (0..400)
        .map(|index| {
            let mut event = event.clone();
            event.id = format!("event-history-{index}");
            event
        })
        .collect();
    let wire_size = serde_json::to_vec(&chart).unwrap().len();
    let source = SelectionSource::Chart(chart);
    assert!(source.is_history());
    assert!(source.retained_bytes() > wire_size);
    assert_eq!(source.targets().len(), 404);
    let target = SelectionTarget::Event {
        event_id: "event-history-399".into(),
    };
    assert_eq!(source.measured_at(&target), Some(2000));
    let selection = frozen(&source, &target);
    let message = selection.preview_message(&[], "Explain this recorded event.", &privacy());
    assert_eq!(
        message["initial_observation"]["events"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(!message.to_string().contains("event-history-"));
    assert!(!message.to_string().contains("Alice"));
    assert!(checked(
        "During the recorded event, CPU usage was 93% [O0].",
        message
    ));
    let cpu = SelectionTarget::Resource {
        resource: Resource::Cpu,
    };
    let message = frozen(&source, &cpu).preview_message(&[], "Explain recorded CPU.", &privacy());
    assert!(checked(
        "During the recording, CPU usage was 93% [O0].",
        message
    ));
    assert!(!source.targets().contains(&SelectionTarget::Event {
        event_id: "event-forged".into()
    }));
}
