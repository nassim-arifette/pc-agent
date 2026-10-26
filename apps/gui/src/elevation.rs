//! One local, reviewed elevated inspection at a time. The model cannot reach this slot.
use crate::failure::{self, Code, Failure};
use pcagent_inspect::elevation::{ElevatedTask, PreparedElevation};
use pcagent_shared::new_id;
use pcagent_storage::CancelToken;
use serde::Serialize;
use std::{
    sync::{Arc, Condvar, Mutex, Weak},
    time::{Duration, Instant},
};

/// C4 stays at the existing JSON level; the optional metadata tree is local UI data only.
#[derive(Serialize)]
pub struct InspectionResult {
    #[serde(flatten)]
    pub inspection: pcagent_inspect::Inspection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_mft_tree: Option<pcagent_inspect::elevation::mft::TreeSummary>,
    /// Closed reasons behind each coverage entry, which the window words in its own language;
    /// the coverage `detail` stays the English C4 text, never shown in the window.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub coverage_notes: Vec<pcagent_inspect::elevation::CoverageNote>,
}
impl From<pcagent_inspect::Inspection> for InspectionResult {
    fn from(inspection: pcagent_inspect::Inspection) -> Self {
        Self {
            inspection,
            local_mft_tree: None,
            coverage_notes: Vec::new(),
        }
    }
}

// These private adapters keep the slot's lifecycle testable without native job files or UAC.
trait PendingTask: Send {
    fn budget(&self) -> Duration;
    fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
        None
    }
    fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, Failure>;
}
trait ApprovedTask: Send {
    fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
        None
    }
    fn execute(self: Box<Self>, cancel: &CancelToken) -> Result<InspectionResult, Failure>;
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
    fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
        Some(Box::new(NativeCompletion(self.0.completion())))
    }
    fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, Failure> {
        let mut prepared = self.0;
        let approval = prepared
            .approve_locally()
            .map_err(|error| failure::elevation(&error))?;
        Ok(Box::new(NativeApproved { prepared, approval }))
    }
}
impl ApprovedTask for NativeApproved {
    fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
        Some(Box::new(NativeCompletion(self.prepared.completion())))
    }
    fn execute(self: Box<Self>, cancel: &CancelToken) -> Result<InspectionResult, Failure> {
        let response = self
            .prepared
            .run_locally_bounded(self.approval, cancel)
            .map_err(|error| failure::elevation(&error))?;
        Ok(InspectionResult {
            inspection: pcagent_inspect::elevation::to_inspection(&response),
            coverage_notes: pcagent_inspect::elevation::coverage_notes(&response),
            local_mft_tree: response.mft.map(|collection| collection.tree),
        })
    }
}

#[derive(Serialize)]
pub struct PreparedView {
    pub request_id: String,
    pub task: ElevatedTask,
    pub budget_ms: u64,
    /// preparing, awaiting_local_approval, running or cancelling; preparing grants no permit.
    pub state: &'static str,
}

enum Job {
    Preparing {
        id: String,
        task: ElevatedTask,
        budget_ms: u64,
        cancel: CancelToken,
    },
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
    Cleanup {
        id: String,
        task: ElevatedTask,
        budget_ms: u64,
    },
}

#[derive(Default)]
struct SlotState {
    job: Option<Job>,
    closing: bool,
    shutdown_waiter: bool,
    completed: Vec<pcagent_tools::elevated_tools::ApprovedInspection>,
}

#[derive(Default)]
pub struct Slot {
    // Cleanup notifications can retain this state without keeping the host slot alive.
    state: Arc<Mutex<SlotState>>,
    idle: Arc<Condvar>,
}

/// One bounded shutdown attempt; dropping it permits a retry, while the slot stays closed.
pub struct Shutdown {
    slot: Arc<Slot>,
}

pub struct Run {
    id: String,
    task: ElevatedTask,
    approved: Option<Box<dyn ApprovedTask>>,
    completion: Option<Box<dyn TaskCompletion>>,
    cancel: CancelToken,
    slot: Weak<Slot>,
}

