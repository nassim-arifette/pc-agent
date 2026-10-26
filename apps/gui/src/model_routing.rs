//! Local model policy only: no credentials, endpoints or provider requests belong here.
use pcagent_agent::{
    credentials,
    routing::{CalibratedRule, Calibration, RoutingMode, RoutingSettings, MAX_CALIBRATION_BYTES},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_SETTINGS_BYTES: usize = 4096;

#[derive(Clone)]
pub struct RoutingReview {
    pub settings: RoutingSettings,
    pub revision: u64,
    pub configured_model_id: Option<String>,
    pub calibration: Option<Calibration>,
}

#[derive(Clone, Serialize)]
pub struct RoutingView {
    pub settings: RoutingSettings,
    pub revision: u64,
    /// Automatic routing is on with a valid calibration, or, in another mode, the window's Auto
    /// choice would be accepted now: the same reviewed calibration `model_routing_set` requires.
    pub automatic_available: bool,
    /// The calibration behind `automatic_available` and `calibrated_rules`.
    pub calibration_reviewed_at_unix_ms: Option<u64>,
    pub calibrated_rules: Vec<CalibratedRule>,
    pub limitation: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSettings {
    schema_version: u32,
    settings: RoutingSettings,
}

#[derive(Default)]
pub struct ModelRouting {
    settings: RoutingSettings,
    revision: u64,
    calibration: Option<Calibration>,
    limitation: Option<String>,
    /// Where the policy and its calibration are read; unknown for a policy built in memory.
    directory: Option<PathBuf>,
    /// Outside automatic mode: the window's Auto choice and the reviewed calibration that
    /// would let it be saved. Display only; a turn never uses it.
    offered: Option<(RoutingSettings, Calibration)>,
}

fn directory() -> Option<PathBuf> {
    Some(credentials::file_path()?.parent()?.to_path_buf())
}

/// What the window's Auto choice saves: the Super and Ultra models it lists, in that order.
fn offered_automatic() -> Option<RoutingSettings> {
    let family = |name: &str| {
        crate::view::MODELS
            .iter()
            .map(|&(id, _)| id)
            .find(|id| id.to_ascii_lowercase().contains(name))
            .map(str::to_owned)
    };
    Some(RoutingSettings {
        mode: RoutingMode::Automatic,
        super_model_id: Some(family("super")?),
        ultra_model_id: Some(family("ultra")?),
    })
}

impl ModelRouting {
    pub fn load() -> Self {
        directory().map_or_else(Self::default, |directory| Self::load_from(&directory))
    }

    fn load_from(directory: &Path) -> Self {
        let mut state = Self {
            directory: Some(directory.to_path_buf()),
            ..Self::default()
        };
        let path = directory.join("model-routing.json");
        if path.exists() {
            match read_settings(&path).and_then(|settings| {
                let calibration = load_calibration(directory, &settings)?;
                Ok((settings, calibration))
            }) {
                Ok((settings, calibration)) => {
                    state.settings = settings;
                    state.calibration = calibration;
                }
                Err(error) => state.limitation = Some(error),
            }
        }
        state.refresh();
        state
    }

    /// Reads the calibration file again for the Auto choice, so a campaign reviewed while the
    /// window is open makes Auto available. The policy in use and its revision do not change.
    pub fn refresh(&mut self) {
        self.offered = None;
        if self.settings.mode == RoutingMode::Automatic {
            return;
        }
        let (Some(directory), Some(settings)) = (&self.directory, offered_automatic()) else {
            return;
        };
        if let Ok(Some(calibration)) = load_calibration(directory, &settings) {
            self.offered = Some((settings, calibration));
        }
    }

    pub fn view(&self) -> RoutingView {
        let (settings, calibration) = match &self.offered {
            Some((settings, calibration)) => (settings, Some(calibration)),
            None => (&self.settings, self.calibration.as_ref()),
        };
        let rules = calibration
            .and_then(|calibration| calibration.rules(settings).ok())
            .unwrap_or_default();
        RoutingView {
            settings: self.settings.clone(),
            revision: self.revision,
            automatic_available: !rules.is_empty(),
            calibration_reviewed_at_unix_ms: calibration
                .map(|calibration| calibration.reviewed_at_unix_ms),
            calibrated_rules: rules,
            limitation: self.limitation.clone(),
        }
    }

    pub fn review(&self, configured_model_id: Option<String>) -> RoutingReview {
        RoutingReview {
            settings: self.settings.clone(),
            revision: self.revision,
            configured_model_id,
            calibration: self.calibration.clone(),
        }
    }

    pub fn matches(&self, review: &RoutingReview, configured_model_id: Option<&str>) -> bool {
        self.revision == review.revision
            && self.settings == review.settings
            && self.calibration == review.calibration
            && review.configured_model_id.as_deref() == configured_model_id
    }

    pub fn set(&mut self, settings: RoutingSettings) -> Result<RoutingView, String> {
        self.set_in(
            &directory().ok_or("The local settings directory is unavailable.")?,
            settings,
        )
    }

    fn set_in(
        &mut self,
        directory: &Path,
        settings: RoutingSettings,
    ) -> Result<RoutingView, String> {
        settings.validate().map_err(|error| error.to_string())?;
        let revision = self
            .revision
            .checked_add(1)
            .ok_or("The local routing revision is exhausted.")?;
        let calibration = load_calibration(directory, &settings)?;
        save_settings(&directory.join("model-routing.json"), &settings)?;
        self.revision = revision;
        self.settings = settings;
        self.calibration = calibration;
        self.limitation = None;
        self.directory = Some(directory.to_path_buf());
        self.refresh();
        Ok(self.view())
    }
}

fn load_calibration(
    directory: &Path,
    settings: &RoutingSettings,
) -> Result<Option<Calibration>, String> {
    if settings.mode != RoutingMode::Automatic {
        return Ok(None);
    }
    let bytes = read_bounded(
        &directory.join("nemotron-routing-calibration.json"),
        MAX_CALIBRATION_BYTES,
    )
    .map_err(|_| "Automatic routing requires a reviewed local Nemotron calibration.".to_owned())?;
    let calibration = Calibration::from_bytes(&bytes).map_err(calibration_error)?;
    calibration.rules(settings).map_err(calibration_error)?;
    Ok(Some(calibration))
}

fn calibration_error(error: &'static str) -> String {
    match error {
        "invalid_calibration_schema" =>
            "Automatic routing calibration is malformed or lacks its required engine identity.".into(),
        "calibration_runtime_mismatch" =>
            "Automatic routing calibration belongs to a different engine version. Run and review a new campaign.".into(),
        "calibration_model_or_version_mismatch" =>
            "Automatic routing calibration does not match these models or the supported format.".into(),
        _ => format!("Automatic routing calibration was rejected: {error}."),
    }
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "Local model policy is unavailable.")?
        .take(maximum as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Local model policy could not be read.")?;
    if bytes.len() > maximum {
        return Err("Local model policy exceeds the supported size.".into());
    }
    Ok(bytes)
}

fn read_settings(path: &Path) -> Result<RoutingSettings, String> {
    let stored: StoredSettings = serde_json::from_slice(&read_bounded(path, MAX_SETTINGS_BYTES)?)
        .map_err(|_| "Local model policy is malformed.")?;
    if stored.schema_version != 1 {
        return Err("Local model policy has an unsupported version.".into());
    }
    stored
        .settings
        .validate()
        .map_err(|error| error.to_string())?;
    Ok(stored.settings)
}

fn save_settings(path: &Path, settings: &RoutingSettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Local model policy has no directory.")?;
    fs::create_dir_all(parent).map_err(|_| "The local settings directory could not be created.")?;
    let temp = parent.join(format!("{}.tmp", pcagent_shared::new_id("model-routing")));
    let result = (|| {
        let bytes = serde_json::to_vec(&StoredSettings {
            schema_version: 1,
            settings: settings.clone(),
        })
        .map_err(|_| "Local model policy could not be encoded.")?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| "Local model policy could not be saved.")?;
        use std::io::Write;
        file.write_all(&bytes)
            .map_err(|_| "Local model policy could not be saved.")?;
        file.sync_all()
            .map_err(|_| "Local model policy could not be synchronized.")?;
        drop(file);
        fs::rename(&temp, path)
            .map_err(|_| "Local model policy could not replace the previous choice.")
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::routing::{
        CalibrationCase, CalibrationOrigin, CalibrationRuntime, EscalationReason,
        CALIBRATION_SCHEMA_VERSION,
    };
    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            Self(std::env::temp_dir().join(pcagent_shared::new_id("model-routing-test")))
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            // Fixed fixture files only; no recursive deletion or arbitrary path traversal.
            let _ = fs::remove_file(self.0.join("model-routing.json"));
            let _ = fs::remove_file(self.0.join("nemotron-routing-calibration.json"));
            let _ = fs::remove_dir(&self.0);
        }
    }

    // Synthetic unit input only, kept in a disposable fixture directory. These fields
    // exercise acceptance/rejection; they are never presented as a reviewed live campaign.
    fn calibration_fixture() -> (RoutingSettings, Calibration) {
        let settings = RoutingSettings {
            mode: RoutingMode::Automatic,
            super_model_id: Some("nvidia/nemotron-super-fixture".into()),
            ultra_model_id: Some("nvidia/nemotron-ultra-fixture".into()),
        };
        let calibration = Calibration {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            runtime: CalibrationRuntime::current(),
            super_model_id: settings.super_model_id.clone().unwrap(),
            ultra_model_id: settings.ultra_model_id.clone().unwrap(),
            reviewed_at_unix_ms: 42,
            maximum_extra_cost_usd: "0.02".into(),
            cases: (1..=3)
                .map(|i| CalibrationCase {
                    capture_sha256: format!("{i:064x}"),
                    primary_trace_sha256: format!("{:064x}", i + 10),
                    ultra_trace_sha256: format!("{:064x}", i + 20),
                    origin: CalibrationOrigin::Live,
                    human_reviewed: true,
                    trigger: EscalationReason::LowConfidence,
                    primary_requests: 2,
                    primary_quality: 1,
                    ultra_quality: 3,
                    primary_critical_error: false,
                    ultra_critical_error: false,
                    primary_cost_usd: "0.001".into(),
                    ultra_cost_usd: "0.01".into(),
                    primary_duration_ms: 10,
                    ultra_duration_ms: 20,
                })
                .collect(),
        };
        (settings, calibration)
    }

    #[test]
    fn default_keeps_the_configured_model_and_revision_binds_the_reviewed_policy() {
        let folder = Folder::new();
        let mut policy = ModelRouting::default();
        assert_eq!(policy.view().settings.mode, RoutingMode::Configured);
        assert!(!policy.view().automatic_available);
        let review = policy.review(Some("nvidia/nemotron-fixture".into()));
        assert!(policy.matches(&review, Some("nvidia/nemotron-fixture")));
        assert!(!policy.matches(&review, Some("nvidia/nemotron-other")));
        policy
            .set_in(
                &folder.0,
                RoutingSettings {
                    mode: RoutingMode::AlwaysSuper,
                    super_model_id: Some("nvidia/nemotron-3-super-120b-a12b".into()),
                    ultra_model_id: None,
                },
            )
            .unwrap();
        assert!(!policy.matches(&review, Some("nvidia/nemotron-fixture")));
        let reloaded = ModelRouting::load_from(&folder.0);
        assert_eq!(reloaded.view().settings, policy.view().settings);
        assert!(!fs::read_to_string(folder.0.join("model-routing.json"))
            .unwrap()
            .contains("api_key"));
    }

    #[test]
    fn automatic_without_real_calibration_and_corrupt_settings_do_not_enable_escalation() {
        let folder = Folder::new();
        let mut policy = ModelRouting::default();
        let automatic = RoutingSettings {
            mode: RoutingMode::Automatic,
            super_model_id: Some("nvidia/nemotron-3-super-120b-a12b".into()),
            ultra_model_id: Some("nvidia/Nemotron-3-Ultra-550b-a55b".into()),
        };
        assert!(policy.set_in(&folder.0, automatic).is_err());
        assert_eq!(policy.view().settings.mode, RoutingMode::Configured);
        fs::create_dir_all(&folder.0).unwrap();
        fs::write(
            folder.0.join("model-routing.json"),
            r#"{"schema_version":1,"settings":{"mode":"configured"},"api_key":"rejected"}"#,
        )
        .unwrap();
        let loaded = ModelRouting::load_from(&folder.0);
        assert_eq!(loaded.view().settings.mode, RoutingMode::Configured);
        assert!(loaded.view().limitation.is_some());
    }

    #[test]
    fn current_engine_calibration_is_loaded_and_frozen_in_the_reviewed_policy() {
        let folder = Folder::new();
        let (settings, calibration) = calibration_fixture();
        fs::create_dir_all(&folder.0).unwrap();
        fs::write(
            folder.0.join("nemotron-routing-calibration.json"),
            serde_json::to_vec(&calibration).unwrap(),
        )
        .unwrap();
        let mut policy = ModelRouting::default();
        let view = policy.set_in(&folder.0, settings.clone()).unwrap();
        assert!(view.automatic_available);
        assert_eq!(view.calibrated_rules.len(), 1);
        let loaded = ModelRouting::load_from(&folder.0);
        assert_eq!(loaded.view().settings, settings);
        assert!(loaded.view().automatic_available);
        assert!(loaded.view().limitation.is_none());
        let review = loaded.review(Some("nvidia/nemotron-fixture".into()));
        assert_eq!(review.calibration, Some(calibration));
        assert!(loaded.matches(&review, Some("nvidia/nemotron-fixture")));
        let mut changed = review.clone();
        changed
            .calibration
            .as_mut()
            .unwrap()
            .runtime
            .harness_version = "old-harness".into();
        assert!(!loaded.matches(&changed, Some("nvidia/nemotron-fixture")));
    }

    #[test]
    fn a_reviewed_calibration_for_the_listed_models_offers_auto_before_it_is_chosen() {
        let folder = Folder::new();
        let offered = offered_automatic().unwrap();
        let (_, mut calibration) = calibration_fixture();
        calibration.super_model_id = offered.super_model_id.clone().unwrap();
        calibration.ultra_model_id = offered.ultra_model_id.clone().unwrap();
        let mut policy = ModelRouting::load_from(&folder.0);
        assert!(!policy.view().automatic_available);
        fs::create_dir_all(&folder.0).unwrap();
        let path = folder.0.join("nemotron-routing-calibration.json");
        fs::write(&path, serde_json::to_vec(&calibration).unwrap()).unwrap();
        // Read again when the window asks; the policy in use and its previews are unchanged.
        let review = policy.review(None);
        policy.refresh();
        let view = policy.view();
        assert_eq!(view.settings.mode, RoutingMode::Configured);
        assert!(view.automatic_available);
        assert_eq!(view.calibrated_rules.len(), 1);
        assert_eq!(view.calibration_reviewed_at_unix_ms, Some(42));
        assert!(policy.matches(&review, None));
        assert!(policy.review(None).calibration.is_none());
        // Without the file, or with one for other models, Auto is not offered.
        fs::remove_file(&path).unwrap();
        policy.refresh();
        assert!(!policy.view().automatic_available);
        let (_, unlisted) = calibration_fixture();
        fs::write(&path, serde_json::to_vec(&unlisted).unwrap()).unwrap();
        assert!(
            !ModelRouting::load_from(&folder.0)
                .view()
                .automatic_available
        );
        // The window's Auto choice saves exactly the offered models, which the file must match.
        fs::write(&path, serde_json::to_vec(&calibration).unwrap()).unwrap();
        policy.refresh();
        assert!(policy.view().automatic_available);
        let automatic = policy.set_in(&folder.0, offered).unwrap();
        assert_eq!(automatic.settings.mode, RoutingMode::Automatic);
        assert!(automatic.automatic_available);
        assert!(policy.review(None).calibration.is_some());
    }

    #[test]
    fn missing_or_stale_engine_identity_cannot_enable_automatic_policy_on_set_or_load() {
        for changed in 0..5 {
            let folder = Folder::new();
            let (settings, calibration) = calibration_fixture();
            let mut stale = serde_json::to_value(calibration).unwrap();
            match changed {
                0 => {
                    stale.as_object_mut().unwrap().remove("runtime");
                }
                1 => stale["schema_version"] = serde_json::json!(1),
                2 => stale["runtime"]["registered_tool_registry_sha256"] = "old-registry".into(),
                3 => stale["runtime"]["analyst_sha256"] = "old-analyst".into(),
                4 => stale["runtime"]["harness_version"] = "old-harness".into(),
                _ => unreachable!(),
            }
            fs::create_dir_all(&folder.0).unwrap();
            let bytes = serde_json::to_vec(&stale).unwrap();
            let path = folder.0.join("nemotron-routing-calibration.json");
            fs::write(&path, &bytes).unwrap();
            let mut policy = ModelRouting::default();
            assert!(
                policy.set_in(&folder.0, settings.clone()).is_err(),
                "case {changed}"
            );
            assert_eq!(policy.view().settings.mode, RoutingMode::Configured);
            assert!(!policy.view().automatic_available);
            save_settings(&folder.0.join("model-routing.json"), &settings).unwrap();
            let loaded = ModelRouting::load_from(&folder.0);
            assert_eq!(loaded.view().settings.mode, RoutingMode::Configured);
            assert!(!loaded.view().automatic_available);
            assert!(loaded.review(None).calibration.is_none());
            let limitation = loaded.view().limitation.unwrap();
            assert!(limitation.starts_with("Automatic routing calibration"));
            assert!(!limitation.contains("calibration_runtime_mismatch"));
            assert_eq!(
                fs::read(&path).unwrap(),
                bytes,
                "loader must never relabel old evidence"
            );
        }
    }
}
