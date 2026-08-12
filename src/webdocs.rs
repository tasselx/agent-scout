//! Web docs options against Windsurf/Devin server-side API
//! (`GetWebDocsOptions`). Reverse-engineered from Devin.app's bundled
//! language-server protocol (`exa.api_server_pb.ApiServerService/GetWebDocsOptions`).
//!
//! The RPC takes only a `metadata` object and returns a list of `WebDocsOption`
//! records — documentation sources (llms.txt style) the IDE offers to attach to
//! a session's context. Pure helpers are separated so they can be unit-tested
//! without network.

use serde_json::{json, Value};

pub const WEB_DOCS_PATH: &str = "/exa.api_server_pb.ApiServerService/GetWebDocsOptions";
pub const SERVER_HOSTS: [&str; 2] = ["server.codeium.com", "server.self-serve.windsurf.com"];
pub const DEFAULT_TIMEOUT_SECS: u64 = 20;

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
        "metadata": {
            "apiKey": api_key,
            "ideName": "windsurf",
            "ideVersion": "1.9600.41",
            "extensionName": "windsurf",
            "extensionVersion": "1.9600.41",
            "locale": "en",
        }
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

impl std::fmt::Display for WebDocsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebDocsError::Timeout => write!(f, "GetWebDocsOptions timed out"),
            WebDocsError::Http(status, raw) => {
                write!(f, "GetWebDocsOptions -> HTTP {}: {}", status, raw)
            }
            WebDocsError::Transport(msg) => write!(f, "GetWebDocsOptions transport: {}", msg),
            WebDocsError::Json(msg) => write!(f, "GetWebDocsOptions response parse: {}", msg),
        }
    }
}

impl std::error::Error for WebDocsError {}

/// POST a JSON body to one host and return the parsed payload.
/// `host` may be a bare hostname (https is assumed) or a full http(s) URL
/// (used for local mock testing).
fn post_json(host: &str, body: &Value, timeout_secs: u64) -> Result<Value, WebDocsError> {
    let url = if host.starts_with("http://") || host.starts_with("https://") {
        format!("{}{}", host, WEB_DOCS_PATH)
    } else {
        format!("https://{}{}", host, WEB_DOCS_PATH)
    };
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(timeout_secs))
        .timeout_read(std::time::Duration::from_secs(timeout_secs))
        .build();
    let response = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .set("Accept", "application/json")
        .set("User-Agent", "windsurf/1.9600.41")
        .send_json(body)
        .map_err(|err| match err {
            ureq::Error::Status(status, resp) => {
                let raw = resp.into_string().unwrap_or_default();
                WebDocsError::Http(status, limit_display(&raw))
            }
            ureq::Error::Transport(t) => {
                let msg = t.to_string();
                if msg.to_lowercase().contains("timeout") || msg.to_lowercase().contains("timed out") {
                    WebDocsError::Timeout
                } else {
                    WebDocsError::Transport(msg)
                }
            }
        })?;
    let status = response.status();
    if status >= 400 {
        let raw = response.into_string().unwrap_or_default();
        return Err(WebDocsError::Http(status, limit_display(&raw)));
    }
    response
        .into_json::<Value>()
        .map_err(|e| WebDocsError::Json(e.to_string()))
}

fn limit_display(s: &str) -> String {
    let mut out: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        out.push_str("…");
    }
    out
}

/// Fetch the web docs option list, trying each host in order until one
/// returns success.
pub fn get_web_docs_options(api_key: &str, opts: &WebDocsOptions) -> Result<Vec<WebDocsOption>, WebDocsError> {
    let body = build_request_body(api_key);
    let timeout = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let hosts: Vec<String> = match &opts.hosts {
        Some(h) if !h.is_empty() => h.clone(),
        _ => SERVER_HOSTS.iter().map(|s| s.to_string()).collect(),
    };
    let mut last_error: Option<WebDocsError> = None;
    for host in &hosts {
        match post_json(host, &body, timeout) {
            Ok(payload) => return Ok(normalize_options(&payload)),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or(WebDocsError::Transport(
        "all hosts failed".to_string(),
    )))
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
