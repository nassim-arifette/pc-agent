//! Explicit CLI routing with a bounded local calibration. Credential storage is unchanged.
use pcagent_agent::{
    routing::{Calibration, RoutedModel, RoutingMode, RoutingSettings, MAX_CALIBRATION_BYTES},
    Config, Nebius,
};
use serde_json::{json, Value};
use std::{fs::File, io::Read, path::Path};

pub(super) struct Routing {
    settings: RoutingSettings,
    calibration: Option<Calibration>,
}
pub(super) fn load(
    mode: &str,
    super_model_id: Option<String>,
    ultra_model_id: Option<String>,
    path: Option<&Path>,
) -> crate::Result<Routing> {
    let mode = match mode {
        "configured" => RoutingMode::Configured,
        "super" => RoutingMode::AlwaysSuper,
        "ultra" => RoutingMode::AlwaysUltra,
        "automatic" => RoutingMode::Automatic,
        _ => return Err("Invalid routing mode.".into()),
    };
    let settings = RoutingSettings {
        mode,
        super_model_id,
        ultra_model_id,
    };
    settings.validate()?;
    let calibration = if let Some(path) = path {
        let mut bytes = Vec::new();
        File::open(path)?
            .take((MAX_CALIBRATION_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        Some(Calibration::from_bytes(&bytes)?)
    } else {
        None
    };
    if mode == RoutingMode::Automatic {
        calibration
            .as_ref()
            .ok_or("Automatic routing requires a real human-reviewed calibration.")?
            .rules(&settings)?;
    }
    Ok(Routing {
        settings,
        calibration,
    })
}
impl Routing {
    pub(super) fn model(&self, configured: &Config) -> crate::Result<RoutedModel<Nebius>> {
        let first = self.settings.initial_model(configured.model_id())?;
        let primary = Nebius::new(configured.for_model(first)?)?;
        let ultra = if self.settings.mode == RoutingMode::Automatic {
            Some(Nebius::new(configured.for_model(
                self.settings.ultra_model_id.as_deref().unwrap(),
            )?)?)
        } else {
            None
        };
        Ok(RoutedModel::new(
            primary,
            ultra,
            &self.settings,
            self.calibration.as_ref(),
        )?)
    }
    pub(super) fn preview(&self) -> Value {
        json!({"settings":self.settings,
            "calibration_reviewed_at_unix_ms":self.calibration.as_ref().map(|c| c.reviewed_at_unix_ms),
            "maximum_escalations":usize::from(self.settings.mode == RoutingMode::Automatic),
            "shared_request_budget":pcagent_agent::MAX_ROUNDS,
            "shared_tool_budget":pcagent_agent::MAX_TOOLS,
            "shared_duration_seconds":pcagent_agent::MAX_DURATION.as_secs()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_modes_require_models_and_automatic_requires_real_calibration() {
        assert!(load("configured", None, None, None).is_ok());
        assert!(load("super", None, None, None).is_err());
        assert!(load(
            "automatic",
            Some("nvidia/nemotron-super".into()),
            Some("nvidia/nemotron-ultra".into()),
            None
        )
        .is_err());
        assert!(load("ultra", None, Some("claude/sonnet".into()), None).is_err());
        assert_eq!(
            load("configured", None, None, None).unwrap().preview()["maximum_escalations"],
            0
        );
    }
}
