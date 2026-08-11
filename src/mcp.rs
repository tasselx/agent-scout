//! MCP stdio server exposing `web_search` as a tool.
//!
//! Speaks both JSON-RPC framings used by MCP clients:
//!   - `Content-Length:` framing (official @modelcontextprotocol/sdk)
//!   - newline-delimited JSON (handcrafted NDJSON clients)
//!
//! Reads requests from stdin, writes responses to stdout.

use serde_json::{json, Value};
use std::io::{Read, Write};

use crate::caption::{caption_image, guess_mime_from_path, CaptionOptions};
use crate::search::{search, SearchOptions};
use crate::transcribe::{transcribe_audio, TranscribeOptions};

pub const SERVER_NAME: &str = "agent-scout";
pub const PROTOCOL_VERSION: &str = "2024-11-05";

const TOOL_DESCRIPTION: &str = concat!(
    "Search the web via Devin/Windsurf server-side search. ",
    "Returns JSON hits [{title,url,snippet,source}]. ",
    "Use for current web facts, docs lookups, and general web research."
);

const CAPTION_TOOL_DESCRIPTION: &str = concat!(
    "Caption / analyze an image via Windsurf/Devin server-side vision (GetImageCaption). ",
    "Pass either image_path (a local file) or image_base64 (raw base64, data: prefix optional). ",
    "Returns the model's text analysis of the image."
);

const TRANSCRIBE_TOOL_DESCRIPTION: &str = concat!(
    "Transcribe an audio file via Windsurf/Devin server-side speech-to-text (GetTranscription, ",
    "backed by OpenAI Whisper). Pass either audio_path (a local file) or audio_base64 (raw base64, ",
    "data: prefix optional). Audio format is auto-detected (wav/mp3/ogg/opus/webm/m4a/flac). ",
    "Returns the transcribed text."
);

const FAST_CONTEXT_TOOL_DESCRIPTION: &str = concat!(
    "AI-driven semantic code search via Windsurf's Devstral model (reverse-engineered SWE-grep ",
    "protocol, same auth as the other tools). Searches a local codebase with a natural-language ",
    "query and returns relevant file paths with line ranges, plus suggested grep keywords.\n",
    "Parameter tuning:\n",
    "- tree_depth (1-6, default 3): how much directory structure the remote AI sees. REDUCE on ",
    "payload/size errors; INCREASE for small projects.\n",
    "- max_turns (1-5, default 3): search rounds. INCREASE for deep tracing; 1 for quick lookups.\n",
    "- max_results (1-30, default 10): max files to return.\n",
    "- exclude_paths: dirs to skip in the repo map, e.g. [\"node_modules\",\"dist\",\".git\"].",
);

fn web_search_schema() -> Value {
    json!({
        "name": "web_search",
        "description": TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query (required)" },
                "limit": { "type": "number", "minimum": 1, "maximum": 10, "default": 5, "description": "Max results (1-10)" },
                "domain": { "type": "string", "description": "Optional domain restriction, e.g. github.com" },
                "mode": { "type": "number", "description": "Optional search mode" },
                "pretty": { "type": "boolean", "default": false, "description": "Pretty-print the JSON output" }
            },
            "required": ["query"],
            "additionalProperties": false
        }
    })
}

fn fast_context_schema() -> Value {
    json!({
        "name": "fast_context_search",
        "description": FAST_CONTEXT_TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Natural-language search query (required)" },
                "project_path": { "type": "string", "description": "Absolute path to project root. Empty = current working directory." },
                "tree_depth": { "type": "number", "minimum": 1, "maximum": 6, "default": 3, "description": "Directory tree depth for the repo map" },
                "max_turns": { "type": "number", "minimum": 1, "maximum": 5, "default": 3, "description": "Search rounds" },
                "max_results": { "type": "number", "minimum": 1, "maximum": 30, "default": 10, "description": "Max files to return" },
                "exclude_paths": { "type": "array", "items": { "type": "string" }, "description": "Dirs/patterns to exclude from the repo map" }
            },
            "required": ["query"],
            "additionalProperties": false
        }
    })
}

fn tool_list() -> Value {
    json!({ "tools": [
        web_search_schema(),
        image_caption_schema(),
        audio_transcribe_schema(),
        fast_context_schema()
    ] })
}

fn image_caption_schema() -> Value {
    json!({
        "name": "image_caption",
        "description": CAPTION_TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "image_path": { "type": "string", "description": "Path to a local image file (PNG/JPG/WebP/GIF)" },
                "image_base64": { "type": "string", "description": "Raw base64 image data (data: prefix optional)" },
                "mime": { "type": "string", "description": "Mime type, e.g. image/png (default guessed from path)" },
                "question": { "type": "string", "description": "Optional question / instruction about the image" },
                "pretty": { "type": "boolean", "default": false, "description": "Pretty-print the JSON output" }
            },
            "additionalProperties": false
        }
    })
}

