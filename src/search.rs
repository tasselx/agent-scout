//! Core search logic against Windsurf/Devin server-side web search
//! (`GetWebSearchResults`). Ported from the reference JS implementation.
//!
//! Pure helpers are separated so they can be unit-tested without network.

use serde_json::{json, Value};

use crate::api;

pub const WEB_SEARCH_PATH: &str = "/exa.api_server_pb.ApiServerService/GetWebSearchResults";
pub use api::SERVER_HOSTS;
pub const DEFAULT_TIMEOUT_SECS: u64 = 20;
pub const MAX_LIMIT: usize = 10;
const RPC: &str = "GetWebSearchResults";

/// A normalized search hit shipped to the client.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub source: String,
}

/// Options that shape a single search request.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub limit: usize,
    pub domain: Option<String>,
    pub mode: Option<Value>,
    pub hosts: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: 5,
            domain: None,
            mode: None,
            hosts: None,
            timeout_secs: None,
        }
    }
}

/// Build the JSON request body sent to the upstream endpoint.
/// Mirrors `buildRequestBody` in the JS reference.
pub fn build_request_body(api_key: &str, query: &str, opts: &SearchOptions) -> Value {
    let trimmed = query.trim();
    let limit = if opts.limit == 0 {
        5
    } else {
        opts.limit.min(MAX_LIMIT)
    };
    let mut body = json!({
        "metadata": api::metadata(api_key),
        "query": trimmed,
        "limit": limit,
    });
    if let Some(domain) = &opts.domain {
        body["domain"] = json!(domain);
    }
    if let Some(mode) = &opts.mode {
        body["mode"] = mode.clone();
    }
    body
}

fn first_string(record: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(s) = record.get(*key) {
            if let Some(st) = s.as_str() {
                let t = st.trim();
                if !t.is_empty() {
                    return t.to_string();
                }
            }
        }
    }
    String::new()
}

/// Normalize one raw upstream record into a `Hit`, or `None` if it lacks a
/// usable title+url. Mirrors `normalizeHit`.
pub fn normalize_hit(raw: &Value) -> Option<Hit> {
    if !raw.is_object() {
        return None;
    }
    let title = first_string(raw, &["title", "name", "webTitle"]);
    let url = first_string(raw, &["url", "sourceUrl", "webUrl", "link"]);
    if title.is_empty() || url.is_empty() {
        return None;
    }
    let snippet = first_string(raw, &["snippet", "summary", "text", "content"]);
    Some(Hit {
        title,
        url,
        snippet,
        source: "windsurf".to_string(),
    })
}

/// Extract and normalize all hits from an upstream payload. Mirrors `normalizeHits`.
pub fn normalize_hits(payload: &Value) -> Vec<Hit> {
    match payload.get("results") {
        Some(Value::Array(results)) => results.iter().filter_map(normalize_hit).collect(),
        _ => Vec::new(),
    }
}

/// Thin error type so the CLI/MCP layer can render a clean message.
#[derive(Debug)]
pub enum SearchError {
    Timeout,
    Http(u16, String),
    Transport(String),
    Json(String),
}

impl From<api::RpcError> for SearchError {
    fn from(error: api::RpcError) -> Self {
        match error.kind {
            api::RpcErrorKind::Timeout => Self::Timeout,
            api::RpcErrorKind::Http(status, raw) => Self::Http(status, raw),
            api::RpcErrorKind::Transport(message) => Self::Transport(message),
            api::RpcErrorKind::Json(message) => Self::Json(message),
        }
    }
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "{RPC} timed out"),
            Self::Http(status, raw) => write!(f, "{RPC} -> HTTP {status}: {raw}"),
            Self::Transport(message) => write!(f, "{RPC} transport: {message}"),
            Self::Json(message) => write!(f, "{RPC} response parse: {message}"),
        }
    }
}

impl std::error::Error for SearchError {}

/// Run one search against the upstream endpoint, trying each host in order
/// until one returns success. Mirrors `searchWindsurf`.
pub fn search(api_key: &str, query: &str, opts: &SearchOptions) -> Result<Vec<Hit>, SearchError> {
    let body = build_request_body(api_key, query, opts);
    let timeout = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let hosts = api::resolve_hosts(opts.hosts.as_deref());
    let payload = api::post_json_failover(WEB_SEARCH_PATH, &body, &hosts, timeout, RPC)
        .map_err(SearchError::from)?;
    Ok(normalize_hits(&payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_limits_and_metadata() {
        let opts = SearchOptions { limit: 99, ..Default::default() };
        let body = build_request_body("tok", "  hello  ", &opts);
        assert_eq!(body["metadata"]["apiKey"], "tok");
        assert_eq!(body["query"], "hello");
        assert_eq!(body["limit"], MAX_LIMIT as u64);
        assert!(body.get("domain").is_none());
    }

    #[test]
    fn build_request_body_domain_and_mode() {
        let opts = SearchOptions {
            limit: 3,
            domain: Some("github.com".into()),
            mode: Some(json!(2)),
            ..Default::default()
        };
        let body = build_request_body("tok", "q", &opts);
        assert_eq!(body["domain"], "github.com");
        assert_eq!(body["mode"], 2);
        assert_eq!(body["limit"], 3);
    }

    #[test]
    fn normalize_skips_incomplete_records() {
        let payload = json!({
            "results": [
                { "title": "A", "url": "https://a.com", "snippet": "sa" },
                { "title": "no url" },
                { "name": "B", "sourceUrl": "https://b.com", "text": "sb" },
                "not-an-object"
            ]
        });
        let hits = normalize_hits(&payload);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "A");
        assert_eq!(hits[0].url, "https://a.com");
        assert_eq!(hits[0].source, "windsurf");
        assert_eq!(hits[1].title, "B");
        assert_eq!(hits[1].snippet, "sb");
    }
}
