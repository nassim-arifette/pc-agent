//! Tavily web search for the typed external tools (contract C6). The query is composed locally
//! from measured references, never written by the model. The key goes only in the Authorization
//! header: never in logs, in the page or in the model's context. No `include_answer` (another
//! model's unverifiable answer), no raw page content and no extraction of a model-chosen URL.
use reqwest::{blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::Read,
    time::{Duration, Instant},
};

const SEARCH_URL: &str = "https://api.tavily.com/search";
/// At most this many results per search (C6).
pub const MAX_RESULTS: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;
/// A snippet longer than this is cut: Tavily's chunks are at most 500 characters each.
const MAX_SNIPPET_CHARS: usize = 1600;
const MAX_TITLE_CHARS: usize = 200;

/// One search, composed locally.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WebQuery {
    pub query: String,
    /// Restricts results to these domains when not empty.
    pub domains: Vec<String>,
    /// Boosts results from this country (Tavily's lowercase English country name).
    pub country: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebResult {
    pub title: String,
    /// Always `https://`.
    pub url: String,
    pub domain: String,
    pub snippet: String,
    /// As the source states it; absent when Tavily does not know it.
    pub published_date: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WebSearch {
    pub results: Vec<WebResult>,
    /// Tavily credits this search used, when reported.
    pub credits: Option<f64>,
    pub duration_ms: u64,
}

/// Why a search gave no result. Never contains the key or a response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WebError {
    Network,
    Timeout,
    /// The key was refused (HTTP 401 or 403).
    Unauthorized,
    /// The plan's credits or rate limit are exhausted (HTTP 429 or 432).
    Limited,
    Http,
    InvalidResponse,
}

/// The search service behind the external tools: Tavily, or a scripted stand-in in tests.
pub trait WebSearcher {
    fn search(&mut self, query: &WebQuery) -> Result<WebSearch, WebError>;
}

pub struct Tavily {
    api_key: String,
    client: Client,
}

impl Tavily {
    /// Builds a client without making a request; `None` without a key.
    pub fn new(api_key: &str) -> Option<Self> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return None;
        }
        let client = Client::builder()
            .https_only(true)
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .ok()?;
        Some(Self {
            api_key: api_key.to_owned(),
            client,
        })
    }
}

impl WebSearcher for Tavily {
    fn search(&mut self, query: &WebQuery) -> Result<WebSearch, WebError> {
        let started = Instant::now();
        let response = self
            .client
            .post(SEARCH_URL)
            .bearer_auth(&self.api_key)
            .timeout(TIMEOUT)
            .json(&request_body(query))
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    WebError::Timeout
                } else {
                    WebError::Network
                }
            })?;
        match response.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(WebError::Unauthorized),
            429 | 432 | 433 => return Err(WebError::Limited),
            _ => return Err(WebError::Http),
        }
        let mut body = Vec::new();
        response
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|_| WebError::Network)?;
        if body.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(WebError::InvalidResponse);
        }
        let value: Value = serde_json::from_slice(&body).map_err(|_| WebError::InvalidResponse)?;
        let mut search = parse_response(&value, &query.domains)?;
        search.duration_ms = started.elapsed().as_millis() as u64;
        Ok(search)
    }
}

pub(crate) fn request_body(query: &WebQuery) -> Value {
    let mut body = json!({
        "query": query.query,
        "search_depth": "basic",
        "topic": "general",
        "max_results": MAX_RESULTS,
        "include_answer": false,
        "include_raw_content": false,
        "include_images": false,
        "include_favicon": false,
        "include_usage": true,
    });
    if !query.domains.is_empty() {
        body["include_domains"] = json!(query.domains);
    }
    if let Some(country) = &query.country {
        body["country"] = json!(country);
    }
    body
}

/// Keeps https results from the allowed domains, with bounded titles and snippets.
pub(crate) fn parse_response(value: &Value, domains: &[String]) -> Result<WebSearch, WebError> {
    let items = value["results"]
        .as_array()
        .ok_or(WebError::InvalidResponse)?;
    let results = items
        .iter()
        .filter_map(|item| {
            let url = item["url"].as_str()?.trim();
            let domain = domain(url)?;
            if !domains.is_empty() && !domains.iter().any(|allowed| within(&domain, allowed)) {
                return None;
            }
            let snippet = clean(
                item["content"].as_str().unwrap_or_default(),
                MAX_SNIPPET_CHARS,
            );
            if snippet.is_empty() {
                return None;
            }
            Some(WebResult {
                title: clean(item["title"].as_str().unwrap_or(&domain), MAX_TITLE_CHARS),
                url: url.to_owned(),
                domain,
                snippet,
                published_date: item["published_date"]
                    .as_str()
                    .map(|d| clean(d, 40))
                    .filter(|d| !d.is_empty()),
            })
        })
        .take(MAX_RESULTS)
        .collect();
    Ok(WebSearch {
        results,
        credits: value["usage"]["credits"].as_f64(),
        duration_ms: 0,
    })
}

