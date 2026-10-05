use super::{
    AgentError, FunctionCall, Model, Pricing, Reply, ToolCall, ToolChoice, MAX_OUTPUT_TOKENS,
};
#[cfg(test)]
use crate::models::catalog_pricing;
use crate::models::{validate_model_id, Catalog, CATALOG_URL};
use crate::ModelCatalog;
use reqwest::{
    blocking::{Client, RequestBuilder, Response},
    redirect::Policy,
};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, BufReader, Read},
    time::{Duration, Instant},
};

const BASE_URL: &str = "https://api.tokenfactory.nebius.com/v1";
/// NVIDIA's sampling for Nemotron 3 tool calling. Greedy decoding (temperature 0) let Super loop
/// until the output limit, with almost no reasoning, on 2026-10-03 and 2026-10-05.
const TEMPERATURE: f64 = 0.6;
const TOP_P: f64 = 0.95;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// A streamed reply repeats its envelope in every event, so it may be larger than a plain one.
const MAX_STREAM_BYTES: usize = 4 * 1024 * 1024;

/// Intentionally does not implement Debug or Serialize: API keys must not enter logs.
pub struct Config {
    api_key: String,
    model_id: String,
}
impl Config {
    pub fn model_id(&self) -> &str {
        &self.model_id
    }
    /// Reuse an already local credential for an explicit catalog-checked routing target.
    pub fn for_model(&self, model_id: &str) -> Result<Self, AgentError> {
        validate_model_id(model_id)?;
        Ok(Self {
            api_key: self.api_key.clone(),
            model_id: model_id.into(),
        })
    }
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
    /// Sends with the key and a deadline that covers reading the whole body.
    fn send(&self, request: RequestBuilder, timeout: Duration) -> Result<Response, AgentError> {
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
        Ok(response)
    }
    fn read_json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        timeout: Duration,
    ) -> Result<T, AgentError> {
        let response = self.send(request, timeout)?;
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
        let catalog = self.available_models(timeout)?;
        let entry = catalog
            .models
            .into_iter()
            .find(|entry| entry.id == self.config.model_id)
            .ok_or(AgentError::ModelUnavailable)?;
        if entry.supports_tools == Some(false) {
            return Err(AgentError::ModelUnavailable);
        }
        self.pricing = entry.pricing;
        self.catalog_checked = true;
        Ok(())
    }
    /// Prices read by the last successful catalog check, if the catalog published them.
    pub fn catalog_pricing(&self) -> Option<&Pricing> {
        self.pricing.as_ref()
    }
    /// Explicit account catalog request; no question, telemetry or identity is sent.
    pub fn available_models(&self, timeout: Duration) -> Result<ModelCatalog, AgentError> {
        let catalog: Catalog = self.read_json(self.client.get(CATALOG_URL), timeout)?;
        catalog.project(pcagent_shared::unix_ms())
    }
    /// A first-launch host can list models with its local credential before choosing an ID.
    pub fn account_catalog(
        lookup: impl Fn(&str) -> Option<String>,
        timeout: Duration,
    ) -> Result<ModelCatalog, AgentError> {
        let api_key = lookup("NEBIUS_API_KEY")
            .filter(|key| !key.trim().is_empty())
            .ok_or(AgentError::MissingKey)?;
        let client = Self::new(Config {
            api_key,
            model_id: String::new(),
        })?;
        client.available_models(timeout)
    }
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
/// One server-sent event of a streamed completion.
#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    usage: Option<Usage>,
}
#[derive(Deserialize)]
struct ChunkChoice {
    #[serde(default)]
    index: usize,
    delta: Option<Delta>,
    finish_reason: Option<String>,
}
#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
    tool_calls: Option<Vec<CallDelta>>,
}
#[derive(Deserialize)]
struct CallDelta {
    index: usize,
    id: Option<String>,
    function: Option<FunctionDelta>,
}
#[derive(Deserialize)]
struct FunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}
#[derive(Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}
/// At most this many tool calls in one streamed reply.
const MAX_STREAMED_CALLS: usize = 64;