fn audio_transcribe_schema() -> Value {
    json!({
        "name": "audio_transcribe",
        "description": TRANSCRIBE_TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "audio_path": { "type": "string", "description": "Path to a local audio file (wav/mp3/ogg/opus/webm/m4a/flac)" },
                "audio_base64": { "type": "string", "description": "Raw base64 audio data (data: prefix optional)" },
                "timeout": { "type": "number", "minimum": 1, "default": 60, "description": "Timeout in seconds (default 60)" },
                "pretty": { "type": "boolean", "default": false, "description": "Pretty-print the JSON output" }
            },
            "additionalProperties": false
        }
    })
}

/// Handle a single JSON-RPC message, returning optional output lines to write.
fn handle_message(message: &Value) -> Option<String> {
    if !message.is_object() {
        return None;
    }
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("");
    let id = message.get("id").cloned();

    // Notifications have no id; never respond to them.
    if id.is_none() {
        return None;
    }
    let id = id.unwrap();

    let result: Value = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": crate::VERSION }
        }),
        "tools/list" => json!({ "tools": tool_list()["tools"].clone() }),
        "tools/call" => {
            let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            match call_tool(name, &args) {
                Ok(content) => json!({ "content": [ { "type": "text", "text": content } ] }),
                Err(err_text) => json!({
                    "isError": true,
                    "content": [ { "type": "text", "text": err_text } ]
                }),
            }
        }
        _ if method.starts_with("notifications/") => return None,
        _ => return Some(render_response(id, json!({}))),
    };
    Some(render_response(id, result))
}

fn call_tool(name: &str, args: &Value) -> Result<String, String> {
    match name {
        "web_search" => call_web_search(args),
        "image_caption" => call_image_caption(args),
        "audio_transcribe" => call_audio_transcribe(args),
        "fast_context_search" => call_fast_context_search(args),
        _ => Err(format!("unknown tool: {}", name)),
    }
}

fn call_web_search(args: &Value) -> Result<String, String> {
    let pretty = args.get("pretty").and_then(Value::as_bool).unwrap_or(false);
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("web_search: query is required".to_string());
    }
    let mut opts = SearchOptions::default();
    if let Some(limit) = args.get("limit").and_then(Value::as_u64) {
        opts.limit = limit as usize;
    }
    if let Some(domain) = args.get("domain").and_then(Value::as_str) {
        if !domain.is_empty() {
            opts.domain = Some(domain.to_string());
        }
    }
    if let Some(mode) = args.get("mode").cloned() {
        opts.mode = Some(mode);
    }

    let home = home::home_dir().unwrap_or_else(std::path::PathBuf::default);
    let api_key =
        crate::auth::resolve_api_key(&home, "", &env_vars(), None).map_err(|e| e.to_string())?;
    let hits = search(&api_key, &query, &opts).map_err(|e| e.to_string())?;
    let payload = json!({ "hits": hits });
    Ok(render_json(&payload, pretty))
}

fn call_image_caption(args: &Value) -> Result<String, String> {
    let pretty = args.get("pretty").and_then(Value::as_bool).unwrap_or(false);
    let image_path = args.get("image_path").and_then(Value::as_str).unwrap_or("").trim();
    let image_base64 = args.get("image_base64").and_then(Value::as_str).unwrap_or("").trim();

    // Resolve image bytes as base64 (data: prefix allowed).
    let base64_data = if !image_path.is_empty() {
        crate::caption::file_to_base64(image_path)?
    } else if !image_base64.is_empty() {
        image_base64.to_string()
    } else {
        return Err("image_caption: provide image_path or image_base64".to_string());
    };

    let mut opts = CaptionOptions::default();
    if let Some(q) = args.get("question").and_then(Value::as_str) {
        if !q.is_empty() {
            opts.message_text = q.to_string();
        }
    }
    if let Some(m) = args.get("mime").and_then(Value::as_str) {
        if !m.trim().is_empty() {
            opts.mime_type = m.trim().to_string();
        } else if !image_path.is_empty() {
            opts.mime_type = guess_mime_from_path(image_path);
        }
    } else if !image_path.is_empty() {
        opts.mime_type = guess_mime_from_path(image_path);
    }

    let home = home::home_dir().unwrap_or_else(std::path::PathBuf::default);
    let api_key =
        crate::auth::resolve_api_key(&home, "", &env_vars(), None).map_err(|e| e.to_string())?;
    let caption = caption_image(&api_key, &base64_data, &opts).map_err(|e| e.to_string())?;
    let payload = json!({ "caption": caption });
    Ok(render_json(&payload, pretty))
}

