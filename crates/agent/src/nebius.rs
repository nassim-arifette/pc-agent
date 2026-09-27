use super::{AgentError, Model, Pricing, Reply, ToolCall, ToolChoice};
use reqwest::{
    blocking::{Client, RequestBuilder},
    redirect::Policy,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    time::{Duration, Instant},
};

const BASE_URL: &str = "https://api.tokenfactory.nebius.com/v1";
/// The verbose catalog also publishes per-token prices, which date-stamped cost estimates use.
const CATALOG_URL: &str = "https://api.tokenfactory.nebius.com/v1/models?verbose=true";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Intentionally does not implement Debug or Serialize: API keys must not enter logs.
pub struct Config {
    api_key: String,
    model_id: String,
}
impl Config {
    pub fn from_env() -> Result<Self, AgentError> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }
    /// `lookup` returns `NEBIUS_API_KEY` / `NEBIUS_MODEL_ID` from whatever source the caller trusts
    /// (environment, a per-user credentials file); empty values count as missing.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Self, AgentError> {
        let get = |name: &str| lookup(name).filter(|s| !s.trim().is_empty());
        let api_key = get("NEBIUS_API_KEY").ok_or(AgentError::MissingKey)?;
        let model_id = get("NEBIUS_MODEL_ID").ok_or(AgentError::MissingModel)?;
        validate_model_id(&model_id)?;
        Ok(Self { api_key, model_id })
    }
}
fn validate_model_id(id: &str) -> Result<(), AgentError> {
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

pub struct Nebius {
    config: Config,
    client: Client,
    catalog_checked: bool,
    pricing: Option<Pricing>,
}
impl Nebius {
    /// Builds a client without making a network request. No custom endpoint or redirects.
    pub fn new(config: Config) -> Result<Self, AgentError> {
        let client = Client::builder()
            .https_only(true)
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| AgentError::Network)?;
        Ok(Self {
            config,
            client,
            catalog_checked: false,
            pricing: None,
        })
    }
    fn read_json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        timeout: Duration,
    ) -> Result<T, AgentError> {
        if timeout.is_zero() {
            return Err(AgentError::Budget);
        }
        let response = request
            .bearer_auth(&self.config.api_key)
            .timeout(timeout)
            .send()
            .map_err(|_| AgentError::Network)?;
        if !response.status().is_success() {
            return Err(AgentError::Http(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
        {
            return Err(AgentError::InvalidResponse);
        }
        let mut bytes = Vec::new();
        response
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| AgentError::Network)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(AgentError::InvalidResponse);
        }
        serde_json::from_slice(&bytes).map_err(|_| AgentError::InvalidResponse)
    }
    /// Explicit preflight: checks the exact configured ID and reads its published prices,
    /// without sending telemetry.
    pub fn check_catalog(&mut self, timeout: Duration) -> Result<(), AgentError> {
        let catalog: Catalog = self.read_json(self.client.get(CATALOG_URL), timeout)?;
        let entry = catalog
            .data
            .into_iter()
            .find(|entry| entry.id == self.config.model_id)
            .ok_or(AgentError::ModelUnavailable)?;
        self.pricing = catalog_pricing(entry, pcagent_shared::unix_ms());
        self.catalog_checked = true;
        Ok(())
    }
    /// Prices read by the last successful catalog check, if the catalog published them.
    pub fn catalog_pricing(&self) -> Option<&Pricing> {
        self.pricing.as_ref()
    }
}
#[derive(Deserialize)]
struct Catalog {
    data: Vec<CatalogEntry>,
}
#[derive(Deserialize)]
struct CatalogEntry {
    id: String,
    #[serde(default)]
    pricing: Option<CatalogPrices>,
}
#[derive(Deserialize)]
struct CatalogPrices {
    prompt: Option<String>,
    completion: Option<String>,
}
/// Only exact, parseable decimal prices count as verified; anything else leaves the cost unknown.
fn catalog_pricing(entry: CatalogEntry, read_at_unix_ms: u64) -> Option<Pricing> {
    let prices = entry.pricing?;
    let pricing = Pricing {
        model_id: entry.id,
        prompt_usd_per_token: prices.prompt?,
        completion_usd_per_token: prices.completion?,
        source: CATALOG_URL.into(),
        read_at_unix_ms,
    };
    pricing.per_token_pico().map(|_| pricing)
}
#[derive(Deserialize)]
struct Envelope {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}
#[derive(Deserialize)]
struct Usage {
    total_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    completion_tokens_details: Option<CompletionDetails>,
}
#[derive(Deserialize)]
struct CompletionDetails {
    reasoning_tokens: Option<u64>,
}
#[derive(Deserialize)]
struct Choice {
    message: Message,
    finish_reason: String,
}
#[derive(Deserialize)]
struct Message {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCall>>,
}
fn parse_reply(envelope: Envelope) -> Result<Reply, AgentError> {
    if envelope.choices.len() != 1 {
        return Err(AgentError::InvalidResponse);
    }
    let choice = envelope
        .choices
        .into_iter()
        .next()
        .ok_or(AgentError::InvalidResponse)?;
    if !matches!(choice.finish_reason.as_str(), "stop" | "tool_calls") {
        return Err(AgentError::IncompleteResponse);
    }
    let calls = choice.message.tool_calls.unwrap_or_default();
    if choice.finish_reason == "tool_calls" && calls.is_empty() {
        return Err(AgentError::InvalidResponse);
    }
    let usage = envelope.usage;
    Ok(Reply {
        calls,
        content: choice.message.content,
        total_tokens: usage.as_ref().and_then(|u| u.total_tokens),
        prompt_tokens: usage.as_ref().and_then(|u| u.prompt_tokens),
        completion_tokens: usage.as_ref().and_then(|u| u.completion_tokens),
        reasoning_tokens: usage
            .as_ref()
            .and_then(|u| u.completion_tokens_details.as_ref())
            .and_then(|d| d.reasoning_tokens),
    })
}
impl Model for Nebius {
    fn complete(
        &mut self,
        messages: &[Value],
        tools: &Value,
        choice: ToolChoice,
        timeout: Duration,
    ) -> Result<Reply, AgentError> {
        let started = Instant::now();
        if !self.catalog_checked {
            self.check_catalog(timeout.min(Duration::from_secs(10)))?;
        }
        let remaining = timeout
            .checked_sub(started.elapsed())
            .ok_or(AgentError::Budget)?;
        let tool_choice = match choice {
            ToolChoice::Auto => "auto",
            ToolChoice::Required => "required",
        };
        let body = json!({"model":self.config.model_id,"messages":messages,"tools":tools,
            "tool_choice":tool_choice,"max_tokens":4096,"temperature":0.0,"stream":false});
        let response = self.read_json(
            self.client
                .post(format!("{BASE_URL}/chat/completions"))
                .json(&body),
            remaining,
        )?;
        parse_reply(response)
    }
    fn pricing(&self) -> Option<Pricing> {
        self.pricing.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_selection_has_no_non_nvidia_fallback() {
        // A syntactic fixture, NOT a claim that this ID exists in the live catalog.
        assert!(validate_model_id("nvidia/Nemotron-fixture").is_ok());
        for id in [
            "meta-llama/example",
            "nvidia/other",
            "nvidia/Nemotron\nsecret",
            "",
            "../../nvidia/Nemotron",
        ] {
            assert_eq!(validate_model_id(id), Err(AgentError::InvalidModel));
        }
    }
    #[test]
    fn truncated_and_filtered_responses_cannot_be_accepted() {
        for reason in ["length", "content_filter", "unknown"] {
            let e = serde_json::from_value(
                json!({"choices":[{"message":{"content":"{}"},"finish_reason":reason}]}),
            )
            .unwrap();
            assert!(matches!(
                parse_reply(e),
                Err(AgentError::IncompleteResponse)
            ));
        }
    }
    #[test]
    fn provider_usage_is_unknown_when_absent() {
        let e = serde_json::from_value(
            json!({"choices":[{"message":{"content":"{}"},"finish_reason":"stop"}]}),
        )
        .unwrap();
        assert_eq!(parse_reply(e).unwrap().total_tokens, None);
    }
    #[test]
    fn usage_is_split_into_input_output_and_reasoning() {
        let e = serde_json::from_value(
            json!({"choices":[{"message":{"content":"{}"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":19120,"completion_tokens":182,"total_tokens":19302,
                     "completion_tokens_details":{"reasoning_tokens":141}}}),
        )
        .unwrap();
        let r = parse_reply(e).unwrap();
        assert_eq!(
            (
                r.prompt_tokens,
                r.completion_tokens,
                r.total_tokens,
                r.reasoning_tokens
            ),
            (Some(19120), Some(182), Some(19302), Some(141))
        );
    }
    #[test]
    fn only_exact_catalog_prices_are_kept_with_their_date() {
        let entry = |prices: serde_json::Value| -> CatalogEntry {
            serde_json::from_value(json!({"id":"nvidia/nemotron-fixture","pricing":prices}))
                .unwrap()
        };
        let p = catalog_pricing(
            entry(json!({"prompt":"0.0000003","completion":"0.0000009"})),
            42,
        )
        .unwrap();
        assert_eq!(p.prompt_usd_per_token, "0.0000003");
        assert_eq!(p.read_at_unix_ms, 42);
        assert_eq!(p.source, CATALOG_URL);
        for prices in [
            json!({"prompt":"3e-7","completion":"0.0000009"}),
            json!({"prompt":"0.0000003"}),
            json!(null),
        ] {
            assert!(catalog_pricing(entry(prices), 42).is_none());
        }
        let absent: CatalogEntry = serde_json::from_value(json!({"id":"x"})).unwrap();
        assert!(catalog_pricing(absent, 42).is_none());
    }
    #[test]
    fn error_messages_never_include_a_provider_response_body() {
        assert_eq!(
            AgentError::Http(401).to_string(),
            "Provider returned HTTP 401; response body was withheld"
        );
    }
}
