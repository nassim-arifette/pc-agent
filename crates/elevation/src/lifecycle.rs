//! An operation result is separate from confirmed helper exit and private-resource cleanup.
use std::{
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

type FinishedCallback = Box<dyn FnOnce() + Send>;

struct CompletionState {
    callbacks: Mutex<Option<Vec<FinishedCallback>>>,
    finished: Condvar,
}

/// Read-only local capability: completion means launch resolved, exit observed and resources dropped.
/// It cannot authorize a task or mark a running helper as finished.
#[derive(Clone)]
pub struct ElevationCompletion(Arc<CompletionState>);

impl ElevationCompletion {
    #[cfg(any(windows, target_os = "linux", test))]
    pub(crate) fn pending() -> Self {
        Self(Arc::new(CompletionState {
            callbacks: Mutex::new(Some(Vec::new())),
            finished: Condvar::new(),
        }))
    }

    pub fn is_complete(&self) -> bool {
        self.0
            .callbacks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none()
    }

    /// Run a small notification after cleanup, outside the completion lock. Never blocks on exit.
    pub fn on_complete(&self, callback: impl FnOnce() + Send + 'static) {
        let mut callbacks = self
            .0
            .callbacks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(pending) = callbacks.as_mut() {
            pending.push(Box::new(callback));
            return;
        }
        drop(callbacks);
        callback();
    }

    /// Only a blocking worker may wait. A timeout never changes the completion state.
    pub fn wait(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        let mut callbacks = self
            .0
            .callbacks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while callbacks.is_some() {
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return false;
            }
            callbacks = self
                .0
                .finished
                .wait_timeout(callbacks, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        true
    }

    pub(crate) fn wait_until_complete(&self) {
        let mut callbacks = self
            .0
            .callbacks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while callbacks.is_some() {
            callbacks = self
                .0
                .finished
                .wait(callbacks)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    #[cfg(any(windows, target_os = "linux", test))]
    pub(crate) fn complete(&self) {
        let callbacks = self
            .0
            .callbacks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        self.0.finished.notify_all();
        for callback in callbacks.into_iter().flatten() {
            // A client notification must not interrupt cleanup or another client's notification.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(callback));
        }
    }
}

#[cfg(any(windows, target_os = "linux", test))]
use super::{
    protocol::ElevationError,
    worker::{supervise, ElevatedProcess},
};
#[cfg(any(windows, target_os = "linux", test))]
use pcagent_storage::CancelToken;

#[cfg(any(windows, target_os = "linux", test))]
pub(crate) trait HostJob: Send + Sync + 'static {
    type Response: Send + 'static;
    fn signal_cancel(&self);
    fn response(&self) -> Result<Self::Response, ElevationError>;
}

/// A persistent supervisor owns launch, process and job even after the caller receives an error.
#[cfg(any(windows, target_os = "linux", test))]
pub(crate) fn run_supervised<J: HostJob, P: ElevatedProcess + Send + 'static>(
    job: Arc<J>,
    cancel: &CancelToken,
    budget: Duration,
    launch: impl FnOnce(&J) -> Result<P, ElevationError> + Send + 'static,
) -> Result<J::Response, ElevationError> {
    if cancel.is_cancelled() {
        job.signal_cancel();
        return Err(ElevationError::Cancelled);
    }
    let started = Instant::now();
    let worker_job = job.clone();
    let worker_cancel = cancel.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("pcagent-elevation-supervisor".into())
        .spawn(move || {
            // ShellExecuteEx may remain in Windows consent. This thread keeps the pinned job alive.
            match launch(&worker_job) {
                Ok(mut process) => {
                    let result = supervise(
                        &mut process,
                        &worker_cancel,
                        budget.saturating_sub(started.elapsed()),
                    );
                    if let Err(error) = result {
                        worker_job.signal_cancel();
                        let _ = sender.send(Err(error));
                        // Termination timeout/query failure is an operation error, never proof of exit.
                        retain_until_exit(&mut process);
                    } else {
                        let _ = sender.send(worker_job.response());
                    }
                    // Process is dropped before the pinned job and its completion notification.
                }
                Err(error) => {
                    let _ = sender.send(Err(error));
                }
            }
        })?;
    loop {
        let failure = if cancel.is_cancelled() {
            Some(ElevationError::Cancelled)
        } else if started.elapsed() >= budget {
            Some(ElevationError::TimedOut)
        } else {
            None
        };
        if let Some(error) = failure {
            job.signal_cancel();
            // The same token is held by the supervisor if Windows consent completes later.
            cancel.cancel();
            return Err(error);
        }
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                job.signal_cancel();
                cancel.cancel();
                return Err(ElevationError::LaunchFailed(0));
            }
        }
    }
}

