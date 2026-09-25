//! Versioned local contracts. Missing measurements are never encoded as zero.
#![forbid(unsafe_code)]
use serde::{Deserialize, Serialize};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub fn new_id(prefix: &str) -> String { format!("{prefix}-{}", Uuid::new_v4()) }
pub fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status { Ok, Partial, Unsupported, PermissionDenied, Stale, Cancelled, Error }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin { Live, SyntheticFixture }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    PhysicalTotalBytes, PhysicalAvailableBytes, SystemCommitBytes, SystemCommitLimitBytes,
    CpuBusyBasisPoints, ProcessWorkingSetBytes, ProcessPrivateCommitBytes,
}
impl Metric {
    pub fn unit(self) -> &'static str {
        if self == Self::CpuBusyBasisPoints { "basis_points_of_total_cpu" } else { "bytes" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: u64,
    /// Monotonic elapsed time, not a subtraction of wall-clock timestamps.
    pub duration_ms: u64,
}
impl Window {
    pub fn finish(started_at_unix_ms: u64, start: Instant) -> Self {
        Self { started_at_unix_ms, finished_at_unix_ms: unix_ms(), duration_ms: start.elapsed().as_millis() as u64 }
    }
    pub fn instant() -> Self {
        let now = unix_ms();
        Self { started_at_unix_ms: now, finished_at_unix_ms: now, duration_ms: 0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub metric: Metric,
    pub entity_ref: String,
    pub value: Option<u64>,
    pub unit: String,
    pub source: String,
    pub window: Window,
    pub status: Status,
    pub origin: Origin,
    pub limitations: Vec<String>,
}
impl Evidence {
    pub fn measured(metric: Metric, entity: &str, value: u64, source: &str, window: Window) -> Self {
        Self { id: new_id("ev"), metric, entity_ref: entity.into(), value: Some(value),
            unit: metric.unit().into(), source: source.into(), window, status: Status::Ok,
            origin: Origin::Live, limitations: Vec::new() }
    }
    pub fn missing(metric: Metric, entity: &str, source: &str, status: Status, reason: &str) -> Self {
        Self { id: new_id("ev"), metric, entity_ref: entity.into(), value: None,
            unit: metric.unit().into(), source: source.into(), window: Window::instant(), status,
            origin: Origin::Live, limitations: vec![reason.into()] }
    }
    pub fn usable_value(&self) -> Option<u64> {
        if self.status == Status::Ok { self.value } else { None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInstance {
    pub entity_ref: String,
    pub pid: u32,
    pub creation_time_filetime: u64,
    /// Not collected in this first slice. Never substitute an estimated boot time.
    pub boot_id: Option<String>,
    pub executable_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCoverage {
    pub status: Status,
    pub enumerated: usize,
    pub readable: usize,
    pub permission_denied: usize,
    pub unavailable: usize,
    pub returned: usize,
    pub enumeration_truncated: bool,
}
impl Default for ProcessCoverage {
    fn default() -> Self {
        Self { status: Status::Unsupported, enumerated: 0, readable: 0,
            permission_denied: 0, unavailable: 0, returned: 0, enumeration_truncated: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    pub origin: Origin,
    pub evidence: Vec<Evidence>,
    pub processes: Vec<ProcessInstance>,
    pub process_coverage: ProcessCoverage,
    pub limitations: Vec<String>,
}
impl Snapshot {
    pub fn empty(session_id: &str) -> Self {
        Self { schema_version: SCHEMA_VERSION, id: new_id("snapshot"), session_id: session_id.into(),
            origin: Origin::Live, evidence: Vec::new(), processes: Vec::new(),
            process_coverage: ProcessCoverage::default(), limitations: Vec::new() }
    }
    pub fn system_value(&self, metric: Metric) -> Option<(&Evidence, u64)> {
        self.evidence.iter().find(|e| e.entity_ref == "system" && e.metric == metric)
            .and_then(|e| e.usable_value().map(|v| (e, v)))
    }
    pub fn has_observations(&self) -> bool { self.evidence.iter().any(|e| e.usable_value().is_some()) }
}

/// Calculate an integer ratio without overflowing a 64-bit byte counter.
pub fn ratio_basis_points(numerator: u64, denominator: u64) -> Option<u64> {
    if denominator == 0 { return None; }
    u64::try_from(u128::from(numerator) * 10_000 / u128::from(denominator)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_is_null_not_zero() {
        let e = Evidence::missing(Metric::PhysicalAvailableBytes, "system", "test", Status::Unsupported, "not collected");
        let json = serde_json::to_value(&e).unwrap();
        assert!(json["value"].is_null());
        assert_eq!(json["status"], "unsupported");
    }
    #[test]
    fn zero_is_a_real_measurement() {
        let e = Evidence::measured(Metric::SystemCommitBytes, "system", 0, "test", Window::instant());
        assert_eq!(e.usable_value(), Some(0));
    }
    #[test]
    fn stale_values_are_not_used() {
        let mut e = Evidence::measured(Metric::SystemCommitBytes, "system", 10, "test", Window::instant());
        e.status = Status::Stale;
        assert_eq!(e.usable_value(), None);
    }
    #[test]
    fn ratios_are_checked() {
        assert_eq!(ratio_basis_points(1, 0), None);
        assert_eq!(ratio_basis_points(u64::MAX, u64::MAX), Some(10_000));
        assert_eq!(ratio_basis_points(1, 4), Some(2_500));
    }
    #[test]
    fn bytes_round_trip_above_javascript_integer_precision() {
        let e = Evidence::measured(Metric::PhysicalTotalBytes, "system", u64::MAX, "test", Window::instant());
        let encoded = serde_json::to_string(&e).unwrap();
        let decoded: Evidence = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.value, Some(u64::MAX));
    }
}
