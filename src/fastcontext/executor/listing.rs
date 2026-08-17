//! tree / ls / glob：目录列举类命令放一起，避免三个只有几十行的文件。

use std::fs;
use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use ignore::gitignore::Gitignore;

use super::fs::{
    build_tree, is_ignored_path, matches_exclude, matches_type, normalize_path, truncate_output,
};
use super::{format_path_fallback_warning, prepend_warning, ToolExecutor};

impl ToolExecutor {
    pub(super) fn tree(&self, path: &str, levels: Option<u8>) -> String {
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

    pub(super) fn ls(&self, path: &str, long_format: bool, all: bool) -> String {
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

    pub(super) fn glob(&self, pattern: &str, path: &str, type_filter: &str) -> String {
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
}

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