impl Slot {
    pub fn prepare(&self, task: ElevatedTask, budget: Duration) -> Result<PreparedView, Failure> {
        self.prepare_with_budget(task.clone(), budget, || {
            PreparedElevation::prepare(task, budget)
                .map(|prepared| Box::new(NativePending(prepared)) as Box<dyn PendingTask>)
                .map_err(|error| failure::elevation(&error))
        })
    }

    #[cfg(test)]
    fn prepare_with(
        &self,
        task: ElevatedTask,
        prepare: impl FnOnce() -> Result<Box<dyn PendingTask>, Failure>,
    ) -> Result<PreparedView, Failure> {
        self.prepare_with_budget(task, Duration::from_secs(1), prepare)
    }

    fn prepare_with_budget(
        &self,
        task: ElevatedTask,
        budget: Duration,
        prepare: impl FnOnce() -> Result<Box<dyn PendingTask>, Failure>,
    ) -> Result<PreparedView, Failure> {
        let budget_ms = u64::try_from(budget.as_millis())
            .ok()
            .filter(|ms| (1..=pcagent_inspect::elevation::MAX_BUDGET_MS).contains(ms))
            .ok_or_else(|| {
                failure::elevation(&pcagent_inspect::elevation::ElevationError::InvalidRequest)
            })?;
        let mut slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        if slot.closing {
            return Err(shutting_down());
        }
        if slot.job.is_some() {
            return Err(Failure::new(
                Code::HelperBusy,
                "An elevated inspection is already prepared or running.",
            ));
        }
        let id = new_id("elevation");
        let cancel = CancelToken::default();
        slot.job = Some(Job::Preparing {
            id: id.clone(),
            task: task.clone(),
            budget_ms,
            cancel: cancel.clone(),
        });
        drop(slot);

        // Native request creation can wait for files or failed-creation cleanup. The reservation
        // stays visible and cancellable without holding the GUI's state mutex across that work.
        let result = prepare();
        let mut slot = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let stopped = preparation_stop(slot.closing, &cancel);
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                // Native preparation errors return only after their private resources are gone.
                if matches!(slot.job.as_ref(), Some(Job::Preparing { id: active, .. }) if active == &id)
                {
                    slot.job = None;
                    self.idle.notify_all();
                }
                return Err(stopped.unwrap_or(error));
            }
        };
        if let Some(error) = stopped {
            slot.job = Some(Job::Cleanup {
                id: id.clone(),
                task,
                budget_ms,
            });
            drop(slot);
            self.abandon_prepared(id, prepared);
            return Err(error);
        }
        let view = PreparedView {
            request_id: id.clone(),
            task: task.clone(),
            budget_ms: prepared.budget().as_millis() as u64,
            state: "awaiting_local_approval",
        };
        slot.job = Some(Job::Prepared { id, task, prepared });
        Ok(view)
    }

    /// Recover local review/progress after a webview reload without issuing another approval.
    pub fn status(&self) -> Result<Option<PreparedView>, Failure> {
        let slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        Ok(slot.job.as_ref().map(|job| match job {
            Job::Preparing {
                id,
                task,
                budget_ms,
                cancel,
            } => PreparedView {
                request_id: id.clone(),
                task: task.clone(),
                budget_ms: *budget_ms,
                state: if cancel.is_cancelled() {
                    "cancelling"
                } else {
                    "preparing"
                },
            },
            Job::Prepared { id, task, prepared } => PreparedView {
                request_id: id.clone(),
                task: task.clone(),
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
                task: task.clone(),
                budget_ms: *budget_ms,
                state: if cancel.is_cancelled() {
                    "cancelling"
                } else {
                    "running"
                },
            },
            Job::Cleanup {
                id,
                task,
                budget_ms,
            } => PreparedView {
                request_id: id.clone(),
                task: task.clone(),
                budget_ms: *budget_ms,
                state: "cancelling",
            },
        }))
    }

    /// Called only by the window's local approval command, after reviewing PreparedView.
    pub fn begin(self: &Arc<Self>, request_id: &str) -> Result<Run, Failure> {
        let mut slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        if slot.closing {
            return Err(shutting_down());
        }
        if !matches!(slot.job.as_ref(), Some(Job::Prepared { id, .. }) if id == request_id) {
            return Err(Failure::new(
                Code::RequestExpired,
                "This elevated inspection is no longer awaiting approval.",
            ));
        }
        let Some(Job::Prepared { id, task, prepared }) = slot.job.take() else {
            unreachable!()
        };
        let budget_ms = prepared.budget().as_millis() as u64;
        let cancel = CancelToken::default();
        slot.job = Some(Job::Preparing {
            id: id.clone(),
            task: task.clone(),
            budget_ms,
            cancel: cancel.clone(),
        });
        drop(slot);
        let pending_completion = prepared.completion();
        // A failed local approval drops its prepared native request. Its filesystem cleanup
        // must not hold the state mutex or hide a simultaneous cancellation/shutdown.
        let result = prepared.approve();
        let mut slot = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let stopped = preparation_stop(slot.closing, &cancel);
        let approved = match result {
            Ok(approved) => approved,
            Err(error) => {
                slot.job = Some(Job::Cleanup {
                    id: id.clone(),
                    task,
                    budget_ms,
                });
                drop(slot);
                self.release_after_completion(id, pending_completion);
                return Err(stopped.unwrap_or(error));
            }
        };
        if let Some(error) = stopped {
            slot.job = Some(Job::Cleanup {
                id: id.clone(),
                task,
                budget_ms,
            });
            drop(slot);
            let completion = approved.completion().or(pending_completion);
            drop(approved);
            self.release_after_completion(id, completion);
            return Err(error);
        }
        let completion = approved.completion().or(pending_completion);
        slot.job = Some(Job::Running {
            id: id.clone(),
            task: task.clone(),
            budget_ms,
            cancel: cancel.clone(),
        });
        Ok(Run {
            id,
            task,
            approved: Some(approved),
            completion,
            cancel,
            slot: Arc::downgrade(self),
        })
    }

    pub fn finish(&self, request_id: &str) {
        finish_state(&self.state, &self.idle, request_id);
    }

    fn release_after_completion(
        &self,
        request_id: String,
        completion: Option<Box<dyn TaskCompletion>>,
    ) {
        let state = self.state.clone();
        let idle = self.idle.clone();
        let finished = Box::new(move || finish_state(&state, &idle, &request_id));
        // An already completed capability may invoke this immediately: no state lock is held.
        if let Some(completion) = completion {
            completion.on_complete(finished);
        } else {
            finished();
        }
    }

    fn abandon_prepared(&self, request_id: String, prepared: Box<dyn PendingTask>) {
        let completion = prepared.completion();
        drop(prepared);
        self.release_after_completion(request_id, completion);
    }
    /// Read only already approved evidence. This grants no access to the approval/run slot.
    pub fn completed_evidence(&self) -> Vec<pcagent_tools::elevated_tools::ApprovedInspection> {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .completed
            .iter()
            .filter(|result| result.available())
            .cloned()
            .collect()
    }

    pub fn cancel(&self, request_id: &str) -> Result<(), Failure> {
        let mut slot = self
            .state
            .lock()
            .map_err(|_| "Elevation state unavailable.")?;
        match slot.job.as_ref() {
            Some(Job::Preparing { id, cancel, .. } | Job::Running { id, cancel, .. })
                if id == request_id =>
            {
                cancel.cancel();
                return Ok(());
            }
            Some(Job::Cleanup { id, .. }) if id == request_id => return Ok(()),
            Some(Job::Prepared { id, .. }) if id == request_id => {}
            _ => {
                return Err(Failure::new(
                    Code::RequestExpired,
                    "This elevated inspection is no longer available.",
                ))
            }
        }
        let Some(Job::Prepared { id, task, prepared }) = slot.job.take() else {
            unreachable!()
        };
        slot.job = Some(Job::Cleanup {
            id: id.clone(),
            task,
            budget_ms: prepared.budget().as_millis() as u64,
        });
        drop(slot);
        self.abandon_prepared(id, prepared);
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
        if let Some(Job::Preparing { cancel, .. } | Job::Running { cancel, .. }) = &slot.job {
            cancel.cancel();
        }
        Some(Shutdown { slot: self.clone() })
    }
}