fn call_audio_transcribe(args: &Value) -> Result<String, String> {
    let pretty = args.get("pretty").and_then(Value::as_bool).unwrap_or(false);
    let audio_path = args.get("audio_path").and_then(Value::as_str).unwrap_or("").trim();
    let audio_base64 = args.get("audio_base64").and_then(Value::as_str).unwrap_or("").trim();

    // Resolve audio bytes as base64 (data: prefix allowed).
    let base64_data = if !audio_path.is_empty() {
        crate::transcribe::file_to_base64(audio_path)?
    } else if !audio_base64.is_empty() {
        audio_base64.to_string()
    } else {
        return Err("audio_transcribe: provide audio_path or audio_base64".to_string());
    };

    let mut opts = TranscribeOptions::default();
    if let Some(t) = args.get("timeout").and_then(Value::as_u64) {
        if t > 0 {
            opts.timeout_secs = Some(t);
        }
    }

    let home = home::home_dir().unwrap_or_else(std::path::PathBuf::default);
    let api_key =
        crate::auth::resolve_api_key(&home, "", &env_vars(), None).map_err(|e| e.to_string())?;
    let text = transcribe_audio(&api_key, &base64_data, &opts).map_err(|e| e.to_string())?;
    let payload = json!({ "transcribedText": text });
    Ok(render_json(&payload, pretty))
}

fn call_fast_context_search(args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        return Err("fast_context_search: query is required".to_string());
    }

    let mut opts = crate::fastcontext::SearchOptions::default();
    opts.query = query;
    if let Some(p) = args.get("project_path").and_then(Value::as_str) {
        if !p.trim().is_empty() {
            opts.project_root = std::path::PathBuf::from(p.trim());
        }
    }
    if let Some(d) = args.get("tree_depth").and_then(Value::as_u64) {
        if (1..=6).contains(&d) {
            opts.tree_depth = d as u8;
        }
    }
    if let Some(t) = args.get("max_turns").and_then(Value::as_u64) {
        if (1..=5).contains(&t) {
            opts.max_turns = t as u8;
        }
    }
    if let Some(r) = args.get("max_results").and_then(Value::as_u64) {
        if (1..=30).contains(&r) {
            opts.max_results = r as u8;
        }
    }
    if let Some(excludes) = args.get("exclude_paths").and_then(Value::as_array) {
        opts.exclude_paths = excludes
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
    }

    if !opts.project_root.is_dir() {
        return Err(format!(
            "fast_context_search: project path does not exist: {}",
            opts.project_root.display()
        ));
    }

    // Reuse the same key resolution as the other tools (env → config → local install).
    let home = home::home_dir().unwrap_or_else(std::path::PathBuf::default);
    let cli_key = args.get("api_key").and_then(Value::as_str).unwrap_or("").trim();
    let api_key =
        crate::auth::resolve_api_key(&home, cli_key, &env_vars(), None).map_err(|e| e.to_string())?;
    opts.api_key = Some(api_key);

    // fast-context 内部为 async（tokio + reqwest），在此创建一个一次性运行时执行。
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let result = runtime
        .block_on(crate::fastcontext::search::search(opts.clone()))
        .map_err(|e| format!("fast_context_search: {}", e))?;
    Ok(crate::fastcontext::search::format_result(&result, &opts))
}

fn render_response(id: Value, result: Value) -> String {
    serde_json::to_string(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        .unwrap_or_default()
}

/// Serialize a payload as compact or pretty JSON, depending on the `pretty` flag.
fn render_json(payload: &Value, pretty: bool) -> String {
    if pretty {
        serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".to_string())
    } else {
        serde_json::to_string(payload).unwrap_or_else(|_| "{}".to_string())
    }
}

fn env_vars() -> Vec<(String, String)> {
    std::env::vars().collect()
}

/// Run the MCP stdio server until EOF on stdin.
pub fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut buffer: Vec<u8> = Vec::new();
    let mut framing: Option<Framing> = None;

    let mut chunk = [0u8; 4096];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break; // EOF
        }
        buffer.extend_from_slice(&chunk[..n]);

        if framing.is_none() {
            framing = detect_framing(&buffer);
        }
        let Some(current) = framing else { continue };

        loop {
            match current {
                Framing::ContentLength => {
                    if !consume_content_length(&mut buffer, &mut out)? {
                        break;
                    }
                }
                Framing::Ndjson => {
                    if !consume_ndjson(&mut buffer, &mut out)? {
                        break;
                    }
                }
            }
        }
    }
    let _ = out.flush();
    Ok(())
}

#[derive(Clone, Copy, PartialEq)]
enum Framing {
    ContentLength,
    Ndjson,
}

fn detect_framing(buffer: &[u8]) -> Option<Framing> {
    if buffer.windows(2).any(|w| w == b"\r\n") {
        Some(Framing::ContentLength)
    } else if buffer.contains(&b'\n') {
        Some(Framing::Ndjson)
    } else {
        None
    }
}

