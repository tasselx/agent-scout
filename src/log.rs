//! Minimal error logging for agent-scout.
//!
//! Writes errors to a per-day log file under the config dir:
//!   `~/.config/windsurf-search/logs/agent-scout-YYYY-MM-DD.log`
//!
//! Old log files are automatically pruned on each write: files older than
//! `max_age_days` (default 7) are removed, and the directory is capped at a
//! max number of files. Logging is best-effort and never fails the caller.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAX_AGE_DAYS: u64 = 7;
pub const DEFAULT_MAX_FILES: usize = 30;

/// Resolve the log directory: `<config dir>/logs`.
/// Config dir is `~/.config/windsurf-search` (or `$XDG_CONFIG_HOME` on Linux).
pub fn log_dir(home: &Path) -> PathBuf {
    let base = config_dir(home);
    base.join("logs")
}

/// Resolve the config directory for agent-scout.
pub fn config_dir(home: &Path) -> PathBuf {
    if std::env::consts::OS == "linux" {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("windsurf-search");
            }
        }
    }
    home.join(".config").join("windsurf-search")
}

/// Build the per-day log file path: `agent-scout-YYYY-MM-DD.log`.
pub fn daily_log_path(home: &Path) -> PathBuf {
    let dir = log_dir(home);
    let stamp = date_stamp();
    dir.join(format!("agent-scout-{}.log", stamp))
}

/// Current local date as `YYYY-MM-DD`.
fn date_stamp() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Convert to (year, month, day) using days since epoch (civil algorithm).
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

// Convert days since 1970-01-01 to (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Append a log line at the given level. Creates the log dir if needed.
/// Auto-prunes old log files. Never returns an error to the caller.
pub fn log_line(home: &Path, level: &str, message: &str) {
    let path = daily_log_path(home);
    if let Some(parent) = path.parent() {
        if fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let line = format!(
        "{} [{}] {}\n",
        timestamp(),
        level,
        message.replace('\n', " ")
    );
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
    prune(home);
}

/// Convenience: log an error-level message.
pub fn log_error(home: &Path, message: &str) {
    log_line(home, "ERROR", message);
}

/// Convenience: log an info-level message.
pub fn log_info(home: &Path, message: &str) {
    log_line(home, "INFO", message);
}

/// Remove log files older than `max_age_days` and cap total file count.
pub fn prune(home: &Path) {
    prune_with(home, DEFAULT_MAX_AGE_DAYS, DEFAULT_MAX_FILES);
}

/// Prune with explicit limits (used mainly for tests).
fn prune_with(home: &Path, max_age_days: u64, max_files: usize) {
    let dir = log_dir(home);
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let now_days = days_since_epoch();

    let mut files: Vec<(PathBuf, i64)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let age_days = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(parse_date_suffix)
            .map(|d| now_days - d)
            .unwrap_or(i64::MAX);
        files.push((path, age_days));
    }

    // Remove files older than max_age_days.
    for (path, age) in files.iter() {
        if *age > max_age_days as i64 {
            let _ = fs::remove_file(path);
        }
    }
    files.retain(|(_, age)| *age <= max_age_days as i64);

    // Cap total files: keep the most recent `max_files`.
    if files.len() > max_files {
        files.sort_by_key(|(_, age)| *age);
        for (path, _) in files.iter().take(files.len() - max_files) {
            let _ = fs::remove_file(path);
        }
    }
}

fn days_since_epoch() -> i64 {
    let now = std::time::SystemTime::now();
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}

/// Parse `agent-scout-YYYY-MM-DD.log` into days-since-epoch. Returns None if
/// the filename doesn't match the expected shape.
fn parse_date_suffix(name: &str) -> Option<i64> {
    // Expected: agent-scout-2026-08-10.log
    let rest = name.strip_prefix("agent-scout-")?;
    let date_part = rest.strip_suffix(".log")?;
    let mut parts = date_part.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

fn timestamp() -> String {
    let now = std::time::SystemTime::now();
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Reuse the date + append HH:MM:SS from the remainder.
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    let rem = secs % 86_400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y, m, d, hh, mm, ss
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_roundtrip() {
        // 1970-01-01 -> epoch day 0
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // A known recent date.
        assert_eq!(civil_from_days(days_from_civil(2026, 8, 10)), (2026, 8, 10));
    }

    #[test]
    fn parse_date_suffix_ok() {
        let d = parse_date_suffix("agent-scout-2026-08-10.log").unwrap();
        assert!(d > days_from_civil(2020, 1, 1));
        assert!(parse_date_suffix("random.log").is_none());
        assert!(parse_date_suffix("agent-scout-2026-08.log").is_none());
    }

    #[test]
    fn log_and_prune() {
        let dir = std::env::temp_dir().join(format!("agent-scout-log-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        log_line(&dir, "ERROR", "test error message");
        let p = daily_log_path(&dir);
        let content = fs::read_to_string(&p).unwrap();
        assert!(content.contains("test error message"));
        assert!(content.contains("[ERROR]"));
        let _ = fs::remove_dir_all(&dir);
    }
}