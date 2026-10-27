//! Provider configuration, model choices and local capabilities for settings.
use pcagent_agent::capabilities::{Capability, Platform, Probes};
use pcagent_agent::credentials::{Credentials, Source};
use pcagent_nebius::Config;
use serde::Serialize;
use std::collections::BTreeMap;

/// Models the settings screen offers, with what the 2026-09-27 comparison measured
/// (docs/archive/model-comparison.md); Ultra was added to the catalog later and is not evaluated
/// yet. Any other `nvidia/` Nemotron ID can still be typed.
pub const MODELS: [(&str, &str); 4] = [
    ("nvidia/nemotron-3-super-120b-a12b", "recommended"),
    ("nvidia/Nemotron-3-Ultra-550b-a55b", "most_capable"),
    ("nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B", "cheaper_slower"),
    ("nvidia/Nemotron-3_5-Lightning", "not_recommended"),
];

#[derive(Debug, Clone, Serialize)]
pub struct Cloud {
    pub key_configured: bool,
    /// The last four characters, so a person can tell keys apart; never the key itself.
    pub key_hint: Option<String>,
    pub key_source: Option<&'static str>,
    pub model_id: Option<String>,
    pub model_source: Option<&'static str>,
    /// Both settings are present and the model ID is an accepted Nemotron ID.
    pub ready: bool,
    /// How much the model reasons: on (default), low or off.
    pub reasoning: &'static str,
    pub problem: Option<String>,
    pub file: Option<String>,
    pub file_problem: Option<String>,
    /// A Tavily key is saved for web lookups (C6); the key itself never reaches the page.
    pub search_key_configured: bool,
    pub search_key_hint: Option<String>,
    /// `environment` when a variable overrides the saved key, as for Nebius.
    pub search_key_source: Option<&'static str>,
}
pub(super) fn source_name(s: Option<Source>) -> Option<&'static str> {
    s.map(|s| match s {
        Source::Environment => "environment",
        Source::File => "file",
    })
}
pub fn cloud(creds: &Credentials) -> Cloud {
    let key = creds.get("NEBIUS_API_KEY");
    let checked = Config::from_lookup(|name| creds.get(name));
    Cloud {
        key_configured: key.is_some(),
        key_hint: key
            .filter(|k| k.chars().count() >= 12)
            .map(|k| k.chars().skip(k.chars().count() - 4).collect()),
        key_source: source_name(creds.source("NEBIUS_API_KEY")),
        model_id: creds.get("NEBIUS_MODEL_ID"),
        model_source: source_name(creds.source("NEBIUS_MODEL_ID")),
        ready: checked.is_ok(),
        reasoning: creds
            .get("NEBIUS_REASONING")
            .and_then(|text| pcagent_nebius::Reasoning::parse(&text))
            .unwrap_or_default()
            .as_str(),
        problem: checked.err().map(|e| e.to_string()),
        file: creds.file().map(|p| p.display().to_string()),
        file_problem: creds.problem.clone(),
        search_key_configured: creds.get("TAVILY_API_KEY").is_some(),
        search_key_hint: creds
            .get("TAVILY_API_KEY")
            .filter(|k| k.chars().count() >= 12)
            .map(|k| k.chars().skip(k.chars().count() - 4).collect()),
        search_key_source: source_name(creds.source("TAVILY_API_KEY")),
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ModelChoice {
    pub id: &'static str,
    pub note: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub platform: &'static str,
    pub cloud: Cloud,
    pub models: Vec<ModelChoice>,
    /// What this PC and installation offer (`pcagent_agent::capabilities`), see [`capabilities`].
    pub capabilities: BTreeMap<&'static str, Option<bool>>,
}
pub fn app_info(
    creds: &Credentials,
    capabilities: BTreeMap<&'static str, Option<bool>>,
) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        cloud: cloud(creds),
        models: MODELS
            .iter()
            .map(|&(id, note)| ModelChoice { id, note })
            .collect(),
        capabilities,
    }
}
/// The capabilities of this PC and installation by their stable names: `true` or `false` once
/// known, `None` while a hardware probe has not answered yet (the window's first measurements);
/// like the model's catalog, the page counts an unknown capability as present. The capabilities
/// of a turn (approved storage scopes, web lookups, action proposals) are decided when it starts.
pub fn capabilities(history_sampler: bool, probes: Probes) -> BTreeMap<&'static str, Option<bool>> {
    let native = Platform::current().has_native_backend();
    BTreeMap::from([
        (Capability::Windows.name(), Some(cfg!(windows))),
        (Capability::Linux.name(), Some(cfg!(target_os = "linux"))),
        (Capability::Macos.name(), Some(cfg!(target_os = "macos"))),
        (Capability::LiveTelemetry.name(), Some(native)),
        (Capability::SystemInspection.name(), Some(native)),
        (Capability::NativeActions.name(), Some(native)),
        (Capability::HistorySampler.name(), Some(history_sampler)),
        (
            Capability::ElevatedHelper.name(),
            Some(native && probes.elevated_helper),
        ),
        (Capability::NvidiaGpu.name(), probes.nvidia_gpu),
        (Capability::Battery.name(), probes.battery),
        (
            Capability::NtfsVolumes.name(),
            if cfg!(windows) {
                probes.ntfs_volumes
            } else {
                Some(false)
            },
        ),
    ])
}
