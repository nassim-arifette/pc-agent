use pcagent_agent::{validate_model_id, AgentError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// NVIDIA's sampling for Nemotron 3: Super and Ultra use 1.0 / 0.95; Nano uses 0.6 for tools.
pub(crate) fn sampling(model_id: &str) -> (f64, f64) {
    if model_id.to_ascii_lowercase().contains("nano") {
        (0.6, 0.95)
    } else {
        (1.0, 0.95)
    }
}

/// Reasoning controls exposed by the provider's chat template.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reasoning {
    #[default]
    On,
    Low,
    Off,
}
impl Reasoning {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "on" => Some(Self::On),
            "low" => Some(Self::Low),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Low => "low",
            Self::Off => "off",
        }
    }
    pub(crate) fn template_kwargs(self, model_id: &str) -> Value {
        let ultra = model_id.to_ascii_lowercase().contains("ultra");
        match self {
            Self::On => json!({"enable_thinking":true,"force_nonempty_content":true}),
            Self::Low if ultra => {
                json!({"enable_thinking":true,"medium_effort":true,"force_nonempty_content":true})
            }
            Self::Low => {
                json!({"enable_thinking":true,"low_effort":true,"force_nonempty_content":true})
            }
            Self::Off => json!({"enable_thinking":false}),
        }
    }
}

/// Intentionally neither Debug nor Serialize: API keys must not enter logs.
pub struct Config {
    pub(crate) api_key: String,
    pub(crate) model_id: String,
    pub(crate) reasoning: Reasoning,
}
impl Config {
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
    pub fn reasoning(&self) -> Reasoning {
        self.reasoning
    }
    pub fn for_model(&self, model_id: &str) -> Result<Self, AgentError> {
        validate_model_id(model_id)?;
        Ok(Self {
            api_key: self.api_key.clone(),
            model_id: model_id.into(),
            reasoning: self.reasoning,
        })
    }
    pub fn from_env() -> Result<Self, AgentError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }
    /// The host supplies environment or saved settings; empty values count as missing.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, AgentError> {
        let get = |name: &str| lookup(name).filter(|s| !s.trim().is_empty());
        let api_key = get("NEBIUS_API_KEY").ok_or(AgentError::MissingKey)?;
        let model_id = get("NEBIUS_MODEL_ID").ok_or(AgentError::MissingModel)?;
        validate_model_id(&model_id)?;
        let reasoning = get("NEBIUS_REASONING")
            .and_then(|text| Reasoning::parse(&text))
            .unwrap_or_default();
        Ok(Self {
            api_key,
            model_id,
            reasoning,
        })
    }
}
