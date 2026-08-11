//! Image captioning against Windsurf/Devin server-side vision
//! (`GetImageCaption`).
//!
//! Pure helpers are separated so they can be unit-tested without network.

use serde_json::{json, Value};

pub const IMAGE_CAPTION_PATH: &str = "/exa.api_server_pb.ApiServerService/GetImageCaption";
pub const SERVER_HOSTS: [&str; 2] = ["server.codeium.com", "server.self-serve.windsurf.com"];
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

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
    // Strip a possible `data:[...];base64,` prefix, like the JS impl.
    let data = match base64_data.find(',') {
        Some(idx) if base64_data[..idx].starts_with("data:") => &base64_data[idx + 1..],
        _ => base64_data,
    };
    let mt = if opts.mime_type.trim().is_empty() {
        "image/png"
    } else {
        opts.mime_type.trim()
    };
    let mut body = json!({
        "metadata": {
            "apiKey": api_key,
            "ideName": "windsurf",
            "ideVersion": "1.9600.41",
            "extensionName": "windsurf",
            "extensionVersion": "1.9600.41",
            "locale": "en",
        },
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

impl std::fmt::Display for CaptionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptionError::Timeout => write!(f, "GetImageCaption timed out"),
            CaptionError::Http(status, raw) => {
                write!(f, "GetImageCaption -> HTTP {}: {}", status, raw)
            }
            CaptionError::Transport(msg) => write!(f, "GetImageCaption transport: {}", msg),
            CaptionError::Json(msg) => write!(f, "GetImageCaption response parse: {}", msg),
        }
    }
}

impl std::error::Error for CaptionError {}

/// POST a JSON body to one host and return the parsed payload.
fn post_json(host: &str, body: &Value, timeout_secs: u64) -> Result<Value, CaptionError> {
    let url = if host.starts_with("http://") || host.starts_with("https://") {
        format!("{}{}", host, IMAGE_CAPTION_PATH)
    } else {
        format!("https://{}{}", host, IMAGE_CAPTION_PATH)
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
                CaptionError::Http(status, limit_display(&raw))
            }
            ureq::Error::Transport(t) => {
                let msg = t.to_string();
                if msg.to_lowercase().contains("timeout") || msg.to_lowercase().contains("timed out") {
                    CaptionError::Timeout
                } else {
                    CaptionError::Transport(msg)
                }
            }
        })?;
    let status = response.status();
    if status >= 400 {
        let raw = response.into_string().unwrap_or_default();
        return Err(CaptionError::Http(status, limit_display(&raw)));
    }
    response
        .into_json::<Value>()
        .map_err(|e| CaptionError::Json(e.to_string()))
}

fn limit_display(s: &str) -> String {
    let mut out: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        out.push_str("…");
    }
    out
}

/// Read an image file from disk and return raw base64 (no data: prefix).
pub fn file_to_base64(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("failed to read {}: {}", path, e))?;
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Caption an image via Windsurf's GetImageCaption endpoint, trying each host
/// in order until one returns success. Mirrors `captionImage`.
pub fn caption_image(api_key: &str, base64_data: &str, opts: &CaptionOptions) -> Result<String, CaptionError> {
    let body = build_caption_request_body(api_key, base64_data, opts);
    let timeout = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let hosts: Vec<String> = match &opts.hosts {
        Some(h) if !h.is_empty() => h.clone(),
        _ => SERVER_HOSTS.iter().map(|s| s.to_string()).collect(),
    };
    let mut last_error: Option<CaptionError> = None;
    for host in &hosts {
        match post_json(host, &body, timeout) {
            Ok(payload) => {
                let caption = payload
                    .get("caption")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                return Ok(caption);
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or(CaptionError::Transport(
        "all hosts failed".to_string(),
    )))
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