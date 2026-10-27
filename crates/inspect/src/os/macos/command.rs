//! Closed read-only command runner: bounded time/output, no shell, no raw errors in evidence.
use crate::CoverageStatus;
use std::{
    io::{self, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

pub(super) enum ReadOnly {
    Services,
    Events {
        start_seconds: u64,
        end_seconds: u64,
    },
    Updates,
}

pub(super) struct Failure(pub CoverageStatus);

pub(super) fn run(read: ReadOnly) -> Result<Vec<u8>, Failure> {
    let mut command = match read {
        ReadOnly::Services => {
            let mut command = Command::new("/bin/launchctl");
            command.arg("list");
            command
        }
        ReadOnly::Events {
            start_seconds,
            end_seconds,
        } => {
            let mut command = Command::new("/usr/bin/log");
            command
                .args(["show", "--style", "json", "--timezone", "UTC", "--start"])
                .arg(format!("@{start_seconds}"))
                .arg("--end")
                .arg(format!("@{end_seconds}"))
                .args([
                    "--predicate",
                    "messageType == error OR messageType == fault",
                ]);
            command
        }
        ReadOnly::Updates => {
            let mut command = Command::new("/usr/sbin/softwareupdate");
            command.arg("--list");
            command
        }
    };
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin");
    execute(command, Duration::from_secs(10), 8 * 1024 * 1024)
}

struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Ok(pid) = i32::try_from(self.0.id()) {
            // SAFETY: this child is an unreaped leader of its own newly created process group.
            // Its PID cannot be reused while we own it. No unrelated process group is signalled.
            unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn nonblocking(pipe: &impl AsRawFd) -> Result<(), Failure> {
    // SAFETY: pipe owns a live descriptor; these fcntl operations change only its file flags.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(Failure(CoverageStatus::Error));
    }
    // SAFETY: same live descriptor; preserving the existing flags while enabling nonblocking I/O.
    if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Failure(CoverageStatus::Error));
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, cap: usize) -> Result<bool, Failure> {
    let mut buffer = [0_u8; 4096];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                if count > cap.saturating_sub(bytes.len()) {
                    return Err(Failure(CoverageStatus::Partial));
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Failure(CoverageStatus::Error)),
        }
    }
}

fn execute(mut command: Command, timeout: Duration, cap: usize) -> Result<Vec<u8>, Failure> {
    let mut child = Process(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|error| {
                Failure(match error.kind() {
                    io::ErrorKind::PermissionDenied => CoverageStatus::Denied,
                    io::ErrorKind::NotFound => CoverageStatus::Unsupported,
                    _ => CoverageStatus::Error,
                })
            })?,
    );
    let mut stdout = child
        .0
        .stdout
        .take()
        .ok_or(Failure(CoverageStatus::Error))?;
    let mut stderr = child
        .0
        .stderr
        .take()
        .ok_or(Failure(CoverageStatus::Error))?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let started = Instant::now();
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    loop {
        if started.elapsed() >= timeout {
            return Err(Failure(CoverageStatus::Partial));
        }
        let stdout_done = drain(&mut stdout, &mut output, cap)?;
        let stderr_done = drain(&mut stderr, &mut errors, 64 * 1024)?;
        if let Some(status) = child
            .0
            .try_wait()
            .map_err(|_| Failure(CoverageStatus::Error))?
        {
            if !status.success() {
                let detail = String::from_utf8_lossy(&errors).to_ascii_lowercase();
                return Err(Failure(
                    if [
                        "permission denied",
                        "operation not permitted",
                        "not authorized",
                    ]
                    .iter()
                    .any(|reason| detail.contains(reason))
                    {
                        CoverageStatus::Denied
                    } else {
                        CoverageStatus::Error
                    },
                ));
            }
            if stdout_done && stderr_done {
                return Ok(output);
            }
            // Exit can race the last drain. Read any remaining bytes without waiting on children.
            let stdout_done = drain(&mut stdout, &mut output, cap)?;
            let stderr_done = drain(&mut stderr, &mut errors, 64 * 1024)?;
            return if stdout_done && stderr_done {
                Ok(output)
            } else {
                Err(Failure(CoverageStatus::Partial))
            };
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_commands_finish_and_unbounded_output_is_refused() {
        let bytes = execute(Command::new("/usr/bin/true"), Duration::from_secs(1), 256);
        assert!(bytes.is_ok());
        let result = execute(Command::new("/usr/bin/yes"), Duration::from_secs(1), 256);
        assert!(matches!(result, Err(Failure(CoverageStatus::Partial))));
    }

    #[test]
    fn a_stalled_child_is_killed_and_reaped_before_returning() {
        let mut command = Command::new("/bin/sleep");
        command.arg("60");
        let started = Instant::now();
        let result = execute(command, Duration::from_millis(40), 256);
        assert!(matches!(result, Err(Failure(CoverageStatus::Partial))));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