/// Rebuilds the reply envelope from server-sent events, reporting each tool call's arguments as
/// they grow. A stream that ends early or carries a malformed event yields an envelope that
/// `parse_reply` rejects, keeping whatever usage was reported.
fn read_stream(
    reader: impl Read,
    on_call: &mut dyn FnMut(&str, &str),
) -> Result<Envelope, AgentError> {
    let mut lines = BufReader::new(reader.take((MAX_STREAM_BYTES + 1) as u64));
    let mut read = 0usize;
    let mut line = String::new();
    let mut content: Option<String> = None;
    let mut calls: Vec<PartialCall> = Vec::new();
    let mut finish: Option<String> = None;
    let mut usage = None;
    let mut seen = false;
    let mut malformed = false;
    loop {
        line.clear();
        let n = lines.read_line(&mut line).map_err(|e| body_error(&e))?;
        if n == 0 {
            break;
        }
        read += n;
        if read > MAX_STREAM_BYTES {
            return Err(AgentError::InvalidResponse);
        }
        // Blank separators, comments and event names carry no data.
        let Some(data) = line.trim_end().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim_start();
        if data == "[DONE]" {
            break;
        }
        let Ok(chunk) = serde_json::from_str::<Chunk>(data) else {
            malformed = true;
            break;
        };
        if chunk.usage.is_some() {
            usage = chunk.usage;
        }
        for choice in chunk.choices {
            if choice.index != 0 {
                malformed = true;
                continue;
            }
            seen = true;
            if let Some(reason) = choice.finish_reason {
                finish = Some(reason);
            }
            let Some(delta) = choice.delta else { continue };
            if let Some(text) = delta.content {
                content.get_or_insert_with(String::new).push_str(&text);
            }
            for part in delta.tool_calls.unwrap_or_default() {
                if part.index > calls.len() || part.index >= MAX_STREAMED_CALLS {
                    malformed = true;
                    continue;
                }
                if part.index == calls.len() {
                    calls.push(PartialCall::default());
                }
                let call = &mut calls[part.index];
                if let Some(id) = part.id {
                    call.id.push_str(&id);
                }
                if let Some(function) = part.function {
                    if let Some(name) = function.name {
                        call.name.push_str(&name);
                    }
                    if let Some(arguments) = function.arguments {
                        call.arguments.push_str(&arguments);
                        on_call(&call.name, &call.arguments);
                    }
                }
            }
        }
    }
    let identified = calls.iter().all(|c| !c.id.is_empty() && !c.name.is_empty());
    if !seen || malformed || !identified {
        return Ok(Envelope {
            choices: Vec::new(),
            usage,
        });
    }
    let tool_calls: Vec<ToolCall> = calls
        .into_iter()
        .map(|c| ToolCall {
            id: c.id,
            kind: "function".into(),
            function: FunctionCall {
                name: c.name,
                arguments: c.arguments,
            },
        })
        .collect();
    Ok(Envelope {
        choices: vec![Choice {
            message: Message {
                content,
                tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            },
            // A stream cut before its last event never says why it finished.
            finish_reason: finish.unwrap_or_default(),
        }],
        usage,
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
        self.complete_streaming(messages, tools, choice, timeout, &mut |_, _| {})
    }
    fn complete_streaming(
        &mut self,
        messages: &[Value],
        tools: &Value,
        choice: ToolChoice,
        timeout: Duration,
        on_call: &mut dyn FnMut(&str, &str),
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
        // Streamed so the answer can be shown while it is written; usage comes in the last event.
        let body = json!({"model":self.config.model_id,"messages":messages,"tools":tools,
            "tool_choice":tool_choice,"max_tokens":MAX_OUTPUT_TOKENS,
            "temperature":TEMPERATURE,"top_p":TOP_P,
            "stream":true,"stream_options":{"include_usage":true}});
        let response = self.send(
            self.client
                .post(format!("{BASE_URL}/chat/completions"))
                .json(&body),
            remaining,
        )?;
        Ok(parse_reply(read_stream(response, on_call)?))
    }
    fn pricing(&self) -> Option<Pricing> {
        self.pricing.clone()
    }
    fn model_id(&self) -> Option<&str> {
        Some(&self.config.model_id)
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
        let entry = |prices: serde_json::Value| -> crate::models::CatalogEntry {
            serde_json::from_value(json!({"id":"nvidia/nemotron-fixture","pricing":prices}))
                .unwrap()
        };
        let p = catalog_pricing(
            &entry(json!({"prompt":"0.0000003","completion":"0.0000009"})),
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
            assert!(catalog_pricing(&entry(prices), 42).is_none());
        }
        let absent: crate::models::CatalogEntry =
            serde_json::from_value(json!({"id":"x"})).unwrap();
        assert!(catalog_pricing(&absent, 42).is_none());
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
    /// Server-sent events as the provider writes them, ending with the `[DONE]` marker.
    fn stream(events: &[Value]) -> String {
        let mut out: String = events.iter().map(|e| format!("data: {e}\n\n")).collect();
        out.push_str("data: [DONE]\n\n");
        out
    }
    fn replay(body: &str) -> Reply {
        parse_reply(read_stream(body.as_bytes(), &mut |_, _| {}).unwrap())
    }

    #[test]
    fn a_streamed_tool_call_is_rebuilt_and_reported_as_it_grows() {
        let body = stream(&[
            json!({"choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,
                "id":"c1","type":"function","function":{"name":"submit_answer","arguments":""}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,
                "function":{"arguments":"{\"text\":\"Vo"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,
                "function":{"arguments":"tre PC\"}"}}]},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}),
        ]);
        let mut seen = Vec::new();
        let envelope = read_stream(body.as_bytes(), &mut |name, args| {
            seen.push(format!("{name}:{args}"))
        })
        .unwrap();
        let reply = parse_reply(envelope);
        assert_eq!(reply.rejected, None);
        assert_eq!(reply.calls.len(), 1);
        assert_eq!(reply.calls[0].id, "c1");
        assert_eq!(reply.calls[0].function.arguments, "{\"text\":\"Votre PC\"}");
        assert_eq!(reply.total_tokens, Some(15));
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[2], "submit_answer:{\"text\":\"Votre PC\"}");
    }

    #[test]
    fn a_stream_cut_short_or_malformed_is_rejected_with_its_usage() {
        let usage = json!({"choices":[],
            "usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}});
        let start = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1",
            "type":"function","function":{"name":"get_processes","arguments":"{"}}]}}]});
        // No finish reason: the stream ended early and nothing is executed.
        let cut = replay(&stream(&[start.clone(), usage]));
        assert_eq!(cut.rejected, Some(AgentError::IncompleteResponse));
        assert!(cut.calls.is_empty());
        assert_eq!(cut.total_tokens, Some(15));
        // A truncated reply still says so.
        let long = json!({"choices":[{"index":0,"delta":{},"finish_reason":"length"}]});
        assert_eq!(
            replay(&stream(&[start.clone(), long])).rejected,
            Some(AgentError::OutputLimit)
        );
        // An event that is not JSON, or a call without an id, is never executed.
        let broken = format!("data: {start}\n\ndata: {{oops\n\n");
        assert_eq!(replay(&broken).rejected, Some(AgentError::InvalidResponse));
        let anonymous = json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,
            "function":{"name":"x","arguments":"{}"}}]},"finish_reason":"tool_calls"}]});
        assert_eq!(
            replay(&stream(&[anonymous])).rejected,
            Some(AgentError::InvalidResponse)
        );
        assert_eq!(replay("").rejected, Some(AgentError::InvalidResponse));
    }

    #[test]
    fn plain_text_replies_stream_too() {
        let reply = replay(&stream(&[
            json!({"choices":[{"index":0,"delta":{"content":"Hel"}}]}),
            json!({"choices":[{"index":0,"delta":{"content":"lo"},"finish_reason":"stop"}]}),
        ]));
        assert_eq!(reply.content.as_deref(), Some("Hello"));
        assert!(reply.calls.is_empty());
        assert_eq!(reply.total_tokens, None);
    }

    #[test]
    fn error_messages_never_include_a_provider_response_body() {
        assert_eq!(
            AgentError::Http(401).to_string(),
            "Provider returned HTTP 401; response body was withheld"
        );
    }
}
