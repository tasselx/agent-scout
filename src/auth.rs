//! API key resolution and config-file management.
//!
//! Resolution order (mirrors the JS reference):
//!   1. `--api-key` CLI value
//!   2. `WINDSURF_API_KEY` / `WINDSURFAPI_CODEIUM_API_KEY` env
//!   3. key file candidates (`~/.config/windsurf-search/api-key`, …)
//!   4. auto-discovery from the local Devin/Windsurf installation
//!      (state.vscdb `windsurfAuthStatus`, or Devin CLI credentials.toml on Linux/WSL)

use std::io::Write;
use std::path::{Path, PathBuf};

/// Result of a key-extraction attempt from a local credential source.
#[derive(Debug, Clone)]
pub struct Extraction {
    pub api_key: Option<String>,
    pub db_path: String,
    pub source_type: &'static str,
    pub error: Option<String>,
    pub hint: Option<String>,
}

const TOML_API_KEY_FIELDS: &[&str] = &[
    "api_key",
    "apiKey",
    "devin_api_key",
    "devinApiKey",
    "windsurf_api_key",
    "windsurfApiKey",
    "access_token",
    "accessToken",
    "token",
];

/// Ordered list of candidate key-file paths (first existing wins).
pub fn candidate_key_file_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(".config").join("windsurf-search").join("api-key"),
        home.join(".windsurf-search").join("api-key"),
        home.join(".piwin").join("windsurf-api-key"), // compat
    ]
}

pub fn default_key_file_path(home: &Path) -> PathBuf {
    candidate_key_file_paths(home)
        .into_iter()
        .next()
        .unwrap()
}

/// Resolve the API key in order: CLI value → env → key files → auto-discovery.
/// Returns an error if none is found.
pub fn resolve_api_key(
    home: &Path,
    cli_value: &str,
    env_vars: &[(String, String)],
    key_file: Option<&Path>,
) -> Result<String, String> {
    let cli = cli_value.trim();
    if !cli.is_empty() {
        return Ok(cli.to_string());
    }
    for (name, value) in env_vars {
        if name == "WINDSURF_API_KEY" || name == "WINDSURFAPI_CODEIUM_API_KEY" {
            let v = value.trim();
            if !v.is_empty() {
                return Ok(v.to_string());
            }
        }
    }
    let paths: Vec<PathBuf> = match key_file {
        Some(p) => vec![p.to_path_buf()],
        None => candidate_key_file_paths(home),
    };
    for path in paths {
        if let Ok(content) = std::fs::read_to_string(&path) {
            let line = content.lines().next().map(|s| s.trim()).unwrap_or("");
            if !line.is_empty() {
                return Ok(line.to_string());
            }
        }
    }
    // Final fallback: auto-discover from the local Devin/Windsurf installation.
    if key_file.is_none() {
        let extraction = extract_key_from_local(home, env_vars);
        if let Some(key) = extraction.api_key {
            return Ok(key);
        }
        if let Some(err) = extraction.error {
            return Err(format!(
                "no API key found. {} ({})",
                err,
                extraction.hint.unwrap_or_else(|| "log in to Devin/Windsurf".to_string())
            ));
        }
    }
    Err(format!(
        "no API key. Set WINDSURF_API_KEY, pass --api-key, or write the key to {}",
        default_key_file_path(home).display()
    ))
}

// ─── auto-discovery from local Devin/Windsurf installation ─────────────

