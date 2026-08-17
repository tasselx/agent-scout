//! 命令形状归一化、结构校验与指纹。
//! 从 ToolExecutor 拆出，避免 LLM 畸形参数兼容逻辑和执行器缠在一起。

use serde_json::{Map, Value};

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
