//! Persistent-history tools. The host supplies a local sampler; no model-selected file is opened.
use crate::{
    tools::{Args, Param, ParamKind, ToolClass, ToolOutput, ToolSpec},
    PrivacyContext, Sharing,
};
use pcagent_shared::{unix_ms, Evidence, Metric, Status, Window};
use pcagent_store::{
    sampler::Sampler, EventObservation, HistoryQuery, HistorySeries, ProcessPeaks,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{atomic::AtomicBool, Arc},
};

const MAX_HISTORY_POINTS: usize = 120;
const MAX_RECORDING_POINTS: usize = 30;
const MAX_PROCESS_PEAKS: usize = 10;
const MAX_EVENTS: usize = 30;
const MAX_GAPS: usize = 30;
// Store::metric_history currently reads at most this many events. Reaching its limit does not
// prove that older matching events are absent, even before the smaller cloud projection.
const STORE_EVENT_LIMIT: usize = 100;

pub const METRICS: &[&str] = &[
    "physical_total_bytes",
    "physical_available_bytes",
    "system_commit_bytes",
    "system_commit_limit_bytes",
    "cpu_busy_basis_points",
    "cpu_busiest_core_basis_points",
    "disk_read_latency_microseconds",
    "disk_write_latency_microseconds",
    "disk_idle_basis_points",
    "disk_queue_length",
    "hard_fault_pages_per_second_x100",
    "gpu_busiest_engine_basis_points",
    "process_working_set_bytes",
    "process_private_commit_bytes",
    "process_cpu_basis_points",
    "process_io_read_bytes_per_second",
    "process_io_write_bytes_per_second",
    "process_io_other_bytes_per_second",
    "process_gpu_basis_points",
];
const WINDOW: Param = Param {
    name: "window_seconds",
    description: "History window, up to seven days.",
    kind: ParamKind::Integer {
        min: 1,
        max: 604_800,
    },
    required: true,
};
const END: Param = Param {
    name: "end_unix_ms",
    description:
        "Optional end of a past window as Unix milliseconds; defaults to now. Never a future time.",
    kind: ParamKind::Integer {
        min: 1,
        max: i64::MAX as u64,
    },
    required: false,
};
pub const TOOLS: &[ToolSpec] = &[
    ToolSpec { name:"get_metric_history", class:ToolClass::Inspection,
        description:"Read persisted local samples, including previous app sessions. Seven-day retention; one-minute averages/minima/maxima after 24 hours. At most 120 response points retain weighted averages, extrema and coverage. Gaps and missing process rows remain unknown. Use end_unix_ms to investigate a past time.",
        params:&[Param {name:"metric",description:"Recorded metric.",kind:ParamKind::OneOf(METRICS),required:true},WINDOW,END,
            Param {name:"entity_ref",description:"Opaque process reference from history/events, or system (default).",kind:ParamKind::OpaqueRef {prefix:""},required:false}] },
    ToolSpec { name:"get_activity_events",class:ToolClass::Inspection,
        description:"Measured CPU, memory and disk latency peaks in local history, limited to 30 recent events with explicit omissions. Contributor means observed resource use, not proven cause; normal recovery is not an incident.",params:&[WINDOW,END] },
    ToolSpec { name:"record_activity",class:ToolClass::Inspection,
        description:"Measure the problem now at one-second cadence for 30–120 seconds. Returns markers, at most 120 system history points, gaps, 10 process CPU peaks, up to 10 separate peaks each for process GPU/read/write activity, and at most 128 battery points across 16 opaque batteries. Activity is limited to sampled processes and indicates correlation, not per-app energy. Battery evolution uses only continuous coherent discharge intervals. Can be cancelled by stopping the turn. Reads only; changes no system setting.",
        params:&[Param{name:"duration_seconds",description:"Recording duration, default 30 seconds.",kind:ParamKind::Integer{min:30,max:120},required:false}] },
];
#[derive(Clone)]
pub struct HistoryTools {
    pub sampler: Sampler,
    pub stop: Arc<AtomicBool>,
}

