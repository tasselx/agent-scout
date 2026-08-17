//! 本地工具执行器。
//!
//! 执行 Devstral 模型生成的 restricted_exec 命令（rg/readfile/tree/ls/glob）。
//! 按命令类型拆到子模块：`rg` / `readfile` / `listing`（tree+ls+glob）。

mod command;
mod fs;
mod listing;
mod readfile;
mod rg;

pub use command::{
    command_fingerprint, count_valid_commands, is_structurally_valid_command, normalize_command_shape,
};
pub use fs::{
    build_fast_context_ignore, build_tree, fast_context_ignore_files, glob_match, has_parent_dir,
    is_ignored_path, list_root, matches_exclude, matches_type, normalize_path, string_array,
    truncate_output, FAST_CONTEXT_IGNORE_FILES,
};

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use futures_util::future::join_all;
use ignore::gitignore::Gitignore;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::fastcontext::SearchStats;
use fs::{fast_context_ignore_files as ignore_files_for, sorted_entries};

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

// ─── ToolExecutor ─────────────────────────────────────────

#[derive(Default)]
struct ToolExecutorState {
    /// rg pattern 收集（向外返回）
    collected_rg_patterns: Vec<String>,
    /// 命令指纹 → 输出缓存：跨 turn 命中可零成本返回，节省 LLM 重复探查的时间
    command_cache: HashMap<String, String>,
    /// readfile 完整文件缓存：(规范化绝对路径 → 文件原文)
    read_cache: HashMap<String, String>,
    /// 本次搜索的本地命令统计
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
        let ignore_files = ignore_files_for(&root);
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

        let mut current_fps = HashSet::new();
        for key in &keys {
            if let PreparedCommand::Valid { command, .. } = self_arc.prepare_command(&obj[key]) {
                let fp = command_fingerprint(&command);
                if !fp.is_empty() {
                    current_fps.insert(fp);
                }
            }
        }

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
    use std::fs;
    use std::sync::Arc;

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
        let (cmd, repaired) =
            normalize_command_shape(&serde_json::json!({"type":"rg","pattern":"x","path":"/codebase"})).unwrap();
        assert!(!repaired);
        assert_eq!(cmd["type"], "rg");

        let (cmd, repaired) = normalize_command_shape(&serde_json::json!({"readfile":"/codebase/a.rs","start_line":1})).unwrap();
        assert!(repaired);
        assert_eq!(cmd["type"], "readfile");
        assert_eq!(cmd["file"], "/codebase/a.rs");
        assert_eq!(cmd["start_line"], 1);

        let (cmd, repaired) = normalize_command_shape(&serde_json::json!({"rg":"Foo","path":"/codebase"})).unwrap();
        assert!(repaired);
        assert_eq!(cmd["type"], "rg");
        assert_eq!(cmd["pattern"], "Foo");

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
        assert_eq!(count_valid_commands(&args), 3);
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
