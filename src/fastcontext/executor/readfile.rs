//! readfile 命令：带内容缓存，同一文件多次按行范围读取只打一次盘。

use std::fs;

use super::fs::{is_ignored_path, normalize_path, truncate_output};
use super::ToolExecutor;

impl ToolExecutor {
    pub(super) async fn readfile(
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
}
