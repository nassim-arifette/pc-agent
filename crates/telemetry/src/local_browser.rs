//! Ephemeral, desktop-only browser text. Never part of a measurement, selection source,
//! recording, model projection or persistence DTO. Deliberately no Debug or Deserialize.
use crate::browser::BrowserFamily;
use pcagent_shared::{ProcessInstance, Status};
use serde::Serialize;

#[derive(Clone)]
pub struct BrowserWindowTarget {
    pub instance: ProcessInstance,
    pub family: BrowserFamily,
}

#[derive(Serialize)]
pub struct LocalTab {
    pub title: String,
    pub selected: Option<bool>,
}

#[derive(Serialize)]
pub struct LocalBrowserWindow {
    pub process_ref: String,
    pub family: BrowserFamily,
    /// Window caption (usually the active tab); not a measured mapping to a renderer.
    pub title: String,
    pub tabs: Vec<LocalTab>,
    pub tabs_status: Status,
}

#[derive(Serialize)]
pub struct LocalBrowserWindows {
    pub status: Status,
    pub windows: Vec<LocalBrowserWindow>,
}

impl LocalBrowserWindows {
    pub fn unavailable(status: Status) -> Self {
        Self {
            status,
            windows: Vec::new(),
        }
    }
}

/// Read local captions; enumerate accessible tab controls only when explicitly requested.
/// Neither window handles nor arbitrary automation selectors are accepted from the desktop.
pub fn read(targets: &[BrowserWindowTarget], include_tabs: bool) -> LocalBrowserWindows {
    #[cfg(windows)]
    {
        crate::windows::browser_windows::read(targets, include_tabs)
    }
    #[cfg(not(windows))]
    {
        let _ = (targets, include_tabs);
        LocalBrowserWindows::unavailable(Status::Unsupported)
    }
}
