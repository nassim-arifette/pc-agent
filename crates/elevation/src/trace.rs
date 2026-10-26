//! Closed ETW aggregates; no paths, addresses, pointers, process IDs or absolute QPC values.
#![cfg_attr(not(windows), allow(dead_code))]
use super::protocol::{ElevationError, ElevationStatus};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, time::Duration};

pub const MAX_TRACE_EVENTS: u64 = 250_000;
pub const TRACE_BUFFER_KIB: u32 = 64;
pub const TRACE_MAX_BUFFERS: u32 = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceClass {
    Dpc,
    Isr,
    DiskRead,
    DiskWrite,
}
impl TraceClass {
    pub(crate) const ALL: [Self; 4] = [Self::Dpc, Self::Isr, Self::DiskRead, Self::DiskWrite];
    fn index(self) -> usize {
        match self {
            Self::Dpc => 0,
            Self::Isr => 1,
            Self::DiskRead => 2,
            Self::DiskWrite => 3,
        }
    }
    fn is_disk(self) -> bool {
        matches!(self, Self::DiskRead | Self::DiskWrite)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLimit {
    CleanupUnvalidated,
    ProviderUnavailable,
    SessionConflict,
    ConsumerUnavailable,
    SchemaUnavailable,
    NoMatchingEvents,
    BoundaryEventsExcluded,
    QpcUnavailable,
    NumericOverflow,
    EventLimit,
    EventsLost,
    BufferLimit,
    ConsumerFailed,
    CleanupFailed,
    Cancelled,
    DurationLimit,
    PlatformUnsupported,
    NativeAccuracyUnvalidated,
    DriverAttributionUnavailable,
    DurationsRoundedDownToMicroseconds,
    ConcurrentRoutinesMayOverlap,
    ConsumerDrainIncomplete,
    LossCountersUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSample {
    pub class: TraceClass,
    pub status: ElevationStatus,
    pub accepted_events: u64,
    pub transfer_bytes: Option<u64>,
    pub total_duration_us: Option<u64>,
    pub maximum_duration_us: Option<u64>,
    pub first_event_offset_us: Option<u64>,
    pub last_event_offset_us: Option<u64>,
    pub schema_errors: u64,
    pub timing_errors: u64,
    pub boundary_events: u64,
}
impl TraceSample {
    fn empty(class: TraceClass) -> Self {
        Self {
            class,
            status: ElevationStatus::Unsupported,
            accepted_events: 0,
            transfer_bytes: None,
            total_duration_us: None,
            maximum_duration_us: None,
            first_event_offset_us: None,
            last_event_offset_us: None,
            schema_errors: 0,
            timing_errors: 0,
            boundary_events: 0,
        }
    }
    fn validate(&self, observed_us: u64) -> Result<(), ElevationError> {
        if self.accepted_events > MAX_TRACE_EVENTS
            || self.schema_errors > MAX_TRACE_EVENTS
            || self.timing_errors > MAX_TRACE_EVENTS
            || self.boundary_events > MAX_TRACE_EVENTS
        {
            return Err(ElevationError::InvalidResponse);
        }
        match (
            self.total_duration_us,
            self.maximum_duration_us,
            self.first_event_offset_us,
            self.last_event_offset_us,
        ) {
            (None, None, None, None)
                if self.accepted_events == 0
                    && self.transfer_bytes.is_none()
                    && self.status == ElevationStatus::Unsupported => {}
            (Some(total), Some(maximum), Some(first), Some(last))
                if self.accepted_events > 0
                    && maximum <= total
                    && maximum <= observed_us
                    && maximum
                        .checked_mul(self.accepted_events)
                        .is_some_and(|bound| total <= bound)
                    && first <= last
                    && last <= observed_us
                    && maximum <= last
                    && (self.accepted_events != 1 || first == last)
                    && self.status == ElevationStatus::Partial
                    && (self.transfer_bytes.is_some() == self.class.is_disk())
                    && self.transfer_bytes.is_none_or(|bytes| {
                        bytes <= self.accepted_events * u64::from(u32::MAX)
                    }) => {}
            _ => return Err(ElevationError::InvalidResponse),
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceSummary {
    pub status: ElevationStatus,
    pub requested_duration_seconds: u8,
    pub observed_duration_us: Option<u64>,
    pub qpc_frequency_hz: Option<u64>,
    pub callback_events: u64,
    pub unrelated_events: u64,
    pub events_lost: Option<u64>,
    pub realtime_buffers_lost: Option<u64>,
    pub session_closed: Option<bool>,
    pub samples: Vec<TraceSample>,
    pub limitations: Vec<TraceLimit>,
}
impl TraceSummary {
    pub(crate) fn unavailable(
        duration: Duration,
        status: ElevationStatus,
        limit: TraceLimit,
    ) -> Self {
        Self {
            status,
            requested_duration_seconds: duration.as_secs() as u8,
            observed_duration_us: None,
            qpc_frequency_hz: None,
            callback_events: 0,
            unrelated_events: 0,
            events_lost: None,
            realtime_buffers_lost: None,
            session_closed: None,
            samples: Vec::new(),
            limitations: vec![limit],
        }
    }
    pub fn validate(&self) -> Result<(), ElevationError> {
        if !matches!(self.requested_duration_seconds, 30 | 60)
            || self.callback_events > MAX_TRACE_EVENTS
            || self.unrelated_events > self.callback_events
            || self.limitations.len() > 16
            || self.samples.len() > 4
            || self.status == ElevationStatus::Completed
            || self
                .limitations
                .iter()
                .enumerate()
                .any(|(index, limit)| self.limitations[..index].contains(limit))
        {
            return Err(ElevationError::InvalidResponse);
        }
        if self.samples.is_empty() {
            if self.observed_duration_us.is_some()
                || self.qpc_frequency_hz.is_some()
                || self.callback_events != 0
                || self.events_lost.is_some()
                || self.realtime_buffers_lost.is_some()
                || self.session_closed.is_some()
                || self.status == ElevationStatus::Partial
                || self.limitations.is_empty()
            {
                return Err(ElevationError::InvalidResponse);
            }
            return Ok(());
        }
        let observed = self
            .observed_duration_us
            .ok_or(ElevationError::InvalidResponse)?;
        if observed > u64::from(self.requested_duration_seconds) * 1_000_000
            || !matches!(
                self.status,
                ElevationStatus::Partial | ElevationStatus::Cancelled
            )
            || self.qpc_frequency_hz.is_none_or(|value| value == 0)
            || self.samples.len() != 4
            || self.session_closed != Some(true)
            || !self
                .limitations
                .contains(&TraceLimit::NativeAccuracyUnvalidated)
            || !self
                .limitations
                .contains(&TraceLimit::DriverAttributionUnavailable)
            || !self
                .limitations
                .contains(&TraceLimit::DurationsRoundedDownToMicroseconds)
            || !self
                .limitations
                .contains(&TraceLimit::ConcurrentRoutinesMayOverlap)
        {
            return Err(ElevationError::InvalidResponse);
        }
        let mut classes = BTreeSet::new();
        let mut accounted = self.unrelated_events;
        for sample in &self.samples {
            sample.validate(observed)?;
            if !classes.insert(sample.class) {
                return Err(ElevationError::InvalidResponse);
            }
            accounted = accounted
                .checked_add(sample.accepted_events)
                .and_then(|v| v.checked_add(sample.schema_errors))
                .and_then(|v| v.checked_add(sample.timing_errors))
                .and_then(|v| v.checked_add(sample.boundary_events))
                .ok_or(ElevationError::InvalidResponse)?;
        }
        if accounted != self.callback_events {
            return Err(ElevationError::InvalidResponse);
        }
        if self.events_lost.is_some() != self.realtime_buffers_lost.is_some()
            || (self.events_lost.is_none()
                && !self
                    .limitations
                    .contains(&TraceLimit::LossCountersUnavailable))
            || (self
                .samples
                .iter()
                .any(|sample| sample.accepted_events == 0)
                && !self.limitations.contains(&TraceLimit::NoMatchingEvents))
            || (self.samples.iter().any(|sample| sample.schema_errors > 0)
                && !self.limitations.contains(&TraceLimit::SchemaUnavailable))
            || (self.samples.iter().any(|sample| sample.boundary_events > 0)
                && !self
                    .limitations
                    .contains(&TraceLimit::BoundaryEventsExcluded))
            || (self.samples.iter().any(|sample| sample.timing_errors > 0)
                && !self.limitations.contains(&TraceLimit::QpcUnavailable)
                && !self.limitations.contains(&TraceLimit::NumericOverflow))
            || (self.status == ElevationStatus::Cancelled
                && !self.limitations.contains(&TraceLimit::Cancelled))
        {
            return Err(ElevationError::InvalidResponse);
        }
        if (self.events_lost.is_some_and(|count| count > 0)
            || self.realtime_buffers_lost.is_some_and(|count| count > 0))
            && !self.limitations.contains(&TraceLimit::EventsLost)
        {
            return Err(ElevationError::InvalidResponse);
        }
        Ok(())
    }
}

pub(crate) fn validate_duration(duration: Duration) -> Result<(), ElevationError> {
    if duration != Duration::from_secs(30) && duration != Duration::from_secs(60) {
        return Err(ElevationError::InvalidArguments);
    }
    Ok(())
}
pub(crate) fn ticks_to_us(ticks: u64, frequency: u64) -> Option<u64> {
    let converted = u128::from(ticks)
        .checked_mul(1_000_000)?
        .checked_div(u128::from(frequency))?;
    u64::try_from(converted).ok()
}

/// Copied allowlisted native numbers only, never a raw event payload or pointer.
pub(crate) struct NumericEvent {
    pub class: TraceClass,
    pub schema_supported: bool,
    pub at_qpc: u64,
    pub initial_qpc: Option<u64>,
    pub response_ticks: Option<u64>,
    pub transfer_bytes: Option<u32>,
}

pub(crate) struct Accumulator {
    duration: Duration,
    start_qpc: u64,
    end_qpc: u64,
    frequency: u64,
    summary: TraceSummary,
}
impl Accumulator {
    pub(crate) fn new(
        duration: Duration,
        start_qpc: u64,
        frequency: u64,
    ) -> Result<Self, ElevationError> {
        validate_duration(duration)?;
        if frequency == 0 {
            return Err(ElevationError::InvalidArguments);
        }
        let span = frequency
            .checked_mul(duration.as_secs())
            .ok_or(ElevationError::InvalidArguments)?;
        let end_qpc = start_qpc
            .checked_add(span)
            .ok_or(ElevationError::InvalidArguments)?;
        let mut summary = TraceSummary::unavailable(
            duration,
            ElevationStatus::Partial,
            TraceLimit::NativeAccuracyUnvalidated,
        );
        summary.limitations.extend([
            TraceLimit::DriverAttributionUnavailable,
            TraceLimit::DurationsRoundedDownToMicroseconds,
            TraceLimit::ConcurrentRoutinesMayOverlap,
        ]);
        summary.qpc_frequency_hz = Some(frequency);
        summary.samples = TraceClass::ALL
            .into_iter()
            .map(TraceSample::empty)
            .collect();
        Ok(Self {
            duration,
            start_qpc,
            end_qpc,
            frequency,
            summary,
        })
    }
    pub(crate) fn limit_reached(&self) -> bool {
        self.summary.callback_events == MAX_TRACE_EVENTS
    }
    fn count_callback(&mut self) -> bool {
        if self.limit_reached() {
            self.limit(TraceLimit::EventLimit);
            false
        } else {
            self.summary.callback_events += 1;
            true
        }
    }
    pub(crate) fn ignored_event(&mut self) {
        if self.count_callback() {
            self.summary.unrelated_events += 1;
        }
    }
    pub(crate) fn limit(&mut self, limit: TraceLimit) {
        if !self.summary.limitations.contains(&limit) {
            self.summary.limitations.push(limit);
        }
    }
    pub(crate) fn observe(&mut self, event: NumericEvent) {
        if !self.count_callback() {
            return;
        }
        let index = event.class.index();
        if !event.schema_supported {
            self.summary.samples[index].schema_errors += 1;
            self.limit(TraceLimit::SchemaUnavailable);
            return;
        }
        if event.at_qpc < self.start_qpc || event.at_qpc > self.end_qpc {
            self.summary.samples[index].boundary_events += 1;
            self.limit(TraceLimit::BoundaryEventsExcluded);
            return;
        }
        let interval = if event.class.is_disk() {
            event
                .response_ticks
                .and_then(|ticks| event.at_qpc.checked_sub(ticks).map(|start| (start, ticks)))
        } else {
            event
                .initial_qpc
                .and_then(|start| event.at_qpc.checked_sub(start).map(|ticks| (start, ticks)))
        };
        let Some((start, ticks)) = interval else {
            self.summary.samples[index].timing_errors += 1;
            self.limit(TraceLimit::QpcUnavailable);
            return;
        };
        if start < self.start_qpc {
            self.summary.samples[index].boundary_events += 1;
            self.limit(TraceLimit::BoundaryEventsExcluded);
            return;
        }
        let Some(duration) = ticks_to_us(ticks, self.frequency) else {
            self.summary.samples[index].timing_errors += 1;
            self.limit(TraceLimit::NumericOverflow);
            return;
        };
        if event.class.is_disk() && event.transfer_bytes.is_none() {
            self.summary.samples[index].schema_errors += 1;
            self.limit(TraceLimit::SchemaUnavailable);
            return;
        }
        let sample = &mut self.summary.samples[index];
        let total = sample.total_duration_us.unwrap_or(0).checked_add(duration);
        let bytes = if event.class.is_disk() {
            sample
                .transfer_bytes
                .unwrap_or(0)
                .checked_add(u64::from(event.transfer_bytes.unwrap()))
        } else {
            Some(0)
        };
        let (Some(total), Some(bytes)) = (total, bytes) else {
            sample.timing_errors += 1;
            self.limit(TraceLimit::NumericOverflow);
            return;
        };
        let offset = ticks_to_us(event.at_qpc - self.start_qpc, self.frequency).unwrap();
        sample.accepted_events += 1;
        sample.status = ElevationStatus::Partial;
        sample.total_duration_us = Some(total);
        sample.maximum_duration_us = Some(sample.maximum_duration_us.unwrap_or(0).max(duration));
        if event.class.is_disk() {
            sample.transfer_bytes = Some(bytes);
        }
        sample.first_event_offset_us = Some(
            sample
                .first_event_offset_us
                .map_or(offset, |first| first.min(offset)),
        );
        sample.last_event_offset_us = Some(
            sample
                .last_event_offset_us
                .map_or(offset, |last| last.max(offset)),
        );
    }
    pub(crate) fn finish(
        mut self,
        end_qpc: u64,
        lost: Option<(u64, u64)>,
        closed: bool,
        terminal: Option<TraceLimit>,
    ) -> TraceSummary {
        let end = end_qpc.clamp(self.start_qpc, self.end_qpc);
        self.summary.observed_duration_us = ticks_to_us(end - self.start_qpc, self.frequency);
        self.summary.session_closed = Some(closed);
        if let Some((events, buffers)) = lost {
            self.summary.events_lost = Some(events);
            self.summary.realtime_buffers_lost = Some(buffers);
            if events != 0 || buffers != 0 {
                self.limit(TraceLimit::EventsLost);
            }
        } else {
            self.limit(TraceLimit::LossCountersUnavailable);
        }
        if self
            .summary
            .samples
            .iter()
            .any(|sample| sample.accepted_events == 0)
        {
            self.limit(TraceLimit::NoMatchingEvents);
        }
        if let Some(limit) = terminal {
            self.limit(limit);
            if limit == TraceLimit::Cancelled {
                self.summary.status = ElevationStatus::Cancelled;
            }
        }
        if !closed {
            // No numeric result is released while successful session cleanup remains unconfirmed.
            return TraceSummary::unavailable(
                self.duration,
                ElevationStatus::Error,
                TraceLimit::CleanupFailed,
            );
        }
        self.summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn dpc(at: u64, start: u64) -> NumericEvent {
        NumericEvent {
            class: TraceClass::Dpc,
            schema_supported: true,
            at_qpc: at,
            initial_qpc: Some(start),
            response_ticks: None,
            transfer_bytes: None,
        }
    }
    #[test]
    fn out_of_order_events_use_real_qpc_intervals_and_exclude_boundaries() {
        let mut reducer = Accumulator::new(Duration::from_secs(30), 100, 1000).unwrap();
        reducer.observe(dpc(3100, 3000));
        reducer.observe(dpc(1100, 1050));
        reducer.observe(dpc(1200, 99));
        reducer.observe(dpc(900, 901));
        reducer.observe(NumericEvent {
            class: TraceClass::DiskRead,
            schema_supported: true,
            at_qpc: 4100,
            initial_qpc: None,
            response_ticks: Some(25),
            transfer_bytes: Some(4096),
        });
        let summary = reducer.finish(30100, Some((0, 0)), true, None);
        summary.validate().unwrap();
        let sample = &summary.samples[0];
        assert_eq!(sample.accepted_events, 2);
        assert_eq!(sample.total_duration_us, Some(150000));
        assert_eq!(sample.maximum_duration_us, Some(100000));
        assert_eq!(sample.first_event_offset_us, Some(1000000));
        assert_eq!(sample.last_event_offset_us, Some(3000000));
        assert_eq!(sample.boundary_events, 1);
        assert_eq!(sample.timing_errors, 1);
        assert_eq!(summary.samples[2].transfer_bytes, Some(4096));
        assert_eq!(summary.samples[1].total_duration_us, None);
    }
    #[test]
    fn loss_schema_and_cleanup_failure_never_claim_complete_coverage() {
        let mut reducer = Accumulator::new(Duration::from_secs(30), 0, 1000).unwrap();
        let mut event = dpc(10, 0);
        event.schema_supported = false;
        reducer.observe(event);
        reducer.ignored_event();
        let summary = reducer.finish(30000, Some((3, 2)), true, None);
        summary.validate().unwrap();
        assert_eq!(summary.samples[0].schema_errors, 1);
        assert!(summary.limitations.contains(&TraceLimit::EventsLost));
        assert_eq!(summary.samples[0].maximum_duration_us, None);
        let reducer = Accumulator::new(Duration::from_secs(30), 0, 1000).unwrap();
        let failed = reducer.finish(30000, None, false, None);
        failed.validate().unwrap();
        assert!(failed.samples.is_empty());
        assert_eq!(failed.observed_duration_us, None);
    }
    #[test]
    fn integer_conversion_event_limit_and_closed_wire_are_bounded() {
        assert_eq!(ticks_to_us(1, 3), Some(333333));
        assert_eq!(ticks_to_us(1, 0), None);
        assert_eq!(ticks_to_us(u64::MAX, 1), None);
        assert!(Accumulator::new(Duration::from_secs(30), u64::MAX, 1).is_err());
        assert!(Accumulator::new(Duration::from_secs(31), 0, 1).is_err());
        let mut reducer = Accumulator::new(Duration::from_secs(30), 0, 1).unwrap();
        for _ in 0..=MAX_TRACE_EVENTS {
            reducer.ignored_event();
        }
        let summary = reducer.finish(30, Some((0, 0)), true, None);
        summary.validate().unwrap();
        assert_eq!(summary.callback_events, MAX_TRACE_EVENTS);
        assert!(summary.limitations.contains(&TraceLimit::EventLimit));
        let mut json = serde_json::to_value(summary).unwrap();
        json["routine_address"] = 123.into();
        assert!(serde_json::from_value::<TraceSummary>(json).is_err());
    }
    #[test]
    fn wire_validation_rejects_invented_totals_counts_and_coverage() {
        let mut reducer = Accumulator::new(Duration::from_secs(30), 0, 1000).unwrap();
        reducer.observe(NumericEvent {
            class: TraceClass::DiskRead,
            schema_supported: true,
            at_qpc: 1000,
            initial_qpc: None,
            response_ticks: Some(1),
            transfer_bytes: Some(4096),
        });
        let summary = reducer.finish(30000, None, true, None);
        summary.validate().unwrap();
        assert_eq!(summary.events_lost, None);
        assert!(summary
            .limitations
            .contains(&TraceLimit::LossCountersUnavailable));
        let mut forged = summary.clone();
        forged.samples[2].total_duration_us = Some(1001);
        assert!(forged.validate().is_err());
        let mut forged = summary.clone();
        forged.samples[2].transfer_bytes = Some(u64::from(u32::MAX) + 1);
        assert!(forged.validate().is_err());
        let mut forged = summary.clone();
        forged.callback_events += 1;
        assert!(forged.validate().is_err());
        let mut forged = summary.clone();
        forged.status = ElevationStatus::Unsupported;
        assert!(forged.validate().is_err());
        let mut forged = summary.clone();
        forged
            .limitations
            .retain(|limit| *limit != TraceLimit::LossCountersUnavailable);
        assert!(forged.validate().is_err());
        let mut forged = summary;
        forged
            .limitations
            .retain(|limit| *limit != TraceLimit::NoMatchingEvents);
        assert!(forged.validate().is_err());
    }
}
