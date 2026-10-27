use crate::{
    ActionKind, Capability, EffectError, EffectOutcome, Effects, FileIdentity, FileTarget, Guard,
    Permit, ProcessIdentity, ProcessTarget, SettingsPage,
};
use pcagent_shared::Status;
use std::{path::Path, time::Duration};

/// Native identity observation. Non-Windows targets report `unsupported`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeGuard;
impl Guard for NativeGuard {
    fn observe_user_startup(
        &self,
        value_name: &str,
    ) -> Result<Option<crate::StartupIdentity>, Status> {
        #[cfg(windows)]
        return crate::windows::startup::observe(value_name);
        #[cfg(target_os = "linux")]
        return super::linux::observe_startup(value_name);
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = value_name;
            Err(Status::Unsupported)
        }
    }
    fn observe_file(&self, path: &Path) -> Result<Option<FileIdentity>, Status> {
        #[cfg(windows)]
        return crate::windows::observe_file(path);
        #[cfg(target_os = "linux")]
        return super::linux::observe_file(path);
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = path;
            Err(Status::Unsupported)
        }
    }
    fn observe_process(&self, pid: u32) -> Result<Option<ProcessIdentity>, Status> {
        #[cfg(windows)]
        return crate::windows::observe_process(pid);
        #[cfg(target_os = "linux")]
        return super::linux::observe_process(pid);
        #[cfg(not(any(windows, target_os = "linux")))]
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
#[cfg(not(any(windows, target_os = "linux")))]
fn unsupported() -> Result<EffectOutcome, EffectError> {
    Err(EffectError::new(
        Status::Unsupported,
        "native actions require Windows",
    ))
}
impl Effects for NativeEffects {
    fn capability(&self, kind: ActionKind) -> Capability {
        #[cfg(target_os = "linux")]
        if kind == ActionKind::RequestNormalClose {
            return Capability::RecommendationOnly("Close the application in its desktop window; this desktop has no verified normal-close protocol.");
        }
        if !cfg!(any(windows, target_os = "linux")) {
            return Capability::Unsupported;
        }
        match kind {
            ActionKind::OpenInExplorer
            | ActionKind::OpenSettingsPage
            | ActionKind::RequestNormalClose
            | ActionKind::RecycleSelectedFiles
            | ActionKind::RunCleanupAdapter
            | ActionKind::DisableUserStartup
            | ActionKind::RestoreUserStartup => Capability::Executable,
        }
    }
    fn open_in_explorer(
        &self,
        _: &Permit,
        file: &FileTarget,
    ) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::open_in_explorer(file);
        #[cfg(target_os = "linux")]
        return super::linux::open_in_explorer(file);
        #[cfg(not(any(windows, target_os = "linux")))]
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
        #[cfg(target_os = "linux")]
        return super::linux::settings(page);
        #[cfg(not(any(windows, target_os = "linux")))]
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
        #[cfg(target_os = "linux")]
        return super::linux::close(process);
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = process;
            unsupported()
        }
    }
    fn recycle_file(&self, _: &Permit, file: &FileTarget) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::recycle_file(file);
        #[cfg(target_os = "linux")]
        return super::linux::recycle_file(file);
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = file;
            unsupported()
        }
    }
    fn set_user_startup(
        &self,
        _: &Permit,
        startup: &crate::StartupTarget,
    ) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::startup::apply(startup);
        #[cfg(target_os = "linux")]
        return super::linux::apply_startup(startup);
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = startup;
            unsupported()
        }
    }
}