fn shutting_down() -> Failure {
    Failure::new(Code::ShuttingDown, "PC Agent is shutting down.")
}

fn preparation_stop(closing: bool, cancel: &CancelToken) -> Option<Failure> {
    if closing {
        Some(shutting_down())
    } else if cancel.is_cancelled() {
        Some(Failure::new(
            Code::Cancelled,
            "This elevated inspection was cancelled before execution.",
        ))
    } else {
        None
    }
}

fn finish_state(state: &Mutex<SlotState>, idle: &Condvar, request_id: &str) {
    let mut slot = state.lock().unwrap_or_else(|p| p.into_inner());
    if matches!(slot.job.as_ref(), Some(Job::Running { id, .. } | Job::Cleanup { id, .. }) if id == request_id)
    {
        slot.job = None;
        idle.notify_all();
    }
}

impl Shutdown {
    /// Called on a blocking worker. A timeout keeps the host alive and allows another attempt.
    pub fn wait(self, timeout: Duration) -> bool {
        let started = Instant::now();
        let prepared = {
            let mut state = self.slot.state.lock().unwrap_or_else(|p| p.into_inner());
            if matches!(state.job.as_ref(), Some(Job::Prepared { .. })) {
                let Some(Job::Prepared { id, task, prepared }) = state.job.take() else {
                    unreachable!()
                };
                state.job = Some(Job::Cleanup {
                    id: id.clone(),
                    task,
                    budget_ms: prepared.budget().as_millis() as u64,
                });
                Some((id, prepared))
            } else {
                None
            }
        };
        // Dropping a prepared native job may only start cleanup. Keep admission closed until its
        // completion confirms the private files disappeared; register outside the state lock.
        if let Some((id, prepared)) = prepared {
            self.slot.abandon_prepared(id, prepared);
        }
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
        let job = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .job
            .take();
        if let Some(Job::Preparing { cancel, .. } | Job::Running { cancel, .. }) = &job {
            cancel.cancel();
        }
        drop(job);
    }
}

