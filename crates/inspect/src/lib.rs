//! Bounded, read-only native inspections. No commands, file contents or machine identifiers.
//! Display names are kept separately and are never serialized implicitly.
#![deny(unsafe_code)]
use os::Inspector;
use pcagent_shared::{new_id, unix_ms, Origin, Status, Window};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

pub mod autostart;
pub mod elevation;

mod os;
#[cfg_attr(not(windows), allow(dead_code))]
mod parse;
#[cfg(windows)]
use os::windows;

pub const QUERY_NAMES: &[&str] = &[
    "operating_system",
    "processor",
    "physical_memory",
    "pagefile",
    "last_updates",
    "thermal_zones",
    "storage_devices",
    "system_events",
    "application_events",
    "windows_version",
];

#[derive(Clone, Copy, Debug)]
pub enum Query<'a> {
    Startup,
    Services {
        limit: usize,
    },
    Events {
        days: u32,
        limit: usize,
    },
    SystemState,
    Power,
    /// Battery-only reads for recording, without querying thermal/WMI providers.
    Battery,
    Apps {
        limit: usize,
    },
    Hardware,
    Drivers {
        limit: usize,
        windows_update: WindowsUpdateMode,
    },
    Cleanup,
    Allowlisted {
        name: &'a str,
        limit: usize,
    },
}

