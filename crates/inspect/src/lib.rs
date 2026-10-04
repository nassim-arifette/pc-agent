//! Bounded, read-only native inspections. No commands, file contents or machine identifiers.
//! Display names are kept separately and are never serialized implicitly.
#![deny(unsafe_code)]
use pcagent_shared::{new_id, unix_ms, Origin, Status, Window};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub mod autostart;
pub mod elevation;

#[cfg_attr(not(windows), allow(dead_code))]
mod parse;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

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

/// The caller cannot supply SQL, WQL, registry paths, event filters or executable commands.
pub fn inspect(query: Query<'_>) -> Inspection {
    #[cfg(windows)]
    {
        windows::inspect(query)
    }
    #[cfg(not(windows))]
    {
        let _ = query;
        let mut result = Inspection::new();
        result.coverage(
            "windows",
            CoverageStatus::Unsupported,
            "Windows APIs",
            "Native inspection is available on Windows only.",
        );
        result.finish()
    }
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
    #[cfg(not(windows))]
    #[test]
    fn battery_on_other_hosts_is_unknown_without_synthetic_live_measurements() {
        let result = inspect(Query::Battery);
        assert_eq!(result.status, Status::Unsupported);
        assert!(result.observations.is_empty());
    }
}
