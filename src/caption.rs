//! Image captioning against Windsurf/Devin server-side vision
//! (`GetImageCaption`).
//!
//! Pure helpers are separated so they can be unit-tested without network.

use serde_json::{json, Value};

use crate::api;

pub const IMAGE_CAPTION_PATH: &str = "/exa.api_server_pb.ApiServerService/GetImageCaption";
pub use api::{file_to_base64, SERVER_HOSTS};
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;
const RPC: &str = "GetImageCaption";

const EXT_TO_MIME: [(&str, &str); 6] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
];

/// Options that shape a single caption request.
#[derive(Debug, Clone)]
pub struct CaptionOptions {
    pub mime_type: String,
    pub message_text: String,
    pub hosts: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}

impl Default for CaptionOptions {
    fn default() -> Self {
        Self {
            mime_type: "image/png".to_string(),
            message_text: String::new(),
            hosts: None,
            timeout_secs: None,
        }
    }
}

/// Guess a mime type from a filename/path extension. Mirrors `mimeFromPath`.
pub fn guess_mime_from_path(file_path: &str) -> String {
    let lower = file_path.to_ascii_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    for (e, mime) in EXT_TO_MIME.iter() {
        if *e == ext {
            return (*mime).to_string();
        }
    }
    "image/png".to_string()
}

/// Build the JSON request body sent to the upstream endpoint.
/// Mirrors `buildCaptionRequestBody` in the reference JS.
pub fn build_caption_request_body(api_key: &str, base64_data: &str, opts: &CaptionOptions) -> Value {
    let data = api::strip_data_url_prefix(base64_data);
    let mt = if opts.mime_type.trim().is_empty() {
        "image/png"
    } else {
        opts.mime_type.trim()
    };
    let mut body = json!({
        "metadata": api::metadata(api_key),
        "image": { "base64Data": data, "mimeType": mt },
    });
    if !opts.message_text.trim().is_empty() {
        body["messageText"] = json!(opts.message_text);
    }
    body
}

/// Thin error type so the CLI/MCP layer can render a clean message.
#[derive(Debug)]
pub enum CaptionError {
    Timeout,
    Http(u16, String),
    Transport(String),
    Json(String),
}

impl From<api::RpcError> for CaptionError {
    fn from(error: api::RpcError) -> Self {
        match error.kind {
            api::RpcErrorKind::Timeout => Self::Timeout,
            api::RpcErrorKind::Http(status, raw) => Self::Http(status, raw),
            api::RpcErrorKind::Transport(message) => Self::Transport(message),
            api::RpcErrorKind::Json(message) => Self::Json(message),
        }
    }
}

impl std::fmt::Display for CaptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "{RPC} timed out"),
            Self::Http(status, raw) => write!(f, "{RPC} -> HTTP {status}: {raw}"),
            Self::Transport(message) => write!(f, "{RPC} transport: {message}"),
            Self::Json(message) => write!(f, "{RPC} response parse: {message}"),
        }
    }
}

impl std::error::Error for CaptionError {}

/// Caption an image via Windsurf's GetImageCaption endpoint, trying each host
/// until one returns success. Mirrors `captionImage`.
pub fn caption_image(api_key: &str, base64_data: &str, opts: &CaptionOptions) -> Result<String, CaptionError> {
    let body = build_caption_request_body(api_key, base64_data, opts);
    let timeout = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let hosts = api::resolve_hosts(opts.hosts.as_deref());
    let payload = api::post_json_failover(IMAGE_CAPTION_PATH, &body, &hosts, timeout, RPC)
        .map_err(CaptionError::from)?;
    let caption = payload
        .get("caption")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(caption)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_metadata_and_image() {
        let opts = CaptionOptions::default();
        let body = build_caption_request_body("tok", "AAAA", &opts);
        assert_eq!(body["metadata"]["apiKey"], "tok");
        assert_eq!(body["image"]["base64Data"], "AAAA");
        assert_eq!(body["image"]["mimeType"], "image/png");
        assert!(body.get("messageText").is_none());
    }

    #[test]
    fn build_body_strips_data_prefix_and_adds_message() {
        let opts = CaptionOptions {
            message_text: "what is this?".into(),
            ..Default::default()
        };
        let body = build_caption_request_body("tok", "data:image/jpeg;base64,BBBB", &opts);
        assert_eq!(body["image"]["base64Data"], "BBBB");
        assert_eq!(body["messageText"], "what is this?");
    }

    #[test]
    fn guess_mime_from_extensions() {
        assert_eq!(guess_mime_from_path("a.PNG"), "image/png");
        assert_eq!(guess_mime_from_path("/x/y/photo.jpg"), "image/jpeg");
        assert_eq!(guess_mime_from_path("noext"), "image/png");
    }
}
