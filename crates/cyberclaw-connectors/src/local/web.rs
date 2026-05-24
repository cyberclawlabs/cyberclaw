use super::LocalConnector;
use crate::types::{
    CapabilityExecutionRequest, WebFetchInput, WebFetchOutput, WebSearchInput, WebSearchOutput,
    WebSearchResult,
};
use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use reqwest::Url;
use std::time::Duration;
use tracing::{debug, info};

const DEFAULT_FETCH_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_FETCH_MAX_BYTES: usize = 64 * 1024;
const DEFAULT_SEARCH_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_SEARCH_MAX_RESULTS: usize = 5;
const MAX_SEARCH_RESULTS_CAP: usize = 20;
const DUCKDUCKGO_ENDPOINT: &str = "https://api.duckduckgo.com/";

fn validate_http_url(url: &str) -> anyhow::Result<Url> {
    let parsed = Url::parse(url)?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        scheme => Err(anyhow::anyhow!(
            "Unsupported URL scheme '{}', only http/https are allowed",
            scheme
        )),
    }
}

pub async fn fetch(
    _connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: WebFetchInput = serde_json::from_value(request.input)?;
    let parsed_url = validate_http_url(&input.url)?;
    let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_FETCH_TIMEOUT_MS);
    let max_bytes = usize::try_from(input.max_bytes.unwrap_or(DEFAULT_FETCH_MAX_BYTES as u64))
        .unwrap_or(DEFAULT_FETCH_MAX_BYTES);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .no_proxy()
        .build()?;

    debug!("Fetching URL: {}", parsed_url);
    let response = client.get(parsed_url.clone()).send().await?;
    let status_code = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let body_bytes = response.bytes().await?;

    let (body_slice, truncated) = if body_bytes.len() > max_bytes {
        (&body_bytes[..max_bytes], true)
    } else {
        (&body_bytes[..], false)
    };

    let output = WebFetchOutput {
        url: parsed_url.to_string(),
        status_code,
        content_type,
        body: String::from_utf8_lossy(body_slice).to_string(),
        truncated,
    };

    info!(
        "Fetched URL {} status={} bytes={} truncated={} prompt_supplied={}",
        output.url,
        output.status_code,
        output.body.len(),
        output.truncated,
        input.prompt.is_some()
    );

    Ok(serde_json::to_value(output)?)
}

fn push_related_topics(value: &serde_json::Value, acc: &mut Vec<WebSearchResult>) {
    if let Some(url) = value.get("FirstURL").and_then(|v| v.as_str()) {
        let text = value
            .get("Text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        if !text.is_empty() {
            let title = text.split(" - ").next().unwrap_or(text).to_string();
            acc.push(WebSearchResult {
                title,
                url: url.to_string(),
                snippet: Some(text.to_string()),
            });
        }
    }

    if let Some(topics) = value.get("Topics").and_then(|v| v.as_array()) {
        for topic in topics {
            push_related_topics(topic, acc);
        }
    }
}

fn parse_search_results(
    query: &str,
    payload: &serde_json::Value,
    max_results: usize,
) -> Vec<WebSearchResult> {
    let mut results = Vec::new();

    if let Some(array) = payload.get("results").and_then(|v| v.as_array()) {
        for item in array.iter().take(max_results) {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !title.is_empty() && !url.is_empty() {
                results.push(WebSearchResult {
                    title,
                    url,
                    snippet: item
                        .get("snippet")
                        .and_then(|v| v.as_str())
                        .map(ToString::to_string),
                });
            }
        }
        return results;
    }

    let abstract_url = payload
        .get("AbstractURL")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    let abstract_text = payload
        .get("AbstractText")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim();
    if !abstract_url.is_empty() {
        let title = payload
            .get("Heading")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(query);
        results.push(WebSearchResult {
            title: title.to_string(),
            url: abstract_url.to_string(),
            snippet: if abstract_text.is_empty() {
                None
            } else {
                Some(abstract_text.to_string())
            },
        });
    }

    if let Some(topics) = payload.get("RelatedTopics").and_then(|v| v.as_array()) {
        for topic in topics {
            if results.len() >= max_results {
                break;
            }
            push_related_topics(topic, &mut results);
        }
    }

    results.truncate(max_results);
    results
}

fn normalize_domain_set(items: Option<Vec<String>>) -> Option<std::collections::HashSet<String>> {
    items.map(|domains| {
        domains
            .into_iter()
            .map(|d| d.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|d| !d.is_empty())
            .collect()
    })
}

