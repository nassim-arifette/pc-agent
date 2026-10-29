//! Bounded recording-only battery history. Native identities and static inventory never enter
//! this table; each opaque battery reference keeps its own dated, unaveraged observations.
use crate::{stored, Recording, Store, StoreError};
use pcagent_inspect::{CoverageStatus, Inspection, Observation};
use pcagent_shared::{Origin, Status, Window};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_BATTERY_POINTS: usize = 128;
pub const MAX_RECORDING_BATTERIES: usize = 16;
const MAX_FRAME_BYTES: usize = 128 * 1024;
const MAX_CONNECTED_INTERVAL_MS: u64 = 1500;
const MAX_CLOCK_SKEW_MS: u64 = 50;
const SOURCES: &[&str] = &[
    "DeviceIoControl/IOCTL_BATTERY_QUERY_INFORMATION",
    "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS",
    "DeviceIoControl/BatteryEstimatedTime",
    "derived/IOCTL_BATTERY_QUERY_INFORMATION+IOCTL_BATTERY_QUERY_STATUS",
];

fn window_clock_consistent(window: &Window) -> bool {
    window
        .finished_at_unix_ms
        .checked_sub(window.started_at_unix_ms)
        .is_some_and(|dated| dated.abs_diff(window.duration_ms) <= MAX_CLOCK_SKEW_MS)
}

pub(super) const MIGRATION: &str = "BEGIN;
CREATE TABLE recording_battery_samples (
 id INTEGER PRIMARY KEY,
 recording_id TEXT NOT NULL REFERENCES recordings(id) ON DELETE CASCADE,
 recorded_at INTEGER NOT NULL, payload TEXT NOT NULL
);
CREATE INDEX recording_battery_by_recording ON recording_battery_samples(recording_id,id);
PRAGMA user_version = 5;
COMMIT;";

fn unit(metric: &str) -> Option<&'static str> {
    match metric {
        "battery_remaining_capacity_mwh" | "battery_critical_bias_mwh" => Some("milliwatt_hours"),
        "battery_charge_percent" => Some("percent"),
        "battery_power_rate_mw" | "battery_discharge_power_mw" | "battery_charge_power_mw" => {
            Some("milliwatts")
        }
        "battery_voltage_mv" => Some("millivolts"),
        "battery_estimated_runtime_windows_seconds"
        | "battery_estimated_runtime_at_current_rate_seconds" => Some("seconds"),
        "battery_charge_state" | "battery_kind" | "battery_capacity_units" => Some("category"),
        "battery_on_ac"
        | "battery_critical"
        | "battery_capacities_consistent"
        | "battery_state_consistent" => Some("boolean"),
        _ => None,
    }
}

