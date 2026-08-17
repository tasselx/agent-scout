//! 路径 / ignore / 目录树等文件系统辅助，供各命令实现与 repo map 共用。

use std::fs;
use std::path::{Component, Path, PathBuf};

use globset::GlobBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};

pub(crate) const RESULT_MAX_LINES: usize = 50;
const LINE_MAX_CHARS: usize = 250;

/// 需要原生加载的 ignore 文件（rg 与 Rust 回退搜索都会读取）。
pub const FAST_CONTEXT_IGNORE_FILES: [&str; 4] = [
    ".gitignore",
    ".codeiumignore",
    ".windsurfignore",
    ".devinignore",
];

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

pub fn string_array(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
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

pub(crate) fn sorted_entries(
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
