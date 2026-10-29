//! Closed sentences for local human renderers (`presentation` and the desktop's `phrases.js`).
use crate::{gpu_slowdown_codes, Metric};
use serde::Serialize;
use std::collections::BTreeMap;

/// One displayed sentence as a closed key and typed values. The English text built beside it
/// stays the text of record: model payloads, traces and the answer checks never read a phrase.
/// A phrase only lets a local human renderer word the same statement in its interface language
/// and format each value by its unit (`{"key":…,"values":{"total":{"unit":"bytes","value":…}}}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Phrase {
    pub key: &'static str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<&'static str, Quantity>,
}

impl Phrase {
    pub fn new(key: &'static str) -> Self {
        Self {
            key,
            values: BTreeMap::new(),
        }
    }

    /// The same phrase with one more named value.
    pub fn with(mut self, name: &'static str, value: Quantity) -> Self {
        self.values.insert(name, value);
        self
    }
}

/// A value of a phrase in the unit the window formats it in. A raw number keeps the unit the
/// core measures it in (basis points, microseconds…), never a rounded text.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "unit", content = "value", rename_all = "snake_case")]
pub enum Quantity {
    Bytes(u64),
    BytesPerSecond(u64),
    /// Hundredths of a percent.
    BasisPoints(u64),
    Microseconds(u64),
    Milliseconds(u64),
    PagesPerSecondX100(u64),
    Count(u64),
    Celsius(u64),
    Megahertz(u64),
    Milliwatts(u64),
    /// Closed NVML slowdown reason codes ([`gpu_slowdown_codes`]); empty means none.
    GpuSlowdown(Vec<&'static str>),
    Metric(Metric),
    Metrics(Vec<Metric>),
    /// A reference the window names itself: `system`, `gpu-nvidia-0`, `gpu-engine-3d`.
    Entity(String),
    /// A process of this PC, named locally: its executable name when one was read, and its PID.
    Process {
        name: Option<String>,
        pid: u32,
    },
    /// Local text shown as it is: a storage alias, an architecture.
    Text(String),
}

impl Quantity {
    /// A value of `metric` in the unit [`Metric::format_value`] formats it in.
    pub fn of(metric: Metric, value: u64) -> Self {
        match metric.unit() {
            "bytes" => Self::Bytes(value),
            "bytes_per_second" => Self::BytesPerSecond(value),
            "microseconds" => Self::Microseconds(value),
            "pages_per_second_x100" => Self::PagesPerSecondX100(value),
            "outstanding_requests" => Self::Count(value),
            "degrees_celsius" => Self::Celsius(value),
            "megahertz" => Self::Megahertz(value),
            "milliwatts" => Self::Milliwatts(value),
            "nvml_slowdown_reason_bits" => Self::GpuSlowdown(gpu_slowdown_codes(value)),
            // A share of the CPU, of a window, of an engine's time or of a sample period.
            unit if unit.starts_with("basis_points") => Self::BasisPoints(value),
            _ => Self::Count(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phrase_serializes_its_key_and_typed_values() {
        let phrase = Phrase::new("fact.available_of_total")
            .with("available", Quantity::Bytes(1 << 30))
            .with("share", Quantity::BasisPoints(980));
        assert_eq!(
            serde_json::to_value(&phrase).unwrap(),
            serde_json::json!({"key":"fact.available_of_total","values":{
                "available":{"unit":"bytes","value":1_073_741_824_u64},
                "share":{"unit":"basis_points","value":980}}})
        );
        let bare = serde_json::to_value(Phrase::new("limit.thresholds")).unwrap();
        assert_eq!(bare, serde_json::json!({"key":"limit.thresholds"}));
        let process = Quantity::Process {
            name: None,
            pid: 42,
        };
        assert_eq!(
            serde_json::to_value(process).unwrap(),
            serde_json::json!({"unit":"process","value":{"name":null,"pid":42}})
        );
    }

    #[test]
    fn every_metric_has_the_unit_its_text_is_formatted_in() {
        use Metric::*;
        let cases = [
            (PhysicalTotalBytes, Quantity::Bytes(7)),
            (ProcessIoReadBytesPerSecond, Quantity::BytesPerSecond(7)),
            (CpuBusyBasisPoints, Quantity::BasisPoints(7)),
            (CpuBusiestCoreBasisPoints, Quantity::BasisPoints(7)),
            (DiskIdleBasisPoints, Quantity::BasisPoints(7)),
            (GpuUtilizationBasisPoints, Quantity::BasisPoints(7)),
            (GpuBusiestEngineBasisPoints, Quantity::BasisPoints(7)),
            (DiskReadLatencyMicroseconds, Quantity::Microseconds(7)),
            (DiskQueueLength, Quantity::Count(7)),
            (HardFaultPagesPerSecondX100, Quantity::PagesPerSecondX100(7)),
            (GpuTemperatureCelsius, Quantity::Celsius(7)),
            (GpuSmClockMegahertz, Quantity::Megahertz(7)),
            (GpuPowerMilliwatts, Quantity::Milliwatts(7)),
        ];
        for (metric, expected) in cases {
            assert_eq!(Quantity::of(metric, 7), expected, "{metric:?}");
        }
        assert_eq!(
            Quantity::of(GpuSlowdownReasons, crate::NVML_SW_THERMAL_SLOWDOWN),
            Quantity::GpuSlowdown(vec!["thermal_driver"])
        );
    }
}
