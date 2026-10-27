use super::*;
use std::process::{Command, Stdio};
fn launch(program: &str, args: &[&OsStr]) -> Result<EffectOutcome, EffectError> {
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map(|mut child|{std::thread::spawn(move||{let _=child.wait();});EffectOutcome::Done(Some("Requested the desktop application to open; completion is controlled by the desktop.".into()))})
        .map_err(|_|EffectError::new(Status::Unsupported,"The desktop application is not installed or could not be started."))
}
pub(super) fn open(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    if observe_file(&file.path).ok().flatten().as_ref() != Some(&file.identity) {
        return Err(error("File changed since review."));
    }
    let path = if file.identity.is_directory {
        file.path.as_path()
    } else {
        file.path
            .parent()
            .ok_or_else(|| error("No parent folder."))?
    };
    launch("/usr/bin/xdg-open", &[path.as_os_str()])
}
pub(super) fn settings(page: SettingsPage) -> Result<EffectOutcome, EffectError> {
    let panel = match page {
        SettingsPage::PowerSleep | SettingsPage::BatteryUsage => "power",
        SettingsPage::AppsFeatures => "applications",
        SettingsPage::StorageSense => "usage",
        SettingsPage::WindowsUpdate | SettingsPage::WindowsUpdateOptionalUpdates => {
            return launch("/usr/bin/update-manager", &[]);
        }
        SettingsPage::StartupApps => return launch("/usr/bin/gnome-session-properties", &[]),
    };
    launch("/usr/bin/gnome-control-center", &[OsStr::new(panel)])
}
pub(super) fn close(_process: &ProcessTarget) -> Result<EffectOutcome, EffectError> {
    Err(EffectError::new(Status::Unsupported,"This desktop has no verified normal-close protocol. No termination signal or force-kill is substituted."))
}
