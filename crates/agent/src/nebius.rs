use super::{AgentError, Model, Pricing, Reply, ToolCall, ToolChoice, MAX_OUTPUT_TOKENS};
use reqwest::{
    blocking::{Client, RequestBuilder},
    redirect::Policy,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    io::{self, Read},
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

/// A request that ran out of time is reported as such; other transport failures stay opaque.
fn transport_error(e: &reqwest::Error) -> AgentError {
    if e.is_timeout() {
        AgentError::Timeout
    } else {
        AgentError::Network
    }
}
/// The blocking body reader wraps its timeout in an `io::Error`.
fn body_error(e: &io::Error) -> AgentError {
    let timed_out = e.kind() == io::ErrorKind::TimedOut
        || e.get_ref()
            .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout);
    if timed_out {
        AgentError::Timeout
    } else {
        AgentError::Network
    }
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
            .map_err(|e| transport_error(&e))?;
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
            .map_err(|e| body_error(&e))?;
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
/// A usable reply, or a rejected one that keeps only the reported usage: an unusable reply
/// (cut off, filtered or malformed) is still billed.
fn parse_reply(envelope: Envelope) -> Reply {
    let usage = envelope.usage.as_ref();
    let billed = Reply {
        total_tokens: usage.and_then(|u| u.total_tokens),
        prompt_tokens: usage.and_then(|u| u.prompt_tokens),
        completion_tokens: usage.and_then(|u| u.completion_tokens),
        reasoning_tokens: usage
            .and_then(|u| u.completion_tokens_details.as_ref())
            .and_then(|d| d.reasoning_tokens),
        ..Reply::default()
    };
    let reject = |error| Reply {
        rejected: Some(error),
        ..billed.clone()
    };
    let mut choices = envelope.choices.into_iter();
    let (Some(choice), None) = (choices.next(), choices.next()) else {
        return reject(AgentError::InvalidResponse);
    };
    match choice.finish_reason.as_str() {
        "stop" | "tool_calls" => {}
        "length" => return reject(AgentError::OutputLimit),
        _ => return reject(AgentError::IncompleteResponse),
    }
    let calls = choice.message.tool_calls.unwrap_or_default();
    if choice.finish_reason == "tool_calls" && calls.is_empty() {
        return reject(AgentError::InvalidResponse);
    }
    Reply {
        calls,
        content: choice.message.content,
        ..billed
    }
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
            "tool_choice":tool_choice,"max_tokens":MAX_OUTPUT_TOKENS,"temperature":0.0,"stream":false});
        let response = self.read_json(
            self.client
                .post(format!("{BASE_URL}/chat/completions"))
                .json(&body),
            remaining,
        )?;
        Ok(parse_reply(response))
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
    fn truncated_filtered_and_malformed_replies_are_rejected_but_keep_their_usage() {
        let usage = json!({"prompt_tokens":4700,"completion_tokens":4096,"total_tokens":8796});
        let call = json!([{"id":"c","type":"function",
            "function":{"name":"get_system_snapshot","arguments":"{}"}}]);
        for (choices, error) in [
            (
                json!([{"message":{"content":"{}","tool_calls":call},"finish_reason":"length"}]),
                AgentError::OutputLimit,
            ),
            (
                json!([{"message":{"content":"{}"},"finish_reason":"content_filter"}]),
                AgentError::IncompleteResponse,
            ),
            (
                json!([{"message":{"content":"{}"},"finish_reason":"unknown"}]),
                AgentError::IncompleteResponse,
            ),
            (
                json!([{"message":{"content":null},"finish_reason":"tool_calls"}]),
                AgentError::InvalidResponse,
            ),
            (json!([]), AgentError::InvalidResponse),
            (
                json!([{"message":{"content":"{}"},"finish_reason":"stop"},
                       {"message":{"content":"{}"},"finish_reason":"stop"}]),
                AgentError::InvalidResponse,
            ),
        ] {
            let e = serde_json::from_value(json!({"choices":choices,"usage":usage})).unwrap();
            let r = parse_reply(e);
            assert_eq!(r.rejected, Some(error));
            // Nothing from a rejected reply can be executed or selected.
            assert!(r.calls.is_empty() && r.content.is_none());
            assert_eq!(
                (r.prompt_tokens, r.completion_tokens, r.total_tokens),
                (Some(4700), Some(4096), Some(8796))
            );
        }
    }
    #[test]
    fn provider_usage_is_unknown_when_absent() {
        let e = serde_json::from_value(
            json!({"choices":[{"message":{"content":"{}"},"finish_reason":"stop"}]}),
        )
        .unwrap();
        let r = parse_reply(e);
        assert_eq!((r.total_tokens, r.rejected), (None, None));
    }
    #[test]
    fn usage_is_split_into_input_output_and_reasoning() {
        let e = serde_json::from_value(
            json!({"choices":[{"message":{"content":"{}"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":19120,"completion_tokens":182,"total_tokens":19302,
                     "completion_tokens_details":{"reasoning_tokens":141}}}),
        )
        .unwrap();
        let r = parse_reply(e);
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
    fn requests_that_run_out_of_time_are_reported_as_timeouts() {
        // A local listener that never answers; nothing leaves the machine.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let silent = Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url)
            .timeout(Duration::from_millis(200))
            .send()
            .unwrap_err();
        assert_eq!(transport_error(&silent), AgentError::Timeout);
        let refused = Client::builder()
            .https_only(true)
            .build()
            .unwrap()
            .get("http://127.0.0.1:9/")
            .send()
            .unwrap_err();
        assert_eq!(transport_error(&refused), AgentError::Network);
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "body");
        assert_eq!(body_error(&timed_out), AgentError::Timeout);
        assert_eq!(body_error(&io::Error::other(silent)), AgentError::Timeout);
        assert_eq!(body_error(&io::Error::other("reset")), AgentError::Network);
    }
    #[test]
    fn error_messages_never_include_a_provider_response_body() {
        assert_eq!(
            AgentError::Http(401).to_string(),
            "Provider returned HTTP 401; response body was withheld"
        );
    }
}
