//! One local, reviewed elevated inspection at a time. The model cannot reach this slot.
use pcagent_inspect::elevation::{ElevatedTask, PreparedElevation};
use pcagent_shared::new_id;
use pcagent_storage::CancelToken;
use serde::Serialize;
use std::{
    sync::{Arc, Condvar, Mutex, Weak},
    time::{Duration, Instant},
};

// These private adapters keep the slot's lifecycle testable without native job files or UAC.
trait PendingTask: Send {
    fn budget(&self) -> Duration;
    fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, String>;
}
trait ApprovedTask: Send {
    fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
        None
    }
    fn execute(
        self: Box<Self>,
        cancel: &CancelToken,
    ) -> Result<pcagent_inspect::Inspection, String>;
}
trait TaskCompletion: Send {
    fn on_complete(self: Box<Self>, finished: Box<dyn FnOnce() + Send>);
}
struct NativeCompletion(pcagent_inspect::elevation::ElevationCompletion);
impl TaskCompletion for NativeCompletion {
    fn on_complete(self: Box<Self>, finished: Box<dyn FnOnce() + Send>) {
        self.0.on_complete(finished);
    }
}
struct NativePending(PreparedElevation);
struct NativeApproved {
    prepared: PreparedElevation,
    approval: pcagent_inspect::elevation::ElevationApproval,
}
impl PendingTask for NativePending {
    fn budget(&self) -> Duration {
        self.0.request().budget()
    }
    fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, String> {
        let mut prepared = self.0;
        let approval = prepared
            .approve_locally()
            .map_err(|error| error.to_string())?;
        Ok(Box::new(NativeApproved { prepared, approval }))
    }
}
impl ApprovedTask for NativeApproved {
    fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
        Some(Box::new(NativeCompletion(self.prepared.completion())))
    }
    fn execute(
        self: Box<Self>,
        cancel: &CancelToken,
    ) -> Result<pcagent_inspect::Inspection, String> {
        let response = self
            .prepared
            .run_locally_bounded(self.approval, cancel)
            .map_err(|error| error.to_string())?;
        Ok(pcagent_inspect::elevation::to_inspection(&response))
    }
}

#[derive(Serialize)]
pub struct PreparedView {
    pub request_id: String,
    pub task: ElevatedTask,
    pub budget_ms: u64,
    pub state: &'static str,
}

enum Job {
    Prepared {
        id: String,
        task: ElevatedTask,
        prepared: Box<dyn PendingTask>,
    },
    Running {
        id: String,
        task: ElevatedTask,
        budget_ms: u64,
        cancel: CancelToken,
    },
}

#[derive(Default)]
struct SlotState {
    job: Option<Job>,
    closing: bool,
    shutdown_waiter: bool,
}

#[derive(Default)]
pub struct Slot {
    state: Mutex<SlotState>,
    idle: Condvar,
}

/// One bounded shutdown attempt; dropping it permits a retry, while the slot stays closed.
pub struct Shutdown {
    slot: Arc<Slot>,
}

pub struct Run {
    id: String,
    approved: Option<Box<dyn ApprovedTask>>,
    completion: Option<Box<dyn TaskCompletion>>,
    cancel: CancelToken,
    slot: Weak<Slot>,
}

impl Slot {
    pub fn prepare(&self, task: ElevatedTask, budget: Duration) -> Result<PreparedView, String> {
        self.prepare_with(task, || {
            PreparedElevation::prepare(task, budget)
                .map(|prepared| Box::new(NativePending(prepared)) as Box<dyn PendingTask>)
                .map_err(|error| error.to_string())
        })
    }

