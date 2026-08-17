//! CLI 与 MCP 共用的业务入口。
//!
//! 两边输入格式不同（flag vs JSON-RPC），解析仍留在各自模块；
//! 鉴权、读文件、调 RPC、JSON 序列化收拢到这里，避免两套调用链漂移。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::auth;
use crate::caption::{caption_image, guess_mime_from_path, CaptionOptions};
use crate::search::{search, Hit, SearchOptions};
use crate::transcribe::{transcribe_audio, TranscribeOptions};
use crate::webdocs::{get_web_docs_options, WebDocsOption, WebDocsOptions};

pub fn home_dir() -> PathBuf {
    home::home_dir().unwrap_or_else(PathBuf::default)
}

fn env_vars() -> Vec<(String, String)> {
    std::env::vars().collect()
}

pub fn resolve_key(home: &Path, override_key: &str) -> Result<String, String> {
    auth::resolve_api_key(home, override_key, &env_vars(), None)
}

pub fn render_json(payload: &Value, pretty: bool) -> String {
    if pretty {
        serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string())
    }
}

pub fn web_search(
    home: &Path,
    key_override: &str,
    query: &str,
    opts: &SearchOptions,
) -> Result<Vec<Hit>, String> {
    let api_key = resolve_key(home, key_override)?;
    search(&api_key, query, opts).map_err(|e| e.to_string())
}

/// `image_path` 与 `image_base64` 至少一个非空；CLI 只传 path，MCP 两者都可能有。
pub fn image_caption(
    home: &Path,
    key_override: &str,
    image_path: &str,
    image_base64: &str,
    question: &str,
    mime: &str,
) -> Result<String, String> {
    let path = image_path.trim();
    let raw = image_base64.trim();
    let base64_data = if !path.is_empty() {
        crate::caption::file_to_base64(path)?
    } else if !raw.is_empty() {
        raw.to_string()
    } else {
        return Err("provide image_path or image_base64".to_string());
    };

    let mut opts = CaptionOptions::default();
    if !question.trim().is_empty() {
        opts.message_text = question.to_string();
    }
    let mime = mime.trim();
    if !mime.is_empty() {
        opts.mime_type = mime.to_string();
    } else if !path.is_empty() {
        opts.mime_type = guess_mime_from_path(path);
    }

    let api_key = resolve_key(home, key_override)?;
    caption_image(&api_key, &base64_data, &opts).map_err(|e| e.to_string())
}

pub fn audio_transcribe(
    home: &Path,
    key_override: &str,
    audio_path: &str,
    audio_base64: &str,
    timeout_secs: Option<u64>,
) -> Result<String, String> {
    let path = audio_path.trim();
    let raw = audio_base64.trim();
    let base64_data = if !path.is_empty() {
        crate::transcribe::file_to_base64(path)?
    } else if !raw.is_empty() {
        raw.to_string()
    } else {
        return Err("provide audio_path or audio_base64".to_string());
    };

    let mut opts = TranscribeOptions::default();
    if let Some(t) = timeout_secs {
        if t > 0 {
            opts.timeout_secs = Some(t);
        }
    }

    let api_key = resolve_key(home, key_override)?;
    transcribe_audio(&api_key, &base64_data, &opts).map_err(|e| e.to_string())
}

pub fn web_docs(home: &Path, key_override: &str) -> Result<Vec<WebDocsOption>, String> {
    let api_key = resolve_key(home, key_override)?;
    get_web_docs_options(&api_key, &WebDocsOptions::default()).map_err(|e| e.to_string())
}

/// 解析 key 后在一次性 tokio runtime 里跑 fast-context。
/// CLI 未传 `--api-key` 时也走同一套 resolve_key，避免两套鉴权路径。
pub fn fast_context(
    home: &Path,
    key_override: &str,
    mut opts: crate::fastcontext::SearchOptions,
) -> Result<crate::fastcontext::SearchResult, String> {
    if !opts.project_root.is_dir() {
        return Err(format!(
            "project path does not exist: {}",
            opts.project_root.display()
        ));
    }
    if opts.api_key.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
        opts.api_key = Some(resolve_key(home, key_override)?);
    }
    block_on(async move {
        crate::fastcontext::search::search(opts)
            .await
            .map_err(|e| e.to_string())
    })
}

fn block_on<F, T>(future: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>> + Send + 'static,
    T: Send + 'static,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
            runtime.block_on(future)
        })
        .join()
        .map_err(|_| "fast-context runtime thread panicked".to_string())?;
    }

    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(future)
}

pub fn hits_json(hits: &[Hit], pretty: bool) -> String {
    render_json(&json!({ "hits": hits }), pretty)
}

pub fn caption_json(caption: &str, pretty: bool) -> String {
    render_json(&json!({ "caption": caption }), pretty)
}

pub fn transcript_json(text: &str, pretty: bool) -> String {
    render_json(&json!({ "transcribedText": text }), pretty)
}

pub fn webdocs_json(options: &[WebDocsOption], pretty: bool) -> String {
    render_json(&json!({ "options": options }), pretty)
}

#[cfg(test)]
mod tests {
    use super::block_on;

    #[tokio::test]
    async fn block_on_is_safe_inside_a_tokio_runtime() {
        assert_eq!(block_on(async { Ok(42) }), Ok(42));
    }
}