fn battery_ref(value: &str) -> bool {
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

fn safe_value(metric: &str, value: &Value) -> bool {
    if value.is_null() {
        return true;
    }
    match metric {
        "battery_charge_state" => {
            matches!(
                value.as_str(),
                Some("charging" | "discharging" | "idle" | "unknown")
            )
        }
        "battery_kind" => matches!(value.as_str(), Some("system" | "short_term" | "non_system")),
        "battery_capacity_units" => matches!(value.as_str(), Some("milliwatt_hours" | "relative")),
        "battery_on_ac"
        | "battery_critical"
        | "battery_capacities_consistent"
        | "battery_state_consistent" => value.is_boolean(),
        _ => value.as_f64().is_some_and(|number| {
            number.is_finite()
                && (metric == "battery_power_rate_mw" || number >= 0.0)
                && (metric != "battery_charge_percent" || number <= 100.0)
        }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryPoint {
    pub recorded_at_unix_ms: u64,
    #[serde(default)]
    pub recording_elapsed_ms: Option<u64>,
    /// UTC agrees with the recording's monotonic anchor and native query windows.
    #[serde(default)]
    pub clock_consistent: bool,
    pub status: Status,
    pub observations: Vec<Observation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatteryFrameCoverage {
    pub returned_batteries: usize,
    pub omitted_batteries: usize,
    pub unavailable_observations: usize,
    pub rejected_observations: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatteryFrame {
    recorded_at_unix_ms: u64,
    #[serde(default)]
    recording_elapsed_ms: Option<u64>,
    #[serde(default)]
    inspection_window: Option<Window>,
    status: Status,
    origin: Origin,
    points: BTreeMap<String, BatteryPoint>,
    coverage: BatteryFrameCoverage,
}

impl BatteryFrame {
    fn from_inspection(
        inspection: &Inspection,
        recorded_at: u64,
        elapsed_ms: Option<u64>,
        recording_started_at: u64,
    ) -> Self {
        // The UTC lag used to align a native read must first agree with independent Instant
        // timing. Otherwise a clock step during/after the first read can be silently absorbed.
        let clock_consistent = elapsed_ms
            .and_then(|elapsed| recording_started_at.checked_add(elapsed))
            .is_some_and(|expected| recorded_at.abs_diff(expected) <= MAX_CLOCK_SKEW_MS)
            && window_clock_consistent(&inspection.window)
            && inspection.measured_at_unix_ms == inspection.window.finished_at_unix_ms
            && inspection.measured_at_unix_ms <= recorded_at;
        let mut points: BTreeMap<String, BatteryPoint> = BTreeMap::new();
        let mut omitted = BTreeSet::new();
        let mut coverage = BatteryFrameCoverage {
            rejected_observations: inspection.observations.len().saturating_sub(512),
            ..BatteryFrameCoverage::default()
        };
        // Native battery inspections are bounded too. This boundary does not retain names,
        // coverage prose, inventory, interfaces, serials, arbitrary strings or unknown metrics.
        for observation in inspection.observations.iter().take(512) {
            let Some(expected_unit) = unit(&observation.metric) else {
                continue;
            };
            if expected_unit != observation.unit
                || !battery_ref(&observation.entity_ref)
                || !SOURCES.contains(&observation.source.as_str())
                || !safe_value(&observation.metric, &observation.value)
                || observation.origin != inspection.origin
                || observation.measured_at_unix_ms != observation.window.finished_at_unix_ms
            {
                coverage.rejected_observations += 1;
                continue;
            }
            if !points.contains_key(&observation.entity_ref)
                && points.len() >= MAX_RECORDING_BATTERIES
            {
                omitted.insert(observation.entity_ref.clone());
                continue;
            }
            let point = points
                .entry(observation.entity_ref.clone())
                .or_insert_with(|| BatteryPoint {
                    recorded_at_unix_ms: recorded_at,
                    recording_elapsed_ms: elapsed_ms,
                    clock_consistent,
                    status: Status::Ok,
                    observations: Vec::new(),
                });
            // Keep real native values through UTC discontinuities, with explicit unusable
            // timing instead of silently dropping the point or guessing its elapsed offset.
            point.clock_consistent &= window_clock_consistent(&observation.window)
                && observation.window.started_at_unix_ms >= inspection.window.started_at_unix_ms
                && observation.measured_at_unix_ms <= inspection.window.finished_at_unix_ms
                && observation.measured_at_unix_ms <= recorded_at;
            if !point.clock_consistent {
                point.status = Status::Partial;
            }
            if let Some(old) = point
                .observations
                .iter_mut()
                .find(|old| old.metric == observation.metric)
            {
                old.value = Value::Null;
                old.status = CoverageStatus::Partial;
                coverage.rejected_observations += 1;
                point.status = Status::Partial;
                continue;
            }
            let mut observation = observation.clone();
            // IDs supplied by a producer are not retained; this ID identifies only this
            // opaque battery/metric in this recorded tick.
            observation.observation_id = format!(
                "{}-measurement-{}-{}",
                observation.entity_ref,
                recorded_at,
                point.observations.len() + 1
            );
            if observation.value.is_null() && observation.status == CoverageStatus::Ok {
                observation.status = CoverageStatus::Unsupported;
            }
            if observation.status != CoverageStatus::Ok {
                point.status = Status::Partial;
                coverage.unavailable_observations += 1;
            }
            point.observations.push(observation);
        }
        coverage.returned_batteries = points.len();
        coverage.omitted_batteries = omitted.len();
        let complete = inspection.status == Status::Ok
            && coverage.omitted_batteries == 0
            && coverage.rejected_observations == 0
            && points.values().all(|point| point.status == Status::Ok);
        Self {
            recorded_at_unix_ms: recorded_at,
            recording_elapsed_ms: elapsed_ms,
            inspection_window: Some(inspection.window.clone()),
            status: if complete {
                Status::Ok
            } else if points.is_empty() {
                inspection.status
            } else {
                Status::Partial
            },
            origin: inspection.origin,
            points,
            coverage,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryGap {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryDerivedObservation {
    #[serde(flatten)]
    pub observation: Observation,
    pub derivation: &'static str,
    pub inputs: Vec<Observation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryEvolution {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub measured_points: usize,
    pub observations: Vec<BatteryDerivedObservation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryHistory {
    pub battery_ref: String,
    pub status: Status,
    pub points: Vec<BatteryPoint>,
    pub gaps: Vec<BatteryGap>,
    /// Separate continuous discharge segments. Empty means unknown, not zero discharge.
    pub evolution: Vec<BatteryEvolution>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryRecordingCoverage {
    pub expected_ticks: u64,
    pub recorded_ticks: usize,
    pub ticks_without_battery: usize,
    pub unavailable_observations: usize,
    pub rejected_observations: usize,
    pub points_returned: usize,
    pub points_omitted_from_response: usize,
    pub batteries_omitted_from_response: usize,
    pub battery_tick_points_omitted_during_collection: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatteryRecording {
    pub status: Status,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: Option<u64>,
    pub interval_ms: u64,
    pub coverage: BatteryRecordingCoverage,
    pub gaps: Vec<BatteryGap>,
    pub batteries: Vec<BatteryHistory>,
    pub limitations: Vec<String>,
}

fn observed<'a>(point: &'a BatteryPoint, metric: &str) -> Option<&'a Observation> {
    point
        .observations
        .iter()
        .find(|observation| observation.metric == metric)
}

fn usable<'a>(point: &'a BatteryPoint, metric: &str) -> Option<&'a Observation> {
    observed(point, metric).filter(|observation| observation.status == CoverageStatus::Ok)
}

fn capacity(point: &BatteryPoint) -> Option<&Observation> {
    let capacity = usable(point, "battery_remaining_capacity_mwh")?;
    let expected = [
        ("battery_kind", Value::from("system")),
        ("battery_capacity_units", Value::from("milliwatt_hours")),
        ("battery_capacities_consistent", Value::from(true)),
        ("battery_state_consistent", Value::from(true)),
        ("battery_charge_state", Value::from("discharging")),
        ("battery_on_ac", Value::from(false)),
    ];
    if !expected.iter().all(|(metric, value)| {
        usable(point, metric).is_some_and(|observation| {
            &observation.value == value
                && observation.origin == capacity.origin
                && observation
                    .measured_at_unix_ms
                    .abs_diff(capacity.measured_at_unix_ms)
                    <= MAX_CONNECTED_INTERVAL_MS
        })
    }) {
        return None;
    }
    if usable(point, "battery_power_rate_mw")
        .and_then(|observation| observation.value.as_f64())
        .is_some_and(|rate| rate >= 0.0)
    {
        return None;
    }
    capacity.value.as_f64().filter(|value| *value >= 0.0)?;
    Some(capacity)
}

fn native_elapsed(point: &BatteryPoint, observation: &Observation) -> Option<u64> {
    if !point.clock_consistent {
        return None;
    }
    // The native point predates completion of its enclosing sampler tick. Align its end to
    // the tick's actual Instant offset only after UTC/Instant coherence has been checked.
    let lag = point
        .recorded_at_unix_ms
        .checked_sub(observation.measured_at_unix_ms)?;
    if lag > MAX_CONNECTED_INTERVAL_MS {
        return None;
    }
    point.recording_elapsed_ms?.checked_sub(lag)
}

fn evolution(points: &[&BatteryPoint]) -> Option<BatteryEvolution> {
    if points.len() < 2 {
        return None;
    }
    let first = capacity(points[0])?;
    let last = capacity(points[points.len() - 1])?;
    let elapsed_ms = native_elapsed(points[points.len() - 1], last)?
        .checked_sub(native_elapsed(points[0], first)?)?;
    if elapsed_ms == 0 {
        return None;
    }
    let decrease = first.value.as_f64()? - last.value.as_f64()?;
    if decrease < 0.0 || !decrease.is_finite() {
        return None;
    }
    let window = Window {
        started_at_unix_ms: first.measured_at_unix_ms,
        finished_at_unix_ms: last.measured_at_unix_ms,
        duration_ms: elapsed_ms,
    };
    let make = |metric: &str, value: Option<f64>, unit: &str, derivation: &'static str| {
        BatteryDerivedObservation {
            observation: Observation {
                observation_id: format!(
                    "{}-evolution-{}-{metric}",
                    first.entity_ref, first.measured_at_unix_ms
                ),
                measured_at_unix_ms: last.measured_at_unix_ms,
                window: window.clone(),
                entity_ref: first.entity_ref.clone(),
                metric: metric.into(),
                value: value
                    .filter(|v| v.is_finite())
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                unit: unit.into(),
                status: if value.is_some_and(|v| v.is_finite()) {
                    CoverageStatus::Ok
                } else {
                    CoverageStatus::Unsupported
                },
                source: "derived/recording_battery_capacity_endpoints".into(),
                origin: first.origin,
            },
            derivation,
            inputs: vec![first.clone(), last.clone()],
        }
    };
    let runtime = last
        .value
        .as_f64()
        .filter(|_| decrease > 0.0)
        .map(|remaining| remaining * elapsed_ms as f64 / decrease / 1000.0);
    let runtime_observation = make(
        "battery_estimated_runtime_from_recording_seconds",
        runtime,
        "seconds",
        "last_remaining_capacity_divided_by_observed_loss_rate_assuming_constant_discharge",
    );
    Some(BatteryEvolution {
        from_unix_ms: first.measured_at_unix_ms,
        to_unix_ms: last.measured_at_unix_ms,
        measured_points: points.len(),
        observations: vec![
            make(
                "battery_measured_capacity_decrease_mwh",
                Some(decrease),
                "milliwatt_hours",
                "first_remaining_capacity_minus_last_remaining_capacity_in_continuous_discharge",
            ),
            make(
                "battery_average_power_rate_mw",
                Some(-decrease * 3_600_000.0 / elapsed_ms as f64),
                "milliwatts",
                "negative_capacity_decrease_times_3600000_divided_by_elapsed_milliseconds",
            ),
            runtime_observation,
        ],
    })
}

fn summarize(recording: &Recording, frames: &[BatteryFrame]) -> BatteryRecording {
    let end = recording.ended_at_unix_ms.unwrap_or_else(|| {
        frames.last().map_or(recording.started_at_unix_ms, |frame| {
            frame.recorded_at_unix_ms
        })
    });
    let expected_ticks = if recording.status == "completed" {
        recording.duration_seconds
    } else {
        frames
            .last()
            .and_then(|frame| frame.recording_elapsed_ms)
            .map(|elapsed| elapsed / 1000 + 1)
            .unwrap_or_else(|| {
                end.saturating_sub(recording.started_at_unix_ms)
                    .div_ceil(1000)
            })
            .min(recording.duration_seconds)
    };
    let mut refs = BTreeSet::new();
    let mut selected: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut total_points = 0;
    let mut returned_points = 0;
    let mut all_refs = BTreeSet::new();
    for (frame_index, frame) in frames.iter().enumerate().rev() {
        for reference in frame.points.keys() {
            total_points += 1;
            all_refs.insert(reference.clone());
            if (refs.contains(reference) || refs.len() < MAX_RECORDING_BATTERIES)
                && returned_points < MAX_BATTERY_POINTS
            {
                refs.insert(reference.clone());
                selected
                    .entry(reference.clone())
                    .or_default()
                    .insert(frame_index);
                returned_points += 1;
            }
        }
    }
    let mut gaps = Vec::new();
    let mut previous_tick = recording.started_at_unix_ms;
    for frame in frames {
        if frame.recorded_at_unix_ms.saturating_sub(previous_tick) > MAX_CONNECTED_INTERVAL_MS {
            gaps.push(BatteryGap {
                from_unix_ms: previous_tick,
                to_unix_ms: frame.recorded_at_unix_ms,
                reason: "not_recorded",
            });
        }
        if frame.points.is_empty() {
            gaps.push(BatteryGap {
                from_unix_ms: frame.recorded_at_unix_ms,
                to_unix_ms: (frame.recorded_at_unix_ms + 1000).min(end),
                reason: "battery_measurement_unavailable",
            });
        }
        previous_tick = frame.recorded_at_unix_ms;
    }
    if end.saturating_sub(previous_tick) > MAX_CONNECTED_INTERVAL_MS {
        gaps.push(BatteryGap {
            from_unix_ms: previous_tick,
            to_unix_ms: end,
            reason: "not_recorded",
        });
    }
    let mut batteries = Vec::new();
    for reference in refs {
        let mut history = BatteryHistory {
            battery_ref: reference.clone(),
            status: Status::Ok,
            points: Vec::new(),
            gaps: Vec::new(),
            evolution: Vec::new(),
        };
        let mut segment: Vec<&BatteryPoint> = Vec::new();
        let mut last_seen = recording.started_at_unix_ms;
        let flush = |segment: &mut Vec<&BatteryPoint>, history: &mut BatteryHistory| {
            if let Some(derived) = evolution(segment) {
                history.evolution.push(derived);
            }
            segment.clear();
        };
        for (frame_index, frame) in frames.iter().enumerate() {
            let point = frame
                .points
                .get(&reference)
                .filter(|_| selected[&reference].contains(&frame_index));
            let Some(point) = point else {
                flush(&mut segment, &mut history);
                history.status = Status::Partial;
                let reason = if frame.points.contains_key(&reference) {
                    "response_limit"
                } else {
                    "battery_not_observed"
                };
                history.gaps.push(BatteryGap {
                    from_unix_ms: frame.recorded_at_unix_ms,
                    to_unix_ms: (frame.recorded_at_unix_ms + 1000).min(end),
                    reason,
                });
                continue;
            };
            if frame.recorded_at_unix_ms.saturating_sub(last_seen) > MAX_CONNECTED_INTERVAL_MS {
                flush(&mut segment, &mut history);
                history.gaps.push(BatteryGap {
                    from_unix_ms: last_seen,
                    to_unix_ms: frame.recorded_at_unix_ms,
                    reason: "not_recorded",
                });
                history.status = Status::Partial;
            }
            history.points.push(point.clone());
            last_seen = frame.recorded_at_unix_ms;
            if point.status != Status::Ok {
                history.status = Status::Partial;
            }
            let Some(current) = capacity(point) else {
                flush(&mut segment, &mut history);
                continue;
            };
            if native_elapsed(point, current).is_none() {
                flush(&mut segment, &mut history);
                history.status = Status::Partial;
                history.gaps.push(BatteryGap {
                    from_unix_ms: current
                        .window
                        .started_at_unix_ms
                        .min(point.recorded_at_unix_ms),
                    to_unix_ms: current.measured_at_unix_ms.max(point.recorded_at_unix_ms),
                    reason: if point.recording_elapsed_ms.is_none() {
                        "monotonic_time_unavailable"
                    } else {
                        "clock_inconsistent"
                    },
                });
                continue;
            }
            if let Some(previous) = segment.last().and_then(|point| capacity(point)) {
                let elapsed = native_elapsed(point, current)
                    .zip(native_elapsed(segment[segment.len() - 1], previous))
                    .and_then(|(current, previous)| current.checked_sub(previous));
                let dated = current
                    .measured_at_unix_ms
                    .checked_sub(previous.measured_at_unix_ms);
                let coherent = current.origin == previous.origin
                    && current.source == previous.source
                    && elapsed.zip(dated).is_some_and(|(elapsed, dated)| {
                        elapsed > 0
                            && dated > 0
                            && elapsed <= MAX_CONNECTED_INTERVAL_MS
                            && dated <= MAX_CONNECTED_INTERVAL_MS
                            && elapsed.abs_diff(dated) <= MAX_CLOCK_SKEW_MS
                    })
                    && current.window.started_at_unix_ms >= previous.window.finished_at_unix_ms;
                let decreased = current
                    .value
                    .as_f64()
                    .zip(previous.value.as_f64())
                    .is_some_and(|(current, previous)| current <= previous);
                if !coherent || !decreased {
                    flush(&mut segment, &mut history);
                    history.gaps.push(BatteryGap {
                        from_unix_ms: previous
                            .measured_at_unix_ms
                            .min(current.measured_at_unix_ms),
                        to_unix_ms: previous
                            .measured_at_unix_ms
                            .max(current.measured_at_unix_ms),
                        reason: "inconsistent_discharge_interval",
                    });
                    history.status = Status::Partial;
                    // An increasing/incoherent point cannot seed an apparent new decrease.
                    continue;
                }
            }
            segment.push(point);
        }
        flush(&mut segment, &mut history);
        if end.saturating_sub(last_seen) > MAX_CONNECTED_INTERVAL_MS {
            history.gaps.push(BatteryGap {
                from_unix_ms: last_seen,
                to_unix_ms: end,
                reason: "not_recorded",
            });
            history.status = Status::Partial;
        }
        batteries.push(history);
    }
    let coverage = BatteryRecordingCoverage {
        expected_ticks,
        recorded_ticks: frames.len(),
        ticks_without_battery: frames
            .iter()
            .filter(|frame| frame.points.is_empty())
            .count(),
        unavailable_observations: frames
            .iter()
            .map(|frame| frame.coverage.unavailable_observations)
            .sum(),
        rejected_observations: frames
            .iter()
            .map(|frame| frame.coverage.rejected_observations)
            .sum(),
        points_returned: returned_points,
        points_omitted_from_response: total_points - returned_points,
        batteries_omitted_from_response: all_refs.len() - batteries.len(),
        battery_tick_points_omitted_during_collection: frames
            .iter()
            .map(|frame| frame.coverage.omitted_batteries)
            .sum(),
    };
    let status = if batteries.is_empty() {
        if frames.iter().any(|frame| frame.status == Status::Error) {
            Status::Error
        } else if frames
            .iter()
            .any(|frame| frame.status == Status::PermissionDenied)
        {
            Status::PermissionDenied
        } else if frames.iter().any(|frame| {
            matches!(
                frame.status,
                Status::Partial | Status::Stale | Status::Cancelled
            )
        }) {
            Status::Partial
        } else {
            Status::Unsupported
        }
    } else if recording.status == "completed"
        && gaps.is_empty()
        && frames.len() as u64 >= expected_ticks
        && frames.iter().all(|frame| frame.status == Status::Ok)
        && coverage.points_omitted_from_response == 0
        && batteries.iter().all(|battery| battery.status == Status::Ok)
    {
        Status::Ok
    } else {
        Status::Partial
    };
    let mut limitations = vec![
        "Battery capacity changes and rates are separate per opaque battery; no multi-battery sum or per-application energy attribution is measured.".into(),
        "Recording-derived runtime uses the remaining capacity reported by the driver and assumes the observed continuous discharge rate remains constant; missing monotonic timing, UTC/Instant clock inconsistencies, relative capacity, charging and inconsistent intervals have no estimate. Critical bias is retained only as a raw observation.".into(),
    ];
    if coverage.points_omitted_from_response > 0 {
        limitations.push(format!("{} older battery points omitted from the response; retained points were not averaged across gaps or battery replacements.", coverage.points_omitted_from_response));
    }
    if frames.is_empty() {
        limitations.push("No battery measurements were recorded for this session; earlier recordings have no synthetic substitute.".into());
    }
    BatteryRecording {
        status,
        started_at_unix_ms: recording.started_at_unix_ms,
        ended_at_unix_ms: recording.ended_at_unix_ms,
        interval_ms: 1000,
        coverage,
        gaps,
        batteries,
        limitations,
    }
}

impl Store {
    /// Appends one bounded battery tick to an existing active problem recording. The inspection
    /// is projected to the dynamic allowlist before SQLite; no raw local inspection is stored.
    pub fn append_recording_battery(
        &self,
        id: &str,
        inspection: &Inspection,
        at: u64,
    ) -> Result<(), StoreError> {
        self.append_recording_battery_timed(id, inspection, at, None)
    }

    pub fn append_recording_battery_timed(
        &self,
        id: &str,
        inspection: &Inspection,
        at: u64,
        elapsed_ms: Option<u64>,
    ) -> Result<(), StoreError> {
        let recording = self.recording(id)?.ok_or(StoreError::InvalidQuery)?;
        if recording.status != "recording"
            || at > i64::MAX as u64
            || elapsed_ms.is_some_and(|elapsed| {
                elapsed > recording.duration_seconds * 1000 + MAX_CONNECTED_INTERVAL_MS
            })
            || (elapsed_ms.is_none()
                && (at < recording.started_at_unix_ms
                    || at
                        > recording.started_at_unix_ms.saturating_add(
                            recording.duration_seconds * 1000 + MAX_CONNECTED_INTERVAL_MS,
                        )))
        {
            return Err(StoreError::InvalidQuery);
        }
        let frame =
            BatteryFrame::from_inspection(inspection, at, elapsed_ms, recording.started_at_unix_ms);
        let text = serde_json::to_string(&frame).map_err(|_| StoreError::Corrupt)?;
        if text.len() > MAX_FRAME_BYTES {
            return Err(StoreError::InvalidQuery);
        }
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO recording_battery_samples(recording_id,recorded_at,payload)
             SELECT ?1,?2,?3 WHERE (SELECT COUNT(*) FROM recording_battery_samples WHERE recording_id=?1)<?4",
            params![id, stored(at), text, MAX_BATTERY_POINTS as i64],
        )?;
        if changed == 0 {
            return Err(StoreError::InvalidQuery);
        }
        Ok(())
    }

    /// None means the recording was removed or never existed. Older recordings survive schema
    /// migration, with explicit unknown battery coverage rather than invented zero measurements.
    pub fn battery_recording(&self, id: &str) -> Result<Option<BatteryRecording>, StoreError> {
        let Some(recording) = self.recording(id)? else {
            return Ok(None);
        };
        let mut statement = self.conn.prepare(
            "SELECT payload FROM recording_battery_samples WHERE recording_id=?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = statement.query_map(params![id, MAX_BATTERY_POINTS as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut frames = Vec::new();
        for text in rows {
            let text = text?;
            if text.len() > MAX_FRAME_BYTES {
                return Err(StoreError::Corrupt);
            }
            let frame: BatteryFrame =
                serde_json::from_str(&text).map_err(|_| StoreError::Corrupt)?;
            let has_inspection_window = frame.inspection_window.is_some();
            let inspection_window = frame.inspection_window.unwrap_or(Window {
                started_at_unix_ms: frame.recorded_at_unix_ms,
                finished_at_unix_ms: frame.recorded_at_unix_ms,
                duration_ms: 0,
            });
            // Reproject persisted fields before publication, even if an older/malformed producer
            // wrote extra static information to the table.
            let inspection = Inspection {
                status: frame.status,
                measured_at_unix_ms: inspection_window.finished_at_unix_ms,
                window: inspection_window,
                origin: frame.origin,
                observations: frame
                    .points
                    .into_values()
                    .flat_map(|point| point.observations)
                    .collect(),
                coverage: Vec::new(),
                names: BTreeMap::new(),
            };
            let mut checked = BatteryFrame::from_inspection(
                &inspection,
                frame.recorded_at_unix_ms,
                frame.recording_elapsed_ms.filter(|elapsed| {
                    *elapsed <= recording.duration_seconds * 1000 + MAX_CONNECTED_INTERVAL_MS
                }),
                recording.started_at_unix_ms,
            );
            if !has_inspection_window && !checked.points.is_empty() {
                checked.status = Status::Partial;
                for point in checked.points.values_mut() {
                    point.clock_consistent = false;
                    point.status = Status::Partial;
                }
            }
            checked.coverage.omitted_batteries += frame.coverage.omitted_batteries.min(512);
            checked.coverage.rejected_observations += frame.coverage.rejected_observations.min(512);
            frames.push(checked);
        }
        Ok(Some(summarize(&recording, &frames)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SAMPLE_RETENTION_MS;

    fn reference(index: usize) -> String {
        format!("battery-00000000-0000-0000-0000-{index:012x}")
    }

    fn fixture(at: u64, remaining: Option<f64>, index: usize) -> Inspection {
        let reference = reference(index);
        let values = [
            ("battery_kind", Value::from("system")),
            ("battery_capacity_units", Value::from("milliwatt_hours")),
            ("battery_capacities_consistent", Value::from(true)),
            ("battery_state_consistent", Value::from(true)),
            ("battery_charge_state", Value::from("discharging")),
            ("battery_on_ac", Value::from(false)),
            ("battery_critical_bias_mwh", Value::from(10.0)),
            (
                "battery_remaining_capacity_mwh",
                remaining.map(Value::from).unwrap_or(Value::Null),
            ),
            ("battery_power_rate_mw", Value::from(-3600)),
            ("battery_discharge_power_mw", Value::from(3600)),
            ("battery_charge_percent", Value::from(80.0)),
        ];
        let window = Window {
            started_at_unix_ms: at.saturating_sub(1),
            finished_at_unix_ms: at,
            duration_ms: 1,
        };
        let observations = values
            .into_iter()
            .map(|(metric, value)| Observation {
                observation_id: r"C:\Users\Alice\private-measurement".into(),
                entity_ref: reference.clone(),
                measured_at_unix_ms: at,
                window: window.clone(),
                metric: metric.into(),
                unit: unit(metric).unwrap().into(),
                status: if value.is_null() {
                    CoverageStatus::Unsupported
                } else {
                    CoverageStatus::Ok
                },
                value,
                source: SOURCES[1].into(),
                origin: Origin::SyntheticFixture,
            })
            .collect();
        Inspection {
            status: Status::Ok,
            measured_at_unix_ms: at,
            window,
            origin: Origin::SyntheticFixture,
            observations,
            coverage: vec![],
            names: BTreeMap::from([(reference, r"C:\Users\Alice\private-battery".into())]),
        }
    }

    fn set(inspection: &mut Inspection, metric: &str, value: Value) {
        inspection
            .observations
            .iter_mut()
            .find(|observation| observation.metric == metric)
            .unwrap()
            .value = value;
    }

    fn completed(points: &[(u64, Option<f64>, usize)]) -> BatteryRecording {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        for &(at, remaining, index) in points {
            store
                .append_recording_battery_timed(
                    &recording.id,
                    &fixture(at, remaining, index),
                    at,
                    Some(at - 1000),
                )
                .unwrap();
        }
        store
            .finish_recording(
                &recording.id,
                points.last().unwrap().0,
                false,
                points.len() as u64,
            )
            .unwrap();
        store.battery_recording(&recording.id).unwrap().unwrap()
    }

    #[test]
    fn continuous_discharge_has_dated_endpoint_evidence_and_signed_average_rate() {
        let result = completed(&[
            (1000, Some(1000.0), 1),
            (2000, Some(999.0), 1),
            (3000, Some(998.0), 1),
        ]);
        let evolution = &result.batteries[0].evolution[0];
        assert_eq!(evolution.measured_points, 3);
        assert_eq!(evolution.from_unix_ms, 1000);
        assert_eq!(evolution.to_unix_ms, 3000);
        assert_eq!(evolution.observations[0].observation.value, 2.0);
        assert_eq!(evolution.observations[1].observation.value, -3600.0);
        assert_eq!(evolution.observations[2].observation.value, 998.0);
        assert_eq!(
            evolution.observations[0].observation.unit,
            "milliwatt_hours"
        );
        assert_eq!(
            evolution.observations[0].observation.origin,
            Origin::SyntheticFixture
        );
        assert_eq!(evolution.observations[0].inputs[0].source, SOURCES[1]);
        assert_eq!(evolution.observations[0].inputs[0].window.duration_ms, 1);
        assert_eq!(
            evolution.observations[0].observation.window.duration_ms,
            2000
        );
        assert_eq!(result.batteries[0].points.len(), 3);
        let wire = serde_json::to_string(&result).unwrap();
        for forbidden in [
            "Alice",
            "C:",
            "session_id",
            "recording_id",
            "private-battery",
        ] {
            assert!(!wire.contains(forbidden));
        }
    }

    #[test]
    fn charging_relative_inconsistent_or_missing_capacity_never_estimates_discharge() {
        for (metric, value) in [
            ("battery_charge_state", Value::from("charging")),
            ("battery_capacity_units", Value::from("relative")),
            ("battery_kind", Value::from("short_term")),
            ("battery_on_ac", Value::from(true)),
            ("battery_state_consistent", Value::from(false)),
            ("battery_capacities_consistent", Value::from(false)),
            ("battery_remaining_capacity_mwh", Value::Null),
        ] {
            let recording = Recording {
                id: "local".into(),
                started_at_unix_ms: 1000,
                ended_at_unix_ms: Some(3000),
                duration_seconds: 30,
                status: "completed".into(),
                samples: 3,
                markers: vec![],
            };
            let frames: Vec<_> = (1..=3)
                .map(|index| {
                    let at = index * 1000;
                    let mut inspection = fixture(at, Some(1000.0 - index as f64), 1);
                    set(&mut inspection, metric, value.clone());
                    BatteryFrame::from_inspection(&inspection, at, Some(at - 1000), 1000)
                })
                .collect();
            assert!(
                summarize(&recording, &frames).batteries[0]
                    .evolution
                    .is_empty(),
                "{metric}"
            );
        }
    }

    #[test]
    fn gaps_replacements_and_null_points_are_never_blended() {
        let result = completed(&[
            (1000, Some(1000.0), 1),
            (2000, Some(999.0), 1),
            (5000, Some(998.0), 1),
            (6000, Some(997.0), 1),
        ]);
        assert_eq!(result.status, Status::Partial);
        assert_eq!(result.batteries[0].evolution.len(), 2);
        assert!(result.batteries[0]
            .evolution
            .iter()
            .all(|evolution| evolution.to_unix_ms - evolution.from_unix_ms == 1000));
        assert!(result.gaps.iter().any(|gap| gap.reason == "not_recorded"));
        let replacement = completed(&[
            (1000, Some(1000.0), 1),
            (2000, Some(999.0), 1),
            (3000, Some(1000.0), 2),
            (4000, Some(999.0), 2),
        ]);
        assert_eq!(replacement.batteries.len(), 2);
        assert!(replacement
            .batteries
            .iter()
            .all(|battery| battery.evolution.len() == 1));
        let unknown = completed(&[
            (1000, Some(1000.0), 1),
            (1500, None, 1),
            (2000, Some(999.0), 1),
        ]);
        assert!(unknown.batteries[0].evolution.is_empty());
        assert!(observed(
            &unknown.batteries[0].points[1],
            "battery_remaining_capacity_mwh"
        )
        .unwrap()
        .value
        .is_null());
        assert_eq!(unknown.batteries[0].points[1].status, Status::Partial);
    }

    #[test]
    fn absent_tick_duplicate_metric_and_increasing_capacity_break_continuity() {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        for at in [1000, 1500, 2000] {
            let mut inspection = fixture(at, Some(1000.0 - at as f64 / 1000.0), 1);
            if at == 1500 {
                inspection.observations.clear();
                inspection.status = Status::Unsupported;
            }
            store
                .append_recording_battery_timed(&recording.id, &inspection, at, Some(at - 1000))
                .unwrap();
        }
        store
            .finish_recording(&recording.id, 3000, false, 3)
            .unwrap();
        let result = store.battery_recording(&recording.id).unwrap().unwrap();
        assert!(result.batteries[0].evolution.is_empty());
        assert_eq!(result.coverage.ticks_without_battery, 1);
        assert!(result.batteries[0]
            .gaps
            .iter()
            .any(|gap| gap.reason == "battery_not_observed"));
        let mut duplicate = fixture(1000, Some(1000.0), 1);
        let mut other = duplicate
            .observations
            .iter()
            .find(|observation| observation.metric == "battery_remaining_capacity_mwh")
            .unwrap()
            .clone();
        other.value = Value::from(500.0);
        duplicate.observations.push(other);
        let frame = BatteryFrame::from_inspection(&duplicate, 1000, Some(0), 1000);
        assert!(capacity(&frame.points[&reference(1)]).is_none());
        let increased = completed(&[
            (1000, Some(1000.0), 1),
            (2000, Some(1001.0), 1),
            (3000, Some(999.0), 1),
        ]);
        assert!(increased.batteries[0].evolution.is_empty());
    }

    #[test]
    fn zero_observed_loss_keeps_runtime_unknown_and_never_invents_a_drain() {
        let result = completed(&[(1000, Some(1000.0), 1), (2000, Some(1000.0), 1)]);
        let observations = &result.batteries[0].evolution[0].observations;
        assert_eq!(observations[0].observation.value, 0.0);
        assert_eq!(observations[1].observation.value, 0.0);
        assert!(observations[2].observation.value.is_null());
        assert_eq!(
            observations[2].observation.status,
            CoverageStatus::Unsupported
        );
    }

    #[test]
    fn utc_clock_steps_backwards_dates_same_offsets_and_missing_offsets_refuse_evolution() {
        let recording = Recording {
            id: "local-clock-fixture".into(),
            started_at_unix_ms: 1000,
            ended_at_unix_ms: Some(5000),
            duration_seconds: 30,
            status: "completed".into(),
            samples: 2,
            markers: vec![],
        };
        for ticks in [
            [(1000, Some(0)), (2400, Some(1000))],
            [(2000, Some(0)), (1500, Some(1000))],
            [(1000, Some(0)), (2000, Some(0))],
            [(1000, None), (2000, None)],
            [(1000, Some(0)), (4000, Some(3000))],
        ] {
            let frames: Vec<_> = ticks
                .into_iter()
                .enumerate()
                .map(|(index, (at, elapsed))| {
                    BatteryFrame::from_inspection(
                        &fixture(at, Some(1000.0 - index as f64), 1),
                        at,
                        elapsed,
                        1000,
                    )
                })
                .collect();
            let result = summarize(&recording, &frames);
            assert!(result.batteries[0].evolution.is_empty());
            assert_eq!(result.status, Status::Partial);
            assert!(!result.batteries[0].gaps.is_empty());
        }
    }

    #[test]
    fn first_read_clock_jump_before_tick_finish_keeps_points_without_drain_estimates() {
        for step_ms in [500_i64, -500] {
            for during_inspection in [false, true] {
                let store = Store::open_in_memory().unwrap();
                let recording = store.begin_recording(30, 1000).unwrap();
                let first_at = (2000 + step_ms) as u64;
                let last_at = (3000 + step_ms) as u64;
                let mut first = fixture(2000, Some(1000.0), 1);
                if during_inspection {
                    first.window = Window {
                        started_at_unix_ms: 1990,
                        finished_at_unix_ms: first_at,
                        duration_ms: 10,
                    };
                    first.measured_at_unix_ms = first_at;
                }
                store
                    .append_recording_battery_timed(&recording.id, &first, first_at, Some(1000))
                    .unwrap();
                store
                    .append_recording_battery_timed(
                        &recording.id,
                        &fixture(last_at - 1, Some(999.0), 1),
                        last_at,
                        Some(2000),
                    )
                    .unwrap();
                store
                    .finish_recording(&recording.id, last_at, false, 2)
                    .unwrap();
                let result = store.battery_recording(&recording.id).unwrap().unwrap();
                let battery = &result.batteries[0];
                assert_eq!(result.status, Status::Partial);
                assert_eq!(result.coverage.rejected_observations, 0);
                assert_eq!(battery.points.len(), 2);
                assert_eq!(battery.points[0].recording_elapsed_ms, Some(1000));
                assert_eq!(battery.points[1].recording_elapsed_ms, Some(2000));
                assert_eq!(
                    observed(&battery.points[0], "battery_remaining_capacity_mwh")
                        .unwrap()
                        .value,
                    1000.0
                );
                assert!(battery.points.iter().all(|point| !point.clock_consistent));
                assert!(battery.evolution.is_empty());
                assert!(battery
                    .gaps
                    .iter()
                    .any(|gap| gap.reason == "clock_inconsistent"));
            }
        }
    }

    #[test]
    fn native_window_clock_skew_is_rechecked_even_when_the_tick_anchor_matches() {
        for native_query_skew in [false, true] {
            let store = Store::open_in_memory().unwrap();
            let recording = store.begin_recording(30, 1000).unwrap();
            for at in [2000, 3000] {
                let mut inspection = fixture(at, Some(1002.0 - at as f64 / 1000.0), 1);
                if at == 2000 {
                    inspection.window = Window {
                        started_at_unix_ms: 1490,
                        finished_at_unix_ms: 2000,
                        duration_ms: if native_query_skew { 510 } else { 10 },
                    };
                    if native_query_skew {
                        for observation in &mut inspection.observations {
                            observation.window = Window {
                                started_at_unix_ms: 1490,
                                finished_at_unix_ms: 2000,
                                duration_ms: 10,
                            };
                        }
                    }
                }
                store
                    .append_recording_battery_timed(&recording.id, &inspection, at, Some(at - 1000))
                    .unwrap();
            }
            store
                .finish_recording(&recording.id, 3000, false, 2)
                .unwrap();
            let result = store.battery_recording(&recording.id).unwrap().unwrap();
            let battery = &result.batteries[0];
            assert_eq!(battery.points.len(), 2);
            assert!(!battery.points[0].clock_consistent);
            assert!(battery.points[1].clock_consistent);
            assert!(battery.evolution.is_empty());
            assert!(battery
                .gaps
                .iter()
                .any(|gap| gap.reason == "clock_inconsistent"));
        }
    }

    #[test]
    fn missing_legacy_inspection_window_cannot_certify_elapsed_alignment() {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        for at in [1000, 2000] {
            store
                .append_recording_battery_timed(
                    &recording.id,
                    &fixture(at, Some(1001.0 - at as f64 / 1000.0), 1),
                    at,
                    Some(at - 1000),
                )
                .unwrap();
            let text: String = store
                .conn
                .query_row(
                    "SELECT payload FROM recording_battery_samples WHERE recorded_at=?1",
                    params![stored(at)],
                    |row| row.get(0),
                )
                .unwrap();
            let mut payload: Value = serde_json::from_str(&text).unwrap();
            payload.as_object_mut().unwrap().remove("inspection_window");
            store
                .conn
                .execute(
                    "UPDATE recording_battery_samples SET payload=?1 WHERE recorded_at=?2",
                    params![serde_json::to_string(&payload).unwrap(), stored(at)],
                )
                .unwrap();
        }
        store
            .finish_recording(&recording.id, 2000, false, 2)
            .unwrap();
        let result = store.battery_recording(&recording.id).unwrap().unwrap();
        let battery = &result.batteries[0];
        assert_eq!(battery.points.len(), 2);
        assert!(battery
            .points
            .iter()
            .all(|point| point.recording_elapsed_ms.is_some() && !point.clock_consistent));
        assert!(battery.evolution.is_empty());
    }

    #[test]
    fn unknown_critical_bias_preserves_capacity_based_runtime() {
        let recording = Recording {
            id: "local-bias-fixture".into(),
            started_at_unix_ms: 1000,
            ended_at_unix_ms: Some(3000),
            duration_seconds: 30,
            status: "completed".into(),
            samples: 3,
            markers: vec![],
        };
        let frames: Vec<_> = (1..=3)
            .map(|index| {
                let at = index * 1000;
                let mut inspection = fixture(at, Some(1001.0 - index as f64), 1);
                set(&mut inspection, "battery_critical_bias_mwh", Value::Null);
                BatteryFrame::from_inspection(&inspection, at, Some(at - 1000), 1000)
            })
            .collect();
        let result = summarize(&recording, &frames);
        let observations = &result.batteries[0].evolution[0].observations;
        assert_eq!(observations[0].observation.value, 2.0);
        assert_eq!(observations[1].observation.value, -3600.0);
        assert_eq!(observations[2].observation.value, 998.0);
        assert_eq!(observations[2].inputs.len(), 2);
        assert!(observations[2]
            .inputs
            .iter()
            .all(|input| input.metric == "battery_remaining_capacity_mwh"));
    }

    #[test]
    fn sqlite_keeps_capture_order_when_wall_time_repeats_or_moves_backwards() {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        for (at, elapsed, remaining) in
            [(2000, 0, 1000.0), (1500, 1000, 999.0), (1500, 2000, 998.0)]
        {
            store
                .append_recording_battery_timed(
                    &recording.id,
                    &fixture(at, Some(remaining), 1),
                    at,
                    Some(elapsed),
                )
                .unwrap();
        }
        store
            .finish_recording(&recording.id, 2000, false, 3)
            .unwrap();
        let result = store.battery_recording(&recording.id).unwrap().unwrap();
        assert_eq!(result.coverage.recorded_ticks, 3);
        let times: Vec<_> = result.batteries[0]
            .points
            .iter()
            .map(|point| point.recorded_at_unix_ms)
            .collect();
        assert_eq!(times, vec![2000, 1500, 1500]);
        assert!(result.batteries[0].evolution.is_empty());
    }

    #[test]
    fn storage_filters_inventory_and_response_limits_preserve_individual_readings() {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        for index in 0..17 {
            let at = 1000 + index * 1000;
            let mut inspection = fixture(at, Some(1000.0 - index as f64), 1);
            for battery in 2..=8 {
                inspection
                    .observations
                    .extend(fixture(at, Some(2000.0 - index as f64), battery).observations);
            }
            let mut inventory = inspection.observations[0].clone();
            inventory.metric = "battery_serial_number".into();
            inventory.value = Value::from("private-device-serial");
            inspection.observations.push(inventory);
            store
                .append_recording_battery(&recording.id, &inspection, at)
                .unwrap();
        }
        store
            .finish_recording(&recording.id, 17000, false, 17)
            .unwrap();
        let result = store.battery_recording(&recording.id).unwrap().unwrap();
        assert_eq!(result.coverage.points_returned, 128);
        assert_eq!(result.coverage.points_omitted_from_response, 8);
        assert_eq!(result.status, Status::Partial);
        assert!(result
            .batteries
            .iter()
            .flat_map(|battery| &battery.points)
            .all(|point| point.recorded_at_unix_ms >= 2000));
        let stored: String = store
            .conn
            .query_row(
                "SELECT payload FROM recording_battery_samples LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.contains("private-device-serial"));
        assert!(!stored.contains("Alice"));
        let mut many = fixture(1000, Some(1000.0), 1);
        for battery in 2..=17 {
            many.observations
                .extend(fixture(1000, Some(1000.0), battery).observations);
        }
        let frame = BatteryFrame::from_inspection(&many, 1000, Some(0), 1000);
        assert_eq!(frame.points.len(), MAX_RECORDING_BATTERIES);
        assert_eq!(frame.coverage.omitted_batteries, 1);
    }

    #[test]
    fn empty_battery_reads_preserve_denied_and_error_statuses() {
        for expected in [Status::PermissionDenied, Status::Error, Status::Unsupported] {
            let store = Store::open_in_memory().unwrap();
            let recording = store.begin_recording(30, 1000).unwrap();
            let mut inspection = fixture(1000, None, 1);
            inspection.observations.clear();
            inspection.status = expected;
            store
                .append_recording_battery_timed(&recording.id, &inspection, 1000, Some(0))
                .unwrap();
            store
                .finish_recording(&recording.id, 2000, false, 1)
                .unwrap();
            let result = store.battery_recording(&recording.id).unwrap().unwrap();
            assert_eq!(result.status, expected);
            assert!(result.batteries.is_empty());
            assert_eq!(result.coverage.ticks_without_battery, 1);
            assert_eq!(result.coverage.points_returned, 0);
        }
    }

    #[test]
    fn sqlite_migrates_existing_recordings_and_cascades_clear_and_retention() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(crate::samples::MIGRATION).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE activity_events ADD COLUMN peak_observation TEXT;
            ALTER TABLE activity_events ADD COLUMN contributor_observation TEXT;
            PRAGMA user_version=4;
            INSERT INTO recordings(id,started_at,ended_at,duration_seconds,status,samples)
            VALUES('legacy',1000,2000,30,'completed',2);",
            )
            .unwrap();
        let store = Store::init(connection).unwrap();
        let legacy = store.battery_recording("legacy").unwrap().unwrap();
        assert_eq!(legacy.status, Status::Unsupported);
        assert_eq!(legacy.coverage.recorded_ticks, 0);
        assert!(legacy.batteries.is_empty());
        assert_eq!(
            store
                .conn
                .pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            crate::SCHEMA_VERSION
        );
        assert!(store.battery_recording("unknown").unwrap().is_none());
        let mut store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        store
            .append_recording_battery(&recording.id, &fixture(1000, Some(1000.0), 1), 1000)
            .unwrap();
        store
            .finish_recording(&recording.id, 2000, false, 1)
            .unwrap();
        store.maintain_samples(SAMPLE_RETENTION_MS + 2000).unwrap();
        assert!(store.battery_recording(&recording.id).unwrap().is_some());
        assert_eq!(
            store
                .conn
                .query_row::<i64, _, _>(
                    "SELECT COUNT(*) FROM recording_battery_samples",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            1
        );
        let recording = store
            .begin_recording(30, SAMPLE_RETENTION_MS + 3000)
            .unwrap();
        store
            .append_recording_battery(
                &recording.id,
                &fixture(SAMPLE_RETENTION_MS + 3000, Some(1000.0), 1),
                SAMPLE_RETENTION_MS + 3000,
            )
            .unwrap();
        store.clear_samples().unwrap();
        assert!(store.battery_recording(&recording.id).unwrap().is_none());
        assert_eq!(
            store
                .conn
                .query_row::<i64, _, _>(
                    "SELECT COUNT(*) FROM recording_battery_samples",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn write_bounds_unknown_status_and_metric_validation_are_fail_closed() {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        for index in 0..MAX_BATTERY_POINTS as u64 {
            let at = 1000 + index * 100;
            store
                .append_recording_battery(&recording.id, &fixture(at, Some(1000.0), 1), at)
                .unwrap();
        }
        assert!(store
            .append_recording_battery(&recording.id, &fixture(14000, Some(1000.0), 1), 14000)
            .is_err());
        assert!(store
            .append_recording_battery("unknown", &fixture(1000, Some(1000.0), 1), 1000)
            .is_err());
        assert!(store
            .append_recording_battery(&recording.id, &fixture(500, Some(1000.0), 1), 500)
            .is_err());
        let mut malformed = fixture(1000, Some(-10.0), 1);
        malformed.observations[0].entity_ref = "battery-Alice".into();
        let frame = BatteryFrame::from_inspection(&malformed, 1000, Some(0), 1000);
        assert_eq!(frame.coverage.rejected_observations, 2);
        assert!(capacity(&frame.points[&reference(1)]).is_none());
        store
            .finish_recording(&recording.id, 14000, false, 128)
            .unwrap();
        assert!(store
            .append_recording_battery(&recording.id, &fixture(15000, Some(1000.0), 1), 15000)
            .is_err());
    }
}