/// Offline reads the local Windows Update cache. Online requires an explicit request;
/// it can contact the configured update service but never downloads or installs a driver.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsUpdateMode {
    None,
    #[default]
    Offline,
    Online,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Ok,
    Partial,
    Denied,
    Unsupported,
    NeedsAdmin,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Coverage {
    pub scope: String,
    pub status: CoverageStatus,
    pub source: String,
    pub detail: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub observation_id: String,
    pub measured_at_unix_ms: u64,
    pub window: Window,
    pub entity_ref: String,
    pub metric: String,
    pub value: Value,
    pub unit: String,
    pub status: CoverageStatus,
    pub source: String,
    pub origin: Origin,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inspection {
    pub status: Status,
    pub measured_at_unix_ms: u64,
    pub window: Window,
    pub origin: Origin,
    pub observations: Vec<Observation>,
    pub coverage: Vec<Coverage>,
    /// Local-only text. The agent selectively shares sanitized names after consent.
    #[serde(skip)]
    pub names: BTreeMap<String, String>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Inspection {
    fn new() -> Self {
        Self {
            status: Status::Ok,
            measured_at_unix_ms: unix_ms(),
            window: Window::instant(),
            origin: Origin::Live,
            observations: Vec::new(),
            coverage: Vec::new(),
            names: BTreeMap::new(),
        }
    }
    fn entity(&mut self, prefix: &str, name: Option<String>) -> String {
        let id = new_id(prefix);
        if let Some(name) = name.filter(|s| !s.is_empty()) {
            self.names.insert(id.clone(), name);
        }
        id
    }
    fn value(
        &mut self,
        entity: &str,
        metric: &str,
        value: impl Into<Value>,
        unit: &str,
        source: &str,
    ) {
        let value = value.into();
        let status = if value.is_null() {
            CoverageStatus::Unsupported
        } else {
            CoverageStatus::Ok
        };
        self.observations.push(Observation {
            observation_id: new_id("measurement"),
            measured_at_unix_ms: self.measured_at_unix_ms,
            window: self.window.clone(),
            entity_ref: entity.into(),
            metric: metric.into(),
            value,
            unit: unit.into(),
            status,
            source: source.into(),
            origin: self.origin,
        });
    }
    /// Display text follows the same explicit C5 consent path as application names.
    fn name_value(&mut self, entity: &str, metric: &str, name: Option<String>, source: &str) {
        let reference = name.map(|name| self.entity("name", Some(name)));
        self.value(
            entity,
            metric,
            reference.map(Value::from).unwrap_or(Value::Null),
            "opaque_ref",
            source,
        );
    }
    fn coverage(&mut self, scope: &str, status: CoverageStatus, source: &str, detail: &str) {
        self.coverage.push(Coverage {
            scope: scope.into(),
            status,
            source: source.into(),
            detail: detail.into(),
        });
    }
    fn finish(mut self) -> Self {
        self.status = if self.coverage.iter().all(|c| c.status == CoverageStatus::Ok)
            && !self.coverage.is_empty()
            && self
                .observations
                .iter()
                .all(|o| o.status == CoverageStatus::Ok)
        {
            Status::Ok
        } else if !self.observations.is_empty() {
            Status::Partial
        } else if self.coverage.iter().any(|c| {
            matches!(
                c.status,
                CoverageStatus::Denied | CoverageStatus::NeedsAdmin
            )
        }) {
            Status::PermissionDenied
        } else if self
            .coverage
            .iter()
            .all(|c| c.status == CoverageStatus::Unsupported)
        {
            Status::Unsupported
        } else {
            Status::Partial
        };
        self
    }
}

/// The two-letter country of the Windows region setting, for country-specific web lookups
/// (C6). It is a setting, not an identifier of the machine.
pub fn region_country() -> Option<String> {
    os::Native::region_country()
}

/// The caller cannot supply SQL, WQL, registry paths, event filters or executable commands.
pub fn inspect(query: Query<'_>) -> Inspection {
    os::Native::inspect(query)
}

/// Battery-only C4 read for the live window: the same observations as `Query::Battery`, run on
/// its own bounded worker outside the shared inspection worker. A live refresh therefore never
/// makes an agent tool or a recording tick report a busy worker, and a long tool never blanks the
/// battery. A second call while a read is still running (for example a driver that stalled past
/// the time bound) returns at once with `collector` partial coverage and no values.
pub fn live_battery() -> Inspection {
    os::Native::live_battery()
}

/// Runs `work` on one named thread at a time per `guard` and waits at most `timeout`. The guard
/// is released when the work returns, not at the timeout, so a blocked native call can never
/// accumulate detached workers. A timeout does not claim that the native call was cancelled.
#[cfg_attr(not(windows), allow(dead_code))]
fn bounded(
    guard: &'static AtomicBool,
    thread: &str,
    timeout: Duration,
    work: impl FnOnce() -> Inspection + Send + 'static,
) -> Inspection {
    fn refused(detail: &str) -> Inspection {
        let mut out = Inspection::new();
        out.coverage(
            "collector",
            CoverageStatus::Partial,
            "Bounded battery worker",
            detail,
        );
        out.finish()
    }
    struct Release(&'static AtomicBool);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    if guard.swap(true, Ordering::AcqRel) {
        return refused("A previous battery read is still running; the last values are kept.");
    }
    let (tx, rx) = mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name(thread.into())
        .spawn(move || {
            let _release = Release(guard);
            let _ = tx.send(work());
        });
    if spawned.is_err() {
        guard.store(false, Ordering::Release);
        return refused("The battery worker could not start; no values were read.");
    }
    rx.recv_timeout(timeout).unwrap_or_else(|error| match error {
        mpsc::RecvTimeoutError::Timeout => refused(
            "The battery read did not finish within its time bound; no incomplete values are represented as zero.",
        ),
        mpsc::RecvTimeoutError::Disconnected => {
            refused("The battery read stopped without a result; no values are represented.")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_are_local_and_missing_values_are_not_zero() {
        let mut result = Inspection::new();
        let entity = result.entity("app", Some("private-person-app".into()));
        result.value(&entity, "estimated_bytes", Value::Null, "bytes", "fixture");
        result.coverage("apps", CoverageStatus::Partial, "fixture", "Size absent.");
        let value = serde_json::to_value(result.finish()).unwrap();
        assert!(!value.to_string().contains("private-person"));
        assert!(value["observations"][0]["value"].is_null());
        assert_eq!(value["status"], "partial");
    }
    #[test]
    fn empty_success_is_distinct_from_unreadable() {
        let mut result = Inspection::new();
        result.coverage("apps", CoverageStatus::Ok, "fixture", "No entries.");
        assert_eq!(result.finish().status, Status::Ok);
        let mut result = Inspection::new();
        result.coverage(
            "apps",
            CoverageStatus::NeedsAdmin,
            "fixture",
            "Access denied.",
        );
        assert_eq!(result.finish().status, Status::PermissionDenied);
    }
    #[test]
    fn selected_hardware_and_event_names_never_serialize_implicitly() {
        let mut result = Inspection::new();
        result.name_value(
            "device",
            "driver_model_ref",
            Some("PRIVATE-MODEL".into()),
            "fixture",
        );
        result.name_value(
            "event",
            "event_module_ref",
            Some("PRIVATE.dll".into()),
            "fixture",
        );
        result.name_value("system", "pc_model_ref", None, "fixture");
        let value = serde_json::to_string(&result).unwrap();
        assert!(!value.contains("PRIVATE"));
        assert_eq!(result.names.len(), 2);
        assert!(result.observations[2].value.is_null());
        assert_eq!(WindowsUpdateMode::default(), WindowsUpdateMode::Offline);
    }
    #[cfg(not(windows))]
    #[test]
    fn drivers_on_other_hosts_return_unsupported_without_fixture_measurements() {
        let result = inspect(Query::Drivers {
            limit: 100,
            windows_update: WindowsUpdateMode::Offline,
        });
        assert_eq!(result.status, Status::Unsupported);
        assert!(result.observations.is_empty());
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn battery_on_other_hosts_is_unknown_without_synthetic_live_measurements() {
        let result = inspect(Query::Battery);
        assert_eq!(result.status, Status::Unsupported);
        assert!(result.observations.is_empty());
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn live_battery_on_other_hosts_is_unsupported_without_observations() {
        let result = live_battery();
        assert_eq!(result.status, Status::Unsupported);
        assert!(result.observations.is_empty());
        assert_eq!(result.coverage[0].scope, "windows");
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("15-second"));
    }

    fn reading(value: f64) -> Inspection {
        let mut out = Inspection::new();
        out.origin = Origin::SyntheticFixture;
        out.value(
            "battery-00000000-0000-4000-8000-00000000b001",
            "battery_charge_percent",
            value,
            "percent",
            "fixture",
        );
        out.coverage("battery_devices", CoverageStatus::Ok, "fixture", "One.");
        out.finish()
    }
    /// A refused or timed-out read: partial, without any value, and naming no other worker's bound.
    fn refused(result: &Inspection) -> bool {
        result.status == Status::Partial
            && result.observations.is_empty()
            && result.coverage.len() == 1
            && result.coverage[0].scope == "collector"
            && !serde_json::to_string(result).unwrap().contains("15-second")
    }
    fn released(guard: &AtomicBool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while guard.load(Ordering::Acquire) {
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        true
    }

    #[test]
    fn bounded_read_refuses_overlap_without_values() {
        static GUARD: AtomicBool = AtomicBool::new(false);
        let (started_tx, started) = mpsc::channel();
        let (finish, finish_rx) = mpsc::channel::<()>();
        let first = std::thread::spawn(move || {
            bounded(
                &GUARD,
                "test-battery-a",
                Duration::from_secs(10),
                move || {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                    reading(50.0)
                },
            )
        });
        started.recv().unwrap();
        let second = bounded(&GUARD, "test-battery-a", Duration::from_secs(10), || {
            reading(99.0)
        });
        assert!(refused(&second), "{second:?}");
        finish.send(()).unwrap();
        let first = first.join().unwrap();
        assert_eq!(first.observations.len(), 1);
        assert_eq!(first.observations[0].value, 50.0);
        assert!(released(&GUARD));
    }

    #[test]
    fn bounded_read_timeout_returns_no_values_and_keeps_the_guard_until_the_work_ends() {
        static GUARD: AtomicBool = AtomicBool::new(false);
        let slow = bounded(&GUARD, "test-battery-b", Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(300));
            reading(10.0)
        });
        assert!(refused(&slow), "{slow:?}");
        let retry = bounded(&GUARD, "test-battery-b", Duration::from_secs(5), || {
            reading(20.0)
        });
        assert!(refused(&retry), "the blocked work still holds the guard");
        assert!(released(&GUARD), "the guard is released when the work ends");
        let next = bounded(&GUARD, "test-battery-b", Duration::from_secs(5), || {
            reading(30.0)
        });
        assert_eq!(next.observations.len(), 1);
        assert_eq!(next.observations[0].value, 30.0);
    }

    #[test]
    fn separate_guards_do_not_block_each_other() {
        static FIRST: AtomicBool = AtomicBool::new(false);
        static SECOND: AtomicBool = AtomicBool::new(false);
        let (started_tx, started) = mpsc::channel();
        let (finish, finish_rx) = mpsc::channel::<()>();
        let first = std::thread::spawn(move || {
            bounded(
                &FIRST,
                "test-battery-c",
                Duration::from_secs(10),
                move || {
                    started_tx.send(()).unwrap();
                    finish_rx.recv().unwrap();
                    reading(40.0)
                },
            )
        });
        started.recv().unwrap();
        let other = bounded(&SECOND, "test-battery-d", Duration::from_secs(10), || {
            reading(60.0)
        });
        assert_eq!(other.observations[0].value, 60.0);
        finish.send(()).unwrap();
        assert_eq!(first.join().unwrap().observations[0].value, 40.0);
    }

    #[test]
    fn a_failed_battery_worker_releases_its_guard_and_reports_no_values() {
        static GUARD: AtomicBool = AtomicBool::new(false);
        let failed = bounded(&GUARD, "test-battery-e", Duration::from_secs(5), || {
            panic!("synthetic battery driver failure (expected by this test)")
        });
        assert!(refused(&failed), "{failed:?}");
        assert!(released(&GUARD));
    }
}
