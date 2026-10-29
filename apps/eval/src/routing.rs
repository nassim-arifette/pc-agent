//! Explicit replay of a human-calibrated continuation, never a substitute for calibration.
use pcagent_agent::routing::{
    Calibration, RoutedModel, RoutingMode, RoutingSettings, MAX_CALIBRATION_BYTES,
};
use pcagent_nebius::Nebius;
use std::{fs::File, io::Read, path::Path};

pub(crate) struct Automatic {
    settings: RoutingSettings,
    calibration: Calibration,
}

impl Automatic {
    pub(crate) fn load(
        path: &Path,
        super_model: String,
        ultra_model: String,
    ) -> crate::Result<Self> {
        let settings = RoutingSettings {
            mode: RoutingMode::Automatic,
            super_model_id: Some(super_model),
            ultra_model_id: Some(ultra_model),
        };
        settings.validate()?;
        let mut bytes = Vec::new();
        File::open(path)?
            .take((MAX_CALIBRATION_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let calibration = Calibration::from_bytes(&bytes)?;
        calibration.rules(&settings)?;
        Ok(Self {
            settings,
            calibration,
        })
    }

    pub(crate) fn model(&self) -> crate::Result<(RoutedModel<Nebius>, String)> {
        let (primary, id) = crate::model(self.settings.super_model_id.clone())?;
        let (ultra, _) = crate::model(self.settings.ultra_model_id.clone())?;
        Ok((
            RoutedModel::new(
                primary,
                Some(ultra),
                &self.settings,
                Some(&self.calibration),
            )?,
            id,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn accounting_opt_in_requires_cloud_and_cannot_change_product_automatic_calibration() {
        let args = ["pcagent-eval", "replay", "capture.json", "--drain-usage"];
        assert!(crate::Cli::try_parse_from(args).is_err());
        assert!(crate::Cli::try_parse_from(args.into_iter().chain(["--allow-cloud"])).is_ok());
        assert!(crate::Cli::try_parse_from(args.into_iter().chain([
            "--allow-cloud",
            "--calibration",
            "reviewed.json",
            "--super-model",
            "nvidia/super",
            "--ultra-model",
            "nvidia/ultra",
        ]))
        .is_err());
        assert!(crate::Cli::try_parse_from(args.into_iter().chain(["--preview-cloud"])).is_err());
        assert!(crate::Cli::try_parse_from([
            "pcagent-eval",
            "capture",
            "--drain-usage",
            "--allow-cloud",
        ])
        .is_err());
    }

    #[test]
    fn replay_cannot_request_automatic_without_cloud_consent_and_both_models() {
        let prefix = [
            "pcagent-eval",
            "replay",
            "capture.json",
            "--calibration",
            "reviewed.json",
        ];
        assert!(crate::Cli::try_parse_from(prefix).is_err());
        assert!(crate::Cli::try_parse_from(prefix.into_iter().chain([
            "--allow-cloud",
            "--super-model",
            "nvidia/super",
            "--ultra-model",
            "nvidia/ultra",
        ]))
        .is_ok());
        assert!(crate::Cli::try_parse_from(prefix.into_iter().chain([
            "--allow-cloud",
            "--super-model",
            "nvidia/super",
            "--ultra-model",
            "nvidia/ultra",
            "--model",
            "nvidia/super",
        ]))
        .is_err());
    }

    #[test]
    fn missing_calibration_is_rejected_before_credentials_are_read() {
        let path = std::env::temp_dir().join(pcagent_shared::new_id("missing-calibration"));
        assert!(Automatic::load(&path, "nvidia/super".into(), "nvidia/ultra".into()).is_err());
    }
}
