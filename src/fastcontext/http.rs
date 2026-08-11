//! Connect-RPC unary/streaming HTTP 客户端（对齐 sanshu 的 fast_context 实现）。
//!
//! 包括：JWT 获取（内存缓存）、限流检查、unary/streaming 请求、
//! 4xx 不重试 + 指数退避 jitter、metadata/请求构建。

use std::time::{Duration, Instant, SystemTime};

use reqwest::Client;

use crate::fastcontext::proto::{
    connect_frame_decode, connect_frame_encode, extract_strings, gzip_bytes, gunzip_bytes,
    ProtobufEncoder,
};
use crate::fastcontext::{ChatMessage, FastContextError, FcResult};

const API_BASE: &str = "https://server.self-serve.windsurf.com/exa.api_server_pb.ApiServerService";
const AUTH_BASE: &str = "https://server.self-serve.windsurf.com/exa.auth_pb.AuthService";
const WS_APP: &str = "windsurf";
const DEFAULT_WS_APP_VER: &str = "1.48.2";
const DEFAULT_WS_LS_VER: &str = "1.9544.35";
const DEFAULT_WS_MODEL: &str = "MODEL_SWE_1_6_FAST";

/// JWT 内存缓存：避免每次查询都重新走 GetUserJwt RTT（约 100-300ms）。
/// 保守地用 10 分钟窗口，远小于 JWT 真实过期时间。
const JWT_CACHE_TTL_SECS: u64 = 600;

pub fn ws_app_ver() -> String {
    std::env::var("WS_APP_VER").unwrap_or_else(|_| DEFAULT_WS_APP_VER.to_string())
}

pub fn ws_ls_ver() -> String {
    std::env::var("WS_LS_VER").unwrap_or_else(|_| DEFAULT_WS_LS_VER.to_string())
}

pub fn ws_model() -> String {
    std::env::var("WS_MODEL").unwrap_or_else(|_| DEFAULT_WS_MODEL.to_string())
}

fn system_info() -> serde_json::Value {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    };
    serde_json::json!({
        "Os": os,
        "Arch": std::env::consts::ARCH,
        "Release": "",
        "Version": "",
        "Machine": std::env::consts::ARCH,
        "Nodename": hostname(),
        "Sysname": if cfg!(target_os = "macos") { "Darwin" } else if cfg!(target_os = "windows") { "Windows_NT" } else { "Linux" },
        "ProductVersion": ""
    })
}

fn cpu_info() -> serde_json::Value {
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    serde_json::json!({
        "NumSockets": 1,
        "NumCores": threads,
        "NumThreads": threads,
        "VendorID": "",
        "Family": "0",
        "Model": "0",
        "ModelName": "Unknown",
        "Memory": 0
    })
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "localhost".to_string())
}

/// 构建 per-request 的 metadata 消息（字段 1..=30）。
pub fn build_metadata(api_key: &str, jwt: &str) -> anyhow::Result<ProtobufEncoder> {
    let mut meta = ProtobufEncoder::default();
    meta.write_string(1, WS_APP);
    meta.write_string(2, &ws_app_ver());
    meta.write_string(3, api_key);
    meta.write_string(4, "zh-cn");
    meta.write_string(5, &serde_json::to_string(&system_info())?);
    meta.write_string(7, &ws_ls_ver());
    meta.write_string(8, &serde_json::to_string(&cpu_info())?);
    meta.write_string(12, WS_APP);
    meta.write_string(21, jwt);
    meta.write_bytes(30, &[0x00, 0x01]);
    Ok(meta)
}

pub fn build_request(
    api_key: &str,
    jwt: &str,
    messages: &[ChatMessage],
    tool_defs: &str,
) -> anyhow::Result<Vec<u8>> {
    let mut request = ProtobufEncoder::default();
    request.write_message(1, &build_metadata(api_key, jwt)?);
    for message in messages {
        request.write_message(2, &build_chat_message(message));
    }
    request.write_string(3, tool_defs);
    Ok(request.into_bytes())
}