    fn prepare_with(
        &self,
        task: ElevatedTask,
        prepare: impl FnOnce() -> Result<Box<dyn PendingTask>, String>,
    ) -> Result<PreparedView, String> {
        if matches!(task, ElevatedTask::MftInventory { .. }) {
            return Err(
                "MFT allocation accuracy has not been validated on a controlled volume.".into(),
            );
        }
        let mut slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        if slot.closing {
            return Err("PC Agent is shutting down.".into());
        }
        if slot.job.is_some() {
            return Err("An elevated inspection is already prepared or running.".into());
        }
        let prepared = prepare()?;
        let id = new_id("elevation");
        let view = PreparedView {
            request_id: id.clone(),
            task,
            budget_ms: prepared.budget().as_millis() as u64,
            state: "awaiting_local_approval",
        };
        slot.job = Some(Job::Prepared { id, task, prepared });
        Ok(view)
    }

    /// Recover local review/progress after a webview reload without issuing another approval.
    pub fn status(&self) -> Result<Option<PreparedView>, String> {
        let slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        Ok(slot.job.as_ref().map(|job| match job {
            Job::Prepared { id, task, prepared } => PreparedView {
                request_id: id.clone(),
                task: *task,
                budget_ms: prepared.budget().as_millis() as u64,
                state: "awaiting_local_approval",
            },
            Job::Running {
                id,
                task,
                budget_ms,
                cancel,
            } => PreparedView {
                request_id: id.clone(),
                task: *task,
                budget_ms: *budget_ms,
                state: if cancel.is_cancelled() {
                    "cancelling"
                } else {
                    "running"
                },
            },
        }))
    }

    /// Called only by the window's local approval command, after reviewing PreparedView.
    pub fn begin(self: &Arc<Self>, request_id: &str) -> Result<Run, String> {
        let mut slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        if slot.closing {
            return Err("PC Agent is shutting down.".into());
        }
        if !matches!(slot.job.as_ref(), Some(Job::Prepared { id, .. }) if id == request_id) {
            return Err("This elevated inspection is no longer awaiting approval.".into());
        }
        let Some(Job::Prepared { id, task, prepared }) = slot.job.take() else {
            unreachable!()
        };
        let budget_ms = prepared.budget().as_millis() as u64;
        let approved = prepared.approve()?;
        let completion = approved.completion();
        let cancel = CancelToken::default();
        slot.job = Some(Job::Running {
            id: id.clone(),
            task,
            budget_ms,
            cancel: cancel.clone(),
        });
        Ok(Run {
            id,
            approved: Some(approved),
            completion,
            cancel,
            slot: Arc::downgrade(self),
        })
    }

    pub fn finish(&self, request_id: &str) {
        let mut slot = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(slot.job.as_ref(), Some(Job::Running { id, .. }) if id == request_id) {
            slot.job = None;
            self.idle.notify_all();
        }
    }

    pub fn cancel(&self, request_id: &str) -> Result<(), String> {
        let mut slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        match slot.job.as_ref() {
            Some(Job::Running { id, cancel, .. }) if id == request_id => cancel.cancel(),
            Some(Job::Prepared { id, .. }) if id == request_id => slot.job = None,
            _ => return Err("This elevated inspection is no longer available.".into()),
        }
        Ok(())
    }

    /// Close admission and signal cancellation before the app's event loop can exit.
    pub fn begin_shutdown(self: &Arc<Self>) -> Option<Shutdown> {
        let mut slot = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if slot.shutdown_waiter {
            return None;
        }
        slot.closing = true;
        slot.shutdown_waiter = true;
        if let Some(Job::Running { cancel, .. }) = &slot.job {
            cancel.cancel();
        }
        Some(Shutdown { slot: self.clone() })
    }
}

impl Shutdown {
    /// Called on a blocking worker. A timeout keeps the host alive and allows another attempt.
    pub fn wait(self, timeout: Duration) -> bool {
        let started = Instant::now();
        let prepared = {
            let mut state = self.slot.state.lock().unwrap_or_else(|p| p.into_inner());
            if matches!(state.job.as_ref(), Some(Job::Prepared { .. })) {
                state.job.take()
            } else {
                None
            }
        };
        // Finish private-file cleanup outside the state lock before permitting process exit.
        drop(prepared);
        let mut state = self.slot.state.lock().unwrap_or_else(|p| p.into_inner());
        while state.job.is_some() {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                drop(state);
                return false;
            }
            state = self
                .slot
                .idle
                .wait_timeout(state, remaining)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        drop(state);
        true
    }
}

