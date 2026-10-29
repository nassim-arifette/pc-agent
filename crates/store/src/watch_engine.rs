//! C7's single-owner evaluator. Only the GUI sampler constructs this engine; durations never
//! depend on SQLite's wall-clock lease or on the timestamps displayed with observations.
use crate::samples::system_observations;
use crate::watches::{Watch, WatchAlert, WatchCloseReason, WatchRule};
use crate::{EventObservation, Store, StoreError, StoredSample};
use pcagent_shared::{new_id, Metric, Origin, Status, VolumeInfo, VolumeKind, Window};
use pcagent_telemetry::ProcessSort;
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WatchPhase {
    Normal,
    Pending,
    Alert,
    Missing,
    Disabled,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct WatchLive {
    pub watch: Watch,
    pub phase: WatchPhase,
    pub reason: Option<String>,
    pub observation: Option<EventObservation>,
    pub lower_bound: bool,
    pub alert_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct WatchEngineStatus {
    pub owns_lease: bool,
    pub rules: Vec<WatchLive>,
    pub unseen_alerts: u64,
    pub error: Option<String>,
}

#[derive(Default)]
struct Evaluation {
    above_since: Option<u64>,
    return_since: Option<u64>,
    return_readings: u64,
    last_tick: Option<u64>,
    last_interval_ms: u64,
    frozen_since: Option<u64>,
}

enum Decision {
    Open,
    Close,
    Hold,
}

impl Evaluation {
    fn freeze(&mut self, frozen: bool, monotonic_ms: u64) {
        if frozen {
            self.frozen_since.get_or_insert(monotonic_ms);
        } else if let Some(start) = self.frozen_since.take() {
            let elapsed = monotonic_ms.saturating_sub(start);
            for clock in [
                &mut self.above_since,
                &mut self.return_since,
                &mut self.last_tick,
            ] {
                if let Some(value) = clock.as_mut() {
                    *value = value.saturating_add(elapsed);
                }
            }
        }
    }

    fn advance(
        &mut self,
        rule: &WatchRule,
        value: Option<u64>,
        open: bool,
        monotonic_ms: u64,
        interval_ms: u64,
    ) -> Decision {
        let gap = self.last_tick.is_some_and(|last| {
            monotonic_ms < last
                || monotonic_ms - last > interval_ms.max(self.last_interval_ms).saturating_mul(3)
        });
        self.last_tick = Some(monotonic_ms);
        self.last_interval_ms = interval_ms;
        if gap || value.is_none() {
            self.above_since = None;
            self.return_since = None;
            self.return_readings = 0;
        }
        let Some(value) = value else {
            return Decision::Hold;
        };
        if open {
            self.above_since = None;
            if rule.returned(value) {
                let since = *self.return_since.get_or_insert(monotonic_ms);
                self.return_readings = self.return_readings.saturating_add(1);
                let returned = if rule.is_volume() {
                    self.return_readings >= 2
                } else {
                    monotonic_ms.saturating_sub(since)
                        >= if rule.is_application() {
                            60_000
                        } else {
                            30_000
                        }
                };
                if returned {
                    self.return_since = None;
                    self.return_readings = 0;
                    return Decision::Close;
                }
            } else {
                self.return_since = None;
                self.return_readings = 0;
            }
        } else if rule.exceeded(value) {
            let since = *self.above_since.get_or_insert(monotonic_ms);
            if monotonic_ms.saturating_sub(since) >= rule.sustained_seconds() * 1000 {
                self.above_since = None;
                return Decision::Open;
            }
        } else {
            self.above_since = None;
        }
        Decision::Hold
    }
}

struct Runtime {
    live: WatchLive,
    evaluation: Evaluation,
}
impl Runtime {
    fn new(watch: Watch) -> Self {
        Self {
            live: WatchLive {
                lower_bound: watch.rule.is_application(),
                phase: if watch.enabled {
                    WatchPhase::Missing
                } else {
                    WatchPhase::Disabled
                },
                reason: watch
                    .enabled
                    .then(|| "Waiting for the first matching sample.".into()),
                watch,
                observation: None,
                alert_id: None,
            },
            evaluation: Evaluation::default(),
        }
    }

    fn tick(
        &mut self,
        store: &Store,
        owner: &str,
        reading: Result<EventObservation, &'static str>,
        monotonic_ms: u64,
        wall_ms: u64,
        interval_ms: u64,
    ) -> Result<Option<WatchAlert>, StoreError> {
        if let Some(id) = &self.live.alert_id {
            if store
                .watch_alert(id)?
                .is_none_or(|alert| alert.ended_at_unix_ms.is_some())
            {
                self.live.alert_id = None;
                self.evaluation = Evaluation::default();
            }
        }
        let value = reading.as_ref().ok().map(|observation| observation.value);
        self.live.reason = reading.as_ref().err().map(|reason| (*reason).into());
        self.live.observation = reading.ok();
        let decision = self.evaluation.advance(
            &self.live.watch.rule,
            value,
            self.live.alert_id.is_some(),
            monotonic_ms,
            interval_ms,
        );
        if let Some(id) = &self.live.alert_id {
            let reason = matches!(decision, Decision::Close).then_some(if self.live.lower_bound {
                WatchCloseReason::NoLongerObservedAbove
            } else {
                WatchCloseReason::Returned
            });
            if !store.progress_watch_alert(
                owner,
                id,
                self.live.observation.as_ref(),
                reason,
                wall_ms,
            )? || reason.is_some()
            {
                self.live.alert_id = None;
            }
        }
        let opened = if matches!(decision, Decision::Open) {
            let alert = store.open_watch_alert(
                owner,
                &self.live.watch,
                self.live.observation.clone().ok_or(StoreError::Corrupt)?,
                wall_ms,
            )?;
            self.live.alert_id = alert.as_ref().map(|a| a.id.clone());
            alert
        } else {
            None
        };
        self.live.phase = if value.is_none() {
            WatchPhase::Missing
        } else if self.live.alert_id.is_some() {
            WatchPhase::Alert
        } else if self.evaluation.above_since.is_some() {
            WatchPhase::Pending
        } else {
            WatchPhase::Normal
        };
        Ok(opened)
    }
}

pub(crate) struct WatchEngine {
    owner: String,
    owns_lease: bool,
    renewed_at: Option<u64>,
    rules: BTreeMap<String, Runtime>,
    latest: Option<StoredSample>,
}

impl WatchEngine {
    pub(crate) fn new() -> Self {
        Self {
            owner: new_id("watch-owner"),
            owns_lease: false,
            renewed_at: None,
            rules: BTreeMap::new(),
            latest: None,
        }
    }

    pub(crate) fn refresh(
        &mut self,
        store: &Store,
        monotonic_ms: u64,
        wall_ms: u64,
        force: bool,
    ) -> Result<(), StoreError> {
        if !force
            && self
                .renewed_at
                .is_some_and(|last| monotonic_ms.saturating_sub(last) < 10_000)
        {
            return Ok(());
        }
        let previously_owned = self.owns_lease;
        self.owns_lease = store.watch_lease(&self.owner, wall_ms)?;
        self.renewed_at = Some(monotonic_ms);
        if self.owns_lease && !previously_owned {
            store.watch_recover(&self.owner)?;
            self.rules.clear();
        }
        let watches = store.watches()?;
        let mut old = std::mem::take(&mut self.rules);
        for watch in watches {
            let mut runtime = match old.remove(&watch.id) {
                Some(mut runtime)
                    if self.owns_lease
                        && runtime.live.watch.rule == watch.rule
                        && runtime.live.watch.enabled == watch.enabled
                        && runtime.live.watch.updated_at_unix_ms == watch.updated_at_unix_ms =>
                {
                    runtime.live.watch = watch;
                    runtime
                }
                _ => {
                    let mut runtime = Runtime::new(watch);
                    if self.owns_lease && runtime.live.watch.enabled {
                        runtime.live.alert_id =
                            store.active_watch_alert(&runtime.live.watch.id, &self.owner)?;
                    }
                    runtime
                }
            };
            if runtime.live.watch.enabled && !self.owns_lease {
                runtime.live.phase = WatchPhase::Paused;
                runtime.live.reason = Some("Another PC Agent window is evaluating watches.".into());
            }
            self.rules.insert(runtime.live.watch.id.clone(), runtime);
        }
        Ok(())
    }

    pub(crate) fn recording_changed(&mut self, recording: bool, monotonic_ms: u64) {
        for runtime in self.rules.values_mut() {
            if matches!(runtime.live.watch.rule, WatchRule::AppMemoryAbove { .. }) {
                runtime.evaluation.freeze(recording, monotonic_ms);
            }
        }
    }

    pub(crate) fn sample(
        &mut self,
        store: &Store,
        sample: StoredSample,
        sort: ProcessSort,
        recording: bool,
        monotonic_ms: u64,
    ) -> Result<Vec<WatchAlert>, StoreError> {
        let wall_ms = sample.measured_at_unix_ms;
        self.latest = Some(sample);
        self.refresh(store, monotonic_ms, wall_ms, false)?;
        self.recording_changed(recording, monotonic_ms);
        if !self.owns_lease {
            return Ok(vec![]);
        }
        // A wall-clock jump invalidates the SQL lease without changing any monotonic durations.
        if !store.watch_lease_valid(&self.owner, wall_ms)? {
            self.refresh(store, monotonic_ms, wall_ms, true)?;
            if !self.owns_lease {
                return Ok(vec![]);
            }
        }
        let sample = self.latest.as_ref().ok_or(StoreError::Corrupt)?;
        let system = system_observations(sample);
        let mut opened = Vec::new();
        for runtime in self
            .rules
            .values_mut()
            .filter(|r| r.live.watch.enabled && !r.live.watch.rule.is_volume())
        {
            let rule = &runtime.live.watch.rule;
            if matches!(rule, WatchRule::AppMemoryAbove { .. }) && recording {
                runtime.live.phase = WatchPhase::Paused;
                runtime.live.reason =
                    Some("Application memory evaluation is frozen while recording.".into());
                continue;
            }
            if matches!(rule, WatchRule::AppCpuAbove { .. }) && sort != ProcessSort::Cpu
                || matches!(rule, WatchRule::AppMemoryAbove { .. })
                    && sort != ProcessSort::WorkingSet
            {
                continue;
            }
            let interval = if rule.is_application() && !recording {
                10_000
            } else {
                sample.interval_ms
            };
            let reading = match rule {
                WatchRule::ResourceAbove { resource, .. } => {
                    let index = match resource {
                        crate::watches::WatchResource::Cpu => 0,
                        crate::watches::WatchResource::Memory => 1,
                        crate::watches::WatchResource::DiskBusy => 2,
                    };
                    system[index]
                        .clone()
                        .ok_or("The system counter is unavailable in this sample.")
                }
                WatchRule::GpuTemperatureAbove { nvml_index, .. } => sample
                    .values
                    .iter()
                    .find(|v| {
                        v.metric == Metric::GpuTemperatureCelsius
                            && v.entity_ref == format!("gpu-nvidia-{nvml_index}")
                    })
                    .and_then(|v| EventObservation::measured(v, sample.origin))
                    .ok_or("NVIDIA temperature is unavailable in this sample."),
                WatchRule::AppCpuAbove { executable, .. } => {
                    app_observation(sample, executable, Metric::ProcessCpuBasisPoints)
                }
                WatchRule::AppMemoryAbove { executable, .. } => {
                    app_observation(sample, executable, Metric::ProcessWorkingSetBytes)
                }
                WatchRule::VolumeFreeBelow { .. } => unreachable!(),
            };
            if let Some(alert) =
                runtime.tick(store, &self.owner, reading, monotonic_ms, wall_ms, interval)?
            {
                opened.push(alert);
            }
        }
        Ok(opened)
    }

    pub(crate) fn wants_volumes(&self) -> bool {
        self.owns_lease
            && self
                .rules
                .values()
                .any(|r| r.live.watch.enabled && r.live.watch.rule.is_volume())
    }

    pub(crate) fn volumes(
        &mut self,
        store: &Store,
        volumes: &[VolumeInfo],
        monotonic_ms: u64,
        wall_ms: u64,
    ) -> Result<Vec<WatchAlert>, StoreError> {
        if !self.owns_lease {
            return Ok(vec![]);
        }
        let mut opened = Vec::new();
        for runtime in self.rules.values_mut().filter(|r| r.live.watch.enabled) {
            let WatchRule::VolumeFreeBelow { root, .. } = &runtime.live.watch.rule else {
                continue;
            };
            let reading = volumes
                .iter()
                .find(|v| v.root.eq_ignore_ascii_case(root) && v.kind == VolumeKind::Fixed)
                .and_then(|v| {
                    v.free_bytes.map(|free| EventObservation {
                        entity_ref: v.root.clone(),
                        metric: "volume_free_bytes".into(),
                        value: free,
                        unit: "bytes".into(),
                        source: v.source.clone(),
                        status: v.status,
                        origin: Origin::Live,
                        measured_at_unix_ms: Some(wall_ms),
                        window: Some(Window {
                            started_at_unix_ms: wall_ms,
                            finished_at_unix_ms: wall_ms,
                            duration_ms: 0,
                        }),
                        derivation: None,
                        inputs: vec![],
                    })
                })
                .ok_or("The fixed volume is absent or its free space could not be read.");
            if let Some(alert) =
                runtime.tick(store, &self.owner, reading, monotonic_ms, wall_ms, 60_000)?
            {
                opened.push(alert);
            }
        }
        Ok(opened)
    }

    pub(crate) fn validate_target(
        &self,
        rule: &WatchRule,
        volumes: &[VolumeInfo],
    ) -> Result<(), StoreError> {
        rule.validate()?;
        let valid = match rule {
            WatchRule::VolumeFreeBelow {
                root,
                threshold_bytes,
            } => volumes.iter().any(|v| {
                v.root.eq_ignore_ascii_case(root)
                    && v.kind == VolumeKind::Fixed
                    && v.free_bytes.is_some()
                    && v.total_bytes.is_some_and(|total| *threshold_bytes <= total)
            }),
            WatchRule::ResourceAbove { .. } => true,
            WatchRule::GpuTemperatureAbove { nvml_index, .. } => {
                self.latest.as_ref().is_some_and(|sample| {
                    sample.values.iter().any(|v| {
                        v.metric == Metric::GpuTemperatureCelsius
                            && v.entity_ref == format!("gpu-nvidia-{nvml_index}")
                            && v.value.is_some()
                    })
                })
            }
            WatchRule::AppCpuAbove { executable, .. }
            | WatchRule::AppMemoryAbove { executable, .. } => {
                self.latest.as_ref().is_some_and(|sample| {
                    sample
                        .process_names
                        .values()
                        .any(|name| name.to_lowercase() == executable.to_lowercase())
                })
            }
        };
        if valid {
            Ok(())
        } else {
            Err(StoreError::InvalidQuery)
        }
    }

    pub(crate) fn status(&self, store: &Store) -> Result<WatchEngineStatus, StoreError> {
        Ok(WatchEngineStatus {
            owns_lease: self.owns_lease,
            rules: self.rules.values().map(|r| r.live.clone()).collect(),
            unseen_alerts: store.watch_unseen()?,
            error: None,
        })
    }

    pub(crate) fn stop(&self, store: &Store) -> Result<(), StoreError> {
        store.watch_release(&self.owner)
    }
}

fn app_observation(
    sample: &StoredSample,
    executable: &str,
    metric: Metric,
) -> Result<EventObservation, &'static str> {
    let name = executable.to_lowercase();
    let refs: Vec<_> = sample
        .process_names
        .iter()
        .filter(|(_, candidate)| candidate.to_lowercase() == name)
        .map(|(entity, _)| entity)
        .collect();
    if refs.is_empty() {
        return Err("No exact executable basename was observed in the sorted top 20.");
    }
    let inputs: Vec<_> = refs
        .iter()
        .filter_map(|entity| {
            sample
                .values
                .iter()
                .find(|v| &v.entity_ref == *entity && v.metric == metric)
                .and_then(|v| EventObservation::measured(v, sample.origin))
        })
        .collect();
    if inputs.is_empty() {
        return Err("Matching processes have no usable measurement.");
    }
    let value = inputs
        .iter()
        .try_fold(0_u64, |sum, input| sum.checked_add(input.value))
        .ok_or("The observed process sum is out of range.")?;
    let mut observation = EventObservation::derived(
        if metric == Metric::ProcessCpuBasisPoints { "app_cpu_basis_points" } else { "app_working_set_bytes" },
        value, "Sum of available readings for exact case-insensitive executable basenames in the sorted top 20; lower bound, not all application children.", inputs,
    );
    observation.unit = metric.unit().into();
    observation.status = Status::Partial;
    // This is a rule-local aggregate, never a transient process identity or a personal name.
    observation.entity_ref = "watched-application".into();
    Ok(observation)
}

#[cfg(test)]
mod tests;