pub fn build_chat_message(message: &ChatMessage) -> ProtobufEncoder {
    let mut msg = ProtobufEncoder::default();
    msg.write_varint(2, message.role);
    msg.write_string(3, &message.content);

    if let (Some(call_id), Some(tool_name), Some(args_json)) = (
        message.tool_call_id.as_deref(),
        message.tool_name.as_deref(),
        message.tool_args_json.as_deref(),
    ) {
        let mut tool_call = ProtobufEncoder::default();
        tool_call.write_string(1, call_id);
        tool_call.write_string(2, tool_name);
        tool_call.write_string(3, args_json);
        msg.write_message(6, &tool_call);
    }

    if let Some(ref_call_id) = message.ref_call_id.as_deref() {
        msg.write_string(7, ref_call_id);
    }

    msg
}

// ─── 错误分类 ─────────────────────────────────────────────

fn classify_reqwest_error(err: reqwest::Error) -> FastContextError {
    if err.is_timeout() {
        FastContextError::timeout(err.to_string())
    } else if let Some(status) = err.status() {
        FastContextError::status(status)
    } else {
        FastContextError::network(err.to_string())
    }
}

/// 判断 streaming 请求失败后是否值得重试：4xx 属于请求/鉴权/限流问题，
/// 继续重试只会浪费配额。
fn should_retry_streaming_error(err: &FastContextError) -> bool {
    !matches!(err.status, Some(400..=499))
}

/// 基于纳秒时间戳的轻量 jitter（0~400ms），无需引入 rand 依赖
fn pseudo_jitter_ms(attempt: usize) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    jitter_ms_from_seed(attempt, nanos)
}

/// 从可控 seed 计算 jitter，保留随机扰动逻辑同时让边界可测试。
fn jitter_ms_from_seed(attempt: usize, nanos: u64) -> u64 {
    let seed = nanos.wrapping_add(attempt as u64 * 7919);
    seed % 400
}

/// 计算最终重试等待时间，保证指数退避基础值与 jitter 组合逻辑可测试。
fn retry_delay_ms(attempt: usize, jitter_ms: u64) -> u64 {
    1000u64 * (attempt as u64 + 1) + jitter_ms
}

// ─── unary 请求 ───────────────────────────────────────────

pub async fn unary_request(
    client: &Client,
    url: &str,
    proto_bytes: &[u8],
    compress: bool,
    timeout_ms: u64,
) -> FcResult<Vec<u8>> {
    let started_at = Instant::now();
    let mut body = proto_bytes.to_vec();
    let mut request = client
        .post(url)
        .timeout(Duration::from_millis(timeout_ms))
        .header("Content-Type", "application/proto")
        .header("Connect-Protocol-Version", "1")
        .header("User-Agent", "connect-go/1.18.1 (go1.25.5)")
        .header("Accept-Encoding", "gzip");

    if compress {
        body = gzip_bytes(proto_bytes)?;
        request = request.header("Content-Encoding", "gzip");
    }

    let response = request
        .body(body)
        .send()
        .await
        .map_err(classify_reqwest_error)?;
    let status = response.status();
    if !status.is_success() {
        log::warn!(
            "[fast-context] unary 请求失败: url={}, status={}, elapsed_ms={}",
            url,
            status,
            started_at.elapsed().as_millis()
        );
        return Err(FastContextError::status(status));
    }
    let bytes = response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(classify_reqwest_error)?;
    log::info!(
        "[fast-context] unary 请求成功: url={}, bytes={}, elapsed_ms={}",
        url,
        bytes.len(),
        started_at.elapsed().as_millis()
    );
    Ok(bytes)
}

// ─── streaming 请求 ───────────────────────────────────────

