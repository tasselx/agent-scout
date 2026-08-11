//! 本地工具执行器。
//!
//! 执行 Devstral 模型生成的 restricted_exec 命令（rg/readfile/tree/ls/glob）：
//! - 命令形状归一化：兼容 LLM 各种扁平/shorthand 畸形参数
//! - 路径回退：rg/glob 路径不存在时自动回退到最近存在的父目录
//! - gitignore 支持：repo map 与全部检索命令统一生效
//! - 命令缓存 + 指纹去重 + readfile 内容缓存 + 统计

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::future::join_all;
use globset::GlobBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use regex::Regex;
use serde_json::{Map, Value};
use tokio::sync::Mutex;

use crate::fastcontext::SearchStats;

const RESULT_MAX_LINES: usize = 50;
const LINE_MAX_CHARS: usize = 250;

/// 需要原生加载的 ignore 文件（rg 与 Rust 回退搜索都会读取）。
pub const FAST_CONTEXT_IGNORE_FILES: [&str; 4] = [
    ".gitignore",
    ".codeiumignore",
    ".windsurfignore",
    ".devinignore",
];

// ─── 共享基础工具 ─────────────────────────────────────────

pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn has_parent_dir(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

/// 截断输出：最多 RESULT_MAX_LINES 行，每行最多 LINE_MAX_CHARS 字符。
pub fn truncate_output(text: &str) -> String {
    let lines = text.lines().collect::<Vec<_>>();
    let mut truncated = lines
        .iter()
        .take(RESULT_MAX_LINES)
        .map(|line| {
            if line.chars().count() > LINE_MAX_CHARS {
                line.chars().take(LINE_MAX_CHARS).collect::<String>()
            } else {
                (*line).to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > RESULT_MAX_LINES {
        truncated.push_str("\n... (lines truncated) ...");
    }
    truncated
}

pub fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn matches_type(type_filter: &str, is_file: bool, is_dir: bool) -> bool {
    match type_filter {
        "file" => is_file,
        "directory" => is_dir,
        _ => true,
    }
}

pub fn glob_match(pattern: &str, text: &str) -> bool {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|glob| glob.compile_matcher().is_match(text))
        .unwrap_or(false)
}

pub fn matches_exclude(name: &str, exclude_paths: &[String]) -> bool {
    exclude_paths.iter().any(|pattern| {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            return false;
        }
        if !pattern.contains('*') && !pattern.contains('?') {
            return pattern == name;
        }
        glob_match(pattern, name)
    })
}

/// 构建 fast-context 专属 ignore matcher（合并 .gitignore 等四类文件）。
pub fn build_fast_context_ignore(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    for file_name in FAST_CONTEXT_IGNORE_FILES {
        let path = root.join(file_name);
        if !path.is_file() {
            continue;
        }
        if let Some(err) = builder.add(&path) {
            log::warn!(
                "[fast-context] ignore 文件部分规则解析失败: path={}, error={}",
                path.display(),
                err
            );
        }
    }
    builder.build().unwrap_or_else(|err| {
        log::warn!("[fast-context] ignore matcher 构建失败: {}", err);
        Gitignore::empty()
    })
}

pub fn fast_context_ignore_files(root: &Path) -> Vec<PathBuf> {
    FAST_CONTEXT_IGNORE_FILES
        .iter()
        .map(|file_name| root.join(file_name))
        .filter(|path| path.is_file())
        .collect()
}

pub fn is_ignored_path(ignore_matcher: &Gitignore, path: &Path) -> bool {
    ignore_matcher
        .matched_path_or_any_parents(path, path.is_dir())
        .is_ignore()
}

fn sorted_entries(
    dir: &Path,
    exclude_paths: &[String],
    ignore_matcher: &Gitignore,
) -> Vec<fs::DirEntry> {
    let mut entries = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir.filter_map(|entry| entry.ok()).collect::<Vec<_>>(),
        Err(_) => return Vec::new(),
    };
    entries.retain(|entry| {
        let name = entry.file_name().to_string_lossy().to_string();
        !matches_exclude(&name, exclude_paths) && !is_ignored_path(ignore_matcher, &entry.path())
    });
    entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
    entries
}

/// 浅层统计目录下条目数（不递归），用于 tree 显示规模线索
fn entry_count(dir: &Path, exclude_paths: &[String], ignore_matcher: &Gitignore) -> usize {
    fs::read_dir(dir)
        .map(|read_dir| {
            read_dir
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    !matches_exclude(&name, exclude_paths)
                        && !is_ignored_path(ignore_matcher, &entry.path())
                })
                .count()
        })
        .unwrap_or(0)
}

pub fn build_tree(
    root: &Path,
    label: &str,
    max_depth: u8,
    exclude_paths: &[String],
    ignore_matcher: &Gitignore,
) -> String {
    let mut lines = vec![label.to_string()];
    append_tree(
        root,
        "",
        1,
        max_depth,
        exclude_paths,
        ignore_matcher,
        &mut lines,
    );
    lines.join("\n")
}

fn append_tree(
    dir: &Path,
    prefix: &str,
    depth: u8,
    max_depth: u8,
    exclude_paths: &[String],
    ignore_matcher: &Gitignore,
    lines: &mut Vec<String>,
) {
    if depth > max_depth {
        return;
    }

    let entries = sorted_entries(dir, exclude_paths, ignore_matcher);
    let len = entries.len();
    for (idx, entry) in entries.into_iter().enumerate() {
        let name = entry.file_name().to_string_lossy().to_string();
        let last = idx + 1 == len;
        // 给目录追加 "(N entries)" 后缀，避免 LLM 盲探巨型目录
        let display_name = if entry.path().is_dir() {
            let count = entry_count(&entry.path(), exclude_paths, ignore_matcher);
            if count > 0 {
                format!("{name}/ ({count} entries)")
            } else {
                format!("{name}/")
            }
        } else {
            name
        };
        lines.push(format!(
            "{}{} {}",
            prefix,
            if last { "`--" } else { "|--" },
            display_name
        ));
        if entry.path().is_dir() {
            let next_prefix = format!("{}{}", prefix, if last { "   " } else { "|  " });
            append_tree(
                &entry.path(),
                &next_prefix,
                depth + 1,
                max_depth,
                exclude_paths,
                ignore_matcher,
                lines,
            );
        }
    }
}

