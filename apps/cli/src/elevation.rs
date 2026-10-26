//! Bounded elevated inspections, interrupt relay and confirmed helper cleanup.
use crate::local::write_json;
use crate::{args as options, Result};
use pcagent_actions::ActionCancellation;
use pcagent_shared::Status;
use serde_json::json;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub(super) trait CliElevationCompletion {
    fn wait(&self, timeout: Duration) -> bool;
}
impl CliElevationCompletion for pcagent_inspect::elevation::ElevationCompletion {
    fn wait(&self, timeout: Duration) -> bool {
        pcagent_inspect::elevation::ElevationCompletion::wait(self, timeout)
    }
}
struct CliCleanup<C: CliElevationCompletion>(C);
impl<C: CliElevationCompletion> Drop for CliCleanup<C> {
    fn drop(&mut self) {
        // A result, timeout or interrupt is never proof that private resources disappeared.
        while !self.0.wait(Duration::from_millis(100)) {}
    }
}
pub(super) fn with_elevation_cleanup<C: CliElevationCompletion, R>(
    completion: C,
    operation: impl FnOnce() -> R,
) -> R {
    let _cleanup = CliCleanup(completion);
    // The operation owns the prepared job; its resources are dropped before this guard waits,
    // including preview/approval errors and unwinding through the operation.
    operation()
}

struct InterruptRelayStop<'a> {
    finished: &'a AtomicBool,
    thread: std::thread::Thread,
}
impl Drop for InterruptRelayStop<'_> {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Release);
        self.thread.unpark();
    }
}
pub(super) fn with_elevation_interrupt<R>(
    signal: &AtomicBool,
    operation: impl FnOnce(&pcagent_storage::CancelToken) -> R,
) -> io::Result<R> {
    let cancel = pcagent_storage::CancelToken::default();
    // Cover an interrupt delivered before the relay gets its first scheduling opportunity.
    if signal.load(Ordering::SeqCst) {
        cancel.cancel();
    }
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let relay = std::thread::Builder::new()
            .name("pcagent-cli-elevation-interrupt".into())
            .spawn_scoped(scope, || {
                while !finished.load(Ordering::Acquire) {
                    if signal.load(Ordering::SeqCst) {
                        cancel.cancel();
                    }
                    std::thread::park_timeout(Duration::from_millis(20));
                }
            })?;
        let _stop = InterruptRelayStop {
            finished: &finished,
            thread: relay.thread().clone(),
        };
        Ok(operation(&cancel))
    })
}

pub(super) fn inspect(options: options::ElevatedInspect) -> Result<u8> {
    let options::ElevatedInspect {
        task,
        volume,
        pid,
        creation_time_filetime,
        duration_seconds,
        timeout_seconds,
        approve_local,
    } = options;
    use pcagent_inspect::elevation::{DriveLetter, ElevatedTask, PreparedElevation};
    let task = match task.as_str() {
        "system-caches" => ElevatedTask::SystemCaches,
        "mft" => ElevatedTask::MftInventory {
            volume: DriveLetter::parse(&volume)?,
        },
        "shadow-storage" => ElevatedTask::ShadowStorage,
        "disk-reliability" => ElevatedTask::DiskReliability,
        "battery-energy" => ElevatedTask::BatteryEnergy,
        "performance-trace" => ElevatedTask::PerformanceTrace {
            duration_seconds: duration_seconds.unwrap_or(30),
        },
        "process-image" => ElevatedTask::ProcessImages {
            targets: vec![pcagent_inspect::elevation::system::ProcessImageTarget {
                pid: pid.ok_or("process-image requires --pid and --creation-time-filetime")?,
                creation_time_filetime: creation_time_filetime
                    .ok_or("process-image requires --pid and --creation-time-filetime")?,
            }],
        },
        _ => return Err("Unsupported elevated task.".into()),
    };
    if !matches!(task, ElevatedTask::ProcessImages { .. })
        && (pid.is_some() || creation_time_filetime.is_some())
    {
        return Err("Process identity flags apply only to process-image.".into());
    }
    if duration_seconds.is_some() && !matches!(task, ElevatedTask::PerformanceTrace { .. }) {
        return Err("Duration applies only to performance-trace.".into());
    }
    if matches!(task, ElevatedTask::PerformanceTrace { .. }) {
        write_json(
            &json!({"task":task,"state":"unsupported","limits":["cleanup_unvalidated"],
            "trace_session_started":false,"note":"Private session cleanup after hard termination still requires validation; no UAC, trace or ACL changes are performed."}),
        )?;
        return Ok(3);
    }
    // Keep Ctrl+C handled until confirmed cleanup, including a declined prepared job.
    let interruption = ActionCancellation::begin()?;
    with_elevation_interrupt(interruption.flag(), |cancel| -> Result<u8> {
        if cancel.is_cancelled() {
            return Err(pcagent_inspect::elevation::ElevationError::Cancelled.into());
        }
        let prepared =
            PreparedElevation::prepare(task.clone(), Duration::from_secs(timeout_seconds))?;
        let completion = prepared.completion();
        with_elevation_cleanup(completion, move || {
            let mut prepared = prepared;
            if cancel.is_cancelled() {
                return Err(pcagent_inspect::elevation::ElevationError::Cancelled.into());
            }
            if !approve_local {
                write_json(
                    &json!({"task":task,"budget_seconds":timeout_seconds,"state":"awaiting_local_approval",
                    "note":"One fixed read-only metadata inspection. It never deletes files or installs drivers. MFT and historical energy coverage remain explicitly partial. Re-run with --approve-local after reviewing this fixed task, then approve the Windows UAC prompt."}),
                )?;
                return Ok(0);
            }
            let approval = prepared.approve_locally()?;
            let response = prepared.run_locally_bounded(approval, cancel)?;
            let inspection = pcagent_inspect::elevation::to_inspection(&response);
            let mut local = serde_json::to_value(&inspection)?;
            if let Some(collection) = &response.mft {
                local["local_mft_tree"] = serde_json::to_value(&collection.tree)?;
            }
            write_json(&local)?;
            Ok(
                if matches!(inspection.status, Status::Ok | Status::Partial) {
                    0
                } else {
                    3
                },
            )
        })
    })?
}
