use crate::CoverageStatus as C;
use std::{
    io::Read,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Fixed native programs only. No shell, stdin, inherited pager or user-supplied command.
pub(super) fn run(program: &str, args: &[&str]) -> Result<String, C> {
    if !matches!(
        program,
        "/usr/bin/busctl"
            | "/usr/bin/systemctl"
            | "/usr/bin/journalctl"
            | "/usr/bin/apt"
            | "/usr/bin/dnf"
            | "/usr/bin/flatpak"
            | "/usr/bin/dpkg-query"
            | "/usr/bin/rpm"
            | "/usr/sbin/smartctl"
            | "/usr/bin/udevadm"
    ) {
        return Err(C::Unsupported);
    }
    let mut child = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .env("SYSTEMD_PAGER", "")
        .env("SYSTEMD_COLORS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => C::Unsupported,
            std::io::ErrorKind::PermissionDenied => C::Denied,
            _ => C::Error,
        })?;
    let stdout = child.stdout.take().ok_or(C::Error)?;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(4_194_305)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let clock = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if clock.elapsed() < Duration::from_secs(8) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(C::Partial);
            }
        }
    };
    let bytes = rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| C::Partial)?
        .map_err(|_| C::Error)?;
    if bytes.len() > 4_194_304 {
        return Err(C::Partial);
    }
    if !(status.success() || program == "/usr/bin/dnf" && status.code() == Some(100)) {
        // smartctl bit flags report drive health as well as invocation errors.
        if program != "/usr/sbin/smartctl" || status.code().is_none_or(|code| code & 7 != 0) {
            return Err(C::Denied);
        }
    }
    String::from_utf8(bytes).map_err(|_| C::Error)
}
