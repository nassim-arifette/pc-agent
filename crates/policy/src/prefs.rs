//! User preferences (spec 12.1, 16.1, 17.1) and their model-safe view.
use crate::classify::CleanupClass;
use crate::path::NormalizedPath;
use serde::{Deserialize, Serialize};

pub const PREFERENCES_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum PreferencesError {
    #[error("invalid preferences document: {0}")]
    Invalid(#[from] serde_json::Error),
    #[error("unsupported preferences schema_version {0}")]
    UnsupportedVersion(u32),
}

/// Explicit permissions. Every default is the most restrictive choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Permissions {
    pub cloud_allowed: bool,
    pub web_search_allowed: bool,
    pub reversible_only: bool,
}
impl Default for Permissions {
    fn default() -> Self {
        Self {
            cloud_allowed: false,
            web_search_allowed: false,
            reversible_only: true,
        }
    }
}

/// An application the user declared necessary (e.g. Docker, a VM, a creative suite). It is
/// never proposed for closing but stays visible as a resource contributor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservedApp {
    pub executable: String,
    /// Free text from the user; kept local, never included in the model view.
    #[serde(default)]
    pub needed_for: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub schema_version: u32,
    #[serde(default)]
    pub excluded_paths: Vec<NormalizedPath>,
    #[serde(default)]
    pub excluded_categories: Vec<CleanupClass>,
    #[serde(default)]
    pub preserved_apps: Vec<PreservedApp>,
    #[serde(default)]
    pub permissions: Permissions,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            excluded_paths: Vec::new(),
            excluded_categories: Vec::new(),
            preserved_apps: Vec::new(),
            permissions: Permissions::default(),
        }
    }
}

fn app_key(executable: &str) -> String {
    let name = executable
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(executable)
        .trim()
        .to_lowercase();
    name.strip_suffix(".exe")
        .map(str::to_string)
        .unwrap_or(name)
}

/// Preferences as sent to the model: categories, counts, permissions and opaque references.
/// No paths, executable names or free text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PreferencesView {
    pub schema_version: u32,
    pub excluded_path_refs: Vec<String>,
    pub excluded_categories: Vec<CleanupClass>,
    pub preserved_app_refs: Vec<String>,
    pub permissions: Permissions,
    pub limitations: Vec<&'static str>,
}

impl Preferences {
    /// Parses and validates a stored document. Any invalid excluded path rejects the document
    /// rather than silently dropping an exclusion.
    pub fn from_json(json: &str) -> Result<Self, PreferencesError> {
        let prefs: Self = serde_json::from_str(json)?;
        if prefs.schema_version != PREFERENCES_SCHEMA_VERSION {
            return Err(PreferencesError::UnsupportedVersion(prefs.schema_version));
        }
        Ok(prefs)
    }
    pub fn to_json(&self) -> Result<String, PreferencesError> {
        Ok(serde_json::to_string_pretty(self)?)
    }
    pub fn is_category_excluded(&self, class: CleanupClass) -> bool {
        self.excluded_categories.contains(&class)
    }
    /// Case-insensitive match on the executable file name, with or without directory or `.exe`.
    pub fn is_preserved_app(&self, executable: &str) -> bool {
        let key = app_key(executable);
        !key.is_empty()
            && self
                .preserved_apps
                .iter()
                .any(|a| app_key(&a.executable) == key)
    }
    pub fn may_propose_closing(&self, executable: &str) -> bool {
        !self.is_preserved_app(executable)
    }
    /// Opaque reference used in the model view; resolved only locally.
    pub fn preserved_app_ref(&self, executable: &str) -> Option<String> {
        let key = app_key(executable);
        self.preserved_apps
            .iter()
            .position(|a| app_key(&a.executable) == key)
            .map(|i| format!("pref-app-{}", i + 1))
    }
    pub fn resolve_preserved_app_ref(&self, reference: &str) -> Option<&PreservedApp> {
        let index: usize = reference.strip_prefix("pref-app-")?.parse().ok()?;
        self.preserved_apps.get(index.checked_sub(1)?)
    }
    pub fn view(&self) -> PreferencesView {
        PreferencesView {
            schema_version: self.schema_version,
            excluded_path_refs: (1..=self.excluded_paths.len()).map(|i| format!("pref-path-{i}")).collect(),
            excluded_categories: self.excluded_categories.clone(),
            preserved_app_refs: (1..=self.preserved_apps.len()).map(|i| format!("pref-app-{i}")).collect(),
            permissions: self.permissions,
            limitations: vec![
                "Paths, application names and user notes are kept local; only opaque references are shared.",
                "Preserved applications remain visible as resource contributors but are never proposed for closing.",
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{
        "schema_version": 1,
        "excluded_paths": ["D:\\Work\\Secret Project"],
        "excluded_categories": ["personal_data"],
        "preserved_apps": [{"executable": "C:\\Program Files\\Docker\\Docker Desktop.exe", "needed_for": "my client VM"}],
        "permissions": {"cloud_allowed": true}
    }"#;

    #[test]
    fn defaults_are_restrictive() {
        let p = Preferences::default();
        assert_eq!(p.schema_version, PREFERENCES_SCHEMA_VERSION);
        assert_eq!(
            p.permissions,
            Permissions {
                cloud_allowed: false,
                web_search_allowed: false,
                reversible_only: true
            }
        );
        let minimal = Preferences::from_json(r#"{"schema_version":1}"#).unwrap();
        assert_eq!(minimal, p);
    }
    #[test]
    fn round_trip_and_versioning() {
        let p = Preferences::from_json(DOC).unwrap();
        assert!(p.permissions.cloud_allowed && !p.permissions.web_search_allowed);
        assert!(p.is_category_excluded(CleanupClass::PersonalData));
        assert_eq!(Preferences::from_json(&p.to_json().unwrap()).unwrap(), p);
        assert!(matches!(
            Preferences::from_json(r#"{"schema_version":2}"#),
            Err(PreferencesError::UnsupportedVersion(2))
        ));
        assert!(
            Preferences::from_json(r#"{"schema_version":1,"excluded_paths":["..\\x"]}"#).is_err()
        );
        assert!(
            Preferences::from_json(r#"{"schema_version":1,"permissions":{"execute":true}}"#)
                .is_err()
        );
    }
    #[test]
    fn preserved_apps_are_matched_by_executable_name() {
        let p = Preferences::from_json(DOC).unwrap();
        assert!(p.is_preserved_app("docker desktop.EXE"));
        assert!(p.is_preserved_app("Docker Desktop"));
        assert!(!p.may_propose_closing(r"C:\Other\Docker Desktop.exe"));
        assert!(p.may_propose_closing("chrome.exe"));
        assert!(!p.is_preserved_app(""));
        let r = p.preserved_app_ref("Docker Desktop.exe").unwrap();
        assert_eq!(
            p.resolve_preserved_app_ref(&r)
                .unwrap()
                .needed_for
                .as_deref(),
            Some("my client VM")
        );
        assert!(p.resolve_preserved_app_ref("pref-app-0").is_none());
    }
    #[test]
    fn model_view_has_no_paths_names_or_notes() {
        let p = Preferences::from_json(DOC).unwrap();
        let json = serde_json::to_string(&p.view()).unwrap();
        for leak in ["Secret", "Work", "Docker", "client VM", "D:"] {
            assert!(!json.contains(leak), "{leak} leaked: {json}");
        }
        let view = p.view();
        assert_eq!(view.excluded_path_refs, ["pref-path-1"]);
        assert_eq!(view.preserved_app_refs, ["pref-app-1"]);
        assert!(view.permissions.cloud_allowed);
    }
}