fn empty(status: Status, message: &str) -> ToolOutput {
    ToolOutput {
        status,
        inspection_complete: false,
        cloud: json!({"status":status,"coverage":{"history":message}}),
        evidence: vec![],
        findings: vec![],
        processes: vec![],
        storage_items: vec![],
        limitations: vec![message.into()],
    }
}
fn bounds(args: &Args) -> Option<(u64, u64)> {
    let end = args.integer("end_unix_ms").unwrap_or_else(unix_ms);
    let seconds = args.integer("window_seconds")?;
    if seconds == 0 || seconds > 604_800 || end > unix_ms() {
        return None;
    }
    Some((end.saturating_sub(seconds * 1000), end))
}
fn process_ref(reference: &str) -> bool {
    reference.strip_prefix("proc-").is_some_and(|rest| {
        rest.len() == 8
            && rest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

pub(crate) fn event_observation(observation: &EventObservation, reference: &str) -> Value {
    if observation.entity_ref != "system" && !process_ref(&observation.entity_ref) {
        return Value::Null;
    }
    let mut value = serde_json::to_value(observation).expect("event observation serializes");
    value["observation_id"] = reference.into();
    if observation.window.is_none() || observation.source.is_empty() {
        value["status"] = json!(Status::Partial);
    }
    value["inputs"] = observation
        .inputs
        .iter()
        .enumerate()
        .map(|(index, input)| event_observation(input, &format!("{reference}-input-{}", index + 1)))
        .filter(|input| !input.is_null())
        .collect::<Vec<_>>()
        .into();
    value
}

pub(crate) fn project(
    mut series: HistorySeries,
    sharing: Sharing,
    privacy: &PrivacyContext,
    max_points: Option<usize>,
) -> ToolOutput {
    let has_measurements = series.points.iter().any(|point| point.value.is_some());
    if let Some(limit) = max_points {
        series.limit_points(limit);
    }
    let event_query_limit_reached = series.events.len() >= STORE_EVENT_LIMIT;
    let omitted_events = series.events.len().saturating_sub(MAX_EVENTS);
    if omitted_events > 0 {
        // Events are returned newest first by the store.
        series.events.truncate(MAX_EVENTS);
        series.limitations.push(format!(
            "Cloud projection omitted {omitted_events} older events returned by the store; narrow the requested window for their measurements."
        ));
    }
    if event_query_limit_reached {
        series.limitations.push(
            "The stored event query reached its limit; additional older events may be absent from this response.".into(),
        );
    }
    let omitted_gaps = series.gaps.len().saturating_sub(MAX_GAPS);
    if omitted_gaps > 0 {
        // Preserve the newest gap details, including the response_limit marker when one was
        // appended by limit_points. The full requested window remains explicit in the output.
        series
            .gaps
            .sort_by_key(|gap| (gap.reason == "response_limit", gap.to_unix_ms));
        series.gaps.drain(..omitted_gaps);
        series
            .gaps
            .sort_by_key(|gap| (gap.from_unix_ms, gap.to_unix_ms));
        series.limitations.push(format!(
            "Cloud projection omitted {omitted_gaps} earlier collection-gap details; their intervals remain unknown. Narrow the requested window for details."
        ));
    }
    if omitted_events > 0 || omitted_gaps > 0 || event_query_limit_reached {
        series.status = Status::Partial;
    }
    if !series.events.is_empty() {
        series.limitations.push(
            "Event boundaries are the first and last anomalous samples, not a measured continuous duration. Peak and contributor observations retain their own measured windows; derived values identify their original operands. A contributor is observed resource use, not a proven cause.".into(),
        );
        if series
            .events
            .iter()
            .any(|event| event.peak_observation.is_none())
        {
            series.status = Status::Partial;
            series.limitations.push(
                "Some older event rows have no retained peak measurement metadata: their exact source, timestamp and window are unknown. Legacy peak values remain context, not complete C4 evidence.".into(),
            );
        }
    }
    let mut relevant = BTreeSet::new();
    if max_points.is_some() && process_ref(&series.entity_ref) {
        relevant.insert(series.entity_ref.clone());
    }
    for event in &mut series.events {
        event.entity_ref = event
            .entity_ref
            .take()
            .filter(|reference| reference == "system" || process_ref(reference));
        if let Some(reference) = event.contributor_ref.as_deref().filter(|r| process_ref(r)) {
            relevant.insert(reference.to_owned());
        } else {
            event.contributor_ref = None;
            event.contributor_value = None;
            event.contributor_metric = None;
            event.contributor_unit = None;
            event.contributor_observation = None;
        }
        if let Some(reference) = event.entity_ref.as_deref().filter(|r| process_ref(r)) {
            relevant.insert(reference.to_owned());
        }
    }
    let events: Vec<_> = series
        .events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let mut value = serde_json::to_value(event).expect("event serializes");
            value.as_object_mut().expect("event object").remove("id");
            let peak = event
                .peak_observation
                .as_ref()
                .filter(|observation| {
                    event.entity_ref.as_deref() == Some(observation.entity_ref.as_str())
                })
                .map(|observation| {
                    event_observation(observation, &format!("event-{}-peak", index + 1))
                })
                .unwrap_or(Value::Null);
            let contributor = event
                .contributor_observation
                .as_ref()
                .filter(|observation| {
                    event.contributor_ref.as_deref() == Some(observation.entity_ref.as_str())
                })
                .map(|observation| {
                    event_observation(observation, &format!("event-{}-contributor", index + 1))
                })
                .unwrap_or(Value::Null);
            value["measurement_metadata_available"] = (!peak.is_null()).into();
            value["status"] = peak
                .get("status")
                .cloned()
                .unwrap_or_else(|| json!(Status::Partial));
            value["peak_observation"] = peak;
            value["contributor_observation"] = contributor;
            value
        })
        .collect();
    if events.iter().any(|event| event["status"] != "ok") {
        series.status = Status::Partial;
    }
    if sharing == Sharing::Anonymized {
        series.process_names.clear();
    } else {
        series.process_names = series
            .process_names
            .into_iter()
            .filter(|(key, _)| relevant.contains(key))
            .filter_map(|(key, name)| privacy.shared_name(&name).map(|name| (key, name)))
            .collect();
    }
    let source = series.sources.join("; ");
    let evidence: Vec<_> = series
        .points
        .iter()
        .filter(|_| max_points.is_some())
        .flat_map(|point| {
            // Extrema are measured observations too; missing points are never manufactured as zero.
            [point.value, point.min, point.max]
                .into_iter()
                .flatten()
                .map(|value| {
                    let mut evidence = Evidence::measured(
                        series.metric,
                        &series.entity_ref,
                        value,
                        &source,
                        Window {
                            started_at_unix_ms: point
                                .observed_from_unix_ms
                                .unwrap_or(point.unix_ms),
                            finished_at_unix_ms: point.observed_to_unix_ms.unwrap_or(point.unix_ms),
                            duration_ms: point.observed_duration_ms,
                        },
                    );
                    evidence.origin = point.origin;
                    evidence.status = point.status;
                    evidence
                })
        })
        .collect();
    let status = series.status;
    let limitations = series.limitations.clone();
    let inspection_complete = if max_points.is_some() {
        !evidence.is_empty()
    } else {
        has_measurements || !series.events.is_empty()
    };
    let mut cloud = if max_points.is_some() {
        serde_json::to_value(series).expect("history serializes")
    } else {
        json!({"status":status,"origin":series.origin,
            "from_unix_ms":series.from_unix_ms,"to_unix_ms":series.to_unix_ms,
            "events":series.events,"gaps":series.gaps,"process_names":series.process_names,
            "limitations":limitations})
    };
    cloud["events"] = events.into();
    cloud["coverage"] = json!({"events_omitted_from_response":omitted_events,
        "gap_details_omitted_from_response":omitted_gaps,
        "event_query_limit_reached":event_query_limit_reached});
    privacy.sanitize_value(&mut cloud);
    ToolOutput {
        status,
        inspection_complete,
        cloud,
        evidence,
        findings: vec![],
        processes: vec![],
        storage_items: vec![],
        limitations,
    }
}

pub(crate) fn measured_at(cloud: &Value) -> Option<u64> {
    let points = cloud["points"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|point| {
            ["value", "min", "max"]
                .iter()
                .any(|key| point[*key].as_u64().is_some())
        })
        .filter_map(|point| {
            point["observed_to_unix_ms"]
                .as_u64()
                .or_else(|| point["unix_ms"].as_u64())
        });
    let events = cloud["events"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|event| {
            [
                &event["peak_observation"],
                &event["contributor_observation"],
            ]
        })
        .filter_map(|observation| observation["measured_at_unix_ms"].as_u64());
    let histories = cloud["histories"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(measured_at);
    let observations = [
        &cloud["observations"],
        &cloud["resource_activity"]["observations"],
    ]
    .into_iter()
    .filter_map(Value::as_array)
    .flatten()
    .filter(|observation| observation["value"].as_f64().is_some_and(f64::is_finite))
    .filter_map(|observation| observation["measured_at_unix_ms"].as_u64());
    let battery = cloud["battery"]["batteries"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|battery| battery["points"].as_array().into_iter().flatten())
        .flat_map(|point| point["observations"].as_array().into_iter().flatten())
        .filter(|observation| !observation["value"].is_null())
        .filter_map(|observation| observation["measured_at_unix_ms"].as_u64());
    points
        .chain(events)
        .chain(histories)
        .chain(observations)
        .chain(battery)
        .max()
}

pub(crate) fn project_process_peaks(
    peaks: ProcessPeaks,
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> ToolOutput {
    project_peaks(
        peaks,
        sharing,
        privacy,
        &[Metric::ProcessCpuBasisPoints],
        "recorded-process",
    )
}

pub(crate) fn project_resource_peaks(
    peaks: ProcessPeaks,
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> ToolOutput {
    project_peaks(
        peaks,
        sharing,
        privacy,
        &[
            Metric::ProcessGpuBasisPoints,
            Metric::ProcessIoReadBytesPerSecond,
            Metric::ProcessIoWriteBytesPerSecond,
        ],
        "recorded-resource",
    )
}

fn project_peaks(
    mut peaks: ProcessPeaks,
    sharing: Sharing,
    privacy: &PrivacyContext,
    metrics: &[Metric],
    prefix: &str,
) -> ToolOutput {
    let original_count = peaks.observations.len();
    let mut per_metric = vec![0; metrics.len()];
    peaks.observations.retain(|evidence| {
        let Some(index) = metrics.iter().position(|metric| *metric == evidence.metric) else {
            return false;
        };
        if !process_ref(&evidence.entity_ref) || per_metric[index] == MAX_PROCESS_PEAKS {
            return false;
        }
        per_metric[index] += 1;
        true
    });
    if original_count != peaks.observations.len() {
        peaks.status = Status::Partial;
        peaks.limitations.push(
            "Additional or invalid process peak records were omitted from the bounded response."
                .into(),
        );
    }
    let observations: Vec<_> = peaks
        .observations
        .iter()
        .enumerate()
        .map(|(index, evidence)| {
            json!({"observation_id":format!("{prefix}-{}", index + 1),
                "entity_ref":evidence.entity_ref,"metric":evidence.metric,
                "value":evidence.value,"unit":evidence.unit,"status":evidence.status,
                "source":evidence.source,"origin":evidence.origin,
                "window":evidence.window,
                "measured_at_unix_ms":evidence.window.finished_at_unix_ms,
                "limitations":evidence.limitations})
        })
        .collect();
    let names: serde_json::Map<_, _> = peaks
        .process_names
        .into_iter()
        .filter(|(reference, _)| {
            sharing == Sharing::Complete
                && peaks
                    .observations
                    .iter()
                    .any(|evidence| &evidence.entity_ref == reference)
        })
        .filter_map(|(reference, name)| {
            privacy
                .shared_name(&name)
                .map(|name| (reference, Value::String(name)))
        })
        .collect();
    let mut cloud = json!({"status":peaks.status,"from_unix_ms":peaks.from_unix_ms,
        "to_unix_ms":peaks.to_unix_ms,"observations":observations,"shared_names":names,
        "limitations":peaks.limitations});
    privacy.sanitize_value(&mut cloud);
    ToolOutput {
        status: peaks.status,
        inspection_complete: !peaks.observations.is_empty(),
        cloud,
        evidence: peaks.observations,
        findings: vec![],
        processes: vec![],
        storage_items: vec![],
        limitations: peaks.limitations,
    }
}

pub fn run(
    spec: &ToolSpec,
    args: &Args,
    context: Option<&HistoryTools>,
    sharing: Sharing,
) -> Option<ToolOutput> {
    if !TOOLS.iter().any(|tool| tool.name == spec.name) {
        return None;
    }
    let Some(context) = context else {
        return Some(empty(
            Status::Unsupported,
            "Persistent collection was not enabled by this local client.",
        ));
    };
    let privacy = PrivacyContext::local();
    let mut out = empty(Status::Partial, "No measurements in the requested window.");
    match spec.name {
        "get_metric_history" => {
            let Some((from, to)) = bounds(args) else {
                return Some(empty(Status::Error, "Invalid or future history window."));
            };
            let Some(metric) = args
                .text("metric")
                .and_then(|m| serde_json::from_value::<Metric>(json!(m)).ok())
            else {
                return Some(empty(Status::Error, "Unsupported recorded metric."));
            };
            let entity = args.text("entity_ref").unwrap_or("system");
            if entity != "system" && !process_ref(entity) {
                return Some(empty(
                    Status::Error,
                    "Unknown opaque history entity reference.",
                ));
            }
            let query = HistoryQuery {
                metric,
                entity_ref: entity.into(),
                from_unix_ms: from,
                to_unix_ms: to,
            };
            match context.sampler.history(&query) {
                Ok(series) => {
                    out = project(series, sharing, &privacy, Some(MAX_HISTORY_POINTS));
                }
                Err(_) => {
                    return Some(empty(Status::Error, "The local history could not be read."))
                }
            }
        }
        "get_activity_events" => {
            let Some((from, to)) = bounds(args) else {
                return Some(empty(Status::Error, "Invalid or future history window."));
            };
            // Reading one series supplies both measured events and collection gaps for this window.
            match context.sampler.history(&HistoryQuery {
                metric: Metric::CpuBusyBasisPoints,
                entity_ref: "system".into(),
                from_unix_ms: from,
                to_unix_ms: to,
            }) {
                Ok(series) => {
                    out = project(series, sharing, &privacy, None);
                }
                Err(_) => {
                    return Some(empty(
                        Status::Error,
                        "The local event history could not be read.",
                    ))
                }
            }
        }
        "record_activity" => {
            let seconds = args.integer("duration_seconds").unwrap_or(30);
            let recording=match context.sampler.record(seconds,&context.stop) {
                Ok(recording)=>recording,
                Err(_)=>return Some(empty(Status::Error,"Recording unavailable, cancelled before starting, or another recording is running.")),
            };
            let to = recording
                .ended_at_unix_ms
                .unwrap_or_else(unix_ms)
                .max(recording.started_at_unix_ms + 1);
            let mut histories = Vec::new();
            let mut complete = recording.status == "completed";
            out.limitations.clear();
            for metric in [
                Metric::CpuBusyBasisPoints,
                Metric::PhysicalAvailableBytes,
                Metric::DiskReadLatencyMicroseconds,
                Metric::DiskWriteLatencyMicroseconds,
            ] {
                match context.sampler.history(&HistoryQuery {
                    metric,
                    entity_ref: "system".into(),
                    from_unix_ms: recording.started_at_unix_ms,
                    to_unix_ms: to,
                }) {
                    Ok(series) => {
                        let projected =
                            project(series, sharing, &privacy, Some(MAX_RECORDING_POINTS));
                        complete &= projected.status == Status::Ok;
                        histories.push(projected.cloud);
                        out.evidence.extend(projected.evidence);
                        for limitation in projected.limitations {
                            if !out.limitations.contains(&limitation) {
                                out.limitations.push(limitation);
                            }
                        }
                    }
                    Err(_) => {
                        complete = false;
                        histories.push(json!({"metric":metric,"entity_ref":"system",
                            "status":Status::Error,"from_unix_ms":recording.started_at_unix_ms,
                            "to_unix_ms":to,"points":[],
                            "coverage":{"history":"The local history could not be read."}}));
                        let limitation = "At least one recording metric could not be read.".into();
                        if !out.limitations.contains(&limitation) {
                            out.limitations.push(limitation);
                        }
                    }
                }
            }
            let peaks = match context.sampler.process_peaks(
                recording.started_at_unix_ms,
                to,
                MAX_PROCESS_PEAKS,
            ) {
                Ok(peaks) => project_process_peaks(peaks, sharing, &privacy),
                Err(_) => empty(
                    Status::Error,
                    "Observed process peaks could not be read for this recording.",
                ),
            };
            complete &= peaks.status == Status::Ok;
            out.evidence.extend(peaks.evidence);
            for limitation in &peaks.limitations {
                if !out.limitations.contains(limitation) {
                    out.limitations.push(limitation.clone());
                }
            }
            let resources = match context.sampler.process_resource_peaks(
                recording.started_at_unix_ms,
                to,
                MAX_PROCESS_PEAKS,
            ) {
                Ok(resources) => project_resource_peaks(resources, sharing, &privacy),
                Err(_) => empty(
                    Status::Error,
                    "Recorded process GPU and I/O peaks could not be read.",
                ),
            };
            if resources.status == Status::Error {
                complete = false;
            }
            out.evidence.extend(resources.evidence);
            for limitation in &resources.limitations {
                if !out.limitations.contains(limitation) {
                    out.limitations.push(limitation.clone());
                }
            }
            let battery = match context.sampler.battery_recording(&recording.id) {
                Ok(Some(battery)) => {
                    if battery.status != Status::Ok && battery.status != Status::Unsupported {
                        complete = false;
                    }
                    out.limitations.extend(battery.limitations.iter().cloned());
                    serde_json::to_value(battery).expect("bounded battery recording serializes")
                }
                Ok(None) | Err(_) => {
                    complete = false;
                    out.limitations
                        .push("Battery measurements could not be read for this recording.".into());
                    json!({"status":Status::Error,"batteries":[],"gaps":[],
                        "coverage":{"battery":"The recorded battery measurements are unavailable."}})
                }
            };
            out.status = if complete {
                Status::Ok
            } else {
                Status::Partial
            };
            out.inspection_complete = !out.evidence.is_empty()
                || battery["batteries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|battery| battery["points"].as_array().into_iter().flatten())
                    .flat_map(|point| point["observations"].as_array().into_iter().flatten())
                    .any(|observation| !observation["value"].is_null());
            out.cloud = json!({"status":out.status,"recording":{
                "started_at_unix_ms":recording.started_at_unix_ms,
                "ended_at_unix_ms":recording.ended_at_unix_ms,
                "status":recording.status,
                "samples":recording.samples,"markers":recording.markers},
                "histories":histories,"battery":battery,"source":"local persistent sampler",
                "resource_activity":resources.cloud,
                "observations":peaks.cloud.get("observations").cloned().unwrap_or_else(||json!([])),
                "shared_names":peaks.cloud.get("shared_names").cloned().unwrap_or_else(||json!({})),
                "process_activity":{"status":peaks.status,
                    "from_unix_ms":recording.started_at_unix_ms,"to_unix_ms":to,
                    "limitations":peaks.limitations},
                "limitations":out.limitations});
        }
        _ => return None,
    }
    out.cloud["measured_at_unix_ms"] = measured_at(&out.cloud).into();
    out.cloud["historical"] = true.into();
    out.cloud["queried_at_unix_ms"] = unix_ms().into();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{Origin, Snapshot};
    use pcagent_store::{Store, StoredSample};

    fn fixture(count: u64) -> HistorySeries {
        let mut store = Store::open_in_memory().unwrap();
        for index in 0..count {
            let at = (index + 1) * 1000;
            let value = 1000 + index;
            let sample: StoredSample = serde_json::from_value(json!({
                "id":format!("sample-{index}"),"session_id":"private-session-identity",
                "measured_at_unix_ms":at,"interval_ms":1000,"resolution_ms":1000,
                "origin":"synthetic_fixture",
                "values":[{"metric":"cpu_busy_basis_points","entity_ref":"system",
                    "value":value,"min":value,"max":value,"measured":1,"expected":1,
                    "status":"ok","source":"fixture::GetSystemTimes",
                    "observed_from_unix_ms":at - 750,"observed_to_unix_ms":at,
                    "observed_duration_ms":750}],
                "process_names":{
                    "proc-aabbccdd":r"C:\Users\Alice\Programs\browser.exe",
                    "proc-aabbccde":"editor.exe",
                    "proc-aabbccdf":"unrelated.exe"}
            }))
            .unwrap();
            store.append_sample(&sample).unwrap();
        }
        store
            .metric_history(&HistoryQuery {
                metric: Metric::CpuBusyBasisPoints,
                entity_ref: "system".into(),
                from_unix_ms: 1,
                to_unix_ms: (count + 1) * 1000,
            })
            .unwrap()
    }

    fn named_reference(series: &HistorySeries, name: &str) -> String {
        series
            .process_names
            .iter()
            .find(|(_, value)| value.ends_with(name))
            .map(|(key, _)| key.clone())
            .unwrap()
    }

    fn add_events(series: &mut HistorySeries, count: usize, contributor: &str) {
        series.events = (0..count)
            .map(|index| {
                serde_json::from_value(json!({
                    "id":format!("private-event-{index}"),"kind":"cpu_busy",
                    "entity_ref":"system","started_at_unix_ms":1000,
                    "ended_at_unix_ms":2000,"peak":9300,"metric":"cpu_busy_basis_points",
                    "unit":"basis_points_of_total_cpu","active":false,
                    "contributor_ref":contributor,"contributor_value":4200,
                    "contributor_metric":"process_cpu_basis_points",
                    "contributor_unit":"basis_points_of_total_cpu","origin":"synthetic_fixture"
                }))
                .unwrap()
            })
            .collect();
    }

    #[test]
    fn cloud_history_cap_preserves_extrema_counts_origins_and_actual_windows() {
        let out = project(
            fixture(241),
            Sharing::Complete,
            &PrivacyContext::default(),
            Some(MAX_HISTORY_POINTS),
        );
        let points = out.cloud["points"].as_array().unwrap();
        assert!(points.len() <= 120);
        assert_eq!(out.status, Status::Ok);
        assert_eq!(out.cloud["status"], "ok");
        assert_eq!(points[0]["value"], 1001);
        assert_eq!(points[0]["min"], 1000);
        assert_eq!(points[0]["max"], 1002);
        assert_eq!(points[0]["observed_from_unix_ms"], 250);
        assert_eq!(points[0]["observed_to_unix_ms"], 3000);
        assert_eq!(points[0]["observed_duration_ms"], 2250);
        assert_eq!(
            points
                .iter()
                .map(|p| p["measured_samples"].as_u64().unwrap())
                .sum::<u64>(),
            241
        );
        assert_eq!(points.last().unwrap()["max"], 1240);
        assert!(out.evidence.iter().all(|evidence| {
            evidence.origin == Origin::SyntheticFixture
                && evidence.source == "fixture::GetSystemTimes"
        }));
        assert_eq!(out.evidence[0].window.started_at_unix_ms, 250);
        assert_eq!(out.evidence[0].window.finished_at_unix_ms, 3000);
        assert_eq!(out.evidence[0].window.duration_ms, 2250);
        let wire = out.cloud.to_string();
        assert!(!wire.contains("session_id"));
        assert!(!wire.contains("private-session-identity"));
        assert!(!wire.contains("browser.exe"));
    }

    #[test]
    fn disconnected_cloud_segments_change_tool_status_after_reduction() {
        let mut series = fixture(241);
        // Different origins must never become one averaged observation, even if a producer
        // reports the overall query as complete before the cloud response limit is applied.
        for (index, point) in series.points.iter_mut().enumerate() {
            if index % 2 == 0 {
                point.origin = Origin::Live;
            }
        }
        series.origin = None;
        let out = project(
            series,
            Sharing::Anonymized,
            &PrivacyContext::default(),
            Some(MAX_HISTORY_POINTS),
        );
        assert_eq!(out.status, Status::Partial);
        assert_eq!(out.cloud["status"], "partial");
        assert_eq!(out.cloud["points"].as_array().unwrap().len(), 120);
        assert_eq!(out.cloud["points"][0]["unix_ms"], 122_000);
        assert_eq!(out.cloud["gaps"][0]["reason"], "response_limit");
        assert_eq!(out.cloud["gaps"][0]["from_unix_ms"], 1);
        assert_eq!(out.cloud["gaps"][0]["to_unix_ms"], 122_000);
        assert!(out.cloud["origin"].is_null());
        assert!(out.evidence.iter().any(|e| e.origin == Origin::Live));
        assert!(out
            .evidence
            .iter()
            .any(|e| e.origin == Origin::SyntheticFixture));
        assert!(out.limitations.iter().any(|s| s.contains("121 older")));
    }

    #[test]
    fn events_and_gaps_are_bounded_with_explicit_omissions_and_relevant_names() {
        let privacy =
            PrivacyContext::from_identifiers(r"C:\Users\Alice", "Alice", "PRIVATE-PC", &[]);
        let mut series = fixture(1);
        let browser = named_reference(&series, "browser.exe");
        let editor = named_reference(&series, "editor.exe");
        let unrelated = named_reference(&series, "unrelated.exe");
        series.entity_ref = editor.clone();
        add_events(&mut series, STORE_EVENT_LIMIT, &browser);
        // A program found only in omitted events is not relevant to the transmitted response.
        series.events[MAX_EVENTS].contributor_ref = Some(unrelated.clone());
        series.gaps = serde_json::from_value(json!((0..45)
            .map(
                |index| json!({"from_unix_ms":index * 10,"to_unix_ms":index * 10 + 5,
                "reason":"not_recorded"})
            )
            .collect::<Vec<_>>()))
        .unwrap();
        let complete = project(
            series.clone(),
            Sharing::Complete,
            &privacy,
            Some(MAX_HISTORY_POINTS),
        );
        assert_eq!(complete.status, Status::Partial);
        assert_eq!(complete.cloud["events"].as_array().unwrap().len(), 30);
        assert_eq!(complete.cloud["gaps"].as_array().unwrap().len(), 30);
        assert_eq!(
            complete.cloud["coverage"]["events_omitted_from_response"],
            70
        );
        assert_eq!(
            complete.cloud["coverage"]["gap_details_omitted_from_response"],
            15
        );
        assert_eq!(
            complete.cloud["coverage"]["event_query_limit_reached"],
            true
        );
        assert_eq!(complete.cloud["process_names"][&browser], "browser.exe");
        assert_eq!(complete.cloud["process_names"][&editor], "editor.exe");
        assert!(complete.cloud["process_names"].get(&unrelated).is_none());
        assert!(complete.cloud["events"][0].get("id").is_none());
        assert_eq!(complete.cloud["events"][0]["origin"], "synthetic_fixture");
        let wire = complete.cloud.to_string();
        assert!(!wire.contains("Alice"));
        assert!(!wire.contains("C:"));
        assert!(!wire.contains("private-event"));

        let events = project(series.clone(), Sharing::Complete, &privacy, None);
        assert!(events.cloud.get("points").is_none());
        assert!(events.evidence.is_empty());
        assert!(events.cloud["process_names"].get(&editor).is_none());
        assert_eq!(events.cloud["process_names"][&browser], "browser.exe");
        assert_eq!(events.limitations, complete.limitations);

        let anonymous = project(series, Sharing::Anonymized, &privacy, None);
        assert!(anonymous.cloud["process_names"]
            .as_object()
            .unwrap()
            .is_empty());
        assert!(!anonymous.cloud.to_string().contains("browser.exe"));
    }

    #[test]
    fn recording_point_budget_keeps_missing_measurements_unknown() {
        let mut series = fixture(121);
        series.points[15].value = None;
        series.points[15].min = None;
        series.points[15].max = None;
        series.points[15].measured_samples = 0;
        series.points[15].status = Status::Unsupported;
        series.status = Status::Partial;
        let outputs: Vec<_> = (0..4)
            .map(|_| {
                project(
                    series.clone(),
                    Sharing::Anonymized,
                    &PrivacyContext::default(),
                    Some(MAX_RECORDING_POINTS),
                )
            })
            .collect();
        assert!(
            outputs
                .iter()
                .map(|out| out.cloud["points"].as_array().unwrap().len())
                .sum::<usize>()
                <= 120
        );
        for out in outputs {
            assert_eq!(out.status, Status::Partial);
            assert!(out.cloud["points"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| { p["value"].is_null() && p["status"] == "unsupported" }));
            assert!(out.evidence.iter().all(|e| e.value != Some(0)));
        }
    }

    #[test]
    fn malformed_process_identifiers_are_never_projected_as_event_contributors() {
        let mut series = fixture(1);
        add_events(&mut series, 1, "48151");
        series.events[0].entity_ref = Some("48151".into());
        let out = project(
            series,
            Sharing::Complete,
            &PrivacyContext::default(),
            Some(MAX_HISTORY_POINTS),
        );
        let event = &out.cloud["events"][0];
        assert!(event["entity_ref"].is_null());
        assert!(event["contributor_ref"].is_null());
        assert!(event["contributor_value"].is_null());
        assert!(!out.cloud.to_string().contains("48151"));
        assert!(process_ref("proc-aabbccdd"));
        assert!(!process_ref("proc-48151-creation-time"));
        assert!(!process_ref("48151"));
    }

    #[test]
    fn observation_date_never_uses_query_or_recording_end_as_a_measurement() {
        let mut cloud = json!({"to_unix_ms":99_000,"recording":{"ended_at_unix_ms":98_000},
            "histories":[{"points":[
                {"unix_ms":1000,"observed_to_unix_ms":1900,"value":1200},
                {"unix_ms":2000,"value":null,"min":null,"max":null}
            ]}],"events":[{"ended_at_unix_ms":9000,
                "peak_observation":{"measured_at_unix_ms":1500}}]});
        assert_eq!(measured_at(&cloud), Some(1900));
        cloud["histories"] = json!([]);
        assert_eq!(measured_at(&cloud), Some(1500));
        cloud["events"] = json!([]);
        assert_eq!(measured_at(&cloud), None);
    }

    #[test]
    fn recording_battery_date_uses_known_typed_points_including_signed_rates() {
        let mut cloud = json!({"queried_at_unix_ms":99000,
        "recording":{"ended_at_unix_ms":98000},
        "battery":{"batteries":[{"points":[{"recorded_at_unix_ms":5000,
            "observations":[
                {"metric":"battery_power_rate_mw","value":-15000,
                    "unit":"milliwatts","measured_at_unix_ms":4500},
                {"metric":"battery_remaining_capacity_mwh","value":null,
                    "unit":"milliwatt_hours","measured_at_unix_ms":4900}
            ]}]}]}});
        assert_eq!(measured_at(&cloud), Some(4500));
        cloud["battery"]["batteries"][0]["points"][0]["observations"][0]["value"] = Value::Null;
        assert_eq!(measured_at(&cloud), None);
    }

    #[test]
    fn event_projection_keeps_precise_c4_metadata_and_marks_legacy_unknowns() {
        let mut series = fixture(1);
        let contributor = named_reference(&series, "browser.exe");
        add_events(&mut series, 1, &contributor);
        series.events[0].ended_at_unix_ms = 9999;
        series.events[0].peak_observation = Some(
            serde_json::from_value(json!({
                "entity_ref":"system","metric":"cpu_busy_basis_points","value":9300,
                "unit":"basis_points","source":"GetSystemTimes","status":"ok",
                "origin":"synthetic_fixture","measured_at_unix_ms":1750,
                "window":{"started_at_unix_ms":1500,"finished_at_unix_ms":1750,"duration_ms":200},
                "derivation":null,"inputs":[]
            }))
            .unwrap(),
        );
        series.events[0].contributor_observation = Some(serde_json::from_value(json!({
            "entity_ref":contributor,"metric":"process_cpu_basis_points","value":4200,
            "unit":"basis_points_of_total_cpu","source":"NtQuerySystemInformation","status":"ok",
            "origin":"synthetic_fixture","measured_at_unix_ms":1700,
            "window":{"started_at_unix_ms":1000,"finished_at_unix_ms":1700,"duration_ms":700},
            "derivation":null,"inputs":[]
        })).unwrap());
        let output = project(
            series.clone(),
            Sharing::Complete,
            &PrivacyContext::default(),
            None,
        );
        let event = &output.cloud["events"][0];
        assert_eq!(event["measurement_metadata_available"], true);
        assert_eq!(event["peak_observation"]["observation_id"], "event-1-peak");
        assert_eq!(event["peak_observation"]["value"], 9300);
        assert_eq!(event["peak_observation"]["source"], "GetSystemTimes");
        assert_eq!(event["peak_observation"]["window"]["duration_ms"], 200);
        assert_eq!(event["peak_observation"]["origin"], "synthetic_fixture");
        assert_eq!(
            event["contributor_observation"]["observation_id"],
            "event-1-contributor"
        );
        assert_eq!(event["contributor_observation"]["entity_ref"], contributor);
        assert_eq!(
            event["contributor_observation"]["window"]["duration_ms"],
            700
        );
        assert_eq!(measured_at(&output.cloud), Some(1750));
        assert_eq!(output.status, Status::Ok);

        series.events[0].peak_observation = None;
        series.events[0].contributor_observation = None;
        let legacy = project(series, Sharing::Complete, &PrivacyContext::default(), None);
        let event = &legacy.cloud["events"][0];
        assert_eq!(event["peak"], 9300);
        assert_eq!(event["contributor_value"], 4200);
        assert_eq!(event["measurement_metadata_available"], false);
        assert!(event["peak_observation"].is_null());
        assert!(event["contributor_observation"].is_null());
        assert_eq!(legacy.status, Status::Partial);
        assert_eq!(measured_at(&legacy.cloud), None);
        assert!(legacy
            .limitations
            .iter()
            .any(|text| text.contains("unknown")));
    }

    #[test]
    fn response_gap_marker_survives_gap_detail_limit() {
        let mut series = fixture(1);
        series.gaps = serde_json::from_value(json!((0..45)
            .map(
                |index| json!({"from_unix_ms":index * 10,"to_unix_ms":index * 10 + 5,
                "reason":if index == 0 {"response_limit"} else {"not_recorded"}})
            )
            .collect::<Vec<_>>()))
        .unwrap();
        let out = project(
            series,
            Sharing::Anonymized,
            &PrivacyContext::default(),
            Some(MAX_HISTORY_POINTS),
        );
        assert_eq!(out.cloud["gaps"].as_array().unwrap().len(), MAX_GAPS);
        assert_eq!(out.cloud["gaps"][0]["reason"], "response_limit");
        assert_eq!(out.status, Status::Partial);
    }

    #[test]
    fn recording_process_peaks_retain_c4_evidence_below_anomaly_thresholds() {
        let mut store = Store::open_in_memory().unwrap();
        for (at, peak) in [(1000, 100), (2000, 1200), (3000, 200)] {
            let mut snapshot = Snapshot::empty("private-recording-session");
            snapshot.origin = Origin::SyntheticFixture;
            let window = Window {
                started_at_unix_ms: at - 750,
                finished_at_unix_ms: at,
                duration_ms: 750,
            };
            snapshot.evidence.push(Evidence::measured(
                Metric::CpuBusyBasisPoints,
                "system",
                2000,
                "fixture::GetSystemTimes",
                window.clone(),
            ));
            for index in 0..12 {
                snapshot.evidence.push(Evidence::measured(
                    Metric::ProcessCpuBasisPoints,
                    &format!("proc-{index:08x}"),
                    if index == 0 { peak } else { 50 - index },
                    "fixture::NtQuerySystemInformation",
                    window.clone(),
                ));
            }
            let mut sample = StoredSample::from_snapshot(&snapshot, at, 1000);
            for index in 0..12 {
                sample.process_names.insert(
                    format!("proc-{index:08x}"),
                    format!(r"C:\Users\Alice\Programs\fixture-{index}.exe"),
                );
            }
            store.append_sample(&sample).unwrap();
        }
        assert!(store.activity_events(1, 4000, 100).unwrap().is_empty());
        let privacy = PrivacyContext::from_identifiers(r"C:\Users\Alice", "Alice", "PC", &[]);
        let out = project_process_peaks(
            store.process_peaks(1, 4000, MAX_PROCESS_PEAKS).unwrap(),
            Sharing::Complete,
            &privacy,
        );
        assert_eq!(out.evidence.len(), MAX_PROCESS_PEAKS);
        assert_eq!(out.status, Status::Partial);
        assert_eq!(out.cloud["observations"].as_array().unwrap().len(), 10);
        assert_eq!(out.cloud["shared_names"].as_object().unwrap().len(), 10);
        let peak = &out.cloud["observations"][0];
        assert_eq!(peak["value"], 1200);
        assert_eq!(peak["status"], "ok");
        assert_eq!(peak["metric"], "process_cpu_basis_points");
        assert_eq!(peak["origin"], "synthetic_fixture");
        assert_eq!(peak["source"], "fixture::NtQuerySystemInformation");
        assert_eq!(peak["window"]["started_at_unix_ms"], 1250);
        assert_eq!(peak["window"]["finished_at_unix_ms"], 2000);
        assert_eq!(peak["window"]["duration_ms"], 750);
        assert_eq!(peak["measured_at_unix_ms"], 2000);
        assert_eq!(measured_at(&out.cloud), Some(2000));
        let reference = peak["entity_ref"].as_str().unwrap();
        assert!(process_ref(reference));
        assert_eq!(out.cloud["shared_names"][reference], "fixture-0.exe");
        let wire = out.cloud.to_string();
        for private in ["Alice", "C:", "private-recording-session", "\"id\":"] {
            assert!(!wire.contains(private));
        }
        assert!(!wire.contains("fixture-11.exe"));
        let anonymous = project_process_peaks(
            store.process_peaks(1, 4000, MAX_PROCESS_PEAKS).unwrap(),
            Sharing::Anonymized,
            &privacy,
        );
        assert!(anonymous.cloud["shared_names"]
            .as_object()
            .unwrap()
            .is_empty());
        assert!(!anonymous.cloud.to_string().contains(".exe"));
        assert_eq!(anonymous.cloud["observations"], out.cloud["observations"]);
    }

    #[test]
    fn recorded_resource_peaks_are_bounded_dated_and_sharing_filtered() {
        let mut store = Store::open_in_memory().unwrap();
        for at in [1000, 2000] {
            let mut snapshot = Snapshot::empty("private-resource-session");
            snapshot.origin = Origin::SyntheticFixture;
            let window = Window {
                started_at_unix_ms: at - 750,
                finished_at_unix_ms: at,
                duration_ms: 750,
            };
            for index in 0..12 {
                let reference = format!("proc-{index:08x}");
                for (metric, value) in [
                    (
                        Metric::ProcessCpuBasisPoints,
                        if at == 1000 {
                            8000 - index
                        } else {
                            1000 - index
                        },
                    ),
                    (
                        Metric::ProcessGpuBasisPoints,
                        if at == 2000 {
                            8000 - index
                        } else {
                            1000 - index
                        },
                    ),
                    (Metric::ProcessIoReadBytesPerSecond, index * 5000),
                    (Metric::ProcessIoWriteBytesPerSecond, (12 - index) * 1000),
                ] {
                    snapshot.evidence.push(Evidence::measured(
                        metric,
                        &reference,
                        value,
                        "fixture resource counter",
                        window.clone(),
                    ));
                }
            }
            let mut sample = StoredSample::from_snapshot(&snapshot, at, 1000);
            for index in 0..12 {
                sample.process_names.insert(
                    format!("proc-{index:08x}"),
                    format!(r"C:\Users\Alice\Programs\resource-{index}.exe"),
                );
            }
            store.append_sample(&sample).unwrap();
        }
        let privacy = PrivacyContext::from_identifiers(r"C:\Users\Alice", "Alice", "PC", &[]);
        let complete = project_resource_peaks(
            store.process_resource_peaks(1, 3000, 10).unwrap(),
            Sharing::Complete,
            &privacy,
        );
        assert_eq!(complete.evidence.len(), 30);
        assert_eq!(complete.cloud["observations"].as_array().unwrap().len(), 30);
        let ids: std::collections::BTreeSet<_> = complete.cloud["observations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|observation| observation["observation_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 30);
        for metric in [
            Metric::ProcessGpuBasisPoints,
            Metric::ProcessIoReadBytesPerSecond,
            Metric::ProcessIoWriteBytesPerSecond,
        ] {
            assert_eq!(
                complete
                    .evidence
                    .iter()
                    .filter(|evidence| evidence.metric == metric)
                    .count(),
                10
            );
        }
        assert!(complete
            .evidence
            .iter()
            .all(|evidence| evidence.window.duration_ms == 750));
        let anonymous = project_resource_peaks(
            store.process_resource_peaks(1, 3000, 10).unwrap(),
            Sharing::Anonymized,
            &privacy,
        );
        assert_eq!(
            anonymous.cloud["observations"],
            complete.cloud["observations"]
        );
        assert!(anonymous.cloud["shared_names"]
            .as_object()
            .unwrap()
            .is_empty());
        assert!(!anonymous.cloud.to_string().contains(".exe"));
        for private in ["Alice", "C:", "private-resource-session"] {
            assert!(!complete.cloud.to_string().contains(private));
        }
        let cpu = project_process_peaks(
            store.process_peaks(1, 3000, 10).unwrap(),
            Sharing::Anonymized,
            &privacy,
        );
        assert_eq!(measured_at(&cpu.cloud), Some(1000));
        let recording =
            json!({"observations":cpu.cloud["observations"],"resource_activity":anonymous.cloud});
        assert_eq!(measured_at(&recording), Some(2000));
    }

    #[test]
    fn history_schemas_reject_arbitrary_paths_future_windows_and_unbounded_recording() {
        assert!(TOOLS[0]
            .validate(r#"{"metric":"cpu_busy_basis_points","window_seconds":604801}"#)
            .is_err());
        assert!(TOOLS[0].validate(r#"{"metric":"cpu_busy_basis_points","window_seconds":60,"entity_ref":"C:\\private"}"#).is_err());
        assert!(TOOLS[2].validate(r#"{"duration_seconds":121}"#).is_err());
        let args = TOOLS[0]
            .validate(&format!(
                r#"{{"metric":"cpu_busy_basis_points","window_seconds":60,"end_unix_ms":{}}}"#,
                unix_ms() + 10_000
            ))
            .unwrap();
        assert!(bounds(&args).is_none());
        assert_eq!(
            run(&TOOLS[0], &Args::default(), None, Sharing::Complete)
                .unwrap()
                .status,
            Status::Unsupported
        );
    }
}
