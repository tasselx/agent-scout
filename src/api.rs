//! 共享的 Windsurf/Devin JSON-RPC 客户端。
//!
//! search / caption / transcribe / webdocs 原先各自复制一份 metadata、
//! `post_json`、host failover 和 Error enum；抽到这里是为了只维护一套
//! HTTP 行为，避免四份实现慢慢漂移。

use serde_json::{json, Value};

pub const SERVER_HOSTS: [&str; 2] = ["server.codeium.com", "server.self-serve.windsurf.com"];
pub const IDE_NAME: &str = "windsurf";
pub const IDE_VERSION: &str = "1.9600.41";
pub const USER_AGENT: &str = "windsurf/1.9600.41";

/// 构造各 RPC 共用的 `metadata` 对象（字段名/取值必须与线上客户端一致）。
pub fn metadata(api_key: &str) -> Value {
    json!({
        "apiKey": api_key,
        "ideName": IDE_NAME,
        "ideVersion": IDE_VERSION,
        "extensionName": IDE_NAME,
        "extensionVersion": IDE_VERSION,
        "locale": "en",
    })
}

/// 去掉可选的 `data:[...];base64,` 前缀；caption / transcribe 共用。
pub fn strip_data_url_prefix(data: &str) -> &str {
    match data.find(',') {
        Some(idx) if data[..idx].starts_with("data:") => &data[idx + 1..],
        _ => data,
    }
}

/// 读本地文件并返回裸 base64（不含 data: 前缀）。
pub fn file_to_base64(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("failed to read {}: {}", path, e))?;
    use base64::Engine as _;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// 调用方传入的 host 列表优先；空则回落到官方双 host。
pub fn resolve_hosts(override_hosts: Option<&[String]>) -> Vec<String> {
    match override_hosts {
        Some(h) if !h.is_empty() => h.to_vec(),
        _ => SERVER_HOSTS.iter().map(|s| (*s).to_string()).collect(),
    }
}

#[derive(Debug)]
pub enum RpcErrorKind {
    Timeout,
    Http(u16, String),
    Transport(String),
    Json(String),
}

/// 带 RPC 名的统一错误，Display 文案与原先各模块 enum 保持一致。
#[derive(Debug)]
pub struct RpcError {
    pub rpc: &'static str,
    pub kind: RpcErrorKind,
}

impl RpcError {
    pub fn timeout(rpc: &'static str) -> Self {
        Self {
            rpc,
            kind: RpcErrorKind::Timeout,
        }
    }

    pub fn http(rpc: &'static str, status: u16, raw: impl Into<String>) -> Self {
        Self {
            rpc,
            kind: RpcErrorKind::Http(status, raw.into()),
        }
    }

    pub fn transport(rpc: &'static str, msg: impl Into<String>) -> Self {
        Self {
            rpc,
            kind: RpcErrorKind::Transport(msg.into()),
        }
    }

    pub fn json(rpc: &'static str, msg: impl Into<String>) -> Self {
        Self {
            rpc,
            kind: RpcErrorKind::Json(msg.into()),
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            RpcErrorKind::Timeout => write!(f, "{} timed out", self.rpc),
            RpcErrorKind::Http(status, raw) => {
                write!(f, "{} -> HTTP {}: {}", self.rpc, status, raw)
            }
            RpcErrorKind::Transport(msg) => write!(f, "{} transport: {}", self.rpc, msg),
            RpcErrorKind::Json(msg) => write!(f, "{} response parse: {}", self.rpc, msg),
        }
    }
}

impl std::error::Error for RpcError {}

fn limit_display(s: &str) -> String {
    let mut out: String = s.chars().take(200).collect();
    if s.chars().count() > 200 {
        out.push('…');
    }
    out
}

fn endpoint_url(host: &str, path: &str) -> String {
    // 裸 hostname 默认 https；完整 URL 留给本地 mock 测试。
    if host.starts_with("http://") || host.starts_with("https://") {
        format!("{}{}", host, path)
    } else {
        format!("https://{}{}", host, path)
    }
}

/// POST JSON 到单个 host，返回解析后的 payload。
///
/// 公开 API 是同步的，并允许从 Tokio 等异步运行时中调用；使用不创建内部
/// runtime 的 ureq，避免 `reqwest::blocking` 在异步上下文中析构时 panic。
pub fn post_json(
    host: &str,
    path: &str,
    body: &Value,
    timeout_secs: u64,
    rpc: &'static str,
) -> Result<Value, RpcError> {
    let url = endpoint_url(host, path);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(timeout_secs))
        .timeout_read(std::time::Duration::from_secs(timeout_secs))
        .build();
    let response = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Connect-Protocol-Version", "1")
        .set("Accept", "application/json")
        .set("User-Agent", USER_AGENT)
        .send_json(body)
        .map_err(|err| match err {
            ureq::Error::Status(status, response) => {
                let raw = response.into_string().unwrap_or_default();
                RpcError::http(rpc, status, limit_display(&raw))
            }
            ureq::Error::Transport(transport) => {
                let message = transport.to_string();
                if message.to_lowercase().contains("timeout")
                    || message.to_lowercase().contains("timed out")
                {
                    RpcError::timeout(rpc)
                } else {
                    RpcError::transport(rpc, message)
                }
            }
        })?;
    let status = response.status();
    if status >= 400 {
        let raw = response.into_string().unwrap_or_default();
        return Err(RpcError::http(rpc, status, limit_display(&raw)));
    }
    response
        .into_json::<Value>()
        .map_err(|e| RpcError::json(rpc, e.to_string()))
}

/// 按顺序尝试每个 host，直到成功。
pub fn post_json_failover(
    path: &str,
    body: &Value,
    hosts: &[String],
    timeout_secs: u64,
    rpc: &'static str,
) -> Result<Value, RpcError> {
    let mut last_error: Option<RpcError> = None;
    for host in hosts {
        match post_json(host, path, body, timeout_secs, rpc) {
            Ok(payload) => return Ok(payload),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| RpcError::transport(rpc, "all hosts failed")))
}
