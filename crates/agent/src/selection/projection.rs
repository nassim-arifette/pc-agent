//! Freeze an exact, bounded model projection without collecting fresh evidence.
use super::{
    is_battery_ref, RecordingSource, Resource, SelectionIntent, SelectionSource, SelectionTarget,
    MAX_HISTORY_POINTS, MAX_SELECTED_BYTES,
};
use crate::{
    conversation, initial_message_with_privacy,
    projections::{activity, history},
    PastTurn, PrivacyContext, Sharing, StorageContext, ToolOutput,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{Snapshot, Status};
use pcagent_storage::LargestItemsPage;
use pcagent_store::{BatteryRecording, ProcessPeaks};
use serde_json::{json, Value};
use std::collections::BTreeSet;

/// Immutable minimized evidence and the capture used to seed the normal report/verifier.
#[derive(Clone)]
pub struct FrozenSelection {
    intent: SelectionIntent,
    target: SelectionTarget,
    snapshot: Snapshot,
    pub(super) output: ToolOutput,
    storage_context: StorageContext,
    sharing: Sharing,
}

impl FrozenSelection {
    #[allow(clippy::too_many_arguments)]
    pub fn freeze(
        source: &SelectionSource,
        target: &SelectionTarget,
        intent: SelectionIntent,
        previewed_at_unix_ms: u64,
        sharing: Sharing,
        file_names: bool,
        privacy: &PrivacyContext,
    ) -> Result<Self, &'static str> {
        if !target.validate() || !source.targets().contains(target) {
            return Err("The selected target is absent from this immutable capture.");
        }
        if file_names && sharing != Sharing::Complete {
            return Err("File names require complete sharing and the finite scan name preview.");
        }
        let mut snapshot = Snapshot::empty("selected-local-capture");
        let mut storage_context = StorageContext::default();
        let (mut output, wire_target) = match (source, target) {
            (SelectionSource::Snapshot(capture), SelectionTarget::Resource { resource }) => {
                snapshot = capture.clone();
                snapshot
                    .evidence
                    .retain(|evidence| resource.contains(evidence.metric));
                let mentioned: BTreeSet<_> = snapshot
                    .evidence
                    .iter()
                    .map(|evidence| evidence.entity_ref.as_str())
                    .collect();
                snapshot
                    .processes
                    .retain(|process| mentioned.contains(process.entity_ref.as_str()));
                (
                    ToolOutput::from_snapshot(&snapshot),
                    json!({"kind":"resource","resource":resource}),
                )
            }
            (SelectionSource::Snapshot(capture), SelectionTarget::Process { entity_ref }) => {
                snapshot = capture.clone();
                snapshot
                    .evidence
                    .retain(|evidence| &evidence.entity_ref == entity_ref);
                snapshot
                    .processes
                    .retain(|process| &process.entity_ref == entity_ref);
                (
                    ToolOutput::from_snapshot(&snapshot),
                    json!({"kind":"process","entity_ref":entity_ref}),
                )
            }
            (
                SelectionSource::ProcessActivity(capture),
                SelectionTarget::Process { entity_ref },
            ) => {
                let mut activity = capture.clone();
                activity
                    .evidence
                    .retain(|evidence| &evidence.entity_ref == entity_ref);
                activity
                    .processes
                    .retain(|process| &process.instance.entity_ref == entity_ref);
                activity.applications.clear();
                snapshot.origin = activity.origin;
                snapshot.evidence = activity.evidence.clone();
                snapshot.processes = activity
                    .processes
                    .iter()
                    .map(|process| process.instance.clone())
                    .collect();
                snapshot.process_coverage = activity.coverage.processes.clone();
                snapshot.limitations = activity.limitations.clone();
                (
                    activity::project(activity, sharing, privacy),
                    json!({"kind":"process","entity_ref":entity_ref}),
                )
            }
            (
                SelectionSource::History(capture) | SelectionSource::Events(capture),
                SelectionTarget::Event { event_id },
            ) => {
                let mut series = capture.clone();
                series.points.clear();
                series.events.retain(|event| &event.id == event_id);
                // A retained event can explain missing coverage. Only its typed peak metadata
                // can certify numerical claims; legacy scalar peaks stay context in C1.
                let output = history::project(series, sharing, privacy, None);
                (output, json!({"kind":"event"}))
            }
            (SelectionSource::History(capture), SelectionTarget::Resource { resource }) => (
                history::project(capture.clone(), sharing, privacy, Some(120)),
                json!({"kind":"resource","resource":resource}),
            ),
            (SelectionSource::Chart(chart), SelectionTarget::Event { event_id }) => {
                let mut series = chart
                    .histories
                    .first()
                    .ok_or("The chart has no retained series.")?
                    .clone();
                series.points.clear();
                series.events = chart
                    .events
                    .iter()
                    .filter(|event| &event.id == event_id)
                    .cloned()
                    .collect();
                series.process_names = chart.process_names.clone();
                (
                    history::project(series, sharing, privacy, None),
                    json!({"kind":"event"}),
                )
            }
            (SelectionSource::Chart(chart), SelectionTarget::Resource { resource }) => {
                let series = chart
                    .histories
                    .iter()
                    .find(|series| resource.contains(series.metric))
                    .ok_or("The chart has no retained series for this resource.")?;
                (
                    history::project(series.clone(), sharing, privacy, Some(120)),
                    json!({"kind":"resource","resource":resource}),
                )
            }
            (SelectionSource::History(capture), SelectionTarget::Process { entity_ref }) => (
                history::project(capture.clone(), sharing, privacy, Some(120)),
                json!({"kind":"process","entity_ref":entity_ref}),
            ),
            (SelectionSource::Recording(capture), SelectionTarget::Recording { .. }) => (
                recording_output(capture, sharing, privacy),
                json!({"kind":"recording"}),
            ),
            (SelectionSource::Storage(capture), SelectionTarget::Storage { item_ref, .. }) => {
                let mut page = capture.page.clone();
                page.items.retain(|item| &item.item_ref == item_ref);
                if file_names {
                    storage_context = capture.context.clone();
                }
                let (mut cloud, local) = storage_context.project_page(
                    &page,
                    &Policy::default(),
                    &Preferences::default(),
                );
                // Scope and scan identifiers are local authorization keys, never model references.
                if let Some(object) = cloud.as_object_mut() {
                    object.remove("scope_id");
                    object.remove("scan_id");
                }
                alias_scan_references(&mut cloud, &capture.page);
                let alias = page
                    .items
                    .first()
                    .ok_or("The selected scan item is missing.")?
                    .alias
                    .clone();
                cloud["window"] = json!(capture.window);
                cloud["measured_at_unix_ms"] = json!(capture.window.finished_at_unix_ms);
                cloud["origin"] = json!(capture.origin);
                let status = if page.scan_status == Status::Ok
                    && page.items.iter().all(|item| item.status == Status::Ok)
                {
                    Status::Ok
                } else {
                    Status::Partial
                };
                (
                    ToolOutput {
                        status,
                        inspection_complete: true,
                        cloud,
                        evidence: vec![],
                        findings: vec![],
                        processes: vec![],
                        storage_items: local,
                        limitations: page.limitations,
                    },
                    json!({"kind":"storage","alias":alias}),
                )
            }
            (
                SelectionSource::Battery(capture),
                SelectionTarget::Resource {
                    resource: Resource::Battery,
                },
            ) => {
                // Projected like get_power_and_thermal; only opaque battery references leave.
                let mut read = capture.clone();
                read.observations
                    .retain(|observation| is_battery_ref(&observation.entity_ref));
                read.names.clear();
                (
                    crate::projections::system::project(&read, sharing, privacy),
                    json!({"kind":"resource","resource":"battery"}),
                )
            }
            (SelectionSource::WatchAlert(alert), SelectionTarget::WatchAlert { .. }) => (
                crate::projections::watches::project(
                    std::slice::from_ref(alert),
                    1,
                    0,
                    sharing,
                    privacy,
                ),
                json!({"kind":"watch_alert"}),
            ),
            _ => return Err("The target does not belong to this source type."),
        };
        // Every name crosses the same final C5 projection as regular tool results.
        if output.processes.is_empty() {
            // Historical names were resolved by the store rather than a live collector.
            if let Some(object) = output.cloud.as_object_mut() {
                if let Some(names) = object.remove("process_names") {
                    object.insert("shared_names".into(), names);
                }
            }
        }
        let mut names = std::collections::BTreeMap::new();
        crate::privacy::project_tool_names(
            &mut output.cloud,
            sharing,
            &output.processes,
            privacy,
            &mut names,
        );
        let measured_at = history::measured_at(&output.cloud)
            .or_else(|| output.cloud["measured_at_unix_ms"].as_u64())
            .filter(|at| *at != 0);
        output.cloud["measured_at_unix_ms"] = json!(measured_at);
        output.cloud["previewed_at_unix_ms"] = json!(previewed_at_unix_ms);
        output.cloud["age_ms"] =
            json!(measured_at.and_then(|at| previewed_at_unix_ms.checked_sub(at)));
        output.cloud["freshness"] = json!(if source.is_history() {
            "historical"
        } else {
            "already_measured"
        });
        output.cloud["historical"] = json!(true);
        output.cloud["clock_consistency"] = json!(match measured_at {
            None => "measurement_time_unknown",
            Some(at) if at > previewed_at_unix_ms => "capture_after_preview_wall_clock",
            _ => "dated",
        });
        output.cloud["selection"] =
            json!({"target":wire_target,"intent":intent,"fresh_collection":false});
        privacy.sanitize_value(&mut output.cloud);
        if serde_json::to_vec(&output.cloud).map_or(true, |value| value.len() > MAX_SELECTED_BYTES)
        {
            return Err("The selected evidence exceeds the bounded model projection.");
        }
        Ok(Self {
            intent,
            target: target.clone(),
            snapshot,
            output,
            storage_context,
            sharing,
        })
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn output(&self) -> &ToolOutput {
        &self.output
    }
    pub fn target(&self) -> &SelectionTarget {
        &self.target
    }
    pub fn intent(&self) -> SelectionIntent {
        self.intent
    }
    pub fn sharing(&self) -> Sharing {
        self.sharing
    }
    pub fn storage_context(&self) -> &StorageContext {
        &self.storage_context
    }

    /// Use after constructing ordinary finding aliases and before conversation::prepare.
    /// The frozen evidence is cited as O0 from the first request on.
    pub fn augment_first_message(&self, mut first: Value) -> Value {
        first["historical"] = json!(true);
        first["initial_observation"] = self.output.cloud.clone();
        first["initial_observation"]["observation_id"] = json!("O0");
        // Display values like a tool reply's, so numbers can be copied rather than converted.
        crate::tool_context::add_readable_values(&mut first["initial_observation"]);
        first["citable_observation_ids"] = json!(["O0"]);
        first
    }

    /// The frozen evidence as a completed read. Reading an existing capture and its coverage
    /// completes the inspection, even when the original sensors returned no usable measurement.
    /// Every native status, value and the exact cloud payload are kept.
    pub fn read(&self) -> ToolOutput {
        let mut output = self.output.clone();
        output.inspection_complete = true;
        if !matches!(output.status, Status::Ok | Status::Partial) {
            output.status = Status::Partial;
        }
        output
    }

    pub fn preview_message(
        &self,
        past: &[PastTurn],
        question: &str,
        privacy: &PrivacyContext,
    ) -> Value {
        let first = self.augment_first_message(initial_message_with_privacy(
            question,
            &self.snapshot,
            self.sharing,
            privacy,
        ));
        conversation::prepare(first, past, self.sharing, privacy).0
    }
}

fn clone_peaks(peaks: &ProcessPeaks) -> ProcessPeaks {
    ProcessPeaks {
        status: peaks.status,
        from_unix_ms: peaks.from_unix_ms,
        to_unix_ms: peaks.to_unix_ms,
        observations: peaks.observations.clone(),
        process_names: peaks.process_names.clone(),
        limitations: peaks.limitations.clone(),
    }
}

fn alias_scan_references(value: &mut Value, page: &LargestItemsPage) {
    match value {
        Value::String(reference) => {
            if let Some(index) = reference.strip_prefix(&format!("{}:", page.scan_id)) {
                if !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit()) {
                    *reference = page
                        .items
                        .iter()
                        .find(|item| item.item_ref == *reference)
                        .map_or_else(
                            || format!("storage-item-{index}"),
                            |item| item.alias.clone(),
                        );
                }
            }
        }
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| alias_scan_references(value, page)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| alias_scan_references(value, page)),
        _ => {}
    }
}

