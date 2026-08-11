//! Fast Context — AI 驱动的语义代码搜索。
//!
//! 通过 Windsurf Devstral 协议（GetDevstralStream / GetUserJwt /
//! CheckUserMessageRateLimit）在本地代码库中做自然语言检索。

pub mod executor;
pub mod http;
pub mod proto;
pub mod search;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use serde_json::Value;
use tokio::sync::Mutex;

pub use search::{search, search_result_json};

// ─── 错误类型 ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FastContextError {
    pub code: String,
    pub message: String,
    pub status: Option<u16>,
}

impl FastContextError {
    pub fn status(status: reqwest::StatusCode) -> Self {
        let code = match status.as_u16() {
            413 => "PAYLOAD_TOO_LARGE",
            429 => "RATE_LIMITED",
            401 | 403 => "AUTH_ERROR",
            _ => "SERVER_ERROR",
        };
        Self {
            code: code.to_string(),
            message: format!("HTTP {}", status.as_u16()),
            status: Some(status.as_u16()),
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            code: "TIMEOUT".to_string(),
            message: message.into(),
            status: None,
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self {
            code: "NETWORK_ERROR".to_string(),
            message: message.into(),
            status: None,
        }
    }
}

impl fmt::Display for FastContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for FastContextError {}

pub type FcResult<T> = std::result::Result<T, FastContextError>;

// ─── JWT 缓存 ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CachedJwt {
    pub api_key_fingerprint: u64,
    pub jwt: String,
    pub fetched_at: SystemTime,
}

static JWT_CACHE: OnceLock<Mutex<Option<CachedJwt>>> = OnceLock::new();

pub fn jwt_cache() -> &'static Mutex<Option<CachedJwt>> {
    JWT_CACHE.get_or_init(|| Mutex::new(None))
}

/// 用 FNV-1a 计算 api_key 指纹，避免把明文丢进缓存键
pub fn api_key_fp(api_key: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for b in api_key.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 计算 ratio 的百分比形式（保留一位小数）
fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        ((numerator as f64 / denominator as f64) * 1000.0).round() / 10.0
    }
}

// ─── 公共数据类型 ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SearchOptions {
    pub query: String,
    pub project_root: PathBuf,
    pub api_key: Option<String>,
    pub tree_depth: u8,
    pub max_turns: u8,
    pub max_results: u8,
    pub max_commands: u8,
    pub timeout_ms: u64,
    pub exclude_paths: Vec<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            query: String::new(),
            project_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            api_key: None,
            tree_depth: 3,
            max_turns: 3,
            max_results: 10,
            max_commands: 8,
            timeout_ms: 30_000,
            exclude_paths: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub files: Vec<FastContextFile>,
    pub rg_patterns: Vec<String>,
    /// ToolExecutor 在 readfile 命令中读取过的文件内容缓存（key 为规范化绝对路径）
    /// 用于格式化层复用，避免重复 IO。
    pub file_cache: std::collections::HashMap<String, String>,
    pub stats: SearchStats,
    pub meta: Value,
    /// 是否已经收到合法 answer 工具调用；用于区分"确实无相关文件"和"搜索中途退化"。
    pub answer_received: bool,
}

#[derive(Debug, Clone)]
pub struct FastContextFile {
    pub path: Option<String>,
    pub full_path: Option<String>,
    pub ranges: Vec<[usize; 2]>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchStats {
    pub commands_seen: usize,
    pub commands_executed: usize,
    pub commands_useful: usize,
    pub commands_invalid: usize,
    pub commands_repaired: usize,
    pub path_missing: usize,
    pub path_repaired: usize,
    pub cache_hits: usize,
    pub error_outputs: usize,
}

impl SearchStats {
    pub fn merge(&mut self, other: &SearchStats) {
        self.commands_seen += other.commands_seen;
        self.commands_executed += other.commands_executed;
        self.commands_useful += other.commands_useful;
        self.commands_invalid += other.commands_invalid;
        self.commands_repaired += other.commands_repaired;
        self.path_missing += other.path_missing;
        self.path_repaired += other.path_repaired;
        self.cache_hits += other.cache_hits;
        self.error_outputs += other.error_outputs;
    }

    pub fn useful_rate(&self) -> f64 {
        ratio(self.commands_useful, self.commands_seen)
    }

    pub fn invalid_rate(&self) -> f64 {
        ratio(self.commands_invalid, self.commands_seen)
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "commandsSeen": self.commands_seen,
            "commandsExecuted": self.commands_executed,
            "commandsUseful": self.commands_useful,
            "commandsInvalid": self.commands_invalid,
            "commandsRepaired": self.commands_repaired,
            "pathMissing": self.path_missing,
            "pathRepaired": self.path_repaired,
            "cacheHits": self.cache_hits,
            "errorOutputs": self.error_outputs,
            "usefulCommandRate": self.useful_rate(),
            "invalidCommandRate": self.invalid_rate()
        })
    }
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: u64,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_args_json: Option<String>,
    pub ref_call_id: Option<String>,
}

