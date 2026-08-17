//! Audio transcription against Windsurf/Devin server-side speech-to-text
//! (`GetTranscription`, backed by OpenAI Whisper).
//!
//! Pure helpers are separated so they can be unit-tested without network.

use serde_json::{json, Value};

use crate::api;

pub const TRANSCRIPTION_PATH: &str = "/exa.api_server_pb.ApiServerService/GetTranscription";
pub use api::{file_to_base64, SERVER_HOSTS};
// Audio transcription can take noticeably longer than a search/caption, so
// default to a generous timeout.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;
const RPC: &str = "GetTranscription";

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
    let data = api::strip_data_url_prefix(audio_base64).trim();
    if data.is_empty() {
        return Err("transcribe: empty audio data".to_string());
    }
    Ok(json!({
        "metadata": api::metadata(api_key),
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

impl From<api::RpcError> for TranscribeError {
    fn from(error: api::RpcError) -> Self {
        match error.kind {
            api::RpcErrorKind::Timeout => Self::Timeout,
            api::RpcErrorKind::Http(status, raw) => Self::Http(status, raw),
            api::RpcErrorKind::Transport(message) => Self::Transport(message),
            api::RpcErrorKind::Json(message) => Self::Json(message),
        }
    }
}

impl std::fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "{RPC} timed out"),
            Self::Http(status, raw) => write!(f, "{RPC} -> HTTP {status}: {raw}"),
            Self::Transport(message) => write!(f, "{RPC} transport: {message}"),
            Self::Json(message) => write!(f, "{RPC} response parse: {message}"),
        }
    }
}

impl std::error::Error for TranscribeError {}

/// Transcribe an audio file via Windsurf's GetTranscription endpoint, trying
/// each host until one returns success. Returns the transcribed text.
pub fn transcribe_audio(
    api_key: &str,
    audio_base64: &str,
    opts: &TranscribeOptions,
) -> Result<String, TranscribeError> {
    let body = build_transcription_request_body(api_key, audio_base64)
        .map_err(TranscribeError::Transport)?;
    let timeout = opts.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let hosts = api::resolve_hosts(opts.hosts.as_deref());
    let payload = api::post_json_failover(TRANSCRIPTION_PATH, &body, &hosts, timeout, RPC)
        .map_err(TranscribeError::from)?;
    let text = payload
        .get("transcribedText")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(text)
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
