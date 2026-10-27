//! Compile-time inspection boundary. Each backend owns its native calls and coverage.
#[cfg(not(any(windows, target_os = "macos")))]
use crate::CoverageStatus;
use crate::{Inspection, Query};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod windows;

pub(crate) trait Inspector {
    fn inspect(query: Query<'_>) -> Inspection;
    fn live_battery() -> Inspection;
    fn region_country() -> Option<String>;
}

pub(crate) struct Native;

#[cfg(not(any(windows, target_os = "macos")))]
impl Inspector for Native {
    fn inspect(_: Query<'_>) -> Inspection {
        let mut result = Inspection::new();
        result.coverage(
            "windows",
            CoverageStatus::Unsupported,
            "Windows APIs",
            "Native inspection is available on Windows only.",
        );
        result.finish()
    }
    fn live_battery() -> Inspection {
        Self::inspect(Query::Battery)
    }
    fn region_country() -> Option<String> {
        None
    }
}

#[cfg(target_os = "macos")]
impl Inspector for Native {
    fn inspect(query: Query<'_>) -> Inspection {
        macos::inspect(query)
    }
    fn live_battery() -> Inspection {
        macos::live_battery()
    }
    fn region_country() -> Option<String> {
        None
    }
}

#[cfg(windows)]
impl Inspector for Native {
    fn inspect(query: Query<'_>) -> Inspection {
        windows::inspect(query)
    }
    fn live_battery() -> Inspection {
        windows::live_battery()
    }
    fn region_country() -> Option<String> {
        windows::region_country()
    }
}