pub(super) fn unavailable(note: &str) -> ToolOutput {
    ToolOutput {
        status: Status::Partial,
        inspection_complete: false,
        cloud: json!({"status":"partial","observations":[],"limitations":[note]}),
        evidence: vec![],
        findings: vec![],
        processes: vec![],
        storage_items: vec![],
        limitations: vec![note.into()],
    }
}

pub(super) fn bounded_battery(source: &BatteryRecording) -> BatteryRecording {
    let mut battery = source.clone();
    let before: usize = battery
        .batteries
        .iter()
        .map(|battery| battery.points.len())
        .sum();
    let original_batteries = battery.batteries.len();
    let mut omitted = false;
    // Leave room for the explicit omission metadata added below. Halving avoids repeated
    // serialization per omitted point, including on a full recording with many batteries.
    while serde_json::to_vec(&battery).map_or(true, |bytes| bytes.len() > 44 * 1024) {
        if let Some(index) = battery
            .batteries
            .iter()
            .enumerate()
            .filter(|(_, battery)| battery.points.len() > 2)
            .max_by_key(|(_, battery)| battery.points.len())
            .map(|(index, _)| index)
        {
            // Keep both original endpoints; no interpolation or new evolution is computed.
            let points = &mut battery.batteries[index].points;
            let last = points.len() - 1;
            let mut index = 0;
            points.retain(|_| {
                let keep = index == 0 || index == last || index.is_multiple_of(2);
                index += 1;
                keep
            });
        } else if let Some(history) = battery
            .batteries
            .iter_mut()
            .find(|battery| battery.evolution.len() > 1)
        {
            let omitted = history.evolution.len() / 2;
            history.evolution.drain(..omitted);
        } else if !battery.batteries.is_empty() {
            battery.batteries.pop();
        } else {
            break;
        }
        omitted = true;
    }
    if omitted {
        let returned: usize = battery
            .batteries
            .iter()
            .map(|battery| battery.points.len())
            .sum();
        battery.status = Status::Partial;
        battery.coverage.points_returned = returned;
        battery.coverage.points_omitted_from_response = battery
            .coverage
            .points_omitted_from_response
            .saturating_add(before.saturating_sub(returned));
        battery.coverage.batteries_omitted_from_response = battery
            .coverage
            .batteries_omitted_from_response
            .saturating_add(original_batteries.saturating_sub(battery.batteries.len()));
        battery.limitations.push("The selected response omitted battery points or older evolution segments to stay bounded. Retained points and derived inputs keep their original measurements; omitted intervals must not be interpolated.".into());
        battery.gaps.push(pcagent_store::BatteryGap {
            from_unix_ms: battery.started_at_unix_ms,
            to_unix_ms: battery
                .ended_at_unix_ms
                .unwrap_or(battery.started_at_unix_ms),
            reason: "selection_response_limit",
        });
    }
    battery
}

