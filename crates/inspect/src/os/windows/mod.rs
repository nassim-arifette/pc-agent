use crate::{CoverageStatus as C, Inspection, Query};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

pub(crate) mod autostart;
mod battery;
mod cleanup;
mod compatibility;
mod drivers;
mod events;
mod hardware;
mod power_counters;
mod registry;
mod sensors;
mod startup;
mod system;
mod updates;
mod wmi;

pub(crate) fn region_country() -> Option<String> {
    registry::region_country()
}

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub(crate) fn error(code: u32) -> C {
    match code {
        5 | 1314 => C::NeedsAdmin,
        2 | 3 | 50 | 1168 => C::Unsupported,
        _ => C::Error,
    }
}
pub(crate) fn com_error(err: &::windows::core::Error) -> C {
    if err.code().0 as u32 == 0x80070005 || err.code().0 as u32 == 0x80041003 {
        C::NeedsAdmin
    } else if err.code().0 as u32 == 0x80041010 || err.code().0 as u32 == 0x8004100e {
        C::Unsupported
    } else {
        C::Error
    }
}

/// At most one shared native worker exists (the live battery read has its own, battery-only
/// worker: `live_battery`). A hung provider cannot accumulate detached workers or hold the
/// agent's turn indefinitely. Timeouts do not imply that a native call was cancelled.
pub(crate) fn inspect(query: Query<'_>) -> Inspection {
    static BUSY: AtomicBool = AtomicBool::new(false);
    let query = match query {
        Query::Allowlisted { name, limit } => {
            match crate::QUERY_NAMES.iter().copied().find(|n| *n == name) {
                Some(name) => Query::Allowlisted { name, limit },
                None => {
                    let mut out = Inspection::new();
                    out.coverage(
                        "query",
                        C::Denied,
                        "Fixed local allowlist",
                        "Unknown query; no native call was made.",
                    );
                    return out.finish();
                }
            }
        }
        Query::Startup => Query::Startup,
        Query::Services { limit } => Query::Services { limit },
        Query::Events { days, limit } => Query::Events { days, limit },
        Query::SystemState => Query::SystemState,
        Query::Power => Query::Power,
        Query::Battery => Query::Battery,
        Query::Apps { limit } => Query::Apps { limit },
        Query::Hardware => Query::Hardware,
        Query::Drivers {
            limit,
            windows_update,
        } => Query::Drivers {
            limit,
            windows_update,
        },
        Query::Cleanup => Query::Cleanup,
    };
    if BUSY.swap(true, Ordering::AcqRel) {
        let mut out = Inspection::new();
        out.coverage(
            "collector",
            C::Partial,
            "Bounded native worker",
            "A previous inspection is still running; try again after it finishes.",
        );
        return out.finish();
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("pcagent-inspect".into())
        .spawn(move || {
            struct Release;
            impl Drop for Release {
                fn drop(&mut self) {
                    BUSY.store(false, Ordering::Release);
                }
            }
            let _release = Release;
            // COM error values may retain IErrorInfo interfaces. Keep the apartment alive until
            // every collector result/error has been consumed, not merely until a query returns.
            let _apartment = wmi::ComApartment::new().ok();
            let mut out = Inspection::new();
            let mut cleanup = None;
            match query {
                Query::Startup => startup::collect(&mut out),
                Query::Services { limit } => system::services(&mut out, limit.clamp(1, 500)),
                Query::Events { days, limit } => {
                    events::collect(&mut out, days.clamp(1, 30), limit.clamp(1, 200), None)
                }
                Query::SystemState => system::state(&mut out),
                Query::Power => system::power(&mut out),
                Query::Battery => battery::collect(&mut out),
                Query::Apps { limit } => registry::apps(&mut out, limit.clamp(1, 500)),
                Query::Hardware => hardware::collect(&mut out),
                Query::Drivers {
                    limit,
                    windows_update,
                } => cleanup = drivers::collect(&mut out, limit.clamp(1, 100), windows_update),
                Query::Cleanup => cleanup::collect(&mut out),
                Query::Allowlisted { name, limit } => match name {
                    "system_events" => {
                        events::collect(&mut out, 7, limit.clamp(1, 200), Some("System"))
                    }
                    "application_events" => {
                        events::collect(&mut out, 7, limit.clamp(1, 200), Some("Application"))
                    }
                    "windows_version" => system::version(&mut out),
                    _ => wmi::named(&mut out, name, limit.clamp(1, 100)),
                },
            }
            let out = out.finish();
            let _ = tx.send(out);
            if let Some(cleanup) = cleanup {
                cleanup.finish();
            }
            drop(_apartment);
            drop(_release);
        });
    if spawned.is_err() {
        BUSY.store(false, Ordering::Release);
    }
    rx.recv_timeout(Duration::from_secs(15)).unwrap_or_else(|_| {
        let mut out = Inspection::new();
        out.coverage("collector", C::Partial, "Bounded native worker", "Inspection did not finish within 15 seconds; no incomplete values are represented as zero.");
        out.finish()
    })
}

/// Battery-only read for the live window on a second, battery-only worker (see
/// `crate::live_battery`). It needs no COM apartment: the battery adapter uses SetupAPI and
/// DeviceIoControl only. Six seconds is the four-second scan budget plus a margin.
pub(crate) fn live_battery() -> Inspection {
    static LIVE: AtomicBool = AtomicBool::new(false);
    crate::bounded(&LIVE, "pcagent-battery", Duration::from_secs(6), || {
        let mut out = Inspection::new();
        battery::collect(&mut out);
        out.finish()
    })
}