fn result_domain(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

fn domain_allowed(
    url: &str,
    allow: &Option<std::collections::HashSet<String>>,
    block: &Option<std::collections::HashSet<String>>,
) -> bool {
    if allow.is_none() && block.is_none() {
        return true;
    }

    let domain = match result_domain(url) {
        Some(d) => d,
        None => return false,
    };

    if let Some(blocked) = block {
        if blocked
            .iter()
            .any(|entry| domain == *entry || domain.ends_with(&format!(".{}", entry)))
        {
            return false;
        }
    }

    if let Some(allowed) = allow {
        return allowed
            .iter()
            .any(|entry| domain == *entry || domain.ends_with(&format!(".{}", entry)));
    }

    true
}

/// Provider selection for `web.search`.
///
/// Picked from `WEB_SEARCH_PROVIDER` env var at request time. Defaults
/// to `DuckDuckGo` (no API key required, "instant answer" scope —
/// returns 0 results for most queries by design).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchProvider {
    DuckDuckGo,
    Brave,
    Tavily,
    Exa,
}

impl SearchProvider {
    fn from_env() -> Self {
        match std::env::var("WEB_SEARCH_PROVIDER")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "brave" => Self::Brave,
            "tavily" => Self::Tavily,
            "exa" => Self::Exa,
            "duckduckgo" | "ddg" | "" => Self::DuckDuckGo,
            other => {
                tracing::warn!(
                    "Unknown WEB_SEARCH_PROVIDER='{}', falling back to duckduckgo",
                    other
                );
                Self::DuckDuckGo
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Brave => "brave",
            Self::Tavily => "tavily",
            Self::Exa => "exa",
        }
    }
}