fn recording_output(
    source: &RecordingSource,
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> ToolOutput {
    let mut output =
        unavailable("Only measurements retained by this existing recording are available.");
    let mut complete = source.recording.status == "completed";
    let mut histories = Vec::new();
    for history in source.histories.iter().take(16) {
        let projected =
            history::project(history.clone(), sharing, privacy, Some(MAX_HISTORY_POINTS));
        complete &= projected.status == Status::Ok;
        output.inspection_complete |= projected.inspection_complete;
        output.evidence.extend(projected.evidence);
        output.limitations.extend(projected.limitations);
        histories.push(projected.cloud);
    }
    if source.histories.len() > 16 {
        complete = false;
        output.limitations.push(
            "Additional recorded metric series were omitted from the bounded response.".into(),
        );
    }
    let cpu = source
        .cpu_peaks
        .as_ref()
        .map(|peaks| history::project_process_peaks(clone_peaks(peaks), sharing, privacy))
        .unwrap_or_else(|| unavailable("Recorded CPU contributors are unavailable."));
    let resources = source
        .resource_peaks
        .as_ref()
        .map(|peaks| history::project_resource_peaks(clone_peaks(peaks), sharing, privacy))
        .unwrap_or_else(|| unavailable("Recorded GPU and I/O contributors are unavailable."));
    for projected in [&cpu, &resources] {
        complete &= projected.status == Status::Ok;
        output.inspection_complete |= projected.inspection_complete;
        output.evidence.extend(projected.evidence.clone());
        output.limitations.extend(projected.limitations.clone());
    }
    let battery = source.battery.as_ref().map(|battery| serde_json::to_value(bounded_battery(battery)).expect("battery serializes")).unwrap_or_else(|| json!({"status":"unsupported","batteries":[],"limitations":["Battery measurements were not retained by this recording."]}));
    if source
        .battery
        .as_ref()
        .is_some_and(|battery| !matches!(battery.status, Status::Ok | Status::Unsupported))
    {
        complete = false;
    }
    if battery["status"] == "partial" {
        complete = false;
    }
    output.inspection_complete |= battery["batteries"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|battery| battery["points"].as_array().into_iter().flatten())
        .flat_map(|point| point["observations"].as_array().into_iter().flatten())
        .any(|observation| !observation["value"].is_null());
    output.status = if complete {
        Status::Ok
    } else {
        Status::Partial
    };
    output.cloud = json!({"status":output.status,"recording":{"started_at_unix_ms":source.recording.started_at_unix_ms,"ended_at_unix_ms":source.recording.ended_at_unix_ms,"status":source.recording.status,"samples":source.recording.samples,"markers":source.recording.markers},"histories":histories,"observations":cpu.cloud["observations"],"shared_names":cpu.cloud["shared_names"],"resource_activity":resources.cloud,"battery":battery,"source":"local persistent sampler","limitations":output.limitations});
    output
}