impl ChatMessage {
    pub fn new(role: u64, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
            tool_name: None,
            tool_args_json: None,
            ref_call_id: None,
        }
    }
}

// ─── API Key 检测 ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ApiKeyDetection {
    pub api_key: String,
    pub source: ApiKeySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeySource {
    Config,
    Env,
    DevinDb,
    WindsurfDb,
}

impl ApiKeySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Env => "env",
            Self::DevinDb => "devin_db",
            Self::WindsurfDb => "windsurf_db",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Config => "已保存配置",
            Self::Env => "WINDSURF_API_KEY 环境变量",
            Self::DevinDb => "Devin 本地登录库",
            Self::WindsurfDb => "Windsurf 本地登录库",
        }
    }
}

pub fn detect_api_key(configured: Option<&str>) -> anyhow::Result<ApiKeyDetection> {
    if let Some(key) = configured.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(ApiKeyDetection {
            api_key: key.to_string(),
            source: ApiKeySource::Config,
        });
    }
    if let Ok(key) = std::env::var("WINDSURF_API_KEY") {
        let key = key.trim().to_string();
        if !key.is_empty() {
            return Ok(ApiKeyDetection {
                api_key: key,
                source: ApiKeySource::Env,
            });
        }
    }
    extract_local_api_key()?.ok_or_else(|| {
        anyhow::anyhow!("Devin / Windsurf 本地登录库中没有 apiKey")
    })
}

pub fn mask_api_key(api_key: &str) -> String {
    let trimmed = api_key.trim();
    let char_count = trimmed.chars().count();
    if char_count <= 8 {
        return "*".repeat(char_count.max(1));
    }
    let prefix = trimmed.chars().take(4).collect::<String>();
    let suffix = trimmed
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

fn extract_local_api_key() -> anyhow::Result<Option<ApiKeyDetection>> {
    // 优先读取 Devin 的新数据目录，并保留 Windsurf 旧目录作为迁移回退。
    for (source, db_path) in local_state_db_candidates()? {
        if !db_path.exists() {
            log::debug!(
                "[fast-context] 本地登录数据库不存在: source={}, path={}",
                source.as_str(),
                db_path.display()
            );
            continue;
        }
        log::debug!(
            "[fast-context] 尝试读取本地登录数据库: source={}, path={}",
            source.as_str(),
            db_path.display()
        );
        match extract_api_key_from_state_db(&db_path) {
            Ok(Some(api_key)) => return Ok(Some(ApiKeyDetection { api_key, source })),
            Ok(None) => log::warn!(
                "[fast-context] 本地登录数据库没有可用 apiKey: source={}, path={}",
                source.as_str(),
                db_path.display()
            ),
            Err(err) => log::warn!(
                "[fast-context] 读取本地登录数据库失败，继续尝试兼容目录: source={}, path={}, error={}",
                source.as_str(),
                db_path.display(),
                err
            ),
        }
    }
    Ok(None)
}

fn extract_api_key_from_state_db(db_path: &Path) -> anyhow::Result<Option<String>> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .with_context(|| format!("打开本地登录数据库失败: {}", db_path.display()))?;
    let value: String = match conn.query_row(
        "SELECT value FROM ItemTable WHERE key = 'windsurfAuthStatus'",
        [],
        |row| row.get(0),
    ) {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(err) => {
            return Err(err).context("读取 windsurfAuthStatus 记录失败");
        }
    };
    let json: Value = serde_json::from_str(&value).context("解析 windsurfAuthStatus JSON 失败")?;
    Ok(json
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned))
}

fn local_state_db_candidates() -> anyhow::Result<Vec<(ApiKeySource, PathBuf)>> {
    if cfg!(target_os = "macos") {
        let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法定位用户主目录"))?;
        let app_support = home.join("Library").join("Application Support");
        return Ok(vec![
            (ApiKeySource::DevinDb, state_db_path(&app_support, "Devin")),
            (
                ApiKeySource::WindsurfDb,
                state_db_path(&app_support, "Windsurf"),
            ),
        ]);
    }
    if cfg!(target_os = "windows") {
        let appdata = std::env::var("APPDATA").context("无法读取 APPDATA 环境变量")?;
        let appdata = PathBuf::from(appdata);
        return Ok(vec![
            (ApiKeySource::DevinDb, state_db_path(&appdata, "devin")),
            (
                ApiKeySource::WindsurfDb,
                state_db_path(&appdata, "Windsurf"),
            ),
        ]);
    }

    let config_root = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| {
            dirs::home_dir()
                .map(|home| home.join(".config"))
                .ok_or(std::env::VarError::NotPresent)
        })
        .context("无法定位 Linux 配置目录")?;
    Ok(vec![
        (ApiKeySource::DevinDb, state_db_path(&config_root, "devin")),
        (
            ApiKeySource::WindsurfDb,
            state_db_path(&config_root, "Windsurf"),
        ),
    ])
}

fn state_db_path(config_root: &Path, product_dir: &str) -> PathBuf {
    config_root
        .join(product_dir)
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

use anyhow::Context;