/// 顶层 list_root：repo map 深度 0 时的兜底（简单 ls）。
pub fn list_root(root: &Path, exclude_paths: &[String], ignore_matcher: &Gitignore) -> String {
    let mut lines = vec!["/codebase".to_string()];
    for entry in sorted_entries(root, exclude_paths, ignore_matcher) {
        lines.push(format!("|-- {}", entry.file_name().to_string_lossy()));
    }
    lines.join("\n")
}

// ─── 命令归一化与校验 ─────────────────────────────────────

/// 把 LLM 各种畸形命令形状归一化为标准 `{type, ...}` 形式。
/// 返回 `(标准化后的命令, 是否发生过修复)`。
pub fn normalize_command_shape(value: &Value) -> Option<(Value, bool)> {
    if value.get("type").and_then(Value::as_str).is_some() {
        return Some((value.clone(), false));
    }

    let obj = value.as_object()?;
    for kind in ["rg", "readfile", "tree", "ls", "glob"] {
        if let Some(nested) = obj.get(kind).and_then(Value::as_object) {
            let mut command = nested.clone();
            command.insert("type".to_string(), Value::String(kind.to_string()));
            return Some((Value::Object(command), true));
        }
    }

    // 兼容 LLM 常见 shorthand：{"readfile": "/codebase/a.rs", "start_line": 1}
    if let Some(file) = obj.get("readfile").and_then(Value::as_str) {
        let mut command = Map::new();
        command.insert("type".to_string(), Value::String("readfile".to_string()));
        command.insert("file".to_string(), Value::String(file.to_string()));
        copy_optional_keys(obj, &mut command, &["start_line", "end_line"]);
        return Some((Value::Object(command), true));
    }

    // 兼容已在运行日志中出现的扁平参数：{"file":"/codebase/a.rs"}。
    if let Some(file) = obj.get("file").and_then(Value::as_str) {
        let mut command = obj.clone();
        command.insert("type".to_string(), Value::String("readfile".to_string()));
        command.insert("file".to_string(), Value::String(file.to_string()));
        return Some((Value::Object(command), true));
    }

    // 兼容已在运行日志中出现的扁平参数：{"rg":"pattern","path":"/codebase"}。
    if let Some(pattern) = obj.get("rg").and_then(Value::as_str) {
        let mut command = obj.clone();
        command.remove("rg");
        command.insert("type".to_string(), Value::String("rg".to_string()));
        command.insert("pattern".to_string(), Value::String(pattern.to_string()));
        return Some((Value::Object(command), true));
    }

    if let Some(path) = obj.get("tree").and_then(Value::as_str) {
        let mut command = Map::new();
        command.insert("type".to_string(), Value::String("tree".to_string()));
        command.insert("path".to_string(), Value::String(path.to_string()));
        copy_optional_keys(obj, &mut command, &["levels"]);
        return Some((Value::Object(command), true));
    }

    if let Some(path) = obj.get("ls").and_then(Value::as_str) {
        let mut command = Map::new();
        command.insert("type".to_string(), Value::String("ls".to_string()));
        command.insert("path".to_string(), Value::String(path.to_string()));
        copy_optional_keys(obj, &mut command, &["long_format", "all"]);
        return Some((Value::Object(command), true));
    }

    None
}

fn copy_optional_keys(source: &Map<String, Value>, target: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        if let Some(value) = source.get(*key) {
            target.insert((*key).to_string(), value.clone());
        }
    }
}

fn non_empty_str(value: &Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|s| !s.trim().is_empty())
}

pub fn is_structurally_valid_command(value: &Value) -> bool {
    match normalize_command_shape(value) {
        Some((command, _)) => match command.get("type").and_then(Value::as_str) {
            Some("rg") => non_empty_str(&command, "pattern") && non_empty_str(&command, "path"),
            Some("readfile") => non_empty_str(&command, "file"),
            Some("tree" | "ls") => non_empty_str(&command, "path"),
            Some("glob") => non_empty_str(&command, "pattern") && non_empty_str(&command, "path"),
            _ => false,
        },
        None => false,
    }
}

pub fn count_valid_commands(args: &Value) -> usize {
    args.as_object()
        .map(|obj| {
            obj.iter()
                .filter(|(key, value)| {
                    key.starts_with("command") && is_structurally_valid_command(value)
                })
                .count()
        })
        .unwrap_or(0)
}

// ─── 输出分类统计 ─────────────────────────────────────────

