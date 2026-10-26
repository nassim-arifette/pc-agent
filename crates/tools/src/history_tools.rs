//! Local history and recording execution. The model cannot open a path or select a store.
use pcagent_agent::catalog::history::{
    MAX_HISTORY_POINTS, MAX_PROCESS_PEAKS, MAX_RECORDING_POINTS,
};
pub use pcagent_agent::catalog::history::{METRICS, TOOLS};
use pcagent_agent::projections::history::{
    empty, measured_at, process_ref, project, project_process_peaks, project_resource_peaks,
};
use pcagent_agent::{Args, PrivacyContext, Sharing, ToolOutput, ToolSpec};
use pcagent_shared::{unix_ms, Metric, Status};
use pcagent_store::{sampler::Sampler, HistoryQuery};
use serde_json::json;
use std::sync::{atomic::AtomicBool, Arc};

#[derive(Clone)]
pub struct HistoryTools {
    pub sampler: Sampler,
    pub stop: Arc<AtomicBool>,
}

fn bounds(args: &Args) -> Option<(u64, u64)> {
    let end = args.integer("end_unix_ms").unwrap_or_else(unix_ms);
    let seconds = args.integer("window_seconds")?;
    if seconds == 0 || seconds > 604_800 || end > unix_ms() {
        return None;
    }
    Some((end.saturating_sub(seconds * 1000), end))
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