/// Consume one Content-Length framed message. Returns false if more data is
/// needed (incomplete frame) and the caller should wait for more input.
fn consume_content_length(
    buffer: &mut Vec<u8>,
    out: &mut impl Write,
) -> Result<bool, String> {
    // Find the header terminator.
    static HEADER_END: &[u8] = b"\r\n\r\n";
    let header_end = find_subslice(buffer, HEADER_END);
    let Some(header_end) = header_end else {
        return Ok(false);
    };
    let header = &buffer[..header_end];
    let header_text = String::from_utf8_lossy(header);
    let body_length = header_text
        .split("\r\n")
        .find_map(|line| {
            let line = line.trim();
            let lower = line.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("content-length:") {
                rest.trim().parse::<usize>().ok()
            } else {
                None
            }
        });
    let Some(body_length) = body_length else {
        // Malformed header; drop the claimed header and try again.
        buffer.drain(..header_end + HEADER_END.len());
        return Ok(true);
    };
    let body_start = header_end + HEADER_END.len();
    if buffer.len() < body_start + body_length {
        return Ok(false); // wait for more
    }
    let body = &buffer[body_start..body_start + body_length];
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        emit_response(value, out)?;
    }
    buffer.drain(..body_start + body_length);
    Ok(true)
}

/// Consume one NDJSON message. Returns false if no complete line is available.
fn consume_ndjson(buffer: &mut Vec<u8>, out: &mut impl Write) -> Result<bool, String> {
    let newline = buffer.iter().position(|&b| b == b'\n');
    let Some(newline) = newline else {
        return Ok(false);
    };
    let line = String::from_utf8_lossy(&buffer[..newline]).trim().to_string();
    buffer.drain(..newline + 1);
    if line.is_empty() {
        return Ok(true);
    }
    if let Ok(value) = serde_json::from_str::<Value>(&line) {
        emit_response(value, out)?;
    }
    Ok(true)
}

fn emit_response(message: Value, out: &mut impl Write) -> Result<(), String> {
    if let Some(line) = handle_message(&message) {
        out.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        out.write_all(b"\n").map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_handshake() {
        let msg = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {} } });
        let out = handle_message(&msg).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(v["result"]["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(v["result"]["capabilities"], json!({ "tools": {} }));
    }

    #[test]
    fn tools_list_exposes_web_search() {
        let msg = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let out = handle_message(&msg).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let names: Vec<&str> = v["result"]["tools"].as_array().unwrap().iter()
            .filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"image_caption"));
        assert!(names.contains(&"audio_transcribe"));
        let ws = &v["result"]["tools"][0];
        assert_eq!(ws["name"], "web_search");
        assert_eq!(ws["inputSchema"]["required"][0], "query");
        assert_eq!(ws["inputSchema"]["properties"]["limit"]["maximum"], 10);
    }

    #[test]
    fn empty_query_is_error() {
        let msg = json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "web_search", "arguments": {} } });
        let out = handle_message(&msg).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("query is required"));
    }

    #[test]
    fn image_caption_requires_image() {
        let msg = json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call",
            "params": { "name": "image_caption", "arguments": {} } });
        let out = handle_message(&msg).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("image_path or image_base64"));
    }

    #[test]
    fn audio_transcribe_requires_audio() {
        let msg = json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": { "name": "audio_transcribe", "arguments": {} } });
        let out = handle_message(&msg).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"].as_str().unwrap().contains("audio_path or audio_base64"));
    }

    #[test]
    fn unknown_tool_is_error() {
        let msg = json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": { "name": "nope", "arguments": {} } });
        let out = handle_message(&msg).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["result"]["isError"], true);
    }

    #[test]
    fn notifications_are_ignored() {
        let msg = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle_message(&msg).is_none());
    }

    #[test]
    fn content_length_framing_roundtrip() {
        let mut buffer: Vec<u8> = Vec::new();
        let body = serde_json::to_vec(&json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list" })).unwrap();
        buffer.extend_from_slice(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes());
        buffer.extend_from_slice(&body);
        let mut out: Vec<u8> = Vec::new();
        let done = consume_content_length(&mut buffer, &mut out).unwrap();
        assert!(done);
        assert!(buffer.is_empty());
        assert!(String::from_utf8_lossy(&out).contains("web_search"));
    }

    #[test]
    fn ndjson_framing_roundtrip() {
        let mut buffer: Vec<u8> = format!("{}\n", json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/list" })).into_bytes();
        let mut out: Vec<u8> = Vec::new();
        let done = consume_ndjson(&mut buffer, &mut out).unwrap();
        assert!(done);
        assert!(String::from_utf8_lossy(&out).contains("web_search"));
    }
}