//! One background collector owns the five-second history and temporary one-second recordings.
//! Control messages never enter the provider context. Dropping the last handle stops the worker.
use crate::{HistoryQuery, HistorySeries, Recording, Store, StoreError, StoredSample};
use pcagent_shared::unix_ms;
use pcagent_telemetry::{current_resource_usage, Collector, ProcessSort};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Default, Serialize)]
pub struct SamplerStatus {
    pub running: bool,
    pub samples: u64,
    pub last_sample_unix_ms: Option<u64>,
    pub recording: Option<Recording>,
    pub error: Option<String>,
    /// CPU time of the collector thread divided by elapsed wall time (one logical CPU).
    pub thread_cpu_basis_points: Option<u64>,
    /// The same measured thread CPU time as a share of the machine's logical CPU capacity.
    pub total_cpu_basis_points: Option<u64>,
    pub logical_processors: Option<u32>,
    pub resource_window_ms: u64,
    /// Whole host-process working set; not incremental sampler allocation.
    pub host_working_set_bytes: Option<u64>,
    /// Target relative to total machine capacity, matching process CPU observations.
    pub cpu_target_basis_points: u64,
    pub memory_target_bytes: u64,
}

enum Command {
    Record(
        u64,
        Arc<AtomicBool>,
        mpsc::Sender<Result<Recording, StoreError>>,
    ),
    Cancel(String),
    Clear(mpsc::Sender<Result<usize, StoreError>>),
    Shutdown,
}
struct Inner {
    path: PathBuf,
    tx: mpsc::Sender<Command>,
    status: Arc<Mutex<SamplerStatus>>,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Ok(worker) = self.worker.get_mut() {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}
#[derive(Clone)]
pub struct Sampler(Arc<Inner>);

impl Sampler {
    /// Expired orphan recordings are recovered on startup, maintenance and recording requests.
    /// A recent row is protected for 180 seconds from its start because another CLI/GUI sampler
    /// may still own it; a new recording request retries recovery once that grace period ends.
    pub fn start(path: &Path) -> Result<Self, StoreError> {
        // Validate the database before reporting a running sampler to the caller.
        let store = Store::open(path)?;
        store.interrupt_old_recordings(unix_ms())?;
        let (tx, rx) = mpsc::channel();
        let status = Arc::new(Mutex::new(SamplerStatus {
            running: true,
            cpu_target_basis_points: 50,
            memory_target_bytes: 50 * 1024 * 1024,
            ..SamplerStatus::default()
        }));
        let worker_status = status.clone();
        let worker = thread::Builder::new()
            .name("pcagent-sampler".into())
            .spawn(move || {
                run(store, rx, &worker_status);
            })
            .map_err(|e| StoreError::Database(e.to_string()))?;
        Ok(Self(Arc::new(Inner {
            path: path.to_owned(),
            tx,
            status,
            worker: Mutex::new(Some(worker)),
        })))
    }
    pub fn status(&self) -> SamplerStatus {
        self.0.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
    pub fn stop(&self) {
        let _ = self.0.tx.send(Command::Shutdown);
        if let Ok(mut slot) = self.0.worker.lock() {
            if let Some(worker) = slot.take() {
                let _ = worker.join();
            }
        }
    }
    pub fn history(&self, query: &HistoryQuery) -> Result<HistorySeries, StoreError> {
        Store::open(&self.0.path)?.metric_history(query)
    }
    pub fn process_peaks(
        &self,
        from_unix_ms: u64,
        to_unix_ms: u64,
        limit: usize,
    ) -> Result<crate::ProcessPeaks, StoreError> {
        Store::open(&self.0.path)?.process_peaks(from_unix_ms, to_unix_ms, limit)
    }
    pub fn process_resource_peaks(
        &self,
        from_unix_ms: u64,
        to_unix_ms: u64,
        limit: usize,
    ) -> Result<crate::ProcessPeaks, StoreError> {
        Store::open(&self.0.path)?.process_resource_peaks(from_unix_ms, to_unix_ms, limit)
    }
    pub fn start_recording(&self, seconds: u64) -> Result<Recording, StoreError> {
        if !(30..=120).contains(&seconds) {
            return Err(StoreError::InvalidQuery);
        }
        let (tx, rx) = mpsc::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        self.0
            .tx
            .send(Command::Record(seconds, abandoned.clone(), tx))
            .map_err(|_| stopped())?;
        match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(result) => result,
            Err(_) => {
                abandoned.store(true, Ordering::Release);
                Err(stopped())
            }
        }
    }
    pub fn cancel_recording(&self, id: &str) -> Result<(), StoreError> {
        self.0
            .tx
            .send(Command::Cancel(id.into()))
            .map_err(|_| stopped())
    }
    pub fn recording(&self, id: &str) -> Result<Option<Recording>, StoreError> {
        Store::open(&self.0.path)?.recording(id)
    }
    pub fn battery_recording(
        &self,
        id: &str,
    ) -> Result<Option<crate::BatteryRecording>, StoreError> {
        Store::open(&self.0.path)?.battery_recording(id)
    }
    /// Wait for a bounded recording while allowing the caller's existing stop flag to cancel it.
    pub fn record(&self, seconds: u64, stop: &AtomicBool) -> Result<Recording, StoreError> {
        if stop.load(Ordering::Acquire) {
            return Err(StoreError::InvalidQuery);
        }
        let started = self.start_recording(seconds)?;
        let deadline = Instant::now() + Duration::from_secs(seconds + 15);
        let mut cancel_sent = false;
        loop {
            if stop.load(Ordering::Acquire) && !cancel_sent {
                self.cancel_recording(&started.id)?;
                cancel_sent = true;
            }
            let status = self.status();
            if let Some(recording) = poll_recording(&status, &started.id, |id| self.recording(id))?
            {
                return Ok(recording);
            }
            if Instant::now() >= deadline {
                let _ = self.cancel_recording(&started.id);
                return self
                    .recording(&started.id)?
                    .filter(|r| r.status != "recording")
                    .ok_or_else(stopped);
            }
            if !status.running {
                return Err(stopped());
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    pub fn clear(&self) -> Result<usize, StoreError> {
        let (tx, rx) = mpsc::channel();
        self.0.tx.send(Command::Clear(tx)).map_err(|_| stopped())?;
        rx.recv_timeout(Duration::from_secs(15))
            .map_err(|_| stopped())?
    }
}
fn stopped() -> StoreError {
    StoreError::Database("The local sampler did not respond".into())
}

fn poll_recording(
    status: &SamplerStatus,
    id: &str,
    read: impl FnOnce(&str) -> Result<Option<Recording>, StoreError>,
) -> Result<Option<Recording>, StoreError> {
    let recording = match status
        .recording
        .as_ref()
        .filter(|recording| recording.id == id)
    {
        Some(recording) => Some(recording.clone()),
        None => read(id)?,
    }
    .ok_or_else(stopped)?;
    Ok((recording.status != "recording").then_some(recording))
}
fn update(status: &Mutex<SamplerStatus>, f: impl FnOnce(&mut SamplerStatus)) {
    if let Ok(mut status) = status.lock() {
        f(&mut status);
    }
}
fn finish(
    store: &Store,
    active: &mut Option<(Recording, Instant, u64)>,
    cancelled: bool,
    status: &Mutex<SamplerStatus>,
) {
    if let Some((recording, _, samples)) = active.take() {
        match store
            .finish_recording(&recording.id, unix_ms(), cancelled, samples)
            .and_then(|()| store.recording(&recording.id))
        {
            Ok(recording) => update(status, |s| s.recording = recording),
            Err(_) => update(status, |s| {
                s.error = Some("Could not finalize the local recording".into())
            }),
        }
    }
}
fn run(mut store: Store, rx: mpsc::Receiver<Command>, status: &Mutex<SamplerStatus>) {
    let mut collector = Collector::new();
    let mut active: Option<(Recording, Instant, u64)> = None;
    let mut next_sample = Instant::now();
    let mut next_maintenance = Instant::now();
    let started = Instant::now();
    let start_cpu = current_resource_usage().thread_cpu_100ns;
    loop {
        if active
            .as_ref()
            .is_some_and(|(_, end, _)| Instant::now() >= *end)
        {
            finish(&store, &mut active, false, status);
        }
        let wait = next_sample
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        match rx.recv_timeout(wait) {
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Record(seconds, abandoned, reply)) => {
                if abandoned.load(Ordering::Acquire) {
                    let _ = reply.send(Err(stopped()));
                    continue;
                }
                // Pair the persisted UTC epoch with the recording's actual Instant anchor
                // before database work, so its delay cannot masquerade as clock skew.
                let recording_started = Instant::now();
                let now = unix_ms();
                let result = store
                    .interrupt_old_recordings(now)
                    .and_then(|_| store.begin_recording(seconds, now));
                let began = result.is_ok();
                if let Ok(recording) = &result {
                    collector.reset_recording_gpu();
                    active = Some((
                        recording.clone(),
                        recording_started + Duration::from_secs(seconds),
                        0,
                    ));
                    update(status, |s| s.recording = Some(recording.clone()));
                    next_sample = Instant::now();
                }
                if reply.send(result).is_err() && began {
                    finish(&store, &mut active, true, status);
                }
            }
            Ok(Command::Cancel(id)) => {
                if active
                    .as_ref()
                    .is_some_and(|(recording, _, _)| recording.id == id)
                {
                    finish(&store, &mut active, true, status);
                }
            }
            Ok(Command::Clear(reply)) => {
                finish(&store, &mut active, true, status);
                let result = store.clear_samples();
                update(status, |s| {
                    s.recording = None;
                    s.samples = 0;
                    s.last_sample_unix_ms = None;
                });
                let _ = reply.send(result);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if Instant::now() < next_sample {
            continue;
        }
        let interval_ms = if active.is_some() { 1000 } else { 5000 };
        let sort = if active.is_some() || status.lock().is_ok_and(|s| s.samples % 2 == 0) {
            ProcessSort::Cpu
        } else {
            ProcessSort::WorkingSet
        };
        let snapshot = if active.is_some() {
            collector.sample_recording_processes(20, sort)
        } else {
            collector.sample_processes(20, sort)
        };
        let now = unix_ms();
        let sample = StoredSample::from_snapshot(&snapshot, now, interval_ms);
        let battery = active.as_ref().map(|(recording, end, _)| {
            let inspection = pcagent_inspect::inspect(pcagent_inspect::Query::Battery);
            let started = *end - Duration::from_secs(recording.duration_seconds);
            let elapsed = Instant::now()
                .saturating_duration_since(started)
                .as_millis() as u64;
            (recording.id.clone(), inspection, unix_ms(), elapsed)
        });
        match store.append_sample(&sample) {
            Ok(_) => {
                update(status, |s| {
                    s.samples += 1;
                    s.last_sample_unix_ms = Some(now);
                    s.error = None;
                });
                if let Some((recording, _, count)) = &mut active {
                    *count += 1;
                    recording.samples = *count;
                    if store.recording_progress(&recording.id, *count).is_err() {
                        update(status, |s| {
                            s.error = Some("Could not persist recording progress".into())
                        });
                    }
                    update(status, |s| s.recording = Some(recording.clone()));
                }
            }
            Err(_) => update(status, |s| {
                s.error = Some("Could not persist a local sample".into())
            }),
        }
        if let Some((id, inspection, at, elapsed)) = battery {
            if store
                .append_recording_battery_timed(&id, &inspection, at, Some(elapsed))
                .is_err()
            {
                update(status, |s| {
                    s.error = Some("Could not persist a recording battery sample".into())
                });
            }
        }
        if Instant::now() >= next_maintenance {
            if store
                .interrupt_old_recordings(now)
                .and_then(|_| store.maintain_samples(now))
                .is_err()
            {
                update(status, |s| {
                    s.error = Some("History retention could not run".into())
                });
            }
            next_maintenance = Instant::now() + Duration::from_secs(60);
        }
        let usage = current_resource_usage();
        let elapsed = started.elapsed().as_nanos() / 100;
        let cpu = start_cpu
            .zip(usage.thread_cpu_100ns)
            .filter(|_| elapsed > 0)
            .map(|(start, end)| {
                (u128::from(end.saturating_sub(start)) * 10_000 / elapsed).min(u128::from(u64::MAX))
                    as u64
            });
        update(status, |s| {
            s.thread_cpu_basis_points = cpu;
            s.total_cpu_basis_points = cpu
                .zip(usage.logical_processors)
                .filter(|(_, n)| *n > 0)
                .map(|(cpu, n)| cpu / u64::from(n));
            s.logical_processors = usage.logical_processors;
            s.resource_window_ms = started.elapsed().as_millis() as u64;
            s.host_working_set_bytes = usage.process_working_set_bytes;
        });
        next_sample += Duration::from_millis(interval_ms);
        if next_sample < Instant::now() {
            next_sample = Instant::now() + Duration::from_millis(interval_ms);
        }
    }
    finish(&store, &mut active, true, status);
    update(status, |s| s.running = false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_waiter_recovers_a_replaced_slot_and_detects_cleared_history() {
        let store = Store::open_in_memory().unwrap();
        let first = store.begin_recording(120, 1000).unwrap();
        let mut status = SamplerStatus {
            running: true,
            recording: Some(first.clone()),
            ..SamplerStatus::default()
        };
        assert!(
            poll_recording(&status, &first.id, |_| panic!("active slot is sufficient"))
                .unwrap()
                .is_none()
        );
        status.recording = None;
        assert!(poll_recording(&status, &first.id, |id| store.recording(id))
            .unwrap()
            .is_none());
        store.finish_recording(&first.id, 2000, true, 1).unwrap();
        status.recording = Some(store.begin_recording(120, 3000).unwrap());
        let finished = poll_recording(&status, &first.id, |id| store.recording(id))
            .unwrap()
            .unwrap();
        assert_eq!(finished.status, "cancelled");
        assert_eq!(finished.samples, 1);
        store.clear_samples().unwrap();
        status.recording = None;
        assert!(poll_recording(&status, &first.id, |id| store.recording(id)).is_err());
    }

    #[test]
    fn timed_out_queued_recording_never_starts_later() {
        let (tx, rx) = mpsc::channel();
        let (reply, result) = mpsc::channel();
        tx.send(Command::Record(30, Arc::new(AtomicBool::new(true)), reply))
            .unwrap();
        tx.send(Command::Shutdown).unwrap();
        let status = Mutex::new(SamplerStatus::default());
        run(Store::open_in_memory().unwrap(), rx, &status);
        assert!(result.recv().unwrap().is_err());
        assert!(status.lock().unwrap().recording.is_none());
    }
    #[test]
    fn sampler_is_joined_and_cancellation_preserves_recording_markers() {
        let dir = std::env::temp_dir().join(pcagent_shared::new_id("sampler-test"));
        let path = dir.join("history.sqlite");
        let sampler = Sampler::start(&path).unwrap();
        let recording = sampler.start_recording(30).unwrap();
        assert!(sampler.start_recording(30).is_err());
        sampler.cancel_recording(&recording.id).unwrap();
        drop(sampler);
        let store = Store::open(&path).unwrap();
        let ended = store.recording(&recording.id).unwrap().unwrap();
        assert_eq!(ended.status, "cancelled");
        assert_eq!(ended.markers.last().unwrap().kind, "cancelled");
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
