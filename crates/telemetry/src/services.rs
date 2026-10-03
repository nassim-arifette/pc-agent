//! Read-only service-to-process attribution shared by telemetry and system inspection.
#![cfg_attr(not(windows), allow(dead_code))]
use pcagent_shared::Status;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceProcess {
    pub pid: u32,
    pub service_name: String,
    pub display_name: String,
    /// A finite SCM state name; no localized or executable command text.
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceProcesses {
    pub services: Vec<ServiceProcess>,
    pub status: Status,
    pub limitations: Vec<String>,
}

/// Lists all Win32 services. PID attribution is valid only for running or paused services;
/// services starting or stopping keep their state but return PID zero.
pub fn service_processes() -> ServiceProcesses {
    #[cfg(windows)]
    {
        crate::windows::services::collect()
    }
    #[cfg(not(windows))]
    {
        ServiceProcesses {
            services: Vec::new(),
            status: Status::Unsupported,
            limitations: vec!["Service Control Manager collection requires Windows.".into()],
        }
    }
}

pub(crate) fn state_name(state: u32) -> &'static str {
    match state {
        1 => "stopped",
        2 => "start_pending",
        3 => "stop_pending",
        4 => "running",
        5 => "continue_pending",
        6 => "pause_pending",
        7 => "paused",
        _ => "unknown",
    }
}

pub(crate) fn attributed_pid(state: u32, pid: u32) -> u32 {
    match state {
        4..=7 => pid,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_scm_states_with_valid_process_ids_are_attributed() {
        for state in 0..=9 {
            assert_eq!(
                attributed_pid(state, 42),
                if (4..=7).contains(&state) { 42 } else { 0 }
            );
        }
        assert_eq!(state_name(4), "running");
        assert_eq!(state_name(u32::MAX), "unknown");
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_platform_does_not_invent_services() {
        let services = service_processes();
        assert_eq!(services.status, Status::Unsupported);
        assert!(services.services.is_empty());
    }
}
