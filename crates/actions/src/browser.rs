use crate::{FileTarget, ProcessTarget};
use serde::{Deserialize, Serialize};

/// Fixed destinations. Neither the model nor the local IPC supplies a URL, key or command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserManager {
    FirefoxProcesses,
    ChromeTaskManager,
    EdgeTaskManager,
}
impl BrowserManager {
    pub fn from_executable(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "firefox.exe" => Some(Self::FirefoxProcesses),
            "chrome.exe" => Some(Self::ChromeTaskManager),
            "msedge.exe" => Some(Self::EdgeTaskManager),
            _ => None,
        }
    }
}

/// Local-only, fingerprinted process and executable identities, resolved before approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserManagerTarget {
    pub process: ProcessTarget,
    pub executable: FileTarget,
    pub destination: BrowserManager,
}
impl BrowserManagerTarget {
    pub(crate) fn valid(&self) -> bool {
        self.process
            .executable_name
            .as_deref()
            .and_then(BrowserManager::from_executable)
            == Some(self.destination)
            && self
                .executable
                .path
                .to_str()
                .and_then(|p| p.rsplit(['\\', '/']).next())
                .and_then(BrowserManager::from_executable)
                == Some(self.destination)
            && crate::is_windows_absolute(&self.executable.path)
            && !self.executable.identity.is_directory
            && !self.executable.identity.is_reparse_point
    }
}
