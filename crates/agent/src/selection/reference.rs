//! Closed local references and the resources offered by existing captures.
use pcagent_shared::Metric;
use serde::{Deserialize, Serialize};

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
    Battery,
}

impl Resource {
    pub(super) fn contains(self, metric: Metric) -> bool {
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
            // Battery readings are C4 observations, never telemetry metrics; only a battery
            // source offers this resource.
            Self::Battery => false,
        }
    }
}

/// The opaque reference shape of the battery collector: `battery-` and a lowercase UUID.
pub fn is_battery_ref(value: &str) -> bool {
    let Some(value) = value.strip_prefix("battery-") else {
        return false;
    };
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
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
            Self::Resource {
                resource: Resource::Battery,
            } => "the selected battery measurements",
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
