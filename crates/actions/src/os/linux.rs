//! Unprivileged Linux effects. All entry points are behind the broker's consumed Permit.
mod desktop;
mod startup;
mod trash;

use crate::{
    EffectError, EffectOutcome, FileTarget, ProcessIdentity, ProcessTarget,
    SettingsPage, StartupIdentity, StartupTarget,
};
use pcagent_shared::Status;
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, FileType, Mode, OFlags},
};
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
};
pub(super) use super::unix::observe_file;
use super::unix::{clean, directory, error, identity, io_status, parent, pinned};
pub(super) fn observe_process(pid: u32) -> Result<Option<ProcessIdentity>, Status> {
    let text = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_status(e)),
    };
    let (_, fields) = text.rsplit_once(") ").ok_or(Status::Partial)?;
    let created = fields
        .split_whitespace()
        .nth(19)
        .and_then(|s| s.parse().ok())
        .ok_or(Status::Partial)?;
    let stat = std::fs::read_to_string("/proc/stat").map_err(io_status)?;
    let boot = stat
        .lines()
        .find_map(|l| l.strip_prefix("btime ")?.parse::<u64>().ok())
        .ok_or(Status::Partial)?;
    Ok(Some(ProcessIdentity {
        pid,
        creation_time_filetime: created,
        boot_id: Some(format!("boot-estimate-unix-s-{}", boot / 60 * 60)),
    }))
}
pub(super) fn observe_startup(name: &str) -> Result<Option<StartupIdentity>, Status> {
    startup::observe(name)
}
pub(super) fn apply_startup(target: &StartupTarget) -> Result<EffectOutcome, EffectError> {
    startup::apply(target)
}
pub(super) fn recycle_file(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    trash::recycle(file)
}
pub(super) fn open_in_explorer(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    desktop::open(file)
}
pub(super) fn settings(page: SettingsPage) -> Result<EffectOutcome, EffectError> {
    desktop::settings(page)
}
pub(super) fn close(process: &ProcessTarget) -> Result<EffectOutcome, EffectError> {
    desktop::close(process)
}
fn xdg(env: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(env)
        .map(PathBuf::from)
        .filter(|p| clean(p))
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(fallback))
                .filter(|p| clean(p))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Effects, NativeEffects};

    #[test]
    fn native_process_identity_matches_telemetry_units_and_never_force_closes() {
        let identity = observe_process(std::process::id()).unwrap().unwrap();
        assert!(identity.creation_time_filetime > 0);
        let process = ProcessTarget {
            identity,
            executable_name: None,
        };
        let result =
            NativeEffects::default().request_normal_close(&crate::broker::test_permit(), &process);
        assert!(result.is_err());
    }
}