pub async fn streaming_request(
    client: &Client,
    proto_bytes: &[u8],
    timeout_ms: u64,
    max_retries: usize,
) -> FcResult<Vec<u8>> {
    let frame = connect_frame_encode(proto_bytes, true)?;
    let url = format!("{API_BASE}/GetDevstralStream");
    let base_timeout_ms = timeout_ms.max(1000);
    let abort_ms = base_timeout_ms + 5000;
    let mut last_error = None;

    for attempt in 0..=max_retries {
        let started_at = Instant::now();
        let trace_id = uuid::Uuid::new_v4().simple().to_string();
        let span_id = uuid::Uuid::new_v4().simple().to_string()[..16].to_string();
        let response = client
            .post(&url)
            .timeout(Duration::from_millis(abort_ms))
            .header("Content-Type", "application/connect+proto")
            .header("Connect-Protocol-Version", "1")
            .header("Connect-Accept-Encoding", "gzip")
            .header("Connect-Content-Encoding", "gzip")
            .header("Connect-Timeout-Ms", base_timeout_ms.to_string())
            .header("User-Agent", "connect-go/1.18.1 (go1.25.5)")
            .header("Accept-Encoding", "identity")
            .header(
                "Baggage",
                format!(
                    "sentry-release=language-server-windsurf@{},sentry-environment=stable,sentry-sampled=false,sentry-trace_id={},sentry-public_key=b813f73488da69eedec534dba1029111",
                    ws_ls_ver(),
                    trace_id
                ),
            )
            .header("Sentry-Trace", format!("{}-{}-0", trace_id, span_id))
            .body(frame.clone())
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(classify_reqwest_error)?;
                log::info!(
                    "[fast-context] 流式请求成功: attempt={}, bytes={}, elapsed_ms={}",
                    attempt + 1,
                    bytes.len(),
                    started_at.elapsed().as_millis()
                );
                return Ok(bytes);
            }
            Ok(resp) => {
                let err = FastContextError::status(resp.status());
                log::warn!(
                    "[fast-context] 流式请求 HTTP 失败: attempt={}, status={:?}, code={}, elapsed_ms={}",
                    attempt + 1,
                    err.status,
                    err.code,
                    started_at.elapsed().as_millis()
                );
                // 429 / 其他 4xx 不应重试，避免无效请求继续消耗远端配额
                if !should_retry_streaming_error(&err) {
                    return Err(err);
                }
                last_error = Some(err);
            }
            Err(err) => {
                let err = classify_reqwest_error(err);
                log::warn!(
                    "[fast-context] 流式请求网络失败: attempt={}, code={}, elapsed_ms={}, message={}",
                    attempt + 1,
                    err.code,
                    started_at.elapsed().as_millis(),
                    err.message
                );
                last_error = Some(err);
            }
        }

        if attempt < max_retries {
            // 指数退避 + jitter：避免雷霆群与服务器同步震荡
            let jitter_ms = pseudo_jitter_ms(attempt);
            tokio::time::sleep(Duration::from_millis(retry_delay_ms(attempt, jitter_ms))).await;
        }
    }

    Err(last_error.unwrap_or_else(|| FastContextError::timeout("streaming request timed out")))
}

// ─── JWT ──────────────────────────────────────────────────

