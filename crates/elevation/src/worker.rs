use super::protocol::{ElevationError, WorkerArguments};
use std::ffi::OsString;

/// The separate helper's only entry point. Arguments carry no command, target path or output path.
pub fn worker_main(args: impl IntoIterator<Item = OsString>) -> Result<(), ElevationError> {
    let args: Vec<_> = args.into_iter().take(9).collect();
    #[cfg(windows)]
    if args.as_slice() == [OsString::from("--trace-guardian")] {
        return super::trace_guardian::worker();
    }
    let arguments = WorkerArguments::parse(args)?;
    #[cfg(windows)]
    {
        let result = super::native::worker(arguments);
        let cleanup = super::trace_native::abort_owned_session();
        result.and(cleanup)
    }
    #[cfg(not(windows))]
    {
        let _ = arguments;
        Err(ElevationError::Unsupported)
    }
}

#[cfg(any(windows, test))]
pub(crate) trait ElevatedProcess {
    fn poll(&mut self) -> Result<Option<u32>, ElevationError>;
    fn terminate(&mut self) -> Result<(), ElevationError>;
    fn has_exited(&mut self) -> Result<bool, ElevationError> {
        self.poll().map(|code| code.is_some())
    }
}

#[cfg(any(windows, test))]
pub(crate) fn supervise(
    process: &mut impl ElevatedProcess,
    cancel: &pcagent_storage::CancelToken,
    budget: std::time::Duration,
) -> Result<(), ElevationError> {
    let clock = std::time::Instant::now();
    loop {
        let failure = if cancel.is_cancelled() {
            Some(ElevationError::Cancelled)
        } else if clock.elapsed() >= budget {
            Some(ElevationError::TimedOut)
        } else {
            None
        };
        if let Some(error) = failure {
            process.terminate()?;
            return Err(error);
        }
        match process.poll() {
            Ok(Some(0)) => return Ok(()),
            Ok(Some(124)) => return Err(ElevationError::TimedOut),
            Ok(Some(125)) => return Err(ElevationError::Cancelled),
            Ok(Some(code)) => return Err(ElevationError::HelperFailed(code)),
            Err(error) => {
                let _ = process.terminate();
                return Err(error);
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_storage::CancelToken;
    use std::time::Duration;

    struct FakeProcess {
        exit: Option<u32>,
        terminated: bool,
    }
    impl ElevatedProcess for FakeProcess {
        fn poll(&mut self) -> Result<Option<u32>, ElevationError> {
            Ok(self.exit)
        }
        fn terminate(&mut self) -> Result<(), ElevationError> {
            self.terminated = true;
            Ok(())
        }
    }
    #[test]
    fn cancellation_and_timeout_only_terminate_the_owned_helper() {
        let cancel = CancelToken::default();
        let mut helper = FakeProcess {
            exit: None,
            terminated: false,
        };
        assert_eq!(
            supervise(&mut helper, &cancel, Duration::ZERO),
            Err(ElevationError::TimedOut)
        );
        assert!(helper.terminated);
        let mut helper = FakeProcess {
            exit: None,
            terminated: false,
        };
        cancel.cancel();
        assert_eq!(
            supervise(&mut helper, &cancel, Duration::from_secs(1)),
            Err(ElevationError::Cancelled)
        );
        assert!(helper.terminated);
    }
    #[test]
    fn helper_exit_and_uac_refusal_are_explicit() {
        let cancel = CancelToken::default();
        for (code, expected) in [
            (0, Ok(())),
            (124, Err(ElevationError::TimedOut)),
            (125, Err(ElevationError::Cancelled)),
            (3, Err(ElevationError::HelperFailed(3))),
        ] {
            let mut helper = FakeProcess {
                exit: Some(code),
                terminated: false,
            };
            assert_eq!(
                supervise(&mut helper, &cancel, Duration::from_secs(1)),
                expected
            );
            assert!(!helper.terminated);
        }
        // Preserve ERROR_CANCELLED without invoking ShellExecuteEx/UAC.
        assert_eq!(
            super::super::protocol::launch_error(1223),
            ElevationError::UacDeclined
        );
        assert_eq!(
            super::super::protocol::launch_error(5),
            ElevationError::LaunchFailed(5)
        );
    }
}
