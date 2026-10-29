//! Fixed official pages: download without sending the identifier; search their text locally.
use crate::tavily::{WebError, WebResult};
use pcagent_shared::unix_ms;
use pcagent_store::web_cache::WebCache;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde_json::json;
use std::{io::Read, time::Duration};

const MAX_PAGE_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const PAGE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const PAGES: &[&str] = &[
    "https://learn.microsoft.com/en-us/windows/release-health/status-windows-11-26h2",
    "https://learn.microsoft.com/en-us/windows/release-health/status-windows-11-25h2",
    "https://learn.microsoft.com/en-us/windows/release-health/status-windows-11-24h2",
    "https://learn.microsoft.com/en-us/windows/release-health/status-windows-10-22h2",
    "https://learn.microsoft.com/en-us/windows/release-health/status-windows-server-2022",
];

pub struct ReleaseHealth {
    client: Client,
    cache: Option<WebCache>,
}

impl ReleaseHealth {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            cache: None,
        }
    }

    pub fn set_cache(&mut self, cache: WebCache) {
        self.cache = Some(cache);
    }

    pub fn lookup(&self, query: &str) -> Result<Vec<WebResult>, WebError> {
        self.lookup_with(query, |url| self.download(url))
    }

    fn lookup_with(
        &self,
        query: &str,
        mut download: impl FnMut(&str) -> Result<String, WebError>,
    ) -> Result<Vec<WebResult>, WebError> {
        let identifier = query
            .split_whitespace()
            .find(|s| s.starts_with("KB") || s.starts_with("0x"))
            .ok_or(WebError::InvalidResponse)?;
        let mut results = Vec::new();
        let mut failed = false;
        for &url in PAGES {
            let now = unix_ms();
            let key = format!("release-health-v1:{url}");
            let cached = self
                .cache
                .as_ref()
                .and_then(|c| c.get(&key, now).ok())
                .flatten();
            let (text, read_at, cached_at) = if let Some(entry) =
                cached.filter(|entry| entry.value.as_str().is_some_and(|s| !s.is_empty()))
            {
                (
                    entry.value.as_str().expect("validated").to_owned(),
                    entry.read_at_unix_ms,
                    Some(entry.read_at_unix_ms),
                )
            } else {
                let Ok(text) = download(url) else {
                    failed = true;
                    continue;
                };
                if let Some(cache) = &self.cache {
                    let _ = cache.put(&key, &json!(text), now, PAGE_TTL_MS);
                }
                (text, now, None)
            };
            if let Some(snippet) = matching_excerpt(&text, identifier) {
                results.push(WebResult {
                    title: format!(
                        "Windows release health — {}",
                        url.rsplit('/').next().unwrap_or("")
                    ),
                    url: url.into(),
                    domain: "learn.microsoft.com".into(),
                    snippet,
                    published_date: None,
                    read_at_unix_ms: Some(read_at),
                    cached_at_unix_ms: cached_at,
                });
            }
        }
        if !failed {
            Ok(results)
        } else {
            Err(WebError::Network)
        }
    }

    fn download(&self, url: &str) -> Result<String, WebError> {
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(8))
            .send()
            .map_err(|_| WebError::Network)?;
        if !response.status().is_success() {
            return Err(WebError::Http);
        }
        if !response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/html"))
        {
            return Err(WebError::InvalidResponse);
        }
        let mut body = Vec::new();
        response
            .take(MAX_PAGE_BYTES + 1)
            .read_to_end(&mut body)
            .map_err(|_| WebError::Network)?;
        if body.len() as u64 > MAX_PAGE_BYTES {
            return Err(WebError::InvalidResponse);
        }
        let html = String::from_utf8(body).map_err(|_| WebError::InvalidResponse)?;
        page_text(&html)
    }
}

fn page_text(html: &str) -> Result<String, WebError> {
    let document = Html::parse_document(html);
    let main = Selector::parse("main").expect("static selector");
    let blocks = Selector::parse("h1, h2, h3, h4, p, td, li").expect("static selector");
    let main = document
        .select(&main)
        .next()
        .ok_or(WebError::InvalidResponse)?;
    let text = main
        .select(&blocks)
        .flat_map(|el| el.descendants())
        .filter(|node| {
            !node.ancestors().any(|ancestor| {
                ancestor
                    .value()
                    .as_element()
                    .is_some_and(|el| matches!(el.name(), "script" | "style"))
            })
        })
        .filter_map(|node| node.value().as_text())
        .flat_map(|text| text.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ");
    if text.len() < 40 {
        Err(WebError::InvalidResponse)
    } else {
        Ok(text)
    }
}

fn matching_excerpt(text: &str, identifier: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let needle = identifier.to_ascii_lowercase();
    let (start, _) = lower.match_indices(&needle).find(|(i, _)| {
        let before = lower[..*i].chars().next_back();
        let after = lower[*i + needle.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })?;
    let prefix: String = text[..start]
        .chars()
        .rev()
        .take(240)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let suffix: String = text[start..].chars().take(1300).collect();
    Some(format!("{prefix}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn official_page_text_and_exact_identifier_search() {
        let text = page_text("<html><script>KB99999 secret</script><main><h1>Known issues</h1><p>Écran noir after KB1234567 &amp; subsequent updates. Resolution is pending.<script>KB99999 secret</script></p></main></html>").unwrap();
        assert!(matching_excerpt(&text, "kb1234567")
            .unwrap()
            .contains("& subsequent"));
        assert!(matching_excerpt(&text, "KB123456").is_none());
        assert!(matching_excerpt(&text, "KB99999").is_none());
        assert!(matching_excerpt(&text, "0x0000009F").is_none());
        assert!(page_text("<html>Not an official page</html>").is_err());
        assert!(page_text("<main><p>Bad gateway</p></main>").is_err());
    }

    #[test]
    fn pages_are_reused_and_errors_never_become_cached_absences() {
        let mut health = ReleaseHealth::new(
            Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        );
        health.set_cache(WebCache::in_memory().unwrap());
        let mut downloads = 0;
        let first = health
            .lookup_with("KB1234567 known issues", |_| {
                downloads += 1;
                Ok("Microsoft confirmed an issue after KB1234567. A fix is available.".into())
            })
            .unwrap();
        assert_eq!(downloads, PAGES.len());
        let hit = health
            .lookup_with("KB1234567 known issues", |_| {
                panic!("cached pages must not be fetched")
            })
            .unwrap();
        assert_eq!(hit.len(), first.len());
        assert_eq!(hit[0].cached_at_unix_ms, first[0].read_at_unix_ms);
        assert!(health
            .lookup_with("KB9999999", |_| panic!("no new download"))
            .unwrap()
            .is_empty());
        let failing = ReleaseHealth::new(Client::new());
        assert!(failing
            .lookup_with("KB1234567", |_| Err(WebError::InvalidResponse))
            .is_err());
        assert!(failing
            .lookup_with("KB1234567", |url| {
                if url == PAGES[0] {
                    Err(WebError::Network)
                } else {
                    Ok("No matching issue on this page".into())
                }
            })
            .is_err());
    }

    #[test]
    #[ignore = "downloads the official Microsoft allowlist; no API key or paid search"]
    fn live_official_pages_are_valid_html() {
        let health = ReleaseHealth::new(
            Client::builder()
                .https_only(true)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        );
        for url in PAGES {
            let text = health.download(url).expect("official page");
            assert!(text.contains("Known issues") || text.contains("known issues"));
            println!("validated {url}: {} text bytes", text.len());
        }
    }
}
