use super::{tool_definitions, AgentError, Model, Reply, ToolCall};
use reqwest::{blocking::{Client, RequestBuilder}, redirect::Policy};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{io::Read, time::{Duration, Instant}};

const BASE_URL: &str = "https://api.tokenfactory.nebius.com/v1";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// Intentionally does not implement Debug or Serialize: API keys must not enter logs.
pub struct Config { api_key: String, model_id: String }
impl Config {
    pub fn from_env() -> Result<Self, AgentError> {
        let api_key = std::env::var("NEBIUS_API_KEY").ok().filter(|s| !s.trim().is_empty()).ok_or(AgentError::MissingKey)?;
        let model_id = std::env::var("NEBIUS_MODEL_ID").ok().filter(|s| !s.trim().is_empty()).ok_or(AgentError::MissingModel)?;
        validate_model_id(&model_id)?;
        Ok(Self { api_key, model_id })
    }
}
fn validate_model_id(id: &str) -> Result<(), AgentError> {
    let lower = id.to_ascii_lowercase();
    if id.len() > 200 || !lower.starts_with("nvidia/") || !lower.contains("nemotron")
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b)) {
        return Err(AgentError::InvalidModel);
    }
    Ok(())
}

pub struct Nebius { config: Config, client: Client, catalog_checked: bool }
impl Nebius {
    /// Builds a client without making a network request. No custom endpoint or redirects.
    pub fn new(config: Config) -> Result<Self, AgentError> {
        let client = Client::builder().https_only(true).redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5)).build().map_err(|_| AgentError::Network)?;
        Ok(Self { config, client, catalog_checked: false })
    }
    fn read_json<T: DeserializeOwned>(&self, request: RequestBuilder, timeout: Duration) -> Result<T, AgentError> {
        if timeout.is_zero() { return Err(AgentError::Budget); }
        let response = request.bearer_auth(&self.config.api_key).timeout(timeout)
            .send().map_err(|_| AgentError::Network)?;
        if !response.status().is_success() { return Err(AgentError::Http(response.status().as_u16())); }
        if response.content_length().is_some_and(|len| len > MAX_RESPONSE_BYTES as u64) { return Err(AgentError::InvalidResponse); }
        let mut bytes = Vec::new();
        response.take((MAX_RESPONSE_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|_| AgentError::Network)?;
        if bytes.len() > MAX_RESPONSE_BYTES { return Err(AgentError::InvalidResponse); }
        serde_json::from_slice(&bytes).map_err(|_| AgentError::InvalidResponse)
    }
    /// Explicit preflight: checks the exact configured ID, without sending telemetry.
    pub fn check_catalog(&mut self, timeout: Duration) -> Result<(), AgentError> {
        #[derive(Deserialize)] struct Entry { id: String }
        #[derive(Deserialize)] struct Catalog { data: Vec<Entry> }
        let catalog: Catalog = self.read_json(self.client.get(format!("{BASE_URL}/models")), timeout)?;
        if !catalog.data.iter().any(|entry| entry.id == self.config.model_id) { return Err(AgentError::ModelUnavailable); }
        self.catalog_checked = true; Ok(())
    }
}
#[derive(Deserialize)]
struct Envelope { choices: Vec<Choice>, usage: Option<Usage> }
#[derive(Deserialize)]
struct Usage { total_tokens: Option<u64> }
#[derive(Deserialize)]
struct Choice { message: Message, finish_reason: String }
#[derive(Deserialize)]
struct Message { content: Option<String>, #[serde(default)] tool_calls: Option<Vec<ToolCall>> }
fn parse_reply(envelope: Envelope) -> Result<Reply, AgentError> {
    if envelope.choices.len() != 1 { return Err(AgentError::InvalidResponse); }
    let choice = envelope.choices.into_iter().next().ok_or(AgentError::InvalidResponse)?;
    if !matches!(choice.finish_reason.as_str(), "stop" | "tool_calls") { return Err(AgentError::IncompleteResponse); }
    let calls = choice.message.tool_calls.unwrap_or_default();
    if choice.finish_reason == "tool_calls" && calls.is_empty() { return Err(AgentError::InvalidResponse); }
    Ok(Reply { calls, content: choice.message.content, total_tokens: envelope.usage.and_then(|u| u.total_tokens) })
}
impl Model for Nebius {
    fn complete(&mut self, messages: &[Value], timeout: Duration) -> Result<Reply, AgentError> {
        let started = Instant::now();
        if !self.catalog_checked { self.check_catalog(timeout.min(Duration::from_secs(10)))?; }
        let remaining = timeout.checked_sub(started.elapsed()).ok_or(AgentError::Budget)?;
        let body = json!({"model":self.config.model_id,"messages":messages,"tools":tool_definitions(),
            "tool_choice":"auto","max_tokens":4096,"temperature":0.0,"stream":false});
        let response = self.read_json(self.client.post(format!("{BASE_URL}/chat/completions")).json(&body), remaining)?;
        parse_reply(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_selection_has_no_non_nvidia_fallback() {
        // A syntactic fixture, NOT a claim that this ID exists in the live catalog.
        assert!(validate_model_id("nvidia/Nemotron-fixture").is_ok());
        for id in ["meta-llama/example", "nvidia/other", "nvidia/Nemotron\nsecret", "", "../../nvidia/Nemotron"] {
            assert_eq!(validate_model_id(id), Err(AgentError::InvalidModel));
        }
    }
    #[test]
    fn truncated_and_filtered_responses_cannot_be_accepted() {
        for reason in ["length", "content_filter", "unknown"] {
            let e = serde_json::from_value(json!({"choices":[{"message":{"content":"{}"},"finish_reason":reason}]})).unwrap();
            assert!(matches!(parse_reply(e),Err(AgentError::IncompleteResponse)));
        }
    }
    #[test]
    fn provider_usage_is_unknown_when_absent() {
        let e = serde_json::from_value(json!({"choices":[{"message":{"content":"{}"},"finish_reason":"stop"}]})).unwrap();
        assert_eq!(parse_reply(e).unwrap().total_tokens,None);
    }
    #[test]
    fn error_messages_never_include_a_provider_response_body() {
        assert_eq!(AgentError::Http(401).to_string(),"Provider returned HTTP 401; response body was withheld");
    }
}
