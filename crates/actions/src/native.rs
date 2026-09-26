use crate::{
    ActionKind, Capability, EffectError, EffectOutcome, Effects, FileIdentity, FileTarget, Guard,
    Permit, ProcessIdentity, ProcessTarget, ProtectionCheck, SettingsPage,
};
use pcagent_shared::Status;
use std::{path::Path, time::Duration};

/// `X:\...` form only; UNC, device (`\\?\`), relative and drive-relative paths are rejected.
pub fn is_windows_absolute(path: &Path) -> bool {
    let Some(s) = path.to_str() else {
        return false;
    };
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// Single raw argument for `explorer.exe`. Characters that Windows forbids in file names (and that
/// could change how Explorer splits its command line) are refused rather than escaped.
pub fn explorer_select_arg(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    let forbidden = |c: char| c.is_control() || "\"<>|?*".contains(c) || c == '/';
    if !is_windows_absolute(path) || s[2..].contains(':') || s.chars().any(forbidden) {
        return None;
    }
    Some(format!("/select,\"{s}\""))
}

/// Minimal fail-closed baseline until the policy crate provides user exclusions.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaselineProtection;
impl ProtectionCheck for BaselineProtection {
    fn is_protected(&self, path: &Path) -> bool {
        const ROOTS: [&str; 9] = [
            "windows",
            "program files",
            "program files (x86)",
            "programdata",
            "$recycle.bin",
            "system volume information",
            "recovery",
            "boot",
            "$windows.~bt",
        ];
        let Some(s) = path.to_str().filter(|_| is_windows_absolute(path)) else {
            return true;
        };
        let parts: Vec<String> = s[3..].split('\\').map(str::to_ascii_lowercase).collect();
        parts
            .first()
            .is_none_or(|p| p.is_empty() || ROOTS.contains(&p.as_str()))
            || parts
                .iter()
                .any(|p| p.is_empty() || p == "." || p == ".." || p.ends_with(['.', ' ']))
    }
}

/// Native identity observation. Non-Windows targets report `unsupported`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeGuard;
impl Guard for NativeGuard {
    fn observe_file(&self, path: &Path) -> Result<Option<FileIdentity>, Status> {
        #[cfg(windows)]
        return crate::windows::observe_file(path);
        #[cfg(not(windows))]
        {
            let _ = path;
            Err(Status::Unsupported)
        }
    }
    fn observe_process(&self, pid: u32) -> Result<Option<ProcessIdentity>, Status> {
        #[cfg(windows)]
        return crate::windows::observe_process(pid);
        #[cfg(not(windows))]
        {
            let _ = pid;
            Err(Status::Unsupported)
        }
    }
}

/// Native catalog effects. Non-Windows targets report `unsupported` for every action.
#[derive(Debug, Clone, Copy)]
pub struct NativeEffects {
    /// Bounded wait for a process to exit after `WM_CLOSE`; clamped to 30 s.
    pub close_wait: Duration,
}
impl Default for NativeEffects {
    fn default() -> Self {
        Self {
            close_wait: Duration::from_secs(5),
        }
    }
}
#[cfg(not(windows))]
fn unsupported() -> Result<EffectOutcome, EffectError> {
    Err(EffectError::new(
        Status::Unsupported,
        "native actions require Windows",
    ))
}
impl Effects for NativeEffects {
    fn capability(&self, kind: ActionKind) -> Capability {
        if !cfg!(windows) {
            return Capability::Unsupported;
        }
        match kind {
            ActionKind::OpenInExplorer
            | ActionKind::OpenSettingsPage
            | ActionKind::RequestNormalClose => Capability::Executable,
            ActionKind::RecycleSelectedFiles => Capability::RecommendationOnly(
                "the shell operation reopens the path; target binding is verified after the \
                 operation, not guaranteed between validation and operation",
            ),
            ActionKind::RunCleanupAdapter => Capability::Unsupported,
        }
    }
    fn open_in_explorer(
        &self,
        _: &Permit,
        file: &FileTarget,
    ) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::open_in_explorer(file);
        #[cfg(not(windows))]
        {
            let _ = file;
            unsupported()
        }
    }
    fn open_settings_page(
        &self,
        _: &Permit,
        page: SettingsPage,
    ) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::open_settings_page(page);
        #[cfg(not(windows))]
        {
            let _ = page;
            unsupported()
        }
    }
    fn request_normal_close(
        &self,
        _: &Permit,
        process: &ProcessTarget,
    ) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::request_normal_close(
            process,
            self.close_wait.min(Duration::from_secs(30)),
        );
        #[cfg(not(windows))]
        {
            let _ = process;
            unsupported()
        }
    }
    fn recycle_file(&self, _: &Permit, file: &FileTarget) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::recycle_file(file);
        #[cfg(not(windows))]
        {
            let _ = file;
            unsupported()
        }
    }
}