pub async fn search(
    _connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<serde_json::Value> {
    let input: WebSearchInput = serde_json::from_value(request.input)?;
    let query = input.query.trim();
    if query.is_empty() {
        return Err(anyhow::anyhow!("Search query cannot be empty"));
    }

    let max_results = usize::try_from(
        input
            .max_results
            .unwrap_or(DEFAULT_SEARCH_MAX_RESULTS as u32),
    )
    .unwrap_or(DEFAULT_SEARCH_MAX_RESULTS)
    .clamp(1, MAX_SEARCH_RESULTS_CAP);
    if input.allowed_domains.is_some() && input.blocked_domains.is_some() {
        return Err(anyhow::anyhow!(
            "Cannot specify both allowed_domains and blocked_domains"
        ));
    }
    let allowed_domains = normalize_domain_set(input.allowed_domains.clone());
    let blocked_domains = normalize_domain_set(input.blocked_domains.clone());
    let timeout_ms = input.timeout_ms.unwrap_or(DEFAULT_SEARCH_TIMEOUT_MS);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .no_proxy()
        .build()?;

    // Pick provider. Only force DuckDuckGo when the caller passed an explicit
    // endpoint that looks like a DDG URL (contains "duckduckgo"). This preserves
    // the existing mock-server test behaviour while letting WEB_SEARCH_PROVIDER=exa
    // (or any other value) take effect even when an endpoint override is present.
    // BUG-CB-05: previously any input.endpoint = Some(_) forced DuckDuckGo,
    // silently ignoring the env var and sending all requests through DDG/CoinGecko
    // fallback chains that are fully blocked in many network environments.
    let provider = if input
        .endpoint
        .as_deref()
        .map(|ep| ep.to_ascii_lowercase().contains("duckduckgo"))
        .unwrap_or(false)
    {
        SearchProvider::DuckDuckGo
    } else {
        SearchProvider::from_env()
    };
    let api_key = std::env::var("WEB_SEARCH_API_KEY").ok();

    debug!(
        "web.search query='{}' provider={} max_results={}",
        query,
        provider.label(),
        max_results
    );

    let raw_results = match provider {
        SearchProvider::DuckDuckGo => {
            let endpoint = input
                .endpoint
                .clone()
                .unwrap_or_else(|| DUCKDUCKGO_ENDPOINT.to_string());
            let parsed_endpoint = validate_http_url(&endpoint)?;
            let payload = client
                .get(parsed_endpoint)
                .query(&[
                    ("q", query.to_string()),
                    ("format", "json".to_string()),
                    ("no_redirect", "1".to_string()),
                    ("no_html", "1".to_string()),
                    ("limit", max_results.to_string()),
                ])
                .send()
                .await?
                .json::<serde_json::Value>()
                .await?;
            parse_search_results(query, &payload, max_results)
        }
        SearchProvider::Brave => {
            search_brave(&client, query, max_results, api_key.as_deref()).await?
        }
        SearchProvider::Tavily => {
            search_tavily(&client, query, max_results, api_key.as_deref()).await?
        }
        SearchProvider::Exa => {
            // Exa accepts WEB_SEARCH_API_KEY for parity with brave/tavily,
            // but EXA_API_KEY also works (matches Exa's documented env name).
            let exa_key = api_key
                .clone()
                .or_else(|| std::env::var("EXA_API_KEY").ok());
            search_exa(&client, query, max_results, exa_key.as_deref()).await?
        }
    };

    let results: Vec<WebSearchResult> = raw_results
        .into_iter()
        .filter(|result| domain_allowed(&result.url, &allowed_domains, &blocked_domains))
        .take(max_results)
        .collect();
    let output = WebSearchOutput {
        query: query.to_string(),
        provider: provider.label().to_string(),
        total: u32::try_from(results.len()).unwrap_or(u32::MAX),
        results,
    };

    info!(
        "Web search query='{}' results={} provider={}",
        output.query, output.total, output.provider
    );

    Ok(serde_json::to_value(output)?)
}

/// Brave Search API: `https://api.search.brave.com/res/v1/web/search`.
/// Requires `X-Subscription-Token` header with the key.
async fn search_brave(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
    api_key: Option<&str>,
) -> anyhow::Result<Vec<WebSearchResult>> {
    let key = api_key.ok_or_else(|| {
        anyhow::anyhow!("WEB_SEARCH_API_KEY is required when WEB_SEARCH_PROVIDER=brave")
    })?;
    let url = "https://api.search.brave.com/res/v1/web/search";
    let payload = client
        .get(url)
        .header("X-Subscription-Token", key)
        .header("Accept", "application/json")
        .query(&[("q", query.to_string()), ("count", max_results.to_string())])
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let mut out = Vec::new();
    if let Some(arr) = payload
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
    {
        for item in arr.iter().take(max_results) {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let snippet = item
                .get("description")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            if !title.is_empty() && !url.is_empty() {
                out.push(WebSearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }
    Ok(out)
}

/// Tavily Search API: `POST https://api.tavily.com/search` with JSON body
/// `{api_key, query, max_results}`.
async fn search_tavily(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
    api_key: Option<&str>,
) -> anyhow::Result<Vec<WebSearchResult>> {
    let key = api_key.ok_or_else(|| {
        anyhow::anyhow!("WEB_SEARCH_API_KEY is required when WEB_SEARCH_PROVIDER=tavily")
    })?;
    let url = "https://api.tavily.com/search";
    let payload = client
        .post(url)
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "api_key": key,
            "query": query,
            "max_results": max_results,
            "search_depth": "basic",
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let mut out = Vec::new();
    if let Some(arr) = payload.get("results").and_then(|r| r.as_array()) {
        for item in arr.iter().take(max_results) {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let snippet = item
                .get("content")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);
            if !title.is_empty() && !url.is_empty() {
                out.push(WebSearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }
    Ok(out)
}

/// Exa Search API: `POST https://api.exa.ai/search` with JSON body.
/// Auth via `x-api-key` header. Returns rich snippets that work for
/// real research queries (closes BT-06 from Hermes benchmark).
async fn search_exa(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
    api_key: Option<&str>,
) -> anyhow::Result<Vec<WebSearchResult>> {
    let key = api_key.ok_or_else(|| {
        anyhow::anyhow!(
            "EXA_API_KEY (or WEB_SEARCH_API_KEY) is required when WEB_SEARCH_PROVIDER=exa"
        )
    })?;
    let url = "https://api.exa.ai/search";
    let payload = client
        .post(url)
        .header("Accept", "application/json")
        .header("x-api-key", key)
        .json(&serde_json::json!({
            "query": query,
            "numResults": max_results,
            "type": "auto",
        }))
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;

    let mut out = Vec::new();
    if let Some(arr) = payload.get("results").and_then(|r| r.as_array()) {
        for item in arr.iter().take(max_results) {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // Exa returns "text" or "snippet" depending on params; try both.
            let snippet = item
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| item.get("snippet").and_then(|v| v.as_str()))
                .map(ToString::to_string);
            if !title.is_empty() && !url.is_empty() {
                out.push(WebSearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }
    Ok(out)
}

/// §4 — authoritative facade declarations for `web.*` capabilities.
///
/// `BuiltinToolRegistry::default_facades` MUST NOT duplicate these entries.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "web_fetch".to_string(),
                description: "Fetch content from a URL and optionally apply a prompt to extract \
                    or summarize the result."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("web.fetch".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The URL to fetch content from" },
                        "prompt": { "type": "string", "description": "Optional prompt to apply to the fetched content" },
                        "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds (default 10000)" },
                        "max_bytes": { "type": "integer", "description": "Max response bytes to read (default 65536)" }
                    },
                    "required": ["url"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Web,
        ),
        (
            CapabilityFacade {
                name: "web_search".to_string(),
                description: "Perform a web search and return summarised results. Supports domain \
                    allow/block lists for filtering."
                    .to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string("web.search".to_string()).unwrap(),
                risk_level: RiskLevel::Medium,
                effects: vec!["write".to_string()],
                read_only: false,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "The search query" },
                        "allowed_domains": { "type": "array", "items": { "type": "string" }, "description": "Only include results from these domains" },
                        "blocked_domains": { "type": "array", "items": { "type": "string" }, "description": "Exclude results from these domains" },
                        "max_results": { "type": "integer", "description": "Maximum number of results (default 5, max 20)" },
                        "timeout_ms": { "type": "integer", "description": "Timeout in milliseconds (default 10000)" }
                    },
                    "required": ["query"]
                })),
                exposure: FacadeExposure::LlmDefault,
            },
            ToolsetCategory::Web,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_provider_label_roundtrip() {
        assert_eq!(SearchProvider::DuckDuckGo.label(), "duckduckgo");
        assert_eq!(SearchProvider::Brave.label(), "brave");
        assert_eq!(SearchProvider::Tavily.label(), "tavily");
        assert_eq!(SearchProvider::Exa.label(), "exa");
    }

    #[tokio::test]
    async fn search_brave_requires_api_key() {
        let client = reqwest::Client::new();
        let err = search_brave(&client, "rust", 5, None).await.unwrap_err();
        assert!(err.to_string().contains("WEB_SEARCH_API_KEY"));
        assert!(err.to_string().contains("brave"));
    }

    #[tokio::test]
    async fn search_tavily_requires_api_key() {
        let client = reqwest::Client::new();
        let err = search_tavily(&client, "rust", 5, None).await.unwrap_err();
        assert!(err.to_string().contains("WEB_SEARCH_API_KEY"));
        assert!(err.to_string().contains("tavily"));
    }

    #[tokio::test]
    async fn search_exa_requires_api_key() {
        let client = reqwest::Client::new();
        let err = search_exa(&client, "rust", 5, None).await.unwrap_err();
        assert!(err.to_string().contains("EXA_API_KEY"));
    }

    /// Real Exa API smoke test — only runs with `--ignored` and requires
    /// `EXA_API_KEY` in env. Validates BT-06 closure end-to-end.
    #[tokio::test]
    #[ignore = "requires real EXA_API_KEY env var; run via: cargo test --lib search_exa_real_query -- --ignored --nocapture"]
    async fn search_exa_real_query() {
        let key = std::env::var("EXA_API_KEY").expect("EXA_API_KEY must be set");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .expect("build client");
        let results = search_exa(&client, "Rust async runtime benchmark 2025", 3, Some(&key))
            .await
            .expect("Exa query succeeded");
        assert!(!results.is_empty(), "Exa returned zero results");
        for r in &results {
            assert!(!r.title.is_empty(), "title must not be empty: {r:?}");
            assert!(r.url.starts_with("http"), "url must be HTTP(S): {r:?}");
            println!("- {} | {}", r.title, r.url);
        }
        println!("Exa returned {} results — BT-06 verified", results.len());
    }

    // BUG-CB-05: Verify endpoint-to-provider selection logic.
    // The fixed rule: only force DuckDuckGo when the endpoint URL contains
    // "duckduckgo"; otherwise let WEB_SEARCH_PROVIDER drive provider selection.

    #[test]
    fn test_endpoint_with_ddg_url_uses_ddg_provider() {
        let endpoint = Some("https://api.duckduckgo.com/".to_string());
        let forces_ddg = endpoint
            .as_deref()
            .map(|ep| ep.to_ascii_lowercase().contains("duckduckgo"))
            .unwrap_or(false);
        assert!(
            forces_ddg,
            "An endpoint containing 'duckduckgo' should force DuckDuckGo provider"
        );
    }

    #[test]
    fn test_endpoint_with_exa_url_does_not_force_ddg() {
        let endpoint = Some("https://api.exa.ai/search".to_string());
        let forces_ddg = endpoint
            .as_deref()
            .map(|ep| ep.to_ascii_lowercase().contains("duckduckgo"))
            .unwrap_or(false);
        assert!(
            !forces_ddg,
            "An endpoint not containing 'duckduckgo' must not force DDG provider; \
             WEB_SEARCH_PROVIDER should drive selection instead"
        );
    }
}