/// Platform-specific candidate paths to Devin/Windsurf's `state.vscdb`.
/// Devin is the current app name; Deviv and Windsurf are compatibility fallbacks.
pub fn get_db_path_candidates(home: &Path, env_vars: &[(String, String)]) -> Vec<PathBuf> {
    let app_names = ["Devin", "Deviv", "Windsurf"];
    let os = std::env::consts::OS;
    match os {
        "macos" => app_names
            .iter()
            .map(|n| {
                home.join("Library")
                    .join("Application Support")
                    .join(n)
                    .join("User")
                    .join("globalStorage")
                    .join("state.vscdb")
            })
            .collect(),
        "windows" => {
            let appdata = env_var(env_vars, "APPDATA").unwrap_or_default();
            if appdata.is_empty() {
                return Vec::new();
            }
            app_names
                .iter()
                .map(|n| {
                    PathBuf::from(&appdata)
                        .join(n)
                        .join("User")
                        .join("globalStorage")
                        .join("state.vscdb")
                })
                .collect()
        }
        _ => {
            let config = env_var(env_vars, "XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            app_names
                .iter()
                .map(|n| {
                    config
                        .join(n)
                        .join("User")
                        .join("globalStorage")
                        .join("state.vscdb")
                })
                .collect()
        }
    }
}

/// Platform-specific Devin CLI credential candidates (Linux/WSL only).
pub fn get_cli_credential_path_candidates(home: &Path) -> Vec<PathBuf> {
    if std::env::consts::OS != "linux" {
        return Vec::new();
    }
    vec![home
        .join(".local")
        .join("share")
        .join("devin")
        .join("credentials.toml")]
}

/// Credential sources in lookup order: Devin CLI credentials.toml first
/// (Linux/WSL), then all state.vscdb candidates.
fn get_credential_sources(home: &Path, env_vars: &[(String, String)]) -> Vec<(SourceType, PathBuf)> {
    let mut sources = Vec::new();
    for p in get_cli_credential_path_candidates(home) {
        sources.push((SourceType::Toml, p));
    }
    for p in get_db_path_candidates(home, env_vars) {
        sources.push((SourceType::Sqlite, p));
    }
    sources
}

#[derive(Clone, Copy, PartialEq)]
enum SourceType {
    Toml,
    Sqlite,
}

/// Extract an API key from Devin CLI `credentials.toml` text.
/// Matches the JS `extractApiKeyFromToml`.
pub fn extract_key_from_toml(text: &str) -> Option<String> {
    extract_key_from_toml_matches(text)
}

/// Implementation of `extract_key_from_toml` without a regex crate.
fn extract_key_from_toml_matches(text: &str) -> Option<String> {
    for field in TOML_API_KEY_FIELDS {
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix(field) else {
                continue;
            };
            let rest = rest.trim_start();
            if !rest.starts_with('=') {
                continue;
            }
            let value = strip_toml_value(rest[1..].trim());
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    fallback_sk_token(text)
}

fn strip_toml_value(val: &str) -> String {
    let val = val.trim();
    if val.starts_with('"') {
        let v = val.trim_start_matches('"');
        let end = v.find('"').unwrap_or(v.len());
        return v[..end].to_string();
    }
    if val.starts_with('\'') {
        let v = val.trim_start_matches('\'');
        let end = v.find('\'').unwrap_or(v.len());
        return v[..end].to_string();
    }
    let end = val
        .find(|c: char| c.is_whitespace() || c == '#')
        .unwrap_or(val.len());
    val[..end].trim().to_string()
}

fn fallback_sk_token(text: &str) -> Option<String> {
    let mut best: Option<String> = None;
    let mut idx = 0;
    while let Some(start) = text[idx..].find("sk-") {
        let abs = idx + start;
        let rest = &text[abs..];
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(rest.len());
        let tok = &rest[..end];
        if tok.len() > 3 && (best.is_none() || tok.len() > best.as_ref().unwrap().len()) {
            best = Some(tok.to_string());
        }
        idx = abs + 1;
    }
    best
}

/// Extract API key from a `state.vscdb` file (windsurfAuthStatus record).
pub fn extract_key_from_db(db_path: &Path) -> Result<String, String> {
    let conn = rusqlite::Connection::open(db_path).map_err(|e| format!("open db: {e}"))?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = 'windsurfAuthStatus'")
        .map_err(|e| format!("prepare: {e}"))?;
    let mut rows = stmt.query([]).map_err(|e| format!("query: {e}"))?;
    let row = rows
        .next()
        .map_err(|e| format!("step: {e}"))?
        .ok_or_else(|| "windsurfAuthStatus record not found".to_string())?;
    let value: String = row.get(0).map_err(|e| format!("get: {e}"))?;
    drop(rows);
    drop(stmt);

    let data: serde_json::Value =
        serde_json::from_str(&value).map_err(|e| format!("parse windsurfAuthStatus: {e}"))?;
    data.get("apiKey")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "apiKey field is empty".to_string())
}

/// Extract the API key from the first available credential source.
/// Returns a full `Extraction` describing what was tried and found.
pub fn extract_key_from_local(home: &Path, env_vars: &[(String, String)]) -> Extraction {
    let sources = get_credential_sources(home, env_vars);
    let mut tried_paths = Vec::new();
    let mut first_existing_error: Option<Extraction> = None;

    for (source_type, path) in sources {
        let path_str = path.display().to_string();
        tried_paths.push(path_str.clone());
        if !path.exists() {
            continue;
        }
        let result = match source_type {
            SourceType::Toml => {
                match std::fs::read_to_string(&path) {
                    Ok(text) => match extract_key_from_toml_matches(&text) {
                        Some(key) => Extraction {
                            api_key: Some(key),
                            db_path: path_str,
                            source_type: "devin_cli_credentials",
                            error: None,
                            hint: None,
                        },
                        None => Extraction {
                            api_key: None,
                            db_path: path_str,
                            source_type: "devin_cli_credentials",
                            error: Some("Devin CLI credentials did not contain an API key".into()),
                            hint: Some("Run devin login inside WSL/Linux, then retry.".into()),
                        },
                    },
                    Err(e) => Extraction {
                        api_key: None,
                        db_path: path_str,
                        source_type: "devin_cli_credentials",
                        error: Some(format!("Failed to read Devin CLI credentials: {e}")),
                        hint: None,
                    },
                }
            }
            SourceType::Sqlite => match extract_key_from_db(&path) {
                Ok(key) => Extraction {
                    api_key: Some(key),
                    db_path: path_str,
                    source_type: "state.vscdb",
                    error: None,
                    hint: None,
                },
                Err(e) => Extraction {
                    api_key: None,
                    db_path: path_str,
                    source_type: "state.vscdb",
                    error: Some(e),
                    hint: Some("Ensure Windsurf or Devin is installed and logged in.".into()),
                },
            },
        };
        if result.api_key.is_some() {
            return result;
        }
        if first_existing_error.is_none() {
            first_existing_error = Some(result);
        }
    }

    if let Some(mut err) = first_existing_error {
        err.hint = Some(
            err.hint
                .unwrap_or_else(|| "log in to Devin/Windsurf".to_string()),
        );
        return err;
    }

    Extraction {
        api_key: None,
        db_path: tried_paths.first().cloned().unwrap_or_default(),
        source_type: "none",
        error: Some("Windsurf/Devin credential source not found".into()),
        hint: Some("Ensure Devin or Windsurf is installed and logged in.".into()),
    }
}

fn env_var<'a>(env_vars: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env_vars
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// Read the first line of a key file (empty string if missing).
pub fn read_configured_key(key_file: &Path) -> String {
    std::fs::read_to_string(key_file)
        .ok()
        .and_then(|c| c.lines().next().map(|s| s.trim().to_string()))
        .unwrap_or_default()
}

/// Write a key to a key file, creating parent dirs, chmod 600.
pub fn save_key(key_file: &Path, key: &str) -> Result<(), String> {
    if let Some(parent) = key_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(key_file, format!("{}\n", key.trim()))
        .map_err(|e| e.to_string())?;
    set_permissions_600(key_file);
    Ok(())
}

#[cfg(unix)]
fn set_permissions_600(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_permissions_600(_path: &Path) {}

/// Mask a key for safe display: first 12 chars + "…" + last 6 chars
/// (or first/last 2 for keys of length <= 12). Matches the JS reference.
pub fn mask_key(key: &str) -> String {
    let text = key.trim();
    if text.is_empty() {
        return "(empty)".to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 12 {
        let head: String = chars[..2].iter().collect();
        let tail: String = chars[chars.len() - 2..].iter().collect();
        return format!("{}…{}", head, tail);
    }
    let head: String = chars[..12].iter().collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{}…{}", head, tail)
}

/// Describe a key's format for `config show` / `config test`.
pub struct KeyFormat {
    pub kind: &'static str,
    pub label: String,
    pub ok: bool,
}

pub fn describe_key_format(key: &str) -> KeyFormat {
    let text = key.trim();
    if text.is_empty() {
        return KeyFormat { kind: "missing", label: "no key".into(), ok: false };
    }
    if text.starts_with("devin-session-token$") {
        return KeyFormat { kind: "session-token", label: "devin-session-token (session token)".into(), ok: true };
    }
    if text.starts_with("sk-ws-") {
        return KeyFormat { kind: "legacy-api-key", label: "sk-ws-* (legacy API key)".into(), ok: true };
    }
    if text.starts_with("ott$") {
        return KeyFormat { kind: "one-time-token", label: "ott$* (one-time token, deprecated)".into(), ok: false };
    }
    KeyFormat { kind: "unknown", label: "unknown key format".into(), ok: false }
}

/// Read a line from stdin (for `config set` interactive prompt).
pub fn read_line_stdin(prompt: &str, stderr: &mut dyn Write) -> Result<String, String> {
    write!(stderr, "{}", prompt).map_err(|e| e.to_string())?;
    stderr.flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_key_short() {
        // JS: slice(0,2) + … + slice(-2)
        assert_eq!(mask_key("abc"), "ab…bc");
        assert_eq!(mask_key("ab"), "ab…ab");
    }

    #[test]
    fn mask_key_long() {
        let key = "devin-session-token$abcdefghijklmnopqrstuvwxyz";
        let masked = mask_key(key);
        assert!(masked.starts_with("devin-sessio"));
        assert!(masked.ends_with("vwxyz"));
        assert!(masked.contains('…'));
        assert!(!masked.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn describe_formats() {
        assert!(describe_key_format("devin-session-token$abc").ok);
        assert!(describe_key_format("sk-ws-abc").ok);
        assert!(!describe_key_format("ott$abc").ok);
        assert!(!describe_key_format("").ok);
        assert_eq!(describe_key_format("").kind, "missing");
    }

    #[test]
    fn extract_toml_double_quoted() {
        let toml = "api_key = \"devin-session-token$abc123\"\n";
        assert_eq!(extract_key_from_toml(toml).as_deref(), Some("devin-session-token$abc123"));
    }

    #[test]
    fn extract_toml_single_quoted_and_comment() {
        let toml = "access_token = 'sk-abcdef123'  # comment\n";
        assert_eq!(extract_key_from_toml(toml).as_deref(), Some("sk-abcdef123"));
    }

    #[test]
    fn extract_toml_sk_fallback() {
        let toml = "some_other = 1\n# nothing matched\n";
        assert!(extract_key_from_toml(toml).is_none());
        let toml2 = "[auth]\nkey = \"x\"\nsk-zyxwvu987 anywhere";
        assert_eq!(extract_key_from_toml(toml2).as_deref(), Some("sk-zyxwvu987"));
    }

    #[test]
    fn extract_key_from_db_roundtrip() {
        let dir = std::env::temp_dir().join(format!("agent-scout-auth-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("state.vscdb");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT UNIQUE, value TEXT)",
            [],
        )
        .unwrap();
        let status = format!(
            r#"{{"apiKey":"devin-session-token$test-from-db","other":1}}"#
        );
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('windsurfAuthStatus', ?1)",
            [&status],
        )
        .unwrap();
        drop(conn);

        let key = extract_key_from_db(&db_path).unwrap();
        assert_eq!(key, "devin-session-token$test-from-db");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_key_from_db_missing_record() {
        let dir = std::env::temp_dir().join(format!("agent-scout-auth-test2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("state.vscdb");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE ItemTable (key TEXT UNIQUE, value TEXT)", []).unwrap();
        drop(conn);
        let err = extract_key_from_db(&db_path).unwrap_err();
        assert!(err.contains("not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}