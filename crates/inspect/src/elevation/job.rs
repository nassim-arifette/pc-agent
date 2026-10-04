use super::{
    lifecycle::ElevationCompletion,
    protocol::{ElevatedTask, ElevationError, ElevationRequest, ElevationResponse},
};
use pcagent_storage::CancelToken;
use std::time::Duration;

/// Local state only: this object cannot be supplied through a model tool or JSON request.
pub struct PreparedElevation {
    request: ElevationRequest,
    approval_issued: bool,
    completion: ElevationCompletion,
    #[cfg(windows)]
    job: std::sync::Arc<super::native::NativeJob>,
}

/// A consumable permit produced by the local approval gesture. It is neither Clone nor serde.
pub struct ElevationApproval {
    job_id: String,
    nonce: String,
}

impl PreparedElevation {
    /// Prepare a private, immutable request without prompting for elevation.
    pub fn prepare(task: ElevatedTask, budget: Duration) -> Result<Self, ElevationError> {
        let request = ElevationRequest::new(task, budget)?;
        // No UAC prompt, trace session or ACL changes until hard-exit cleanup is validated.
        if matches!(request.task, ElevatedTask::PerformanceTrace { .. }) {
            return Err(ElevationError::Unsupported);
        }
        super::ensure_unelevated_host()?;
        #[cfg(windows)]
        {
            let job = std::sync::Arc::new(super::native::NativeJob::create(&request)?);
            Ok(Self {
                request,
                approval_issued: false,
                completion: job.completion(),
                job,
            })
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            Err(ElevationError::Unsupported)
        }
    }
    pub fn request(&self) -> &ElevationRequest {
        &self.request
    }

    /// Retain this capability before a bounded run; its result alone does not permit host exit.
    pub fn completion(&self) -> ElevationCompletion {
        self.completion.clone()
    }

    /// Call only from the approved local UI/CLI gesture, after displaying this fixed task.
    pub fn approve_locally(&mut self) -> Result<ElevationApproval, ElevationError> {
        self.request.validate_at(pcagent_shared::unix_ms())?;
        if self.approval_issued {
            return Err(ElevationError::ApprovalConsumed);
        }
        self.approval_issued = true;
        Ok(ElevationApproval {
            job_id: self.request.job_id.clone(),
            nonce: self.request.nonce.clone(),
        })
    }

    /// Consume the job once and wait for confirmed exit/cleanup before returning to a CLI caller.
    /// Windows owns its consent UI; a pending consent dialog can extend this cleanup wait.
    pub fn run_locally(
        self,
        approval: ElevationApproval,
        cancel: &CancelToken,
    ) -> Result<ElevationResponse, ElevationError> {
        let completion = self.completion();
        let result = self.run_locally_bounded(approval, cancel);
        completion.wait_until_complete();
        result
    }

    /// Return an operation result within its budget while retaining supervision of late cleanup.
    /// The host must retain completion() and stay alive until it reports confirmed cleanup.
    /// Call on a blocking worker, never on the GUI thread.
    pub fn run_locally_bounded(
        self,
        approval: ElevationApproval,
        cancel: &CancelToken,
    ) -> Result<ElevationResponse, ElevationError> {
        validate_approval(&self.request, self.approval_issued, &approval, cancel)?;
        #[cfg(windows)]
        {
            super::native::run_approved(self.job, &self.request, cancel)
        }
        #[cfg(not(windows))]
        {
            Err(ElevationError::Unsupported)
        }
    }
}

fn validate_approval(
    request: &ElevationRequest,
    issued: bool,
    approval: &ElevationApproval,
    cancel: &CancelToken,
) -> Result<(), ElevationError> {
    request.validate_at(pcagent_shared::unix_ms())?;
    if !issued || approval.job_id != request.job_id || approval.nonce != request.nonce {
        return Err(ElevationError::ApprovalRequired);
    }
    if cancel.is_cancelled() {
        return Err(ElevationError::Cancelled);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_is_bound_to_one_request_and_cancelled_before_launch() {
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let approval = ElevationApproval {
            job_id: request.job_id.clone(),
            nonce: request.nonce.clone(),
        };
        let cancel = CancelToken::default();
        assert_eq!(
            validate_approval(&request, false, &approval, &cancel),
            Err(ElevationError::ApprovalRequired)
        );
        let another =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        assert_eq!(
            validate_approval(&another, true, &approval, &cancel),
            Err(ElevationError::ApprovalRequired)
        );
        assert!(validate_approval(&request, true, &approval, &cancel).is_ok());
        cancel.cancel();
        assert_eq!(
            validate_approval(&request, true, &approval, &cancel),
            Err(ElevationError::Cancelled)
        );
    }
}