impl Drop for Shutdown {
    fn drop(&mut self) {
        self.slot
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .shutdown_waiter = false;
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        // Shutdown cannot leave the separately running helper without its cancellation signal.
        let slot = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(Job::Running { cancel, .. }) = slot.job.take() {
            cancel.cancel();
        }
    }
}

impl Run {
    pub fn execute(mut self) -> Result<pcagent_inspect::Inspection, String> {
        let approved = self
            .approved
            .take()
            .ok_or_else(|| "This elevated inspection was already consumed.".to_owned())?;
        approved.execute(&self.cancel)
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancel.cancel();
        // An operation result can precede native cleanup or even resolution of Windows consent.
        drop(self.approved.take());
        let slot = self.slot.clone();
        let id = self.id.clone();
        let finished = Box::new(move || {
            if let Some(slot) = slot.upgrade() {
                slot.finish(&id);
            }
        });
        if let Some(completion) = self.completion.take() {
            completion.on_complete(finished);
        } else {
            finished();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{Origin, Status, Window};
    use std::{
        collections::BTreeMap,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct Counters {
        approvals: AtomicUsize,
        executions: AtomicUsize,
        reject_approval: AtomicBool,
        fail_execution: AtomicBool,
        pending_drops: AtomicUsize,
        approved_drops: AtomicUsize,
    }
    struct FixturePending(Arc<Counters>);
    struct FixtureApproved(Arc<Counters>);
    impl PendingTask for FixturePending {
        fn budget(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, String> {
            self.0.approvals.fetch_add(1, Ordering::SeqCst);
            if self.0.reject_approval.load(Ordering::SeqCst) {
                return Err("fixture expired".into());
            }
            Ok(Box::new(FixtureApproved(self.0.clone())))
        }
    }
    impl Drop for FixturePending {
        fn drop(&mut self) {
            self.0.pending_drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Drop for FixtureApproved {
        fn drop(&mut self) {
            self.0.approved_drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl ApprovedTask for FixtureApproved {
        fn execute(
            self: Box<Self>,
            cancel: &CancelToken,
        ) -> Result<pcagent_inspect::Inspection, String> {
            if cancel.is_cancelled() {
                return Err("fixture cancelled".into());
            }
            self.0.executions.fetch_add(1, Ordering::SeqCst);
            if self.0.fail_execution.load(Ordering::SeqCst) {
                return Err("fixture failed".into());
            }
            Ok(pcagent_inspect::Inspection {
                status: Status::Ok,
                measured_at_unix_ms: 0,
                window: Window {
                    started_at_unix_ms: 0,
                    finished_at_unix_ms: 0,
                    duration_ms: 0,
                },
                origin: Origin::SyntheticFixture,
                observations: Vec::new(),
                coverage: Vec::new(),
                names: BTreeMap::new(),
            })
        }
    }
    fn prepare(slot: &Slot, counters: &Arc<Counters>) -> PreparedView {
        let counters = counters.clone();
        slot.prepare_with(ElevatedTask::SystemCaches, || {
            Ok(Box::new(FixturePending(counters)))
        })
        .unwrap()
    }

    #[test]
    fn matching_local_approval_is_consumed_once_and_execution_releases_the_slot() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        assert_eq!(view.state, "awaiting_local_approval");
        assert_eq!(counters.approvals.load(Ordering::SeqCst), 0);
        assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
        assert!(slot.begin("unknown").is_err());
        assert!(slot.cancel("unknown").is_err());
        let run = slot.begin(&view.request_id).unwrap();
        assert_eq!(counters.approvals.load(Ordering::SeqCst), 1);
        assert!(slot.begin(&view.request_id).is_err());
        assert!(slot
            .prepare_with(ElevatedTask::SystemCaches, || panic!("occupied"))
            .is_err());
        assert!(run.execute().is_ok());
        assert_eq!(counters.executions.load(Ordering::SeqCst), 1);
        assert!(slot.begin(&view.request_id).is_err());
        let next = prepare(&slot, &counters);
        slot.cancel(&next.request_id).unwrap();
    }

    #[test]
    fn declining_and_cancelling_before_execution_do_not_execute_a_fixture() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        slot.cancel(&view.request_id).unwrap();
        assert!(slot.begin(&view.request_id).is_err());
        assert_eq!(counters.approvals.load(Ordering::SeqCst), 0);
        let next = prepare(&slot, &counters);
        let run = slot.begin(&next.request_id).unwrap();
        slot.cancel(&next.request_id).unwrap();
        assert!(slot
            .prepare_with(ElevatedTask::SystemCaches, || panic!("still running"))
            .is_err());
        assert!(run.execute().is_err());
        assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
        let next = prepare(&slot, &counters);
        slot.cancel(&next.request_id).unwrap();
    }

    #[test]
    fn abandonment_and_failed_execution_release_only_their_own_request() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        let run = slot.begin(&view.request_id).unwrap();
        drop(run);
        let next = prepare(&slot, &counters);
        let run = slot.begin(&next.request_id).unwrap();
        slot.finish(&view.request_id);
        assert!(slot
            .prepare_with(ElevatedTask::SystemCaches, || panic!("new job remains"))
            .is_err());
        counters.fail_execution.store(true, Ordering::SeqCst);
        assert!(run.execute().is_err());
        let last = prepare(&slot, &counters);
        slot.cancel(&last.request_id).unwrap();
    }

    #[test]
    fn dropping_the_host_slot_cancels_the_run_without_holding_the_slot_alive() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        let run = slot.begin(&view.request_id).unwrap();
        let weak = Arc::downgrade(&slot);
        drop(slot);
        assert!(weak.upgrade().is_none());
        assert!(run.execute().is_err());
        assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn failed_approval_and_unsupported_mft_never_occupy_a_running_slot() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        counters.reject_approval.store(true, Ordering::SeqCst);
        let view = prepare(&slot, &counters);
        assert!(slot.begin(&view.request_id).is_err());
        assert!(slot.cancel(&view.request_id).is_err());
        assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
        counters.reject_approval.store(false, Ordering::SeqCst);
        let next = prepare(&slot, &counters);
        slot.cancel(&next.request_id).unwrap();
        let task = ElevatedTask::MftInventory {
            volume: pcagent_inspect::elevation::DriveLetter::parse("C").unwrap(),
        };
        assert!(slot
            .prepare_with(task, || panic!("unvalidated MFT must not be prepared"))
            .is_err());
    }

    #[test]
    fn status_recovers_the_same_review_and_tracks_cancellation_without_reapproval() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        assert!(slot.status().unwrap().is_none());
        let prepared = prepare(&slot, &counters);
        let recovered = slot.status().unwrap().unwrap();
        assert_eq!(recovered.request_id, prepared.request_id);
        assert_eq!(recovered.task, ElevatedTask::SystemCaches);
        assert_eq!(recovered.budget_ms, 1000);
        assert_eq!(recovered.state, "awaiting_local_approval");
        assert_eq!(counters.approvals.load(Ordering::SeqCst), 0);
        let run = slot.begin(&recovered.request_id).unwrap();
        let running = slot.status().unwrap().unwrap();
        assert_eq!(running.request_id, prepared.request_id);
        assert_eq!(running.task, recovered.task);
        assert_eq!(running.budget_ms, recovered.budget_ms);
        assert_eq!(running.state, "running");
        slot.cancel(&running.request_id).unwrap();
        assert_eq!(slot.status().unwrap().unwrap().state, "cancelling");
        assert_eq!(counters.approvals.load(Ordering::SeqCst), 1);
        drop(run);
        assert!(slot.status().unwrap().is_none());
        assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn shutdown_declines_prepared_work_and_permanently_closes_admission() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        let shutdown = slot.begin_shutdown().unwrap();
        assert!(slot.begin_shutdown().is_none());
        assert!(slot.begin(&view.request_id).is_err());
        assert!(slot
            .prepare_with(ElevatedTask::SystemCaches, || panic!("closed"))
            .is_err());
        assert!(shutdown.wait(Duration::ZERO));
        assert!(slot.status().unwrap().is_none());
        assert_eq!(counters.pending_drops.load(Ordering::SeqCst), 1);
        assert_eq!(counters.approvals.load(Ordering::SeqCst), 0);
        assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn shutdown_timeout_keeps_the_run_cancelled_and_allows_a_bounded_retry() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        let run = slot.begin(&view.request_id).unwrap();
        let shutdown = slot.begin_shutdown().unwrap();
        assert!(run.cancel.is_cancelled());
        assert!(!shutdown.wait(Duration::ZERO));
        assert_eq!(slot.status().unwrap().unwrap().state, "cancelling");
        let retry = slot.begin_shutdown().unwrap();
        drop(run);
        assert!(retry.wait(Duration::ZERO));
        assert_eq!(counters.approved_drops.load(Ordering::SeqCst), 1);
        assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn shutdown_wait_releases_the_mutex_for_status_and_completion() {
        use std::sync::mpsc;
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        let run = slot.begin(&view.request_id).unwrap();
        let shutdown = slot.begin_shutdown().unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            done_tx.send(shutdown.wait(Duration::from_secs(3))).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let reader_slot = slot.clone();
        let (status_tx, status_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            status_tx.send(reader_slot.status()).unwrap();
        });
        assert_eq!(
            status_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()
                .unwrap()
                .state,
            "cancelling"
        );
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(run);
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        waiter.join().unwrap();
        reader.join().unwrap();
    }

    struct CleanupPending {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        cleaned: Arc<AtomicBool>,
    }
    struct CleanupApproved(CleanupPending);
    impl PendingTask for CleanupPending {
        fn budget(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, String> {
            Ok(Box::new(CleanupApproved(*self)))
        }
    }
    impl ApprovedTask for CleanupApproved {
        fn execute(
            self: Box<Self>,
            _: &CancelToken,
        ) -> Result<pcagent_inspect::Inspection, String> {
            Err("cleanup fixture executes no task".into())
        }
    }
    impl Drop for CleanupApproved {
        fn drop(&mut self) {
            self.0.entered.send(()).unwrap();
            self.0.release.recv_timeout(Duration::from_secs(3)).unwrap();
            self.0.cleaned.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn shutdown_does_not_complete_before_abandoned_approved_resources_are_cleaned() {
        use std::sync::mpsc;
        let slot = Arc::new(Slot::default());
        let (entered, entered_rx) = mpsc::channel();
        let (release_tx, release) = mpsc::channel();
        let cleaned = Arc::new(AtomicBool::new(false));
        let cleaned_fixture = cleaned.clone();
        let view = slot
            .prepare_with(ElevatedTask::SystemCaches, || {
                Ok(Box::new(CleanupPending {
                    entered,
                    release,
                    cleaned: cleaned_fixture,
                }))
            })
            .unwrap();
        let run = slot.begin(&view.request_id).unwrap();
        let shutdown = slot.begin_shutdown().unwrap();
        let cleaner = std::thread::spawn(move || drop(run));
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(!shutdown.wait(Duration::ZERO));
        assert!(!cleaned.load(Ordering::SeqCst));
        assert_eq!(slot.status().unwrap().unwrap().state, "cancelling");
        release_tx.send(()).unwrap();
        cleaner.join().unwrap();
        assert!(slot.begin_shutdown().unwrap().wait(Duration::ZERO));
        assert!(cleaned.load(Ordering::SeqCst));
    }

    type CompletionCallback = Box<dyn FnOnce() + Send>;

    #[derive(Default)]
    struct CompletionGate {
        state: Mutex<(bool, Vec<CompletionCallback>)>,
        cleaned: AtomicBool,
    }
    impl CompletionGate {
        fn finish_cleanup(&self) {
            self.cleaned.store(true, Ordering::SeqCst);
            let callbacks = {
                let mut state = self.state.lock().unwrap();
                state.0 = true;
                std::mem::take(&mut state.1)
            };
            for callback in callbacks {
                callback();
            }
        }
    }
    struct GatedCompletion(Arc<CompletionGate>);
    impl TaskCompletion for GatedCompletion {
        fn on_complete(self: Box<Self>, finished: Box<dyn FnOnce() + Send>) {
            let mut state = self.0.state.lock().unwrap();
            if !state.0 {
                state.1.push(finished);
            } else {
                drop(state);
                finished();
            }
        }
    }
    struct GatedTask {
        completion: Arc<CompletionGate>,
        failure: &'static str,
    }
    impl PendingTask for GatedTask {
        fn budget(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, String> {
            Ok(self)
        }
    }
    impl ApprovedTask for GatedTask {
        fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
            Some(Box::new(GatedCompletion(self.completion.clone())))
        }
        fn execute(
            self: Box<Self>,
            _: &CancelToken,
        ) -> Result<pcagent_inspect::Inspection, String> {
            Err(self.failure.into())
        }
    }

    #[test]
    fn bounded_failure_keeps_admission_closed_and_shutdown_pending_until_native_cleanup() {
        for failure in ["consent pending after timeout", "exit query failed"] {
            let slot = Arc::new(Slot::default());
            let completion = Arc::new(CompletionGate::default());
            let gate = completion.clone();
            let view = slot
                .prepare_with(ElevatedTask::SystemCaches, || {
                    Ok(Box::new(GatedTask {
                        completion: gate,
                        failure,
                    }))
                })
                .unwrap();
            let run = slot.begin(&view.request_id).unwrap();
            assert!(matches!(run.execute(), Err(error) if error == failure));
            assert_eq!(slot.status().unwrap().unwrap().state, "cancelling");
            assert!(slot
                .prepare_with(ElevatedTask::SystemCaches, || panic!("cleanup pending"))
                .is_err());
            assert!(!slot.begin_shutdown().unwrap().wait(Duration::ZERO));
            assert!(!completion.cleaned.load(Ordering::SeqCst));
            completion.finish_cleanup();
            assert!(completion.cleaned.load(Ordering::SeqCst));
            assert!(slot.status().unwrap().is_none());
            assert!(slot.begin_shutdown().unwrap().wait(Duration::ZERO));
        }
    }

    #[test]
    fn completion_after_an_abandoned_run_releases_only_the_matching_request() {
        let slot = Arc::new(Slot::default());
        let completion = Arc::new(CompletionGate::default());
        let gate = completion.clone();
        let view = slot
            .prepare_with(ElevatedTask::SystemCaches, || {
                Ok(Box::new(GatedTask {
                    completion: gate,
                    failure: "must not execute",
                }))
            })
            .unwrap();
        drop(slot.begin(&view.request_id).unwrap());
        assert_eq!(slot.status().unwrap().unwrap().state, "cancelling");
        completion.finish_cleanup();
        assert!(slot.status().unwrap().is_none());
        let counters = Arc::new(Counters::default());
        let next = prepare(&slot, &counters);
        let next_run = slot.begin(&next.request_id).unwrap();
        completion.finish_cleanup();
        slot.finish(&view.request_id);
        assert_eq!(slot.status().unwrap().unwrap().request_id, next.request_id);
        drop(next_run);
        assert!(slot.status().unwrap().is_none());
    }
}