/// The host of an `https://` URL without `www.`, lowercase; `None` for any other scheme.
pub fn domain(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    // URLs with user information, a backslash, a bad port or any space, quote or control
    // character are refused: browsers and this parser could disagree on their host.
    if !url
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\' | b'^' | b'`' | b'<' | b'>'))
    {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let (host, port) = authority
        .rsplit_once(':')
        .map_or((authority, None), |(host, port)| (host, Some(port)));
    if port.is_some_and(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let valid = host.contains('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        });
    valid.then(|| host.strip_prefix("www.").unwrap_or(&host).to_owned())
}

/// `support.microsoft.com` is within `microsoft.com`; `evilmicrosoft.com` is not.
fn within(host: &str, allowed: &str) -> bool {
    host == allowed || host.ends_with(&format!(".{allowed}"))
}

/// Control characters removed, whitespace runs collapsed, length bounded.
fn clean(text: &str, max: usize) -> String {
    let collapsed = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    if collapsed.chars().count() > max {
        collapsed.chars().take(max).collect::<String>() + "…"
    } else {
        collapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_never_asks_for_another_models_answer_or_raw_pages() {
        let body = request_body(&WebQuery {
            query: "NVIDIA GeForce RTX 3060 driver 32.0.15.6094 release notes".into(),
            domains: vec!["nvidia.com".into()],
            country: None,
        });
        assert_eq!(body["include_answer"], false);
        assert_eq!(body["include_raw_content"], false);
        assert_eq!(body["search_depth"], "basic");
        assert_eq!(body["max_results"], MAX_RESULTS);
        assert_eq!(body["include_domains"], json!(["nvidia.com"]));
        assert!(body.get("country").is_none());
    }

    #[test]
    fn only_https_results_from_allowed_domains_are_kept() {
        let response = json!({"results":[
            {"title":"Release notes","url":"https://www.nvidia.com/en-us/drivers/","content":"Version 566.36\n\n fixes  stutter","published_date":"2025-01-10"},
            {"title":"Mirror","url":"https://evilnvidia.com/x","content":"fake"},
            {"title":"Plain","url":"http://nvidia.com/y","content":"no tls"},
            {"title":"Support","url":"https://support.nvidia.com/a","content":"Known issue"},
            {"title":"Empty","url":"https://nvidia.com/z","content":"  "}],
            "usage":{"credits":1}});
        let search = parse_response(&response, &["nvidia.com".into()]).unwrap();
        let domains: Vec<_> = search.results.iter().map(|r| r.domain.as_str()).collect();
        assert_eq!(domains, ["nvidia.com", "support.nvidia.com"]);
        assert_eq!(search.results[0].snippet, "Version 566.36 fixes stutter");
        assert_eq!(
            search.results[0].published_date.as_deref(),
            Some("2025-01-10")
        );
        assert_eq!(search.results[1].published_date, None);
        assert_eq!(search.credits, Some(1.0));
        assert!(parse_response(&json!({"error":"x"}), &[]).is_err());
    }

    /// One real search with the saved key (1 credit): `cargo test -p pcagent-agent -- --ignored
    /// live_search`. Prints only the result domains, never the key.
    #[test]
    #[ignore]
    fn live_search_with_the_saved_key() {
        let Some(key) = pcagent_agent::credentials::Credentials::load().get("TAVILY_API_KEY")
        else {
            eprintln!("no TAVILY_API_KEY saved");
            return;
        };
        let mut tavily = Tavily::new(&key).expect("client");
        let search = tavily
            .search(&WebQuery {
                query: "KB5034441 known issues".into(),
                domains: vec!["learn.microsoft.com".into(), "support.microsoft.com".into()],
                country: None,
            })
            .expect("search");
        let domains: Vec<_> = search.results.iter().map(|r| r.domain.as_str()).collect();
        eprintln!(
            "{} results from {domains:?} in {} ms, credits {:?}",
            search.results.len(),
            search.duration_ms,
            search.credits
        );
        assert!(!search.results.is_empty());
    }

    #[test]
    fn domains_come_only_from_https_urls() {
        assert_eq!(
            domain("https://www.Intel.com/a?b").as_deref(),
            Some("intel.com")
        );
        assert_eq!(
            domain("https://learn.microsoft.com:443/x").as_deref(),
            Some("learn.microsoft.com")
        );
        assert_eq!(domain("https://user@learn.microsoft.com/x"), None);
        assert_eq!(domain("https://evil.com\\@nvidia.com/"), None);
        assert_eq!(domain("https://nvidia.com:80a/"), None);
        assert_eq!(domain("http://intel.com"), None);
        assert_eq!(domain("javascript:alert(1)"), None);
        assert_eq!(domain("https://localhost/x"), None);
        assert!(!within("evilmicrosoft.com", "microsoft.com"));
        assert!(within("support.microsoft.com", "microsoft.com"));
    }
}
