//! Audio transcription against Windsurf/Devin server-side speech-to-text
//! (`GetTranscription`, backed by OpenAI Whisper).
//!
//! Pure helpers are separated so they can be unit-tested without network.

use serde_json::{json, Value};

pub const TRANSCRIPTION_PATH: &str = "/exa.api_server_pb.ApiServerService/GetTranscription";
pub const SERVER_HOSTS: [&str; 2] = ["server.codeium.com", "server.self-serve.windsurf.com"];
// Audio transcription can take noticeably longer than a search/caption, so
// default to a generous timeout.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Options that shape a single transcription request.
#[derive(Debug, Clone)]
pub struct TranscribeOptions {
    pub hosts: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        Self {
            hosts: None,
            timeout_secs: None,
        }
    }
}

/// Build the JSON request body sent to the upstream endpoint.
/// `audio_base64` must be raw base64; a `data:` URL prefix is stripped if
/// present (audio files rarely carry one). The backend auto-detects the audio
/// format from the file header (wav/mp3/ogg/opus/webm/m4a/flac), so there is
/// no mime field on the request.
pub fn build_transcription_request_body(
    api_key: &str,
    audio_base64: &str,
) -> Result<Value, String> {
    let data = match audio_base64.find(',') {
        Some(idx) if audio_base64[..idx].starts_with("data:") => &audio_base64[idx + 1..],
        _ => audio_base64,
    };
    let data = data.trim();
    if data.is_empty() {
        return Err("transcribe: empty audio data".to_string());
    }
    Ok(json!({
        "metadata": {
            "apiKey": api_key,
            "ideName": "windsurf",
            "ideVersion": "1.9600.41",
            "extensionName": "windsurf",
            "extensionVersion": "1.9600.41",
            "locale": "en",
        },
        "audioData": data,
    }))
}

/// Thin error type so the CLI/MCP layer can render a clean message.
#[derive(Debug)]
pub enum TranscribeError {
    Timeout,
    Http(u16, String),
    Transport(String),
    Json(String),
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TranscribeError::Timeout => write!(f, "GetTranscription timed out"),
            TranscribeError::Http(status, raw) => {
                write!(f, "GetTranscription -> HTTP {}: {}", status, raw)
            }
            TranscribeError::Transport(msg) => write!(f, "GetTranscription transport: {}", msg),
            TranscribeError::Json(msg) => write!(f, "GetTranscription response parse: {}", msg),
        }
    }
}

impl std::error::Error for TranscribeError {}

/// POST a JSON body to one host and return the parsed payload.
fn post_json(host: &str, body: &Value, timeout_secs: u64) -> Result<Value, TranscribeError> {
    let url = if host.starts_with("http://") || host.starts_with("https://") {
        format!("{}{}", host, TRANSCRIPTION_PATH)
    } else {
        format!("https://{}{}", host, TRANSCRIPTION_PATH)
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
                TranscribeError::Http(status, limit_display(&raw))
            }
            ureq::Error::Transport(t) => {
                let msg = t.to_string();
                if msg.to_lowercase().contains("timeout") || msg.to_lowercase().contains("timed out") {
                    TranscribeError::Timeout
                } else {
                    TranscribeError::Transport(msg)
                }
            }
        })?;
    let status = response.status();
    if status >= 400 {
        let raw = response.into_string().unwrap_or_default();
        return Err(TranscribeError::Http(status, limit_display(&raw)));
    }
    response
        .into_json::<Value>()
        .map_err(|e| TranscribeError::Json(e.to_string()))
}

fn limit_display(s: &str) -> String {
    let mut out: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        out.push_str("…");
    }
    out
}

/// Read an audio file from disk and return raw base64 (no data: prefix).
pub fn file_to_base64(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("failed to read {}: {}", path, e))?;
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Transcribe an audio file via Windsurf's GetTranscription endpoint, trying
/// each host in order until one returns success. Returns the transcribed text.
pub fn transcribe_audio(
    api_key: &str,
    audio_base64: &str,
    opts: &TranscribeOptions,
) -> Result<String, TranscribeError> {
    let body = build_transcription_request_body(api_key, audio_base64)
        .map_err(|e| TranscribeError::Transport(e))?;
    let timeout = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let hosts: Vec<String> = match &opts.hosts {
        Some(h) if !h.is_empty() => h.clone(),
        _ => SERVER_HOSTS.iter().map(|s| s.to_string()).collect(),
    };
    let mut last_error: Option<TranscribeError> = None;
    for host in &hosts {
        match post_json(host, &body, timeout) {
            Ok(payload) => {
                let text = payload
                    .get("transcribedText")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                return Ok(text);
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or(TranscribeError::Transport(
        "all hosts failed".to_string(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_metadata_and_audio() {
        let body = build_transcription_request_body("tok", "AAAA").unwrap();
        assert_eq!(body["metadata"]["apiKey"], "tok");
        assert_eq!(body["audioData"], "AAAA");
        assert!(body.get("messageText").is_none());
    }

    #[test]
    fn build_body_strips_data_prefix_and_trims() {
        let body = build_transcription_request_body("tok", "data:audio/wav;base64,  BBBB  ").unwrap();
        assert_eq!(body["audioData"], "BBBB");
    }

    #[test]
    fn build_body_rejects_empty_audio() {
        assert!(build_transcription_request_body("tok", "").is_err());
        assert!(build_transcription_request_body("tok", "data:audio/wav;base64,").is_err());
    }
}
