//! Host-owned captures, their exact selectable targets and original measurement dates.
use super::{is_battery_ref, Resource, SelectionTarget, MAX_SOURCE_TARGETS};
use crate::StorageContext;
use pcagent_inspect::{CoverageStatus, Inspection};
use pcagent_shared::{Origin, Snapshot, Window};
use pcagent_storage::LargestItemsPage;
use pcagent_store::{BatteryRecording, HistorySeries, ProcessPeaks, Recording};
use pcagent_telemetry::ProcessActivity;
use serde::Serialize;
use std::collections::BTreeSet;

/// All members come from the local host. This type deliberately has no Deserialize implementation.
pub enum SelectionSource {
    Snapshot(Snapshot),
    ProcessActivity(ProcessActivity),
    History(HistorySeries),
    Events(HistorySeries),
    Recording(RecordingSource),
    Storage(StorageSource),
    /// One live battery read: C4 observations of the batteries the window shows. The host keeps
    /// only the metrics it displays, partial or contradicting values included.
    Battery(Inspection),
    ElevatedInspection {
        id: String,
        inspection: Inspection,
    },
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
            Self::ElevatedInspection { id, .. } => {
                targets.push(SelectionTarget::ElevatedInspection {
                    inspection_id: id.clone(),
                })
            }
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
            Self::Battery(read) => {
                if read.observations.iter().any(|observation| {
                    is_battery_ref(&observation.entity_ref)
                        && !observation.value.is_null()
                        && matches!(
                            observation.status,
                            CoverageStatus::Ok | CoverageStatus::Partial
                        )
                }) {
                    targets.push(SelectionTarget::Resource {
                        resource: Resource::Battery,
                    });
                }
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
            Self::Battery(value) => bytes(value),
            Self::ElevatedInspection { id, inspection } => id
                .len()
                .saturating_add(bytes(inspection))
                .saturating_add(bytes(&inspection.names)),
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
            Self::History(_)
                | Self::Events(_)
                | Self::Recording(_)
                | Self::ElevatedInspection { .. }
        )
    }

    pub fn available_at(&self, now_unix_ms: u64) -> bool {
        match self {
            Self::ElevatedInspection { inspection, .. } => now_unix_ms
                .checked_sub(inspection.measured_at_unix_ms)
                .is_some_and(|age| age < pcagent_store::ELEVATED_RETENTION_MS),
            _ => true,
        }
    }

    pub fn measured_at(&self, target: &SelectionTarget) -> Option<u64> {
        let at = match (self, target) {
            (
                Self::ElevatedInspection { inspection, .. },
                SelectionTarget::ElevatedInspection { .. },
            ) => Some(inspection.measured_at_unix_ms),
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
            (
                Self::Battery(read),
                SelectionTarget::Resource {
                    resource: Resource::Battery,
                },
            ) => read
                .observations
                .iter()
                .filter(|observation| {
                    is_battery_ref(&observation.entity_ref) && !observation.value.is_null()
                })
                .map(|observation| observation.measured_at_unix_ms)
                .max(),
            _ => None,
        };
        at.filter(|at| *at != 0)
    }
}
