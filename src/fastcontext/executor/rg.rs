//! rg 命令：优先调系统 ripgrep，失败则走 Rust 内置正则搜索。

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use ignore::gitignore::Gitignore;
use regex::Regex;

use super::fs::{
    glob_match, is_ignored_path, normalize_path, truncate_output, RESULT_MAX_LINES,
};
use super::{format_path_fallback_warning, prepend_warning, ToolExecutor};

impl ToolExecutor {
    pub(super) async fn rg(
        &self,
        pattern: &str,
        path: &str,
        include: Vec<String>,
        exclude: Vec<String>,
    ) -> String {
        if pattern.trim().is_empty() {
            log::warn!("[fast-context] rg 缺少 pattern");
            return "Error: missing or invalid pattern".to_string();
        }
        let Ok(real_path) = self.real_path(path) else {
            log::warn!("[fast-context] rg 路径无法映射: {}", path);
            return format!("Error: path does not exist: {path}");
        };
        if is_ignored_path(&self.ignore_matcher, &real_path) {
            return format!("Error: path is ignored: {path}");
        }
        let (real_path, path_warning) = if real_path.exists() {
            (real_path, None)
        } else if let Some(fallback) = self.path_fallback(path) {
            log::warn!(
                "[fast-context] rg 路径不存在，已回退到最近存在父目录: requested={}, fallback={}",
                path,
                fallback.fallback_label
            );
            let warning = format_path_fallback_warning("rg", &fallback);
            (fallback.fallback_path, Some(warning))
        } else {
            log::warn!("[fast-context] rg 路径不存在: {}", real_path.display());
            return format!("Error: path does not exist: {path}");
        };
        {
            let mut state = self.state.lock().await;
            state.collected_rg_patterns.push(pattern.to_string());
        }

        let mut command = tokio::process::Command::new("rg");
        command
            .arg("--no-heading")
            .arg("-n")
            .arg("--max-count")
            .arg("50");
        for glob in &include {
            command.arg("--glob").arg(glob);
        }
        for glob in &exclude {
            command.arg("--glob").arg(format!("!{glob}"));
        }
        // rg 原生加载四类 ignore 文件，行为与 Rust 回退搜索保持一致。
        for ignore_file in &self.ignore_files {
            command.arg("--ignore-file").arg(ignore_file);
        }
        command
            .arg(pattern)
            .arg(&real_path)
            .current_dir(&self.root)
            .env("RIPGREP_CONFIG_PATH", "")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        match tokio::time::timeout(Duration::from_secs(30), command.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                let result =
                    truncate_output(&self.remap(&String::from_utf8_lossy(&output.stdout)));
                log::info!(
                    "[fast-context] rg 成功: path={}, output_len={}",
                    real_path.display(),
                    result.len()
                );
                prepend_warning(path_warning.as_deref(), &result)
            }
            Ok(Ok(output)) if output.status.code() == Some(1) => {
                log::info!("[fast-context] rg 无匹配: pattern={}", pattern);
                prepend_warning(path_warning.as_deref(), "(no matches)")
            }
            Ok(Ok(output)) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let result = truncate_output(&self.remap(if stderr.trim().is_empty() {
                    "Error: rg failed"
                } else {
                    &stderr
                }));
                log::warn!(
                    "[fast-context] rg 执行失败: status={:?}, output_len={}",
                    output.status.code(),
                    result.len()
                );
                prepend_warning(path_warning.as_deref(), &result)
            }
            // 本机未安装 rg 时走 Rust 内置搜索，保证 fast-context 不因外部二进制缺失不可用。
            Ok(Err(err)) => {
                log::warn!(
                    "[fast-context] 启动 rg 失败，改用 Rust 内置搜索: error={}",
                    err
                );
                prepend_warning(
                    path_warning.as_deref(),
                    &self.rg_fallback(pattern, &real_path, &include, &exclude),
                )
            }
            Err(_) => {
                log::warn!("[fast-context] rg 超时: pattern={}", pattern);
                "Error: rg timed out".to_string()
            }
        }
    }

    fn rg_fallback(
        &self,
        pattern: &str,
        real_path: &Path,
        include: &[String],
        exclude: &[String],
    ) -> String {
        let regex = match Regex::new(pattern) {
            Ok(regex) => regex,
            Err(err) => {
                log::warn!("[fast-context] Rust 内置搜索 regex 无效: {}", err);
                return format!("Error: invalid regex: {err}");
            }
        };
        let mut matches = Vec::new();
        collect_rg_matches(
            &self.root,
            real_path,
            &regex,
            include,
            exclude,
            &self.ignore_matcher,
            &mut matches,
        );
        if matches.is_empty() {
            log::info!("[fast-context] Rust 内置搜索无匹配: pattern={}", pattern);
            "(no matches)".to_string()
        } else {
            let result = truncate_output(&self.remap(&matches.join("\n")));
            log::info!(
                "[fast-context] Rust 内置搜索完成: matches={}, output_len={}",
                matches.len(),
                result.len()
            );
            result
        }
    }
}

fn collect_rg_matches(
    root: &Path,
    path: &Path,
    regex: &Regex,
    include: &[String],
    exclude: &[String],
    ignore_matcher: &Gitignore,
    matches: &mut Vec<String>,
) {
    if matches.len() >= RESULT_MAX_LINES {
        return;
    }

    if path.is_file() {
        if !path_matches_filters(root, path, include, exclude, ignore_matcher) {
            return;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        for (idx, line) in content.lines().enumerate() {
            if matches.len() >= RESULT_MAX_LINES {
                return;
            }
            if regex.is_match(line) {
                matches.push(format!("{}:{}:{}", normalize_path(path), idx + 1, line));
            }
        }
        return;
    }

    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        if matches.len() >= RESULT_MAX_LINES {
            return;
        }
        let entry_path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(".git")
            || exclude.iter().any(|pattern| glob_match(pattern, &name))
            || is_ignored_path(ignore_matcher, &entry_path)
        {
            continue;
        }
        collect_rg_matches(
            root,
            &entry_path,
            regex,
            include,
            exclude,
            ignore_matcher,
            matches,
        );
    }
}

fn path_matches_filters(
    root: &Path,
    path: &Path,
    include: &[String],
    exclude: &[String],
    ignore_matcher: &Gitignore,
) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let rel_slash = normalize_path(rel);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();

    if is_ignored_path(ignore_matcher, path)
        || exclude
            .iter()
            .any(|pattern| glob_match(pattern, &rel_slash) || glob_match(pattern, &file_name))
    {
        return false;
    }
    include.is_empty()
        || include
            .iter()
            .any(|pattern| glob_match(pattern, &rel_slash) || glob_match(pattern, &file_name))
}
