//! Web docs options against Windsurf/Devin server-side API
//! (`GetWebDocsOptions`). Reverse-engineered from Devin.app's bundled
//! language-server protocol (`exa.api_server_pb.ApiServerService/GetWebDocsOptions`).
//!
//! The RPC takes only a `metadata` object and returns a list of `WebDocsOption`
//! records — documentation sources (llms.txt style) the IDE offers to attach to
//! a session's context. Pure helpers are separated so they can be unit-tested
//! without network.

use serde_json::{json, Value};

use crate::api;

pub const WEB_DOCS_PATH: &str = "/exa.api_server_pb.ApiServerService/GetWebDocsOptions";
pub use api::SERVER_HOSTS;
pub const DEFAULT_TIMEOUT_SECS: u64 = 20;
const RPC: &str = "GetWebDocsOptions";

/// One documentation-source option returned by the server.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebDocsOption {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_search_domain: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub synonyms: Vec<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub is_featured: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Options that shape a single GetWebDocsOptions request.
#[derive(Debug, Clone)]
pub struct WebDocsOptions {
    pub hosts: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}

impl Default for WebDocsOptions {
    fn default() -> Self {
        Self {
            hosts: None,
            timeout_secs: None,
        }
    }
}

/// Build the JSON request body sent to the upstream endpoint.
/// Mirrors the `{ metadata }` shape the Devin.app UI sends.
pub fn build_request_body(api_key: &str) -> Value {
    json!({
        "metadata": api::metadata(api_key),
    })
}

fn opt_string(record: &Value, key: &str) -> Option<String> {
    record
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Normalize one raw upstream record into a `WebDocsOption`, or `None` if it
/// lacks a usable label and both value variants.
pub fn normalize_option(raw: &Value) -> Option<WebDocsOption> {
    if !raw.is_object() {
        return None;
    }
    let label = opt_string(raw, "label")?;
    if label.is_empty() {
        return None;
    }
    let docs_url = opt_string(raw, "docsUrl");
    let docs_search_domain = opt_string(raw, "docsSearchDomain");
    if docs_url.is_none() && docs_search_domain.is_none() {
        return None;
    }
    let synonyms = raw
        .get("synonyms")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let is_featured = raw.get("isFeatured").and_then(Value::as_bool).unwrap_or(false);
    Some(WebDocsOption {
        label,
        docs_url,
        docs_search_domain,
        synonyms,
        is_featured,
    })
}

/// Extract and normalize all options from an upstream payload.
pub fn normalize_options(payload: &Value) -> Vec<WebDocsOption> {
    match payload.get("options") {
        Some(Value::Array(options)) => options.iter().filter_map(normalize_option).collect(),
        _ => Vec::new(),
    }
}

/// Thin error type so the CLI/MCP layer can render a clean message.
#[derive(Debug)]
pub enum WebDocsError {
    Timeout,
    Http(u16, String),
    Transport(String),
    Json(String),
}

impl From<api::RpcError> for WebDocsError {
    fn from(error: api::RpcError) -> Self {
        match error.kind {
            api::RpcErrorKind::Timeout => Self::Timeout,
            api::RpcErrorKind::Http(status, raw) => Self::Http(status, raw),
            api::RpcErrorKind::Transport(message) => Self::Transport(message),
            api::RpcErrorKind::Json(message) => Self::Json(message),
        }
    }
}

impl std::fmt::Display for WebDocsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "{RPC} timed out"),
            Self::Http(status, raw) => write!(f, "{RPC} -> HTTP {status}: {raw}"),
            Self::Transport(message) => write!(f, "{RPC} transport: {message}"),
            Self::Json(message) => write!(f, "{RPC} response parse: {message}"),
        }
    }
}

impl std::error::Error for WebDocsError {}

/// Fetch the web docs option list, trying each host in order until one
/// returns success.
pub fn get_web_docs_options(api_key: &str, opts: &WebDocsOptions) -> Result<Vec<WebDocsOption>, WebDocsError> {
    let body = build_request_body(api_key);
    let timeout = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let hosts = api::resolve_hosts(opts.hosts.as_deref());
    let payload = api::post_json_failover(WEB_DOCS_PATH, &body, &hosts, timeout, RPC)
        .map_err(WebDocsError::from)?;
    Ok(normalize_options(&payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_body_contains_metadata_with_api_key() {
        let body = build_request_body("tok");
        assert_eq!(body["metadata"]["apiKey"], "tok");
        assert_eq!(body["metadata"]["ideName"], "windsurf");
        assert!(body.get("options").is_none());
    }

    #[test]
    fn normalize_option_parses_docs_url_variant() {
        let raw = json!({ "label": "cloudflare", "docsUrl": "https://developers.cloudflare.com/llms-full.txt" });
        let opt = normalize_option(&raw).unwrap();
        assert_eq!(opt.label, "cloudflare");
        assert_eq!(opt.docs_url.as_deref(), Some("https://developers.cloudflare.com/llms-full.txt"));
        assert!(opt.docs_search_domain.is_none());
        assert!(opt.synonyms.is_empty());
        assert!(!opt.is_featured);
    }

    #[test]
    fn normalize_option_parses_docs_search_domain_variant() {
        let raw = json!({
            "label": "react",
            "docsSearchDomain": "react.dev",
            "synonyms": ["reactjs"],
            "isFeatured": true
        });
        let opt = normalize_option(&raw).unwrap();
        assert_eq!(opt.label, "react");
        assert_eq!(opt.docs_search_domain.as_deref(), Some("react.dev"));
        assert!(opt.docs_url.is_none());
        assert_eq!(opt.synonyms, vec!["reactjs"]);
        assert!(opt.is_featured);
    }

    #[test]
    fn normalize_option_skips_incomplete_records() {
        assert!(normalize_option(&json!({ "label": "no value" })).is_none());
        assert!(normalize_option(&json!({ "docsUrl": "https://x" })).is_none());
        assert!(normalize_option(&json!("not-an-object")).is_none());
        assert!(normalize_option(&json!({ "label": "", "docsUrl": "https://x" })).is_none());
    }

    #[test]
    fn normalize_options_skips_invalid_entries() {
        let payload = json!({
            "options": [
                { "label": "bun", "docsUrl": "https://bun.sh/llms.txt" },
                { "label": "no value" },
                { "label": "duckdb", "docsUrl": "https://duckdb.org/duckdb-docs.md" },
                "not-an-object"
            ]
        });
        let opts = normalize_options(&payload);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].label, "bun");
        assert_eq!(opts[1].label, "duckdb");
    }

    #[test]
    fn normalize_options_empty_payload() {
        assert!(normalize_options(&json!({})).is_empty());
        assert!(normalize_options(&json!({ "options": [] })).is_empty());
    }

    #[test]
    fn serialized_json_uses_camel_case_upstream_shape() {
        let opt = WebDocsOption {
            label: "react".into(),
            docs_url: None,
            docs_search_domain: Some("react.dev".into()),
            synonyms: vec!["reactjs".into()],
            is_featured: true,
        };
        let v = serde_json::to_value(&opt).unwrap();
        assert_eq!(v["label"], "react");
        assert_eq!(v["docsSearchDomain"], "react.dev");
        assert_eq!(v["synonyms"][0], "reactjs");
        assert_eq!(v["isFeatured"], true);
        // absent fields are skipped, not emitted as null
        assert!(v.get("docsUrl").is_none());
    }
}