impl Run {
    pub fn execute(mut self) -> Result<InspectionResult, Failure> {
        let approved = self.approved.take().ok_or_else(|| {
            Failure::new(
                Code::RequestExpired,
                "This elevated inspection was already consumed.",
            )
        })?;
        let result = approved.execute(&self.cancel);
        if !self.cancel.is_cancelled() {
            if let (Some(slot), Ok(inspection)) = (self.slot.upgrade(), &result) {
                if let Ok(captured) = pcagent_tools::elevated_tools::ApprovedInspection::new(
                    &self.task,
                    inspection.inspection.clone(),
                ) {
                    let mut state = slot.state.lock().unwrap_or_else(|p| p.into_inner());
                    if !state.closing && !self.cancel.is_cancelled() {
                        state
                            .completed
                            .retain(|r| r.available() && r.task_name() != captured.task_name());
                        state.completed.push(captured);
                        if state.completed.len()
                            > pcagent_tools::elevated_tools::MAX_APPROVED_INSPECTIONS
                        {
                            state.completed.remove(0);
                        }
                    }
                }
            }
        }
        result
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
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, Failure> {
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
        fn execute(self: Box<Self>, cancel: &CancelToken) -> Result<InspectionResult, Failure> {
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
            }
            .into())
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
    fn failed_approval_never_occupies_a_running_slot() {
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
    }

    #[test]
    fn slot_refusals_carry_codes_the_page_translates() {
        let slot = Arc::new(Slot::default());
        let counters = Arc::new(Counters::default());
        let view = prepare(&slot, &counters);
        let busy = slot
            .prepare_with(ElevatedTask::SystemCaches, || panic!("occupied"))
            .err()
            .expect("refused");
        assert_eq!(busy.code, Code::HelperBusy);
        assert_eq!(
            slot.begin("unknown").err().expect("refused").code,
            Code::RequestExpired
        );
        assert_eq!(
            slot.cancel("unknown").unwrap_err().code,
            Code::RequestExpired
        );
        slot.cancel(&view.request_id).unwrap();
        let shutdown = slot.begin_shutdown().unwrap();
        let closed = slot
            .prepare_with(ElevatedTask::SystemCaches, || panic!("closed"))
            .err()
            .expect("refused");
        assert_eq!(closed.code, Code::ShuttingDown);
        assert!(shutdown.wait(Duration::ZERO));
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
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, Failure> {
            Ok(Box::new(CleanupApproved(*self)))
        }
    }
    impl ApprovedTask for CleanupApproved {
        fn execute(self: Box<Self>, _: &CancelToken) -> Result<InspectionResult, Failure> {
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
    struct GatedPending {
        completion: Arc<CompletionGate>,
        counters: Arc<Counters>,
        complete_on_drop: bool,
    }
    impl PendingTask for GatedPending {
        fn budget(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
            Some(Box::new(GatedCompletion(self.completion.clone())))
        }
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, Failure> {
            self.counters.approvals.fetch_add(1, Ordering::SeqCst);
            Err("fixture expired".into())
        }
    }
    impl Drop for GatedPending {
        fn drop(&mut self) {
            self.counters.pending_drops.fetch_add(1, Ordering::SeqCst);
            if self.complete_on_drop {
                self.completion.finish_cleanup();
            }
        }
    }
    fn prepare_gated_pending(
        slot: &Slot,
        complete_on_drop: bool,
    ) -> (PreparedView, Arc<CompletionGate>, Arc<Counters>) {
        let completion = Arc::new(CompletionGate::default());
        let counters = Arc::new(Counters::default());
        let gate = completion.clone();
        let pending_counters = counters.clone();
        let view = slot
            .prepare_with(ElevatedTask::BatteryEnergy, || {
                Ok(Box::new(GatedPending {
                    completion: gate,
                    counters: pending_counters,
                    complete_on_drop,
                }))
            })
            .unwrap();
        (view, completion, counters)
    }

    fn abandon_gated_pending(slot: &Arc<Slot>, view: &PreparedView, path: &str, pending: bool) {
        match path {
            "cancel" => slot.cancel(&view.request_id).unwrap(),
            "approval" => assert!(
                matches!(slot.begin(&view.request_id), Err(error) if error.message == "fixture expired")
            ),
            "shutdown" => assert_eq!(
                slot.begin_shutdown().unwrap().wait(Duration::ZERO),
                !pending
            ),
            _ => panic!("unknown fixture path"),
        }
    }

    #[test]
    fn blocked_preparation_keeps_status_cancel_and_shutdown_responsive_until_cleanup() {
        use std::sync::mpsc;
        for stop in ["publish", "cancel", "shutdown"] {
            let slot = Arc::new(Slot::default());
            let completion = Arc::new(CompletionGate::default());
            let counters = Arc::new(Counters::default());
            std::thread::scope(|scope| {
                // A failed assertion drops this sender, releasing the synthetic factory.
                let (release_tx, release_rx) = mpsc::channel();
                let (entered_tx, entered_rx) = mpsc::channel();
                let (status_tx, status_rx) = mpsc::channel();
                let preparing_slot = slot.clone();
                let gate = completion.clone();
                let pending_counters = counters.clone();
                let preparing = scope.spawn(move || {
                    preparing_slot.prepare_with(ElevatedTask::BatteryEnergy, || {
                        let _ = entered_tx.send(());
                        release_rx
                            .recv_timeout(Duration::from_secs(10))
                            .map_err(|_| "fixture preparation released without publishing")?;
                        Ok(Box::new(GatedPending {
                            completion: gate,
                            counters: pending_counters,
                            complete_on_drop: false,
                        }))
                    })
                });
                entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                let reading_slot = slot.clone();
                let reader = scope.spawn(move || {
                    let preparing = reading_slot.status().unwrap().unwrap();
                    assert_eq!(preparing.state, "preparing");
                    assert!(reading_slot.begin(&preparing.request_id).is_err());
                    assert!(reading_slot
                        .prepare_with(ElevatedTask::SystemCaches, || panic!("reserved"))
                        .is_err());
                    reading_slot.finish(&preparing.request_id);
                    assert!(reading_slot.status().unwrap().is_some());
                    if stop == "cancel" {
                        reading_slot.cancel(&preparing.request_id).unwrap();
                    } else if stop == "shutdown" {
                        assert!(!reading_slot.begin_shutdown().unwrap().wait(Duration::ZERO));
                    }
                    let status = reading_slot.status().unwrap().unwrap();
                    let _ = status_tx.send((preparing, status));
                });
                let (before, after) = status_rx
                    .recv_timeout(Duration::from_secs(3))
                    .expect("status/cancellation waited for native preparation");
                reader.join().unwrap();
                assert_eq!(before.request_id, after.request_id);
                assert_eq!(before.task, ElevatedTask::BatteryEnergy);
                assert_eq!(before.budget_ms, 1000);
                assert_eq!(
                    after.state,
                    if stop == "publish" {
                        "preparing"
                    } else {
                        "cancelling"
                    }
                );
                assert_eq!(counters.approvals.load(Ordering::SeqCst), 0);
                release_tx.send(()).unwrap();
                let result = preparing.join().unwrap();
                if stop == "publish" {
                    let view = result.unwrap();
                    assert_eq!(view.request_id, before.request_id);
                    assert_eq!(view.state, "awaiting_local_approval");
                    slot.cancel(&view.request_id).unwrap();
                } else {
                    let expected = if stop == "cancel" {
                        Code::Cancelled
                    } else {
                        Code::ShuttingDown
                    };
                    assert_eq!(result.err().expect("stopped").code, expected);
                }
                assert_eq!(slot.status().unwrap().unwrap().state, "cancelling");
                assert_eq!(counters.pending_drops.load(Ordering::SeqCst), 1);
                assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
                assert!(slot
                    .prepare_with(ElevatedTask::SystemCaches, || panic!("cleanup pending"))
                    .is_err());
                assert!(!slot.begin_shutdown().unwrap().wait(Duration::ZERO));
                completion.finish_cleanup();
                assert!(slot.status().unwrap().is_none());
                assert!(slot.begin_shutdown().unwrap().wait(Duration::ZERO));
            });
        }
    }

    struct BlockingApprovalPending {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        completion: Arc<CompletionGate>,
        counters: Arc<Counters>,
    }
    impl PendingTask for BlockingApprovalPending {
        fn budget(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
            Some(Box::new(GatedCompletion(self.completion.clone())))
        }
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, Failure> {
            self.counters.approvals.fetch_add(1, Ordering::SeqCst);
            let _ = self.entered.send(());
            let _ = self.release.recv_timeout(Duration::from_secs(10));
            Err("fixture approval failed after blocking cleanup".into())
        }
    }
    impl Drop for BlockingApprovalPending {
        fn drop(&mut self) {
            self.counters.pending_drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn a_blocked_local_approval_does_not_block_shutdown_or_release_admission() {
        use std::sync::mpsc;
        let slot = Arc::new(Slot::default());
        let completion = Arc::new(CompletionGate::default());
        let counters = Arc::new(Counters::default());
        std::thread::scope(|scope| {
            let (release_tx, release_rx) = mpsc::channel();
            let (entered_tx, entered_rx) = mpsc::channel();
            let (status_tx, status_rx) = mpsc::channel();
            let gate = completion.clone();
            let pending_counters = counters.clone();
            let view = slot
                .prepare_with(ElevatedTask::BatteryEnergy, || {
                    Ok(Box::new(BlockingApprovalPending {
                        entered: entered_tx,
                        release: release_rx,
                        completion: gate,
                        counters: pending_counters,
                    }))
                })
                .unwrap();
            let approving_slot = slot.clone();
            let request_id = view.request_id.clone();
            let approving = scope.spawn(move || approving_slot.begin(&request_id));
            entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
            let reading_slot = slot.clone();
            let reader = scope.spawn(move || {
                let status = reading_slot.status().unwrap().unwrap();
                assert_eq!(status.state, "preparing");
                reading_slot.cancel(&status.request_id).unwrap();
                assert!(!reading_slot.begin_shutdown().unwrap().wait(Duration::ZERO));
                let _ = status_tx.send(reading_slot.status().unwrap().unwrap());
            });
            let status = status_rx
                .recv_timeout(Duration::from_secs(3))
                .expect("local approval held the GUI state mutex");
            reader.join().unwrap();
            assert_eq!(status.request_id, view.request_id);
            assert_eq!(status.state, "cancelling");
            release_tx.send(()).unwrap();
            assert!(approving.join().unwrap().is_err());
            assert_eq!(counters.pending_drops.load(Ordering::SeqCst), 1);
            assert_eq!(counters.approvals.load(Ordering::SeqCst), 1);
            assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
            assert!(!slot.begin_shutdown().unwrap().wait(Duration::ZERO));
            completion.finish_cleanup();
            assert!(slot.begin_shutdown().unwrap().wait(Duration::ZERO));
        });
    }

    #[test]
    fn abandoning_prepared_work_keeps_admission_and_shutdown_pending_until_cleanup() {
        for path in ["cancel", "approval", "shutdown"] {
            let slot = Arc::new(Slot::default());
            let (view, completion, counters) = prepare_gated_pending(&slot, false);
            abandon_gated_pending(&slot, &view, path, true);
            let status = slot.status().unwrap().unwrap();
            assert_eq!(status.request_id, view.request_id);
            assert_eq!(status.task, ElevatedTask::BatteryEnergy);
            assert_eq!(status.budget_ms, 1000);
            assert_eq!(status.state, "cancelling");
            assert_eq!(counters.pending_drops.load(Ordering::SeqCst), 1);
            assert_eq!(
                counters.approvals.load(Ordering::SeqCst),
                usize::from(path == "approval")
            );
            assert_eq!(counters.executions.load(Ordering::SeqCst), 0);
            assert!(!completion.cleaned.load(Ordering::SeqCst));
            slot.cancel(&view.request_id).unwrap();
            assert!(slot
                .prepare_with(ElevatedTask::SystemCaches, || panic!("cleanup pending"))
                .is_err());
            assert!(!slot.begin_shutdown().unwrap().wait(Duration::ZERO));
            completion.finish_cleanup();
            assert!(slot.status().unwrap().is_none());
            assert!(slot.begin_shutdown().unwrap().wait(Duration::ZERO));
            assert_eq!(counters.pending_drops.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn prepared_cleanup_completed_before_registration_releases_without_locking_itself() {
        for path in ["cancel", "approval", "shutdown"] {
            let slot = Arc::new(Slot::default());
            let (view, completion, counters) = prepare_gated_pending(&slot, true);
            abandon_gated_pending(&slot, &view, path, false);
            assert!(completion.cleaned.load(Ordering::SeqCst));
            assert!(slot.status().unwrap().is_none());
            assert_eq!(counters.pending_drops.load(Ordering::SeqCst), 1);
            assert!(slot.begin_shutdown().unwrap().wait(Duration::ZERO));
        }
    }

    #[test]
    fn late_prepared_cleanup_notifications_cannot_release_a_new_request() {
        let slot = Arc::new(Slot::default());
        let (view, completion, _) = prepare_gated_pending(&slot, false);
        slot.cancel(&view.request_id).unwrap();
        completion.finish_cleanup();
        let counters = Arc::new(Counters::default());
        let next = prepare(&slot, &counters);
        completion.finish_cleanup();
        slot.finish(&view.request_id);
        assert_eq!(slot.status().unwrap().unwrap().request_id, next.request_id);
        let run = slot.begin(&next.request_id).unwrap();
        slot.finish(&view.request_id);
        assert_eq!(slot.status().unwrap().unwrap().request_id, next.request_id);
        drop(run);
        assert!(slot.status().unwrap().is_none());
    }

    struct GatedTask {
        completion: Arc<CompletionGate>,
        failure: &'static str,
    }
    impl PendingTask for GatedTask {
        fn budget(&self) -> Duration {
            Duration::from_secs(1)
        }
        fn approve(self: Box<Self>) -> Result<Box<dyn ApprovedTask>, Failure> {
            Ok(self)
        }
    }
    impl ApprovedTask for GatedTask {
        fn completion(&self) -> Option<Box<dyn TaskCompletion>> {
            Some(Box::new(GatedCompletion(self.completion.clone())))
        }
        fn execute(self: Box<Self>, _: &CancelToken) -> Result<InspectionResult, Failure> {
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
            assert!(matches!(run.execute(), Err(error) if error.message == failure));
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
