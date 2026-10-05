//! Host-owned, already measured selections. Neither references nor model calls collect data.
use crate::{
    activity_tools, conversation, history_tools, initial_message_with_privacy, Args, PastTurn,
    PrivacyContext, Sharing, StorageContext, ToolClass, ToolOutput, ToolSpec, Toolbox,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{Metric, Origin, Snapshot, Status, Window};
use pcagent_storage::LargestItemsPage;
use pcagent_store::{BatteryRecording, HistorySeries, ProcessPeaks, Recording};
use pcagent_telemetry::ProcessActivity;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub const MAX_SELECTED_BYTES: usize = 128 * 1024;
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_SOURCE_TARGETS: usize = 256;
const MAX_HISTORY_POINTS: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionIntent {
    Explain,
    Investigate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Memory,
    Cpu,
    Disk,
    Gpu,
    ThermalPower,
}

impl Resource {
    fn contains(self, metric: Metric) -> bool {
        use Metric::*;
        match self {
            Self::Memory => matches!(
                metric,
                PhysicalTotalBytes
                    | PhysicalAvailableBytes
                    | SystemCommitBytes
                    | SystemCommitLimitBytes
                    | ProcessWorkingSetBytes
                    | ProcessPrivateCommitBytes
                    | ProcessWorkingSetIncreaseBytes
                    | ProcessWorkingSetDecreaseBytes
                    | ProcessPrivateCommitIncreaseBytes
                    | ProcessPrivateCommitDecreaseBytes
                    | HardFaultPagesPerSecondX100
            ),
            Self::Cpu => matches!(
                metric,
                CpuBusyBasisPoints | CpuBusiestCoreBasisPoints | ProcessCpuBasisPoints
            ),
            Self::Disk => matches!(
                metric,
                DiskReadLatencyMicroseconds
                    | DiskWriteLatencyMicroseconds
                    | DiskIdleBasisPoints
                    | DiskQueueLength
                    | ProcessIoReadBytes
                    | ProcessIoWriteBytes
                    | ProcessIoOtherBytes
                    | ProcessIoReadBytesPerSecond
                    | ProcessIoWriteBytesPerSecond
                    | ProcessIoOtherBytesPerSecond
            ),
            Self::Gpu => matches!(
                metric,
                GpuEngineBasisPoints
                    | GpuBusiestEngineBasisPoints
                    | GpuUtilizationBasisPoints
                    | ProcessGpuBasisPoints
                    | GpuMemoryUsedBytes
                    | GpuMemoryTotalBytes
            ),
            Self::ThermalPower => matches!(
                metric,
                GpuTemperatureCelsius
                    | GpuSmClockMegahertz
                    | GpuPowerMilliwatts
                    | GpuSlowdownReasons
            ),
        }
    }
}

/// Local references are accepted only after exact resolution against a host-owned capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SelectionTarget {
    Resource { resource: Resource },
    Process { entity_ref: String },
    Event { event_id: String },
    Recording { recording_id: String },
    Storage { scope_id: String, item_ref: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectionRef {
    pub source_id: String,
    pub target: SelectionTarget,
}

impl SelectionRef {
    pub fn validate(&self) -> bool {
        opaque(&self.source_id, "selection-source-") && self.target.validate()
    }
}

impl SelectionTarget {
    pub fn validate(&self) -> bool {
        match self {
            Self::Resource { .. } => true,
            Self::Process { entity_ref } => crate::is_process_ref(entity_ref),
            Self::Event { event_id } => opaque(event_id, "event-"),
            Self::Recording { recording_id } => opaque(recording_id, "recording-"),
            Self::Storage { scope_id, item_ref } => {
                opaque(scope_id, "scope-")
                    && item_ref.len() <= 128
                    && item_ref.rsplit_once(':').is_some_and(|(scan, index)| {
                        opaque(scan, "scan-")
                            && !index.is_empty()
                            && index.bytes().all(|byte| byte.is_ascii_digit())
                    })
            }
        }
    }

    pub fn default_question(&self, intent: SelectionIntent) -> String {
        let verb = if intent == SelectionIntent::Explain {
            "Explain"
        } else {
            "Investigate"
        };
        let noun = match self {
            Self::Resource {
                resource: Resource::Memory,
            } => "the selected memory measurements",
            Self::Resource {
                resource: Resource::Cpu,
            } => "the selected CPU measurements",
            Self::Resource {
                resource: Resource::Disk,
            } => "the selected disk measurements",
            Self::Resource {
                resource: Resource::Gpu,
            } => "the selected GPU measurements",
            Self::Resource {
                resource: Resource::ThermalPower,
            } => "the selected temperature and power measurements",
            Self::Process { .. } => "the selected process measurements",
            Self::Event { .. } => "the selected recorded activity event",
            Self::Recording { .. } => "the selected existing recording",
            Self::Storage { .. } => "the selected storage scan item",
        };
        format!("{verb} {noun}, stating what is measured and what remains uncertain.")
    }
}

fn opaque(value: &str, prefix: &str) -> bool {
    value.len() <= 128
        && value.strip_prefix(prefix).is_some_and(|rest| {
            !rest.is_empty()
                && rest
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// All members come from the local host. This type deliberately has no Deserialize implementation.
pub enum SelectionSource {
    Snapshot(Snapshot),
    ProcessActivity(ProcessActivity),
    History(HistorySeries),
    Events(HistorySeries),
    Recording(RecordingSource),
    Storage(StorageSource),
}

pub struct RecordingSource {
    pub recording: Recording,
    pub histories: Vec<HistorySeries>,
    pub cpu_peaks: Option<ProcessPeaks>,
    pub resource_peaks: Option<ProcessPeaks>,
    pub battery: Option<BatteryRecording>,
}

pub struct StorageSource {
    /// Completed scan page: item references are bound to this page's scan_id.
    pub page: LargestItemsPage,
    pub window: Window,
    pub origin: Origin,
    /// A finite, host-reviewed name allowlist for this exact scan; no path is shared directly.
    pub context: StorageContext,
}

impl SelectionSource {
    pub fn targets(&self) -> Vec<SelectionTarget> {
        let mut targets = Vec::new();
        match self {
            Self::Snapshot(snapshot) => {
                for resource in [
                    Resource::Memory,
                    Resource::Cpu,
                    Resource::Disk,
                    Resource::Gpu,
                    Resource::ThermalPower,
                ] {
                    if snapshot
                        .evidence
                        .iter()
                        .any(|evidence| resource.contains(evidence.metric))
                    {
                        targets.push(SelectionTarget::Resource { resource });
                    }
                }
                targets.extend(
                    snapshot
                        .processes
                        .iter()
                        .map(|process| SelectionTarget::Process {
                            entity_ref: process.entity_ref.clone(),
                        }),
                );
            }
            Self::ProcessActivity(activity) => targets.extend(activity.processes.iter().map(
                |process| SelectionTarget::Process {
                    entity_ref: process.instance.entity_ref.clone(),
                },
            )),
            Self::History(series) => {
                for resource in [
                    Resource::Memory,
                    Resource::Cpu,
                    Resource::Disk,
                    Resource::Gpu,
                    Resource::ThermalPower,
                ] {
                    if resource.contains(series.metric) {
                        targets.push(SelectionTarget::Resource { resource });
                    }
                }
                if crate::is_process_ref(&series.entity_ref) {
                    targets.push(SelectionTarget::Process {
                        entity_ref: series.entity_ref.clone(),
                    });
                }
                targets.extend(series.events.iter().map(|event| SelectionTarget::Event {
                    event_id: event.id.clone(),
                }));
            }
            Self::Events(series) => {
                targets.extend(series.events.iter().map(|event| SelectionTarget::Event {
                    event_id: event.id.clone(),
                }))
            }
            Self::Recording(recording) => targets.push(SelectionTarget::Recording {
                recording_id: recording.recording.id.clone(),
            }),
            Self::Storage(storage) => {
                targets.extend(
                    storage
                        .page
                        .items
                        .iter()
                        .map(|item| SelectionTarget::Storage {
                            scope_id: storage.page.scope_id.clone(),
                            item_ref: item.item_ref.clone(),
                        }),
                )
            }
        }
        let mut seen = BTreeSet::new();
        targets.retain(|target| {
            target.validate()
                && seen.insert(serde_json::to_string(target).expect("target serializes"))
        });
        targets.truncate(MAX_SOURCE_TARGETS);
        targets
    }

    /// Upper bound of retained serialized source data, used before accepting a registry entry.
    pub fn retained_bytes(&self) -> usize {
        fn bytes(value: &impl Serialize) -> usize {
            serde_json::to_vec(value).map_or(usize::MAX, |value| value.len())
        }
        match self {
            Self::Snapshot(value) => bytes(value),
            Self::ProcessActivity(value) => bytes(value),
            Self::History(value) | Self::Events(value) => bytes(value),
            Self::Recording(value) => [
                bytes(&value.recording),
                bytes(&value.histories),
                bytes(&value.cpu_peaks),
                bytes(&value.resource_peaks),
                bytes(&value.battery),
            ]
            .into_iter()
            .fold(0usize, usize::saturating_add),
            Self::Storage(value) => {
                bytes(&value.page).saturating_add(value.context.retained_bytes())
            }
        }
    }

    pub fn process_session(&self) -> Option<&str> {
        match self {
            Self::Snapshot(snapshot) => Some(&snapshot.session_id),
            Self::ProcessActivity(activity) => Some(&activity.session_id),
            _ => None,
        }
    }

    pub fn recording_id(&self) -> Option<&str> {
        match self {
            Self::Recording(source) => Some(&source.recording.id),
            _ => None,
        }
    }

    pub fn storage_identity(&self) -> Option<(&str, &str)> {
        match self {
            Self::Storage(source) => Some((&source.page.scope_id, &source.page.scan_id)),
            _ => None,
        }
    }

    pub fn is_history(&self) -> bool {
        matches!(
            self,
            Self::History(_) | Self::Events(_) | Self::Recording(_)
        )
    }

    pub fn measured_at(&self, target: &SelectionTarget) -> Option<u64> {
        let at = match (self, target) {
            (Self::Snapshot(snapshot), SelectionTarget::Resource { resource }) => snapshot
                .evidence
                .iter()
                .filter(|evidence| resource.contains(evidence.metric))
                .map(|evidence| evidence.window.finished_at_unix_ms)
                .max(),
            (Self::Snapshot(snapshot), SelectionTarget::Process { entity_ref }) => snapshot
                .evidence
                .iter()
                .filter(|evidence| &evidence.entity_ref == entity_ref)
                .map(|evidence| evidence.window.finished_at_unix_ms)
                .max(),
            (Self::ProcessActivity(activity), SelectionTarget::Process { entity_ref }) => activity
                .evidence
                .iter()
                .filter(|evidence| &evidence.entity_ref == entity_ref)
                .map(|evidence| evidence.window.finished_at_unix_ms)
                .max(),
            (Self::History(series) | Self::Events(series), SelectionTarget::Event { event_id }) => {
                series
                    .events
                    .iter()
                    .filter(|event| &event.id == event_id)
                    .flat_map(|event| [&event.peak_observation, &event.contributor_observation])
                    .filter_map(|observation| {
                        observation
                            .as_ref()
                            .and_then(|observation| observation.measured_at_unix_ms)
                    })
                    .max()
            }
            (
                Self::History(series),
                SelectionTarget::Resource { .. } | SelectionTarget::Process { .. },
            ) => series
                .points
                .iter()
                .filter(|point| point.value.is_some() || point.min.is_some() || point.max.is_some())
                .map(|point| point.observed_to_unix_ms.unwrap_or(point.unix_ms))
                .max(),
            (Self::Recording(source), SelectionTarget::Recording { .. }) => source
                .histories
                .iter()
                .flat_map(|history| &history.points)
                .filter(|point| point.value.is_some() || point.min.is_some() || point.max.is_some())
                .filter_map(|point| point.observed_to_unix_ms.or(Some(point.unix_ms)))
                .chain(
                    source
                        .cpu_peaks
                        .iter()
                        .chain(&source.resource_peaks)
                        .flat_map(|peaks| &peaks.observations)
                        .map(|evidence| evidence.window.finished_at_unix_ms),
                )
                .chain(
                    source
                        .battery
                        .iter()
                        .flat_map(|battery| &battery.batteries)
                        .flat_map(|battery| &battery.points)
                        .flat_map(|point| &point.observations)
                        .filter(|observation| !observation.value.is_null())
                        .map(|observation| observation.measured_at_unix_ms),
                )
                .max(),
            (Self::Storage(source), SelectionTarget::Storage { .. }) => {
                Some(source.window.finished_at_unix_ms)
            }
            _ => None,
        };
        at.filter(|at| *at != 0)
    }
}

/// Immutable minimized evidence and the capture used to seed the normal report/verifier.
#[derive(Clone)]
pub struct FrozenSelection {
    intent: SelectionIntent,
    target: SelectionTarget,
    snapshot: Snapshot,
    output: ToolOutput,
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
                    activity_tools::project(activity, sharing, privacy),
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
                let output = history_tools::project(series, sharing, privacy, None);
                (output, json!({"kind":"event"}))
            }
            (SelectionSource::History(capture), SelectionTarget::Resource { resource }) => (
                history_tools::project(capture.clone(), sharing, privacy, Some(120)),
                json!({"kind":"resource","resource":resource}),
            ),
            (SelectionSource::History(capture), SelectionTarget::Process { entity_ref }) => (
                history_tools::project(capture.clone(), sharing, privacy, Some(120)),
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
        let measured_at = history_tools::measured_at(&output.cloud)
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
    pub fn augment_first_message(&self, mut first: Value) -> Value {
        first["historical"] = json!(true);
        first["initial_observation"] = self.output.cloud.clone();
        first["initial_observation"]["observation_id"] = json!("O0");
        first
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

fn unavailable(note: &str) -> ToolOutput {
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

fn bounded_battery(source: &BatteryRecording) -> BatteryRecording {
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
            history_tools::project(history.clone(), sharing, privacy, Some(MAX_HISTORY_POINTS));
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
        .map(|peaks| history_tools::project_process_peaks(clone_peaks(peaks), sharing, privacy))
        .unwrap_or_else(|| unavailable("Recorded CPU contributors are unavailable."));
    let resources = source
        .resource_peaks
        .as_ref()
        .map(|peaks| history_tools::project_resource_peaks(clone_peaks(peaks), sharing, privacy))
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

pub const GET_SELECTED_EVIDENCE: ToolSpec = ToolSpec {
    name: "get_selected_evidence",
    class: ToolClass::Inspection,
    description: "Read the exact selected measurements frozen for this turn, with their original dates, windows, coverage and limitations. Starts no collector, scan, recording or action. Historical evidence describes its recorded period, not the current PC.",
    params: &[],
};
pub use GET_SELECTED_EVIDENCE as SELECTED_EVIDENCE;

/// Explain cannot reach any delegate method. Investigate uses the existing read-only tool boundary.
pub struct SelectedTools<'a, T: Toolbox> {
    delegate: &'a mut T,
    selection: &'a FrozenSelection,
    specs: Vec<ToolSpec>,
}

impl<'a, T: Toolbox> SelectedTools<'a, T> {
    pub fn new(delegate: &'a mut T, selection: &'a FrozenSelection) -> Self {
        let mut specs = vec![GET_SELECTED_EVIDENCE];
        if selection.intent == SelectionIntent::Investigate {
            specs.extend(
                delegate
                    .specs()
                    .iter()
                    .filter(|spec| spec.name != GET_SELECTED_EVIDENCE.name),
            );
        }
        Self {
            delegate,
            selection,
            specs,
        }
    }
}

impl<T: Toolbox> Toolbox for SelectedTools<'_, T> {
    fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }
    fn sharing(&self) -> Sharing {
        self.selection.sharing
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        if spec.name == GET_SELECTED_EVIDENCE.name {
            let mut output = self.selection.output.clone();
            // Reading an existing capture and its coverage completes this inspection, even
            // when the original sensors returned no usable measurement. Keep every native
            // status/value and the exact cloud payload; only the read operation is completed.
            output.inspection_complete = true;
            if !matches!(output.status, Status::Ok | Status::Partial) {
                output.status = Status::Partial;
            }
            return output;
        }
        if self.selection.intent == SelectionIntent::Investigate
            && self
                .delegate
                .specs()
                .iter()
                .any(|allowed| allowed.name == spec.name)
        {
            return self.delegate.run(spec, args);
        }
        let mut output = unavailable("This tool is unavailable for the frozen Explain selection.");
        output.status = Status::PermissionDenied;
        output.cloud["status"] = json!(Status::PermissionDenied);
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{answer, CitationKind, SnapshotTools};
    use pcagent_shared::{Evidence, ProcessInstance};
    use pcagent_storage::{ItemKind, ItemView, LastUse};
    use pcagent_store::{HistoryQuery, Store, StoredSample};

    fn capture() -> Snapshot {
        let mut snapshot = Snapshot::empty("private-selection-session");
        snapshot.id = "private-selection-snapshot".into();
        snapshot.origin = Origin::SyntheticFixture;
        snapshot.processes.push(ProcessInstance {
            entity_ref: "proc-a1b2c3d4".into(),
            pid: 777777,
            creation_time_filetime: 987654321,
            boot_id: Some("private-boot".into()),
            executable_name: Some(r"C:\Users\Alice\Programs\browser.exe".into()),
        });
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 2000,
            duration_ms: 1000,
        };
        snapshot.evidence.push(Evidence::measured(
            Metric::ProcessWorkingSetBytes,
            "proc-a1b2c3d4",
            200_000_000,
            "fixture measured process",
            window.clone(),
        ));
        snapshot.evidence.push(Evidence::measured(
            Metric::CpuBusyBasisPoints,
            "system",
            9300,
            "fixture GetSystemTimes",
            window,
        ));
        for evidence in &mut snapshot.evidence {
            evidence.origin = Origin::SyntheticFixture;
        }
        snapshot
    }

    fn privacy() -> PrivacyContext {
        PrivacyContext::from_identifiers(
            r"C:\Users\Alice",
            "Alice",
            "private-PC",
            &["fixture-secret-token"],
        )
    }

    fn frozen(source: &SelectionSource, target: &SelectionTarget) -> FrozenSelection {
        FrozenSelection::freeze(
            source,
            target,
            SelectionIntent::Explain,
            6000,
            Sharing::Anonymized,
            false,
            &privacy(),
        )
        .unwrap()
    }

    fn checked(text: &str, received: Value) -> bool {
        answer::review(&json!({"answer":text,"confidence":"low","category":"insufficient_evidence","action_refs":[],"follow_ups":[]}).to_string(), &answer::Context { received: &[received], references: &[("O0".into(), CitationKind::Observation, "selected-local".into())], plans: &[] }).is_ok()
    }

    #[test]
    fn frozen_measurements_keep_the_original_window_and_minimize_identities() {
        let source = SelectionSource::Snapshot(capture());
        let target = SelectionTarget::Process {
            entity_ref: "proc-a1b2c3d4".into(),
        };
        let selection = frozen(&source, &target);
        let message = selection.preview_message(
            &[],
            "Alice fixture-secret-token browser.exe uses memory",
            &privacy(),
        );
        let wire = message.to_string();
        for private in [
            "Alice",
            "fixture-secret-token",
            "browser.exe",
            "777777",
            "987654321",
            "private-boot",
            "private-selection-session",
            "private-selection-snapshot",
        ] {
            assert!(!wire.contains(private), "leaked {private}");
        }
        assert_eq!(message["initial_observation"]["measured_at_unix_ms"], 2000);
        assert_eq!(message["initial_observation"]["age_ms"], 4000);
        assert_eq!(selection.output.evidence[0].window.started_at_unix_ms, 1000);
        assert_eq!(selection.output.evidence[0].window.duration_ms, 1000);
        assert_eq!(selection.output.evidence.len(), 1);
        assert!(checked(
            "During the selected capture, proc-a1b2c3d4 used 200 MB [O0].",
            message.clone()
        ));
        assert!(!checked("proc-a1b2c3d4 uses 900 MB [O0].", message));
        let named = FrozenSelection::freeze(
            &source,
            &target,
            SelectionIntent::Explain,
            6000,
            Sharing::Complete,
            false,
            &privacy(),
        )
        .unwrap();
        assert_eq!(
            named.output.cloud["process_names"]["proc-a1b2c3d4"],
            "browser.exe"
        );
    }

    #[test]
    fn resource_selection_excludes_unrelated_measurements_and_explain_never_collects() {
        let source = SelectionSource::Snapshot(capture());
        let selection = frozen(
            &source,
            &SelectionTarget::Resource {
                resource: Resource::Cpu,
            },
        );
        let mut delegate = SnapshotTools::new(|_, _| panic!("Explain must never collect"));
        let mut selected = SelectedTools::new(&mut delegate, &selection);
        assert_eq!(selected.specs().len(), 1);
        assert_eq!(selected.specs()[0].name, GET_SELECTED_EVIDENCE.name);
        assert!(GET_SELECTED_EVIDENCE.validate(r#"{"fresh":true}"#).is_err());
        let result = selected.run(&GET_SELECTED_EVIDENCE, &Args::default());
        assert_eq!(result.evidence.len(), 1);
        assert_eq!(result.evidence[0].metric, Metric::CpuBusyBasisPoints);
        let forbidden = ToolSpec {
            name: "get_system_snapshot",
            class: ToolClass::Inspection,
            description: "",
            params: &[],
        };
        assert_eq!(
            selected.run(&forbidden, &Args::default()).status,
            Status::PermissionDenied
        );
        let message = selection.preview_message(&[], "Explain the CPU capture.", &privacy());
        assert!(checked(
            "During the selected capture, the CPU used 93% [O0].",
            message.clone()
        ));
        assert!(!checked("The process used 200 MB [O0].", message));
    }

    #[test]
    fn investigate_delegates_fresh_tools_after_the_local_selection_boundary() {
        let source = SelectionSource::Snapshot(capture());
        let selection = FrozenSelection::freeze(
            &source,
            &SelectionTarget::Resource {
                resource: Resource::Cpu,
            },
            SelectionIntent::Investigate,
            6000,
            Sharing::Anonymized,
            false,
            &privacy(),
        )
        .unwrap();
        let mut calls = 0;
        {
            let mut delegate = SnapshotTools::new(|_, _| {
                calls += 1;
                capture()
            });
            let mut selected = SelectedTools::new(&mut delegate, &selection);
            let spec = *selected
                .specs()
                .iter()
                .find(|spec| spec.name == "get_system_snapshot")
                .unwrap();
            selected.run(&spec, &Args::default());
        }
        assert_eq!(calls, 1);
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

    fn storage_source() -> SelectionSource {
        let item = ItemView {
            item_ref: "scan-private:17".into(),
            alias: "file-17".into(),
            alias_path: "<scope>/file-17".into(),
            parent_ref: Some("scan-private:0".into()),
            kind: ItemKind::Files,
            path: Some(r"C:\Users\Alice\secret.pdf".into()),
            redacted_path: "%USERPROFILE%/secret.pdf".into(),
            logical_bytes: 2048,
            allocated_bytes: Some(4096),
            allocation_unknown_files: 0,
            estimated_reclaim_bytes: None,
            files: Some(1),
            link_count: Some(1),
            sparse: false,
            compressed: false,
            modified_unix_ms: Some(500),
            last_used: LastUse::Unknown,
            status: Status::Ok,
        };
        SelectionSource::Storage(StorageSource {
            page: LargestItemsPage {
                schema_version: 1,
                scope_id: "scope-private".into(),
                scan_id: "scan-private".into(),
                kind: ItemKind::Files,
                sort_basis: "allocated_bytes".into(),
                offset: 0,
                limit: 1,
                total: 1,
                next_offset: None,
                items: vec![item],
                scan_status: Status::Ok,
                limitations: vec![],
            },
            window: Window {
                started_at_unix_ms: 1000,
                finished_at_unix_ms: 2000,
                duration_ms: 1000,
            },
            origin: Origin::SyntheticFixture,
            context: StorageContext::default(),
        })
    }

    #[test]
    fn battery_response_omissions_preserve_original_endpoints_and_report_gaps() {
        use pcagent_inspect::{CoverageStatus, Observation};
        use pcagent_store::{BatteryHistory, BatteryPoint, BatteryRecordingCoverage};
        let points: Vec<_> = (0..128)
            .map(|index| {
                let at = 2000 + index * 1000;
                let observations = [
                    (
                        "battery_remaining_capacity_mwh",
                        11000 - index * 2,
                        "milliwatt_hours",
                    ),
                    ("battery_full_charge_capacity_mwh", 15000, "milliwatt_hours"),
                ]
                .into_iter()
                .map(|(metric, value, unit)| Observation {
                    observation_id: format!("battery-1-measurement-{index}-{metric}"),
                    measured_at_unix_ms: at,
                    window: Window {
                        started_at_unix_ms: at - 10,
                        finished_at_unix_ms: at,
                        duration_ms: 10,
                    },
                    entity_ref: "battery-1".into(),
                    metric: metric.into(),
                    value: json!(value),
                    unit: unit.into(),
                    status: CoverageStatus::Ok,
                    source: "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS".into(),
                    origin: Origin::SyntheticFixture,
                })
                .collect();
                BatteryPoint {
                    recorded_at_unix_ms: at,
                    recording_elapsed_ms: Some(at - 1000),
                    clock_consistent: true,
                    status: Status::Ok,
                    observations,
                }
            })
            .collect();
        let battery = BatteryRecording {
            status: Status::Ok,
            started_at_unix_ms: 1000,
            ended_at_unix_ms: Some(129000),
            interval_ms: 1000,
            coverage: BatteryRecordingCoverage {
                expected_ticks: 128,
                recorded_ticks: 128,
                ticks_without_battery: 0,
                unavailable_observations: 0,
                rejected_observations: 0,
                points_returned: 128,
                points_omitted_from_response: 0,
                batteries_omitted_from_response: 0,
                battery_tick_points_omitted_during_collection: 0,
            },
            gaps: vec![],
            batteries: vec![BatteryHistory {
                battery_ref: "battery-1".into(),
                status: Status::Ok,
                points,
                gaps: vec![],
                evolution: vec![],
            }],
            limitations: vec![],
        };
        assert!(serde_json::to_vec(&battery).unwrap().len() > 48 * 1024);
        let bounded = bounded_battery(&battery);
        assert!(serde_json::to_vec(&bounded).unwrap().len() <= 48 * 1024);
        assert_eq!(bounded.status, Status::Partial);
        let retained = &bounded.batteries[0].points;
        assert_eq!(
            serde_json::to_value(retained.first()).unwrap(),
            serde_json::to_value(battery.batteries[0].points.first()).unwrap()
        );
        assert_eq!(
            serde_json::to_value(retained.last()).unwrap(),
            serde_json::to_value(battery.batteries[0].points.last()).unwrap()
        );
        assert_eq!(
            bounded.coverage.points_returned + bounded.coverage.points_omitted_from_response,
            128
        );
        assert_eq!(bounded.coverage.recorded_ticks, 128);
        assert_eq!(bounded.gaps[0].reason, "selection_response_limit");
        assert_eq!(bounded.gaps[0].from_unix_ms, 1000);
        assert_eq!(
            battery.batteries[0].points.len(),
            128,
            "the captured source remains immutable"
        );
    }

    #[test]
    fn storage_projection_keeps_allocations_and_aliases_but_no_scan_authorization_keys_or_paths() {
        let source = storage_source();
        let selection = frozen(
            &source,
            &SelectionTarget::Storage {
                scope_id: "scope-private".into(),
                item_ref: "scan-private:17".into(),
            },
        );
        let message = selection.preview_message(&[], "Explain this item.", &privacy());
        let item = &message["initial_observation"]["items"][0];
        assert_eq!(item["logical_bytes"], 2048);
        assert_eq!(item["allocated_bytes"], 4096);
        assert_eq!(item["item_ref"], "file-17");
        assert_eq!(item["last_used"], "unknown");
        assert!(item["estimated_reclaim_bytes"].is_null());
        for private in ["scan-private", "scope-private", "secret.pdf", "Alice", "C:"] {
            assert!(!message.to_string().contains(private));
        }
        assert!(FrozenSelection::freeze(
            &source,
            &SelectionTarget::Storage {
                scope_id: "scope-private".into(),
                item_ref: "scan-new:17".into()
            },
            SelectionIntent::Explain,
            6000,
            Sharing::Anonymized,
            false,
            &privacy()
        )
        .is_err());
    }

    #[test]
    fn local_references_and_tool_arguments_are_closed_and_wall_clock_rollback_does_not_invent_age()
    {
        for bad in [
            json!({"source_id":"selection-source-valid","target":{"kind":"resource","resource":"cpu","path":"C:/private"}}),
            json!({"source_id":"selection-source-valid","target":{"kind":"process","entity_ref":"proc-a1b2c3d4"},"payload":{}}),
        ] {
            assert!(serde_json::from_value::<SelectionRef>(bad).is_err());
        }
        let source = SelectionSource::Snapshot(capture());
        let selection = FrozenSelection::freeze(
            &source,
            &SelectionTarget::Resource {
                resource: Resource::Cpu,
            },
            SelectionIntent::Explain,
            100,
            Sharing::Anonymized,
            false,
            &privacy(),
        )
        .unwrap();
        assert!(selection.output.cloud["age_ms"].is_null());
        assert_eq!(
            selection.output.cloud["clock_consistency"],
            "capture_after_preview_wall_clock"
        );
        assert_eq!(selection.output.cloud["measured_at_unix_ms"], 2000);
    }

    #[test]
    fn the_public_selected_engine_sends_the_exact_preview_and_verifies_the_selected_tool_result() {
        use crate::{
            converse_with_selection, AgentError, FunctionCall, Model, Reply, ToolCall, ToolChoice,
            SUBMIT_ANSWER,
        };
        use std::{collections::VecDeque, time::Duration};
        struct Script {
            replies: VecDeque<Reply>,
            requests: Vec<Vec<Value>>,
            definitions: Vec<Value>,
        }
        impl Model for Script {
            fn complete(
                &mut self,
                messages: &[Value],
                tools: &Value,
                _: ToolChoice,
                _: Duration,
            ) -> Result<Reply, AgentError> {
                self.requests.push(messages.to_vec());
                self.definitions.push(tools.clone());
                Ok(self.replies.pop_front().expect("script exhausted"))
            }
        }
        let reply = |id: &str, name: &str, arguments: Value| Reply {
            calls: vec![ToolCall {
                id: id.into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: arguments.to_string(),
                },
            }],
            total_tokens: Some(10),
            prompt_tokens: Some(8),
            completion_tokens: Some(2),
            ..Reply::default()
        };
        let source = SelectionSource::Snapshot(capture());
        let selection = frozen(
            &source,
            &SelectionTarget::Resource {
                resource: Resource::Cpu,
            },
        );
        let past = vec![PastTurn {
            question: "What did we observe earlier?".into(),
            answer: "The earlier capture was incomplete.".into(),
            ..PastTurn::default()
        }];
        let expected =
            selection.preview_message(&past, "Explain the selected CPU capture.", &privacy());
        let mut model = Script {
            replies: VecDeque::from([
                reply("selected-read", GET_SELECTED_EVIDENCE.name, json!({})),
                reply(
                    "answer",
                    SUBMIT_ANSWER,
                    json!({"answer":"The selected capture measured CPU usage of 93% [O1]. This describes the saved capture; the present state remains unknown.","confidence":"low","category":"cpu","action_refs":[],"follow_ups":[]}),
                ),
            ]),
            requests: vec![],
            definitions: vec![],
        };
        let mut delegate =
            SnapshotTools::new(|_, _| panic!("Explain cannot reach the native collector"));
        let mut tools = SelectedTools::new(&mut delegate, &selection);
        let report = converse_with_selection(
            &mut model,
            &past,
            "Explain the selected CPU capture.",
            &selection,
            &mut tools,
            &privacy(),
            |_| {},
            |_, _| false,
        );
        let actual: Value =
            serde_json::from_str(model.requests[0][1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(actual, expected);
        // The re-read lists O0 among the citable IDs, as the checker accepts it, and repeats no
        // finding the turn already holds.
        let tool_reply: Value = model.requests[1]
            .iter()
            .rev()
            .find(|message| message["role"] == "tool")
            .map(|message| serde_json::from_str(message["content"].as_str().unwrap()).unwrap())
            .unwrap();
        assert_eq!(tool_reply["citable_observation_ids"], json!(["O0", "O1"]));
        let ids = tool_reply["current_finding_ids"].as_array().unwrap();
        let unique: std::collections::BTreeSet<_> = ids.iter().filter_map(Value::as_str).collect();
        assert_eq!(unique.len(), ids.len(), "{ids:?}");
        assert_eq!(model.definitions[0].as_array().unwrap().len(), 2);
        assert_eq!(report.stats.tool_calls, 1);
        assert_eq!(report.initial_observation.measured_at_unix_ms, 2000);
        assert_eq!(report.observations[0].measured_at_unix_ms, 2000);
        assert!(report
            .answer
            .as_ref()
            .is_some_and(|answer| answer.unverified.is_empty()));
        assert_eq!(
            report.observations[0].observation["measured_at_unix_ms"],
            2000
        );
    }

    #[test]
    fn explain_reports_missing_coverage_and_repairs_an_invented_number_without_collecting() {
        use crate::{
            converse_with_selection, AgentError, FunctionCall, Model, Reply, ToolCall, ToolChoice,
            SUBMIT_ANSWER,
        };
        use std::{collections::VecDeque, time::Duration};
        struct Script(VecDeque<Reply>);
        impl Model for Script {
            fn complete(
                &mut self,
                _: &[Value],
                _: &Value,
                _: ToolChoice,
                _: Duration,
            ) -> Result<Reply, AgentError> {
                Ok(self.0.pop_front().expect("script exhausted"))
            }
        }
        let reply = |id: &str, name: &str, arguments: Value| Reply {
            calls: vec![ToolCall {
                id: id.into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: arguments.to_string(),
                },
            }],
            ..Reply::default()
        };
        let answer = |text: &str| json!({"answer":text,"confidence":"low","category":"insufficient_evidence","action_refs":[],"follow_ups":[]});
        let mut snapshot = capture();
        for evidence in &mut snapshot.evidence {
            evidence.value = None;
            evidence.status = Status::Unsupported;
        }
        let source = SelectionSource::Snapshot(snapshot);
        let selection = frozen(
            &source,
            &SelectionTarget::Resource {
                resource: Resource::Cpu,
            },
        );
        assert!(!selection.output().inspection_complete);
        let first = selection.preview_message(&[], "Explain unavailable CPU coverage.", &privacy());
        assert!(!checked(
            "During the selected capture, CPU usage was 93% [O0].",
            first
        ));
        let mut delegate =
            SnapshotTools::new(|_, _| panic!("Explain cannot start a collector for missing data"));
        // Unsupported capture coverage remains exact even when the local read succeeds.
        {
            let mut unsupported = selection.clone();
            unsupported.output.status = Status::Unsupported;
            unsupported.output.cloud["status"] = json!(Status::Unsupported);
            let mut tools = SelectedTools::new(&mut delegate, &unsupported);
            let read = tools.run(&GET_SELECTED_EVIDENCE, &Args::default());
            assert!(read.inspection_complete);
            assert_eq!(read.status, Status::Partial);
            assert_eq!(read.cloud, unsupported.output().cloud);
            assert!(
                read.evidence
                    .iter()
                    .all(|evidence| evidence.value.is_none()
                        && evidence.status == Status::Unsupported)
            );
        }
        let conclusion = "The selected capture did not provide usable CPU measurements [O1]. The evidence is insufficient to diagnose CPU load.";
        let mut model = Script(VecDeque::from([
            reply("selected-read", GET_SELECTED_EVIDENCE.name, json!({})),
            reply(
                "invented-answer",
                SUBMIT_ANSWER,
                answer("During the selected capture, CPU usage was 93% [O1]."),
            ),
            reply("coverage-answer", SUBMIT_ANSWER, answer(conclusion)),
        ]));
        let mut tools = SelectedTools::new(&mut delegate, &selection);
        let report = converse_with_selection(
            &mut model,
            &[],
            "Explain unavailable CPU coverage.",
            &selection,
            &mut tools,
            &privacy(),
            |_| {},
            |_, _| false,
        );
        assert_eq!(report.stats.inference_calls, 3);
        assert_eq!(report.stats.inspection_calls, 1);
        assert_eq!(report.stats.tool_calls, 1);
        let answer = report
            .answer
            .expect("missing coverage can receive an honest model explanation");
        assert_eq!(answer.text, conclusion);
        assert_eq!(answer.category, crate::Category::InsufficientEvidence);
        assert_eq!(answer.repairs, 1);
        assert!(answer.unverified.is_empty());
        assert!(model.0.is_empty());
    }
}