/// Retry only the owned process; keep its capability when termination or exit queries fail.
/// This runs on the supervisor, never on the GUI thread. No elapsed time implies confirmed exit.
#[cfg(any(windows, target_os = "linux", test))]
pub(crate) fn retain_until_exit(process: &mut impl ElevatedProcess) {
    while !matches!(process.has_exited(), Ok(true)) {
        let _ = process.terminate();
        if matches!(process.has_exited(), Ok(true)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::ErrorKind,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
    };

    struct FixtureJob {
        completion: ElevationCompletion,
        cleaned: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
    }
    impl HostJob for FixtureJob {
        type Response = ();
        fn signal_cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
        }
        fn response(&self) -> Result<(), ElevationError> {
            Ok(())
        }
    }
    impl Drop for FixtureJob {
        fn drop(&mut self) {
            self.cleaned.store(true, Ordering::SeqCst);
            self.completion.complete();
        }
    }
    fn fixture() -> (Arc<FixtureJob>, ElevationCompletion, Arc<AtomicBool>) {
        let completion = ElevationCompletion::pending();
        let cleaned = Arc::new(AtomicBool::new(false));
        let job = Arc::new(FixtureJob {
            completion: completion.clone(),
            cleaned: cleaned.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        (job, completion, cleaned)
    }

    struct FixtureProcess {
        exited: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
        attempts: Arc<AtomicUsize>,
        poll_error: bool,
    }
    impl ElevatedProcess for FixtureProcess {
        fn poll(&mut self) -> Result<Option<u32>, ElevationError> {
            if self.exited.load(Ordering::SeqCst) {
                Ok(Some(125))
            } else if self.poll_error {
                Err(ElevationError::Io(ErrorKind::PermissionDenied))
            } else {
                Ok(None)
            }
        }
        fn terminate(&mut self) -> Result<(), ElevationError> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            Err(ElevationError::TimedOut)
        }
    }
    impl Drop for FixtureProcess {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn pending_consent_and_late_helper_keep_completion_pending_after_cancellation() {
        let (job, completion, cleaned) = fixture();
        let cancel = CancelToken::default();
        let worker_cancel = cancel.clone();
        let marked_cancel = job.cancelled.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let exited = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(AtomicUsize::new(0));
        let process = FixtureProcess {
            exited: exited.clone(),
            dropped: dropped.clone(),
            attempts,
            poll_error: false,
        };
        let (result_tx, result_rx) = mpsc::channel();
        let caller = std::thread::spawn(move || {
            let result = run_supervised(job, &worker_cancel, Duration::from_secs(3), move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(process)
            });
            result_tx.send(result).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        cancel.cancel();
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(ElevationError::Cancelled)
        );
        caller.join().unwrap();
        assert!(marked_cancel.load(Ordering::SeqCst));
        assert!(!completion.wait(Duration::ZERO));
        assert!(!cleaned.load(Ordering::SeqCst));
        release_tx.send(()).unwrap();
        assert!(!completion.wait(Duration::from_millis(30)));
        assert!(!dropped.load(Ordering::SeqCst));
        exited.store(true, Ordering::SeqCst);
        assert!(completion.wait(Duration::from_secs(1)));
        assert!(dropped.load(Ordering::SeqCst));
        assert!(cleaned.load(Ordering::SeqCst));
    }

    #[test]
    fn poll_and_termination_failure_return_an_error_but_retain_process_until_exit() {
        let (job, completion, cleaned) = fixture();
        let exited = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(AtomicUsize::new(0));
        let process = FixtureProcess {
            exited: exited.clone(),
            dropped: dropped.clone(),
            attempts: attempts.clone(),
            poll_error: true,
        };
        assert_eq!(
            run_supervised(job, &CancelToken::default(), Duration::from_secs(3), |_| {
                Ok(process)
            }),
            Err(ElevationError::Io(ErrorKind::PermissionDenied))
        );
        assert!(attempts.load(Ordering::SeqCst) > 0);
        assert!(!completion.wait(Duration::from_millis(30)));
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(!cleaned.load(Ordering::SeqCst));
        exited.store(true, Ordering::SeqCst);
        assert!(completion.wait(Duration::from_secs(1)));
        assert!(dropped.load(Ordering::SeqCst));
        assert!(cleaned.load(Ordering::SeqCst));
    }

    #[test]
    fn consent_refusal_after_operation_timeout_still_completes_cleanup() {
        let (job, completion, cleaned) = fixture();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let caller = std::thread::spawn(move || {
            let result = run_supervised::<_, FixtureProcess>(
                job,
                &CancelToken::default(),
                Duration::from_millis(30),
                move |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Err(ElevationError::UacDeclined)
                },
            );
            result_tx.send(result).unwrap();
        });
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(ElevationError::TimedOut)
        );
        caller.join().unwrap();
        assert!(!completion.is_complete());
        release_tx.send(()).unwrap();
        assert!(completion.wait(Duration::from_secs(1)));
        assert!(cleaned.load(Ordering::SeqCst));
    }

    #[test]
    fn completion_callbacks_run_once_after_cleanup_without_holding_the_state_lock() {
        let completion = ElevationCompletion::pending();
        let called = Arc::new(AtomicUsize::new(0));
        let observed = completion.clone();
        let count = called.clone();
        completion.on_complete(move || {
            assert!(observed.is_complete());
            count.fetch_add(1, Ordering::SeqCst);
        });
        assert!(!completion.wait(Duration::ZERO));
        completion.complete();
        completion.complete();
        let count = called.clone();
        completion.on_complete(move || {
            count.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(called.load(Ordering::SeqCst), 2);
    }
}
