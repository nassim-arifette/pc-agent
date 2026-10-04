//! The account catalog is the authority for availability and dated prices. Missing metadata
//! stays unknown; model names never imply a context limit, capability or quality ranking.
use crate::{AgentError, Pricing};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const CATALOG_URL: &str = "https://api.tokenfactory.nebius.com/v1/models?verbose=true";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub pricing: Option<Pricing>,
    pub context_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_tools: Option<bool>,
    pub supports_reasoning: Option<bool>,
    /// Feature vocabulary published by the account API, without inferring absent capabilities.
    pub supported_features: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelCatalog {
    pub source: String,
    pub read_at_unix_ms: u64,
    pub models: Vec<ModelInfo>,
}

#[derive(Deserialize)]
pub(crate) struct Catalog {
    pub data: Vec<CatalogEntry>,
}

#[derive(Deserialize)]
pub(crate) struct CatalogEntry {
    pub id: String,
    #[serde(default)]
    pricing: Value,
    #[serde(default)]
    context_length: Value,
    #[serde(default)]
    max_output_tokens: Value,
    #[serde(default)]
    capabilities: Value,
    #[serde(default)]
    supported_features: Value,
}

/// Submission scope: an explicit NVIDIA Nemotron ID, with no fallback to another vendor.
pub fn validate_model_id(id: &str) -> Result<(), AgentError> {
    let lower = id.to_ascii_lowercase();
    if id.len() > 200
        || !lower.starts_with("nvidia/")
        || !lower.contains("nemotron")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b))
    {
        return Err(AgentError::InvalidModel);
    }
    Ok(())
}

pub(crate) fn catalog_pricing(entry: &CatalogEntry, read_at_unix_ms: u64) -> Option<Pricing> {
    let pricing = Pricing {
        model_id: entry.id.clone(),
        prompt_usd_per_token: entry.pricing["prompt"].as_str()?.into(),
        completion_usd_per_token: entry.pricing["completion"].as_str()?.into(),
        source: CATALOG_URL.into(),
        read_at_unix_ms,
    };
    pricing.per_token_pico().map(|_| pricing)
}

impl Catalog {
    pub(crate) fn project(self, read_at_unix_ms: u64) -> Result<ModelCatalog, AgentError> {
        if self.data.len() > 2048 {
            return Err(AgentError::InvalidResponse);
        }
        let mut ids = BTreeSet::new();
        let mut models = Vec::new();
        for entry in self.data {
            if validate_model_id(&entry.id).is_err() {
                continue;
            }
            if !ids.insert(entry.id.clone()) {
                return Err(AgentError::InvalidResponse);
            }
            let positive = |value: &Value| value.as_u64().filter(|n| *n > 0);
            let published_feature = |feature: &str| {
                entry.supported_features.as_array().and_then(|features| {
                    features
                        .iter()
                        .any(|value| value.as_str() == Some(feature))
                        .then_some(true)
                })
            };
            models.push(ModelInfo {
                pricing: catalog_pricing(&entry, read_at_unix_ms),
                context_tokens: positive(&entry.context_length),
                max_output_tokens: positive(&entry.max_output_tokens),
                supports_tools: entry.capabilities["tools"]
                    .as_bool()
                    .or_else(|| published_feature("tools")),
                supports_reasoning: entry.capabilities["reasoning"]
                    .as_bool()
                    .or_else(|| published_feature("reasoning")),
                supported_features: entry.supported_features.as_array().and_then(|features| {
                    if features.len() > 64 {
                        return None;
                    }
                    features
                        .iter()
                        .map(|feature| {
                            let feature = feature.as_str()?;
                            (feature.len() <= 100
                                && feature
                                    .bytes()
                                    .all(|b| b.is_ascii_alphanumeric() || b"_.- ".contains(&b)))
                            .then(|| feature.to_owned())
                        })
                        .collect()
                }),
                id: entry.id,
            });
        }
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(ModelCatalog {
            source: CATALOG_URL.into(),
            read_at_unix_ms,
            models,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_account_controls_availability_and_unknown_metadata_stays_unknown() {
        let catalog: Catalog = serde_json::from_value(json!({"data":[
            {"id":"other/nemotron-fixture"},
            {"id":"nvidia/nemotron-fixture","pricing":{"prompt":"0.0000003","completion":"0.0000009"},
                "context_length":262144,"capabilities":{"tools":true,"reasoning":false}},
            {"id":"nvidia/Nemotron-second","context_length":"unverified","pricing":{"prompt":1}},
            {"id":"nvidia/not-a-supported-family"}
        ]})).unwrap();
        let projected = catalog.project(42).unwrap();
        assert_eq!(projected.models.len(), 2);
        let known = projected
            .models
            .iter()
            .find(|m| m.id.ends_with("fixture"))
            .unwrap();
        assert_eq!(known.context_tokens, Some(262144));
        assert_eq!(known.supports_tools, Some(true));
        assert_eq!(known.supports_reasoning, Some(false));
        assert_eq!(known.pricing.as_ref().unwrap().read_at_unix_ms, 42);
        let unknown = &projected.models[0];
        assert!(unknown.context_tokens.is_none());
        assert!(unknown.pricing.is_none());
        assert!(unknown.supports_tools.is_none());
    }

    #[test]
    fn conflicting_duplicate_ids_never_pick_arbitrary_prices() {
        let catalog: Catalog = serde_json::from_value(json!({"data":[
            {"id":"nvidia/nemotron-fixture"},{"id":"nvidia/nemotron-fixture"}
        ]}))
        .unwrap();
        assert_eq!(catalog.project(1).unwrap_err(), AgentError::InvalidResponse);
    }

    #[test]
    fn published_features_are_positive_evidence_and_absent_flags_remain_unknown() {
        let catalog: Catalog = serde_json::from_value(json!({"data":[
            {"id":"nvidia/nemotron-fixture", "supported_features":["tools","reasoning"]},
            {"id":"nvidia/nemotron-unknown", "supported_features":[]}
        ]}))
        .unwrap();
        let models = catalog.project(1).unwrap().models;
        assert_eq!(models[0].supports_tools, Some(true));
        assert_eq!(models[0].supports_reasoning, Some(true));
        assert_eq!(models[1].supports_tools, None);
        assert_eq!(models[1].supports_reasoning, None);
    }
}
