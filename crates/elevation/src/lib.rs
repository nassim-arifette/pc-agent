//! Locally approved elevation for one fixed, read-only task. No model or network entry point.
#![deny(unsafe_code)]
#[cfg(any(windows, test))]
mod cache;
pub mod energy;
#[cfg(windows)]
#[allow(unsafe_code)]
mod energy_native;
mod job;
mod lifecycle;
#[cfg(unix)]
#[allow(unsafe_code)]
mod unix_host;
pub use pcagent_mft as mft;
#[cfg(windows)]
#[allow(unsafe_code)]
mod mft_native;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
mod protocol;
pub mod system;
#[cfg(windows)]
#[allow(unsafe_code)]
mod system_native;
pub mod trace;
#[cfg(windows)]
#[allow(unsafe_code)]
mod trace_native;
mod worker;

pub use job::{ElevationApproval, PreparedElevation};
pub use lifecycle::ElevationCompletion;
pub use protocol::{
    CacheCategory, CacheCoverage, CacheSummary, DriveLetter, ElevatedTask, ElevationError,
    ElevationLimit, ElevationRequest, ElevationResponse, ElevationStatus, WorkerArguments,
    MAX_BUDGET_MS, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, PROTOCOL_VERSION,
};
pub use worker::worker_main;

/// The fixed helper's file name; it is only ever launched from the running app's own folder.
pub const HELPER_FILE_NAME: &str = "pcagent-elevated.exe";

/// Whether the fixed helper is installed next to the running app, so that a host can offer
/// locally approved tasks at all. One file-system check, no launch; the launch checks again.
/// Always false on other systems.
pub fn helper_installed() -> bool {
    cfg!(windows)
        && std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join(HELPER_FILE_NAME)))
            .is_some_and(|helper| helper.is_file())
}

/// The desktop and model host must remain unelevated; only the fixed worker may be admin.
pub fn ensure_unelevated_host() -> Result<(), ElevationError> {
    #[cfg(windows)]
    if native::is_elevated()? {
        return Err(ElevationError::ElevatedHost);
    }
    #[cfg(unix)]
    if unix_host::is_privileged() {
        return Err(ElevationError::ElevatedHost);
    }
    Ok(())
}

pub fn elevated_host_notice(french: bool) -> (&'static str, &'static str) {
    #[cfg(not(unix))]
    let body = if french {
        "Pour votre sécurité, PC Agent ne s'exécute pas en tant qu'administrateur.\n\nFermez ce message, puis relancez PC Agent normalement (double-clic, sans « Exécuter en tant qu'administrateur »).\n\nLes analyses qui demandent des droits administrateur se lancent depuis l'application, avec l'invite de Windows."
    } else {
        "For your safety, PC Agent does not run as administrator.\n\nClose this message, then start PC Agent normally (double-click, without \"Run as administrator\").\n\nChecks that need administrator rights are started from inside the app, through the Windows prompt."
    };
    #[cfg(unix)]
    let body = if french {
        "Pour votre sécurité, PC Agent ne s'exécute pas avec les privilèges root ou setuid.\n\nFermez ce message, puis relancez PC Agent depuis votre session utilisateur, sans sudo.\n\nSeul le helper d'une vérification explicitement approuvée peut être élevé."
    } else {
        "For your safety, PC Agent does not run with root or setuid privileges.\n\nClose this message, then start PC Agent from your regular user session, without sudo.\n\nOnly the helper for an explicitly approved check may be elevated."
    };
    ("PC Agent", body)
}

pub fn notify_elevated_host() {
    #[cfg(windows)]
    native::show_elevated_host_notice();
}

/// Whether Windows shows its own interface in French, for the few texts drawn outside the page
/// (the notice above, the tray menu and the folder dialog's title). False on other systems.
pub fn windows_ui_is_french() -> bool {
    #[cfg(windows)]
    return native::ui_language_is_french();
    #[cfg(not(windows))]
    false
}

#[cfg(test)]
mod tests {
    use super::elevated_host_notice;

    #[test]
    fn elevated_host_notice_has_localized_bodies() {
        let (_, french) = elevated_host_notice(true);
        let (_, english) = elevated_host_notice(false);

        assert!(!french.is_empty());
        assert!(!english.is_empty());
        assert_ne!(french, english);
        assert!(french.contains("Fermez"));
        assert!(french.contains("relancez"));
        assert!(!french.contains(" tu "));
        assert!(!french.contains('\0'));
        assert!(!english.contains('\0'));
    }
}