fn strip_diagnostic_lines(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("Warning: requested path missing")
                && !trimmed.starts_with("Hint: requested path missing")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_useful_output(output: &str) -> bool {
    let trimmed = output.trim();
    !trimmed.is_empty() && !trimmed.starts_with("Error:") && trimmed != "(no matches)"
}

fn classify_output(output: &str, stats: &mut SearchStats) {
    let trimmed = output.trim();
    let has_repair_warning = trimmed.contains("Warning: requested path missing");
    let has_missing_hint = trimmed.contains("Hint: requested path missing");
    if has_repair_warning {
        stats.path_missing = 1;
        stats.path_repaired = 1;
    } else if has_missing_hint {
        stats.path_missing = 1;
    }

    let effective = strip_diagnostic_lines(trimmed);
    if is_useful_output(&effective) {
        stats.commands_useful = 1;
        return;
    }

    if effective.starts_with("Error:") {
        stats.error_outputs = 1;
        if effective.contains("path does not exist")
            || effective.contains("file not found")
            || effective.contains("dir not found")
            || effective.contains("not a directory")
        {
            stats.path_missing = 1;
        }
    }
}

fn normalize_virtual_path(path: &str) -> String {
    let normalized = path.trim().replace('\\', "/");
    if normalized.starts_with("/codebase") {
        normalized
    } else {
        format!("/codebase/{}", normalized.trim_start_matches('/'))
    }
}

fn format_path_fallback_warning(command: &str, fallback: &PathFallback) -> String {
    let candidates = if fallback.candidates.is_empty() {
        "(no siblings)".to_string()
    } else {
        fallback.candidates.join(", ")
    };
    format!(
        "Warning: requested path missing for {command}; requested={}; searched_nearest_existing_parent={}; sibling_candidates={}",
        fallback.requested, fallback.fallback_label, candidates
    )
}

fn format_path_missing_hint(command: &str, fallback: &PathFallback) -> String {
    let candidates = if fallback.candidates.is_empty() {
        "(no siblings)".to_string()
    } else {
        fallback.candidates.join(", ")
    };
    format!(
        "Hint: requested path missing for {command}; requested={}; nearest_existing_parent={}; sibling_candidates={}",
        fallback.requested, fallback.fallback_label, candidates
    )
}

fn prepend_warning(warning: Option<&str>, output: &str) -> String {
    match warning {
        Some(warning) => format!("{warning}\n{output}"),
        None => output.to_string(),
    }
}

// ─── 命令指纹 ─────────────────────────────────────────────

/// 计算单个命令的指纹（用于缓存与重复检测）
pub fn command_fingerprint(cmd: &Value) -> String {
    let Some(kind) = cmd.get("type").and_then(Value::as_str) else {
        return String::new();
    };
    let canonical_strings = |key: &str| -> String {
        cmd.get(key)
            .and_then(Value::as_array)
            .map(|arr| {
                let mut v: Vec<String> = arr
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect();
                v.sort();
                v.join(",")
            })
            .unwrap_or_default()
    };
    match kind {
        "rg" => format!(
            "rg|{}|{}|{}|{}",
            cmd.get("pattern").and_then(Value::as_str).unwrap_or(""),
            cmd.get("path").and_then(Value::as_str).unwrap_or(""),
            canonical_strings("include"),
            canonical_strings("exclude"),
        ),
        "readfile" => format!(
            "readfile|{}|{:?}|{:?}",
            cmd.get("file").and_then(Value::as_str).unwrap_or(""),
            cmd.get("start_line").and_then(Value::as_u64),
            cmd.get("end_line").and_then(Value::as_u64),
        ),
        "tree" => format!(
            "tree|{}|{:?}",
            cmd.get("path").and_then(Value::as_str).unwrap_or(""),
            cmd.get("levels").and_then(Value::as_u64),
        ),
        "ls" => format!(
            "ls|{}|{}|{}",
            cmd.get("path").and_then(Value::as_str).unwrap_or(""),
            cmd.get("long_format")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            cmd.get("all").and_then(Value::as_bool).unwrap_or(false),
        ),
        "glob" => format!(
            "glob|{}|{}|{}",
            cmd.get("pattern").and_then(Value::as_str).unwrap_or(""),
            cmd.get("path").and_then(Value::as_str).unwrap_or(""),
            cmd.get("type_filter")
                .and_then(Value::as_str)
                .unwrap_or("all"),
        ),
        _ => String::new(),
    }
}

// ─── 递归搜索（Rust 内置 rg 回退 + glob 遍历）────────────

fn collect_glob_matches(
    base: &Path,
    dir: &Path,
    matcher: &globset::GlobMatcher,
    type_filter: &str,
    exclude: &[String],
    ignore_matcher: &Gitignore,
    matches: &mut Vec<PathBuf>,
) {
    if matches.len() >= 100 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|entry| entry.ok()) {
        if matches.len() >= 100 {
            return;
        }
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let rel = path.strip_prefix(base).unwrap_or(&path);
        let name = entry.file_name().to_string_lossy().to_string();
        let rel_slash = normalize_path(rel);
        if matches_exclude(&name, exclude)
            || matches_exclude(&rel_slash, exclude)
            || is_ignored_path(ignore_matcher, &path)
        {
            continue;
        }
        let matched = matcher.is_match(rel) || matcher.is_match(&name);
        if matched && matches_type(type_filter, metadata.is_file(), metadata.is_dir()) {
            matches.push(path.clone());
        }
        if metadata.is_dir() && !name.starts_with('.') {
            collect_glob_matches(
                base,
                &path,
                matcher,
                type_filter,
                exclude,
                ignore_matcher,
                matches,
            );
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
        let Ok(content) = fs::read_to_string(path) else {
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

    let Ok(entries) = fs::read_dir(path) else {
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
        if entry_path.is_dir() {
            collect_rg_matches(
                root,
                &entry_path,
                regex,
                include,
                exclude,
                ignore_matcher,
                matches,
            );
        } else {
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

// ─── ToolExecutor ─────────────────────────────────────────

#[derive(Default)]
struct ToolExecutorState {
    /// rg pattern 收集（向外返回）
    collected_rg_patterns: Vec<String>,
    /// 命令指纹 → 输出缓存：跨 turn 命中可零成本返回，节省 LLM 重复探查的时间
    command_cache: HashMap<String, String>,
    /// readfile 完整文件缓存：(规范化绝对路径 → 文件原文)
    /// 供格式化层复用；同一文件的多个 readfile 也共享一次磁盘 IO
    read_cache: HashMap<String, String>,
    /// 本次搜索的本地命令统计，用于输出命中率和诊断 LLM 工具调用质量
    stats: SearchStats,
    /// 上一次 turn 的命令指纹集合，用于重复检测
    last_turn_fingerprints: HashSet<String>,
}

#[derive(Debug)]
pub struct BatchExecution {
    pub output: String,
    pub stats: SearchStats,
}

#[derive(Debug)]
struct CommandExecution {
    output: String,
    stats: SearchStats,
}

#[derive(Debug, Clone)]
struct PathFallback {
    requested: String,
    fallback_path: PathBuf,
    fallback_label: String,
    candidates: Vec<String>,
}

#[derive(Debug)]
enum PreparedCommand {
    Valid { command: Value, repaired: bool },
    Invalid { message: String },
}

pub struct ToolExecutor {
    root: PathBuf,
    root_slash: String,
    exclude_paths: Vec<String>,
    pub ignore_matcher: Gitignore,
    ignore_files: Vec<PathBuf>,
    state: Mutex<ToolExecutorState>,
}

impl ToolExecutor {
    pub fn new(root: PathBuf, exclude_paths: Vec<String>, ignore_matcher: Gitignore) -> Self {
        // canonicalize 消除符号链接差异（如 macOS 上 /var → /private/var），
        // 否则 real_path 的 starts_with 校验会误判路径在 root 之外。
        let root = root.canonicalize().unwrap_or(root);
        let root_slash = normalize_path(&root);
        let ignore_files = fast_context_ignore_files(&root);
        Self {
            root,
            root_slash,
            exclude_paths,
            ignore_matcher,
            ignore_files,
            state: Mutex::new(ToolExecutorState::default()),
        }
    }

    pub async fn collected_rg_patterns(&self) -> Vec<String> {
        let state = self.state.lock().await;
        let mut seen = HashSet::new();
        state
            .collected_rg_patterns
            .iter()
            .filter(|pattern| seen.insert((*pattern).clone()))
            .cloned()
            .collect()
    }

    /// 暴露 readfile 内容快照，供格式化层复用，避免重复读盘
    pub async fn snapshot_read_cache(&self) -> HashMap<String, String> {
        self.state.lock().await.read_cache.clone()
    }

    pub async fn snapshot_stats(&self) -> SearchStats {
        self.state.lock().await.stats.clone()
    }

    /// 统计当前 args 中与上一次相同的命令指纹数量
    pub async fn count_repeat_commands(&self, args: &Value) -> usize {
        let state = self.state.lock().await;
        let mut dup = 0usize;
        if let Some(obj) = args.as_object() {
            for (key, value) in obj {
                if !key.starts_with("command") {
                    continue;
                }
                if let PreparedCommand::Valid { command, .. } = self.prepare_command(value) {
                    let fp = command_fingerprint(&command);
                    if !fp.is_empty() && state.last_turn_fingerprints.contains(&fp) {
                        dup += 1;
                    }
                }
            }
        }
        dup
    }

    /// 并发执行一次 restricted_exec 中的所有子命令，返回拼接结果与本轮统计。
    /// 接收 Arc<Self> 是为了在 join_all 里把同一个 executor 复制给多个并发 future。
    pub async fn exec_tool_call(self_arc: Arc<Self>, args: &Value) -> BatchExecution {
        let Some(obj) = args.as_object() else {
            log::warn!("[fast-context] restricted_exec 参数缺失或格式错误");
            let stats = SearchStats {
                commands_seen: 1,
                commands_invalid: 1,
                ..SearchStats::default()
            };
            return BatchExecution {
                output: "Error: missing or invalid tool args".to_string(),
                stats,
            };
        };
        let mut keys = obj
            .keys()
            .filter(|key| key.starts_with("command"))
            .cloned()
            .collect::<Vec<_>>();
        keys.sort();

        log::info!(
            "[fast-context] restricted_exec 开始并发执行本地命令: count={}",
            keys.len()
        );

        // 记录本轮有效命令指纹（覆盖上一轮），用于下一轮的重复检测。
        let mut current_fps = HashSet::new();
        for key in &keys {
            if let PreparedCommand::Valid { command, .. } = self_arc.prepare_command(&obj[key]) {
                let fp = command_fingerprint(&command);
                if !fp.is_empty() {
                    current_fps.insert(fp);
                }
            }
        }

        // 并发：每条命令一个 future
        let futures = keys.iter().map(|key| {
            let executor = self_arc.clone();
            let key_owned = key.clone();
            let cmd = obj[key].clone();
            async move {
                let started_at = Instant::now();
                let execution = executor.exec_command(&cmd).await;
                log::info!(
                    "[fast-context] restricted_exec 本地命令完成: key={}, output_len={}, elapsed_ms={}",
                    key_owned,
                    execution.output.len(),
                    started_at.elapsed().as_millis()
                );
                (
                    format!(
                        "<{key_owned}_result>\n{}\n</{key_owned}_result>",
                        execution.output
                    ),
                    execution.stats,
                )
            }
        });
        let executions: Vec<(String, SearchStats)> = join_all(futures).await;
        let mut batch_stats = SearchStats::default();
        let mut parts = Vec::with_capacity(executions.len());
        for (output, stats) in executions {
            batch_stats.merge(&stats);
            parts.push(output);
        }

        // 更新最后一轮指纹（不影响当轮 dup_count，因为 dup_count 在 exec 之前已检测）
        {
            let mut state = self_arc.state.lock().await;
            state.last_turn_fingerprints = current_fps;
            state.stats.merge(&batch_stats);
        }

        BatchExecution {
            output: parts.join(""),
            stats: batch_stats,
        }
    }

    async fn exec_command(&self, raw_cmd: &Value) -> CommandExecution {
        let mut stats = SearchStats {
            commands_seen: 1,
            ..SearchStats::default()
        };
        let (cmd, repaired) = match self.prepare_command(raw_cmd) {
            PreparedCommand::Valid { command, repaired } => (command, repaired),
            PreparedCommand::Invalid { message } => {
                log::warn!("[fast-context] 本地命令无效: {}, raw={}", message, raw_cmd);
                stats.commands_invalid = 1;
                stats.error_outputs = 1;
                return CommandExecution {
                    output: format!("Error: invalid command: {message}"),
                    stats,
                };
            }
        };
        if repaired {
            stats.commands_repaired = 1;
        };
        stats.commands_executed = 1;

        // 命令缓存：相同指纹直接复用结果（跨 turn 都生效）
        let fp = command_fingerprint(&cmd);
        let kind = cmd.get("type").and_then(Value::as_str).unwrap_or_default();
        if !fp.is_empty() {
            let state = self.state.lock().await;
            if let Some(cached) = state.command_cache.get(&fp) {
                log::info!(
                    "[fast-context] 命令缓存命中: kind={}, fp_len={}, output_len={}",
                    kind,
                    fp.len(),
                    cached.len()
                );
                stats.cache_hits = 1;
                classify_output(cached, &mut stats);
                return CommandExecution {
                    output: cached.clone(),
                    stats,
                };
            }
        }

        let output = match kind {
            "rg" => {
                let pattern = cmd.get("pattern").and_then(Value::as_str).unwrap_or("");
                let path = cmd.get("path").and_then(Value::as_str).unwrap_or("");
                let include = string_array(cmd.get("include"));
                let exclude = self.merge_excludes(string_array(cmd.get("exclude")));
                log::info!(
                    "[fast-context] 本地命令 rg: path={}, pattern_len={}, include_count={}, exclude_count={}",
                    path,
                    pattern.chars().count(),
                    include.len(),
                    exclude.len()
                );
                self.rg(pattern, path, include, exclude).await
            }
            "readfile" => {
                let file = cmd.get("file").and_then(Value::as_str).unwrap_or("");
                let start = cmd
                    .get("start_line")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize);
                let end = cmd
                    .get("end_line")
                    .and_then(Value::as_u64)
                    .map(|v| v as usize);
                log::info!(
                    "[fast-context] 本地命令 readfile: file={}, start_line={:?}, end_line={:?}",
                    file,
                    start,
                    end
                );
                self.readfile(file, start, end).await
            }
            "tree" => {
                let path = cmd.get("path").and_then(Value::as_str).unwrap_or("");
                let levels = cmd.get("levels").and_then(Value::as_u64).map(|v| v as u8);
                log::info!(
                    "[fast-context] 本地命令 tree: path={}, levels={:?}",
                    path,
                    levels
                );
                self.tree(path, levels)
            }
            "ls" => {
                let path = cmd.get("path").and_then(Value::as_str).unwrap_or("");
                let long_format = cmd
                    .get("long_format")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let all = cmd.get("all").and_then(Value::as_bool).unwrap_or(false);
                log::info!(
                    "[fast-context] 本地命令 ls: path={}, long_format={}, all={}",
                    path,
                    long_format,
                    all
                );
                self.ls(path, long_format, all)
            }
            "glob" => {
                let pattern = cmd.get("pattern").and_then(Value::as_str).unwrap_or("");
                let path = cmd.get("path").and_then(Value::as_str).unwrap_or("");
                let type_filter = cmd
                    .get("type_filter")
                    .and_then(Value::as_str)
                    .unwrap_or("all");
                log::info!(
                    "[fast-context] 本地命令 glob: path={}, pattern={}, type_filter={}",
                    path,
                    pattern,
                    type_filter
                );
                self.glob(pattern, path, type_filter)
            }
            other => {
                log::warn!("[fast-context] 未知本地命令类型: {}", other);
                format!("Error: unknown command type '{other}'")
            }
        };

        classify_output(&output, &mut stats);

        // 只缓存有用输出，避免空 pattern / 路径不存在这类错误被后续误判为缓存命中。
        if !fp.is_empty() && is_useful_output(&output) {
            let mut state = self.state.lock().await;
            state.command_cache.insert(fp, output.clone());
        }
        CommandExecution { output, stats }
    }

    fn prepare_command(&self, raw_cmd: &Value) -> PreparedCommand {
        let (cmd, repaired) = match normalize_command_shape(raw_cmd) {
            Some(normalized) => normalized,
            None => {
                return PreparedCommand::Invalid {
                    message: "missing command type".to_string(),
                };
            }
        };

        let Some(kind) = cmd.get("type").and_then(Value::as_str) else {
            return PreparedCommand::Invalid {
                message: "missing command type".to_string(),
            };
        };

        let error = match kind {
            "rg" => {
                let pattern = cmd.get("pattern").and_then(Value::as_str).unwrap_or("");
                let path = cmd.get("path").and_then(Value::as_str).unwrap_or("");
                if pattern.trim().is_empty() {
                    Some("rg.pattern is required")
                } else if !self.is_safe_virtual_path(path) {
                    Some("rg.path is missing or outside /codebase")
                } else {
                    None
                }
            }
            "readfile" => {
                let file = cmd.get("file").and_then(Value::as_str).unwrap_or("");
                if !self.is_safe_virtual_path(file) {
                    Some("readfile.file is missing or outside /codebase")
                } else {
                    None
                }
            }
            "tree" | "ls" => {
                let path = cmd.get("path").and_then(Value::as_str).unwrap_or("");
                if !self.is_safe_virtual_path(path) {
                    Some("path is missing or outside /codebase")
                } else {
                    None
                }
            }
            "glob" => {
                let pattern = cmd.get("pattern").and_then(Value::as_str).unwrap_or("");
                let path = cmd.get("path").and_then(Value::as_str).unwrap_or("");
                if pattern.trim().is_empty() {
                    Some("glob.pattern is required")
                } else if !self.is_safe_virtual_path(path) {
                    Some("glob.path is missing or outside /codebase")
                } else {
                    None
                }
            }
            _ => Some("unsupported command type"),
        };

        if let Some(message) = error {
            return PreparedCommand::Invalid {
                message: message.to_string(),
            };
        }

        PreparedCommand::Valid {
            command: cmd,
            repaired,
        }
    }

    fn merge_excludes(&self, command_excludes: Vec<String>) -> Vec<String> {
        let mut seen = HashSet::new();
        self.exclude_paths
            .iter()
            .chain(command_excludes.iter())
            .filter_map(|pattern| {
                let trimmed = pattern.trim();
                if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
            .collect()
    }

    fn is_safe_virtual_path(&self, value: &str) -> bool {
        self.real_path(value).is_ok()
    }

    fn path_fallback(&self, requested: &str) -> Option<PathFallback> {
        let missing = self.real_path(requested).ok()?;
        if missing.exists() {
            return None;
        }
        let mut parent = missing.parent();
        while let Some(candidate) = parent {
            if candidate.exists() && candidate.is_dir() && candidate.starts_with(&self.root) {
                let candidates = self.path_candidates(candidate);
                return Some(PathFallback {
                    requested: normalize_virtual_path(requested),
                    fallback_path: candidate.to_path_buf(),
                    fallback_label: self.remap(&normalize_path(candidate)),
                    candidates,
                });
            }
            parent = candidate.parent();
        }
        None
    }

    fn path_candidates(&self, dir: &Path) -> Vec<String> {
        let mut entries = sorted_entries(dir, &self.exclude_paths, &self.ignore_matcher)
            .into_iter()
            .take(8)
            .map(|entry| {
                let path = entry.path();
                let suffix = if path.is_dir() { "/" } else { "" };
                format!("{}{}", self.remap(&normalize_path(&path)), suffix)
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }

    fn path_missing_message(&self, command: &str, requested: &str, prefix: &str) -> String {
        if let Some(fallback) = self.path_fallback(requested) {
            format!(
                "{prefix}: {requested}\n{}",
                format_path_missing_hint(command, &fallback)
            )
        } else {
            format!("{prefix}: {requested}")
        }
    }

    // ─── rg ──────────────────────────────────────────────

    async fn rg(
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

    // ─── readfile ────────────────────────────────────────

    async fn readfile(
        &self,
        file: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> String {
        let Ok(path) = self.real_path(file) else {
            log::warn!("[fast-context] readfile 文件无法映射: {}", file);
            return self.path_missing_message("readfile", file, "Error: file not found");
        };
        if !path.is_file() {
            log::warn!("[fast-context] readfile 文件不存在: {}", path.display());
            return self.path_missing_message("readfile", file, "Error: file not found");
        }
        if is_ignored_path(&self.ignore_matcher, &path) {
            log::warn!(
                "[fast-context] readfile 已拒绝 ignore 文件: {}",
                path.display()
            );
            return format!("Error: file is ignored: {file}");
        }
        let key = normalize_path(&path);
        // 读文件缓存：同一 path 全量内容仅读盘一次，多次 readfile（不同 range）零额外 IO
        let content = {
            let state = self.state.lock().await;
            state.read_cache.get(&key).cloned()
        };
        let content = match content {
            Some(c) => c,
            None => match fs::read_to_string(&path) {
                Ok(c) => {
                    let mut state = self.state.lock().await;
                    state.read_cache.insert(key.clone(), c.clone());
                    c
                }
                Err(err) => {
                    log::warn!(
                        "[fast-context] readfile 读取失败: path={}, error={}",
                        path.display(),
                        err
                    );
                    return format!("Error: {err}");
                }
            },
        };
        let start = start_line.unwrap_or(1).max(1);
        let end = end_line.unwrap_or_else(|| content.lines().count()).max(start);
        let output = content
            .lines()
            .enumerate()
            .filter_map(|(idx, line)| {
                let line_no = idx + 1;
                (line_no >= start && line_no <= end).then(|| format!("{line_no}:{line}"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        let result = truncate_output(&output);
        log::info!(
            "[fast-context] readfile 完成: path={}, range={}-{}, output_len={}",
            path.display(),
            start,
            end,
            result.len()
        );
        result
    }

    // ─── tree / ls / glob ────────────────────────────────

    fn tree(&self, path: &str, levels: Option<u8>) -> String {
        let Ok(real_path) = self.real_path(path) else {
            log::warn!("[fast-context] tree 目录无法映射: {}", path);
            return self.path_missing_message("tree", path, "Error: dir not found");
        };
        if !real_path.is_dir() {
            log::warn!("[fast-context] tree 目录不存在: {}", real_path.display());
            return self.path_missing_message("tree", path, "Error: dir not found");
        }
        if is_ignored_path(&self.ignore_matcher, &real_path) {
            return format!("Error: path is ignored: {path}");
        }
        let label = self.virtual_label(path);
        let result = truncate_output(&self.remap(&build_tree(
            &real_path,
            &label,
            levels.unwrap_or(3).clamp(1, 6),
            &self.exclude_paths,
            &self.ignore_matcher,
        )));
        log::info!(
            "[fast-context] tree 完成: path={}, output_len={}",
            real_path.display(),
            result.len()
        );
        result
    }

    fn ls(&self, path: &str, long_format: bool, all: bool) -> String {
        let Ok(real_path) = self.real_path(path) else {
            log::warn!("[fast-context] ls 目录无法映射: {}", path);
            return self.path_missing_message("ls", path, "Error: dir not found");
        };
        if !real_path.is_dir() {
            log::warn!("[fast-context] ls 不是目录: {}", real_path.display());
            return self.path_missing_message("ls", path, "Error: not a directory");
        }
        if is_ignored_path(&self.ignore_matcher, &real_path) {
            return format!("Error: path is ignored: {path}");
        }
        let mut entries = match fs::read_dir(&real_path) {
            Ok(entries) => entries.filter_map(|entry| entry.ok()).collect::<Vec<_>>(),
            Err(err) => {
                log::warn!(
                    "[fast-context] ls 读取目录失败: path={}, error={}",
                    real_path.display(),
                    err
                );
                return format!("Error: {err}");
            }
        };
        entries.retain(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            !matches_exclude(&name, &self.exclude_paths)
                && !is_ignored_path(&self.ignore_matcher, &entry.path())
        });
        entries.sort_by_key(|entry| entry.file_name().to_string_lossy().to_ascii_lowercase());
        if !all {
            entries.retain(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
        }
        if !long_format {
            let result = truncate_output(
                &entries
                    .iter()
                    .map(|entry| entry.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            log::info!(
                "[fast-context] ls 完成: path={}, entries={}, output_len={}",
                real_path.display(),
                entries.len(),
                result.len()
            );
            return result;
        }

        let mut lines = vec![format!("total {}", entries.len())];
        for entry in entries {
            let metadata = entry.metadata().ok();
            let kind = if metadata.as_ref().is_some_and(|m| m.is_dir()) {
                "d"
            } else {
                "-"
            };
            let size = metadata.map(|m| m.len()).unwrap_or(0);
            lines.push(format!(
                "{kind}rwxr-xr-x  1 user staff {size:>8} {}",
                entry.file_name().to_string_lossy()
            ));
        }
        let result = truncate_output(&lines.join("\n"));
        log::info!(
            "[fast-context] ls 长格式完成: path={}, output_len={}",
            real_path.display(),
            result.len()
        );
        result
    }

    fn glob(&self, pattern: &str, path: &str, type_filter: &str) -> String {
        if pattern.trim().is_empty() {
            log::warn!("[fast-context] glob 缺少 pattern");
            return "Error: missing or invalid pattern".to_string();
        }
        let Ok(root) = self.real_path(path) else {
            log::warn!("[fast-context] glob 路径无法映射: {}", path);
            return format!("Error: path does not exist: {path}");
        };
        let (root, path_warning) = if root.exists() {
            (root, None)
        } else if let Some(fallback) = self.path_fallback(path) {
            log::warn!(
                "[fast-context] glob 路径不存在，已回退到最近存在父目录: requested={}, fallback={}",
                path,
                fallback.fallback_label
            );
            let warning = format_path_fallback_warning("glob", &fallback);
            (fallback.fallback_path, Some(warning))
        } else {
            log::warn!("[fast-context] glob 路径不存在: {}", root.display());
            return format!("Error: path does not exist: {path}");
        };
        if is_ignored_path(&self.ignore_matcher, &root) {
            return format!("Error: path is ignored: {path}");
        }
        let matcher = match GlobBuilder::new(pattern).literal_separator(true).build() {
            Ok(glob) => glob.compile_matcher(),
            Err(err) => {
                log::warn!("[fast-context] glob 表达式无效: {}", err);
                return format!("Error: invalid glob: {err}");
            }
        };
        let mut matches = Vec::new();
        collect_glob_matches(
            &root,
            &root,
            &matcher,
            type_filter,
            &self.exclude_paths,
            &self.ignore_matcher,
            &mut matches,
        );
        matches.sort();
        matches.truncate(100);
        if matches.is_empty() {
            log::info!("[fast-context] glob 无匹配: pattern={}", pattern);
            prepend_warning(path_warning.as_deref(), "(no matches)")
        } else {
            let result = self.remap(
                &matches
                    .iter()
                    .map(|path| normalize_path(path))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            log::info!(
                "[fast-context] glob 完成: matches={}, output_len={}",
                matches.len(),
                result.len()
            );
            prepend_warning(path_warning.as_deref(), &result)
        }
    }

    // ─── 路径映射 ────────────────────────────────────────

    fn real_path(&self, value: &str) -> std::result::Result<PathBuf, ()> {
        if value.trim().is_empty() {
            return Err(());
        }
        let normalized = value.trim().replace('\\', "/");
        let candidate = if normalized.starts_with("/codebase") {
            let rel = normalized
                .trim_start_matches("/codebase")
                .trim_start_matches('/');
            let rel_path = Path::new(rel);
            if has_parent_dir(rel_path) {
                return Err(());
            }
            self.root.join(rel_path)
        } else {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                path
            } else {
                if has_parent_dir(&path) {
                    return Err(());
                }
                self.root.join(path)
            }
        };

        let absolute = candidate.canonicalize().unwrap_or(candidate);
        if absolute.starts_with(&self.root) {
            Ok(absolute)
        } else {
            Err(())
        }
    }

    fn remap(&self, text: &str) -> String {
        text.replace(&self.root_slash, "/codebase")
            .replace(&self.root.to_string_lossy().to_string(), "/codebase")
            .replace('\\', "/")
    }

    fn virtual_label(&self, path: &str) -> String {
        if path.trim().is_empty() {
            return "/codebase".to_string();
        }
        self.remap(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_fingerprint_is_stable_and_order_insensitive() {
        let a = serde_json::json!({"type":"rg","pattern":"Foo","path":"/codebase/src","include":["**/*.rs","**/*.py"],"exclude":["**/test/**"]});
        let b = serde_json::json!({"type":"rg","pattern":"Foo","path":"/codebase/src","include":["**/*.py","**/*.rs"],"exclude":["**/test/**"]});
        assert_eq!(command_fingerprint(&a), command_fingerprint(&b));
        let c = serde_json::json!({"type":"rg","pattern":"Bar","path":"/codebase/src","include":["**/*.rs","**/*.py"],"exclude":["**/test/**"]});
        assert_ne!(command_fingerprint(&a), command_fingerprint(&c));
        assert!(!command_fingerprint(&serde_json::json!({})).is_empty() == false);
    }

    #[test]
    fn normalize_command_shape_handles_flattened_shapes() {
        // 标准形式不修复
        let (cmd, repaired) =
            normalize_command_shape(&serde_json::json!({"type":"rg","pattern":"x","path":"/codebase"})).unwrap();
        assert!(!repaired);
        assert_eq!(cmd["type"], "rg");

        // shorthand：{"readfile": "/codebase/a.rs", "start_line": 1}
        let (cmd, repaired) = normalize_command_shape(&serde_json::json!({"readfile":"/codebase/a.rs","start_line":1})).unwrap();
        assert!(repaired);
        assert_eq!(cmd["type"], "readfile");
        assert_eq!(cmd["file"], "/codebase/a.rs");
        assert_eq!(cmd["start_line"], 1);

        // 扁平参数：{"rg":"pattern","path":"/codebase"}
        let (cmd, repaired) = normalize_command_shape(&serde_json::json!({"rg":"Foo","path":"/codebase"})).unwrap();
        assert!(repaired);
        assert_eq!(cmd["type"], "rg");
        assert_eq!(cmd["pattern"], "Foo");

        // 扁平参数：{"file":"/codebase/a.rs"}
        let (cmd, repaired) = normalize_command_shape(&serde_json::json!({"file":"/codebase/a.rs"})).unwrap();
        assert!(repaired);
        assert_eq!(cmd["type"], "readfile");
    }

    #[test]
    fn count_valid_commands_rejects_empty_and_accepts_repairable() {
        let args = serde_json::json!({
            "command1": {"type":"rg","pattern":"","path":"/codebase"},
            "command2": {"readfile":"/codebase/a.rs","start_line":1},
            "command3": {"type":"tree","path":"/codebase"},
            "command4": {"type":"glob","pattern":"**/*.rs","path":"/codebase"}
        });
        assert_eq!(count_valid_commands(&args), 3); // command1 空 pattern 无效
    }

    #[test]
    fn count_valid_commands_accepts_observed_flattened_shapes() {
        let args = serde_json::json!({
            "command1": {"rg":"SouTool","path":"/codebase/src"},
            "command2": {"file":"/codebase/src/lib.rs","start_line":2},
            "command3": {"type":"readfile","file":"/codebase/src/main.rs"}
        });
        assert_eq!(count_valid_commands(&args), 3);
    }

    fn tmp_root() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("fc-test-{}-{}", std::process::id(), n));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn hello() {}\n// auth logic here\npub fn world() {}\n").unwrap();
        fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.join("README.md"), "docs\n").unwrap();
        dir
    }

    #[tokio::test]
    async fn tool_executor_repairs_readfile_shorthand_and_tracks_stats() {
        let root = tmp_root();
        let ignore = build_fast_context_ignore(&root);
        let executor = Arc::new(ToolExecutor::new(root.clone(), Vec::new(), ignore));
        let args = serde_json::json!({
            "command1": {"readfile":"/codebase/src/lib.rs","start_line":1,"end_line":2},
            "command2": {"type":"rg","pattern":"auth","path":"src"}
        });
        let batch = ToolExecutor::exec_tool_call(executor.clone(), &args).await;
        assert!(batch.output.contains("1:pub fn hello()"));
        assert!(batch.output.contains("auth logic"));
        assert!(batch.stats.commands_repaired >= 1, "shorthand readfile 应被标记为修复");
        assert!(batch.stats.commands_useful >= 1);
        let cache = executor.snapshot_read_cache().await;
        assert!(cache.values().any(|v| v.contains("hello")), "readfile 缓存应含文件内容");
    }

    #[tokio::test]
    async fn invalid_commands_are_not_executed_or_cached() {
        let root = tmp_root();
        let ignore = build_fast_context_ignore(&root);
        let executor = Arc::new(ToolExecutor::new(root.clone(), Vec::new(), ignore));
        let args = serde_json::json!({
            "command1": {"type":"rg","pattern":"","path":"src"},
            "command2": {"type":"tree","path":""}
        });
        let batch = ToolExecutor::exec_tool_call(executor.clone(), &args).await;
        assert!(batch.output.contains("Error: invalid command"));
        assert!(batch.stats.commands_invalid >= 1);
        let stats = executor.snapshot_stats().await;
        assert!(stats.commands_invalid >= 1);
    }

    #[tokio::test]
    async fn missing_rg_path_falls_back_to_nearest_parent() {
        let root = tmp_root();
        let ignore = build_fast_context_ignore(&root);
        let executor = Arc::new(ToolExecutor::new(root.clone(), Vec::new(), ignore));
        // 不存在的深层路径 → 回退到最近存在的父目录 src
        let args = serde_json::json!({
            "command1": {"type":"rg","pattern":"auth","path":"/codebase/src/nonexistent/deep"}
        });
        let batch = ToolExecutor::exec_tool_call(executor.clone(), &args).await;
        assert!(
            batch.output.contains("Warning: requested path missing"),
            "路径缺失应输出回退警告: {}",
            batch.output
        );
        assert!(batch.stats.path_repaired >= 1);
    }

    #[tokio::test]
    async fn missing_readfile_path_reports_candidates_without_repairing() {
        let root = tmp_root();
        let ignore = build_fast_context_ignore(&root);
        let executor = Arc::new(ToolExecutor::new(root.clone(), Vec::new(), ignore));
        let args = serde_json::json!({
            "command1": {"type":"readfile","file":"/codebase/src/payment.rs"}
        });
        let batch = ToolExecutor::exec_tool_call(executor.clone(), &args).await;
        assert!(
            batch.output.contains("Hint: requested path missing"),
            "readfile 缺失应输出候选提示: {}",
            batch.output
        );
        assert!(batch.stats.path_missing >= 1);
        assert_eq!(batch.stats.path_repaired, 0, "readfile 不应自动修复路径");
    }

    #[tokio::test]
    async fn tool_executor_caches_repeated_command() {
        let root = tmp_root();
        let ignore = build_fast_context_ignore(&root);
        let executor = Arc::new(ToolExecutor::new(root.clone(), Vec::new(), ignore));
        let args = serde_json::json!({
            "command1": {"type":"rg","pattern":"auth","path":"src"}
        });
        let first = ToolExecutor::exec_tool_call(executor.clone(), &args).await;
        let second = ToolExecutor::exec_tool_call(executor.clone(), &args).await;
        assert_eq!(first.output, second.output, "相同命令应命中缓存，输出一致");
        assert!(second.stats.cache_hits >= 1, "第二次执行应命中命令缓存");
    }

    #[tokio::test]
    async fn count_repeat_commands_detects_cross_turn_duplicates() {
        let root = tmp_root();
        let ignore = build_fast_context_ignore(&root);
        let executor = Arc::new(ToolExecutor::new(root.clone(), Vec::new(), ignore));
        let args = serde_json::json!({
            "command1": {"type":"rg","pattern":"auth","path":"src"}
        });
        ToolExecutor::exec_tool_call(executor.clone(), &args).await;
        let dup = executor.count_repeat_commands(&args).await;
        assert_eq!(dup, 1, "第二轮的相同命令应被识别为重复");
    }
}
