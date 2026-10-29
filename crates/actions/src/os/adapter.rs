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
    fn observe_browser(
        &self,
        process: &ProcessTarget,
    ) -> Result<Option<crate::BrowserManagerTarget>, Status> {
        #[cfg(windows)]
        return crate::windows::browser::observe(process);
        #[cfg(not(windows))]
        {
            let _ = process;
            Err(Status::Unsupported)
        }
    }
    fn observe_user_startup(
        &self,
        value_name: &str,
    ) -> Result<Option<crate::StartupIdentity>, Status> {
        #[cfg(windows)]
        return crate::windows::startup::observe(value_name);
        #[cfg(not(windows))]
        {
            let _ = value_name;
            Err(Status::Unsupported)
        }
    }
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
    fn open_browser_process_manager(
        &self,
        _: &Permit,
        browser: &crate::BrowserManagerTarget,
    ) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::browser::open(browser);
        #[cfg(not(windows))]
        {
            let _ = browser;
            unsupported()
        }
    }
    fn capability(&self, kind: ActionKind) -> Capability {
        if !cfg!(windows) {
            return Capability::Unsupported;
        }
        match kind {
            ActionKind::OpenInExplorer
            | ActionKind::OpenBrowserProcessManager
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
    fn set_user_startup(
        &self,
        _: &Permit,
        startup: &crate::StartupTarget,
    ) -> Result<EffectOutcome, EffectError> {
        #[cfg(windows)]
        return crate::windows::startup::apply(startup);
        #[cfg(not(windows))]
        {
            let _ = startup;
            unsupported()
        }
    }
}