/// 从 GetUserJwt 响应中提取 JWT（支持 gzip、Connect 帧、多字符串扫描）。
fn extract_jwt_from_response(response: &[u8]) -> Option<String> {
    let mut candidates = vec![response.to_vec()];

    if let Ok(decoded) = gunzip_bytes(response) {
        candidates.push(decoded);
    }
    candidates.extend(connect_frame_decode(response));

    for bytes in candidates {
        for value in extract_strings(&bytes) {
            let trimmed = value.trim();
            if trimmed.starts_with("eyJ") && trimmed.contains('.') {
                return Some(trimmed.to_string());
            }
        }

        let raw_text = String::from_utf8_lossy(&bytes);
        for part in raw_text.split(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | ','))
        {
            let trimmed = part.trim();
            if trimmed.starts_with("eyJ") && trimmed.contains('.') {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

pub async fn fetch_jwt(client: &Client, api_key: &str) -> anyhow::Result<String> {
    // JWT 内存缓存：命中则跳过远端 GetUserJwt（节省 100-300ms / 查询）
    let fp = crate::fastcontext::api_key_fp(api_key);
    {
        let cache = crate::fastcontext::jwt_cache().lock().await;
        if let Some(cached) = cache.as_ref() {
            if cached.api_key_fingerprint == fp {
                if let Ok(age) = SystemTime::now().duration_since(cached.fetched_at) {
                    if age.as_secs() < JWT_CACHE_TTL_SECS {
                        log::info!(
                            "[fast-context] JWT 缓存命中: age_secs={}, ttl_secs={}",
                            age.as_secs(),
                            JWT_CACHE_TTL_SECS
                        );
                        return Ok(cached.jwt.clone());
                    }
                }
            }
        }
    }

    let mut meta = ProtobufEncoder::default();
    meta.write_string(1, WS_APP);
    meta.write_string(2, &ws_app_ver());
    meta.write_string(3, api_key);
    meta.write_string(4, "zh-cn");
    meta.write_string(7, &ws_ls_ver());
    meta.write_string(12, WS_APP);
    meta.write_bytes(30, &[0x00, 0x01]);

    let mut outer = ProtobufEncoder::default();
    outer.write_message(1, &meta);
    let response = unary_request(
        client,
        &format!("{AUTH_BASE}/GetUserJwt"),
        &outer.into_bytes(),
        false,
        30_000,
    )
    .await
    .map_err(|e| anyhow::anyhow!("获取 Devin / Windsurf JWT 失败: {}", e))?;

    let jwt = extract_jwt_from_response(&response)
        .ok_or_else(|| anyhow::anyhow!("无法从 GetUserJwt 响应中提取 JWT"))?;

    // 写入缓存
    {
        let mut cache = crate::fastcontext::jwt_cache().lock().await;
        *cache = Some(crate::fastcontext::CachedJwt {
            api_key_fingerprint: fp,
            jwt: jwt.clone(),
            fetched_at: SystemTime::now(),
        });
    }

    Ok(jwt)
}

// ─── 限流 ─────────────────────────────────────────────────

pub async fn check_rate_limit(client: &Client, api_key: &str, jwt: &str) -> anyhow::Result<bool> {
    let mut request = ProtobufEncoder::default();
    request.write_message(1, &build_metadata(api_key, jwt)?);
    request.write_string(3, &ws_model());

    handle_rate_limit_result(
        unary_request(
            client,
            &format!("{API_BASE}/CheckUserMessageRateLimit"),
            &request.into_bytes(),
            true,
            30_000,
        )
        .await,
    )
}

/// 将 rate-limit HTTP 调用结果收敛为业务语义，便于单元测试覆盖错误分支。
fn handle_rate_limit_result(result: FcResult<Vec<u8>>) -> anyhow::Result<bool> {
    match result {
        Ok(_) => Ok(true),
        Err(err) if err.status == Some(429) => Ok(false),
        // 严格化：非 429 网络/HTTP 错误向上抛，避免后续浪费 LLM 配额
        Err(err) => Err(anyhow::anyhow!("rate-limit 检查失败: {}", err)),
    }
}

#[allow(dead_code)]
fn jwt_exp(token: &str) -> Option<u64> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value.get("exp").and_then(serde_json::Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_429_returns_false() {
        let result = handle_rate_limit_result(Err(FastContextError::status(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
        )))
        .expect("429 应被转换为限流状态而不是错误");

        assert!(!result, "429 应返回 false，提示调用方当前被限流");
    }

    #[test]
    fn rate_limit_non_429_error_is_propagated() {
        let error = handle_rate_limit_result(Err(FastContextError::status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        )))
        .expect_err("非 429 错误应向上抛出，避免静默消耗配额");

        assert!(error.to_string().contains("rate-limit 检查失败"));
        assert!(error.to_string().contains("SERVER_ERROR"));
    }

    #[test]
    fn streaming_4xx_errors_are_not_retryable() {
        for status in [
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::UNAUTHORIZED,
            reqwest::StatusCode::FORBIDDEN,
            reqwest::StatusCode::BAD_REQUEST,
        ] {
            let err = FastContextError::status(status);
            assert!(
                !should_retry_streaming_error(&err),
                "HTTP {} 不应进入重试",
                status.as_u16()
            );
        }
    }

    #[test]
    fn streaming_5xx_timeout_and_network_errors_are_retryable() {
        let server_error = FastContextError::status(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        let timeout = FastContextError::timeout("请求超时");
        let network = FastContextError::network("网络断开");

        assert!(should_retry_streaming_error(&server_error));
        assert!(should_retry_streaming_error(&timeout));
        assert!(should_retry_streaming_error(&network));
    }

    #[test]
    fn jitter_and_retry_delay_are_bounded_and_additive() {
        for attempt in 0..8 {
            let jitter = jitter_ms_from_seed(attempt, u64::MAX - attempt as u64);
            assert!(jitter < 400, "jitter 必须保持在 0..400ms 范围内");
            assert_eq!(
                retry_delay_ms(attempt, jitter),
                1000u64 * (attempt as u64 + 1) + jitter
            );
        }
    }

    #[test]
    fn build_request_contains_metadata_messages_and_tools() {
        let message = ChatMessage::new(1, "hello-there");
        let bytes = build_request("api-key-long", "jwt-token-long", &[message], "[]")
            .expect("请求应可构建");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("api-key-long"));
        assert!(text.contains("jwt-token-long"));
        assert!(text.contains("hello-there"));
    }
}
