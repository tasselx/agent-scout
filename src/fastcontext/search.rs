//! AI 驱动的语义代码搜索主循环。
//!
//! 主循环：repo map + 项目摘要 → Devstral 多轮 tool-call/result 交换 →
//! 本地命令执行 → `<ANSWER>` XML 解析为文件路径 + 行号范围。
//! 增强：中文 query 提示、空 answer 自动重试、未解析响应补偿重试、
//! 上下文裁剪重试、命令统计与诊断。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use ignore::gitignore::Gitignore;
use regex::Regex;
use serde_json::{json, Value};

use crate::fastcontext::executor::{
    build_fast_context_ignore, build_tree, count_valid_commands, list_root, normalize_path,
    is_ignored_path, ToolExecutor,
};
use crate::fastcontext::http::{build_request, check_rate_limit, fetch_jwt, streaming_request};
use crate::fastcontext::proto::{connect_frame_decode, extract_strings};
use crate::fastcontext::{
    ChatMessage, SearchOptions, SearchResult, FastContextFile, SearchStats, detect_api_key,
    mask_api_key,
};

const MAX_TREE_BYTES: usize = 250 * 1024;
const FINAL_FORCE_ANSWER: &str =
    "You have no turns left. Now you MUST provide your final ANSWER, even if it's not complete.";
const MAX_COMPENSATIONS: usize = 2;

// ─── 内部类型 ─────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RepoMap {
    tree: String,
    depth: u8,
    size_bytes: usize,
    fell_back: bool,
}

#[derive(Debug)]
struct ParsedToolCall {
    thinking: String,
    name: String,
    args: Value,
}

/// 计算字符串中 CJK 汉字字符占比（用于触发中文翻译提示）
fn chinese_ratio(text: &str) -> f32 {
    let total = text.chars().count();
    if total == 0 {
        return 0.0;
    }
    let cjk = text
        .chars()
        .filter(|c| {
            let v = *c as u32;
            // CJK 统一表意文字 + CJK 扩展 A + CJK 兼容表意文字
            (0x4E00..=0x9FFF).contains(&v)
                || (0x3400..=0x4DBF).contains(&v)
                || (0xF900..=0xFAFF).contains(&v)
        })
        .count();
    cjk as f32 / total as f32
}

// ─── System prompt ─────────────────────────────────────────

fn build_system_prompt(max_turns: u8, max_commands: u8, max_results: u8) -> String {
    format!(
        r#"You are an expert software engineer providing code context for another engineer.
Return only the files and inclusive line ranges needed to understand and implement the user's request.

Environment:
- Working directory is /codebase.
- Tool-call protocol is text based: call tools by outputting `[TOOL_CALLS]restricted_exec[ARGS]{{...}}` or `[TOOL_CALLS]answer[ARGS]{{...}}` exactly.
- You may use exactly one restricted_exec tool call per search turn.
- Each restricted_exec call may include at most {max_commands} commands.
- **STRONGLY PREFER batching multiple commands within a single restricted_exec call** — they run in parallel locally, so issuing 2–4 commands per turn is dramatically faster than 1 command per turn.
- Available command types: rg, readfile, tree, ls, glob.
- Prefer narrow rg searches first, then read complete semantic blocks.
- Avoid generated, vendored, dependency, build, and cache directories unless directly relevant.
- You have at most {max_turns} search turns before final answer.

Language handling (IMPORTANT):
- If the Problem Statement is not in English (e.g. Chinese, Japanese), first internally translate the user's intent to English. Code identifiers (class/function/file names) in most repositories are English, so search using English keywords.
- When the question uses Chinese terms like "类" / "函数" / "调用链" / "实现"，treat them as English "class" / "function" / "call chain" / "implementation" and search for the corresponding English identifiers in the codebase.
- If the question mentions a domain concept (e.g. "屏幕截图" → "screenshot/capture", "剪贴板" → "clipboard"), translate to English and try multiple synonyms in your rg patterns.

Final answer:
- Use the answer tool by outputting `[TOOL_CALLS]answer[ARGS]{{"answer":"<ANSWER>...</ANSWER>"}}`.
- answer must be XML with root <ANSWER>.
- Use <file path="/codebase/path"><range>start-end</range></file>.
- Aim for at most {max_results} files.
- If nothing relevant exists, return <ANSWER></ANSWER>.
"#
    )
}

fn command_schema(n: u8) -> Value {
    json!({
        "type": "object",
        "description": format!("Command {n} to execute."),
        "oneOf": [
            {
                "properties": {
                    "type": { "type": "string", "const": "rg" },
                    "pattern": { "type": "string" },
                    "path": { "type": "string" },
                    "include": { "type": "array", "items": { "type": "string" } },
                    "exclude": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["type", "pattern", "path"]
            },
            {
                "properties": {
                    "type": { "type": "string", "const": "readfile" },
                    "file": { "type": "string" },
                    "start_line": { "type": "integer" },
                    "end_line": { "type": "integer" }
                },
                "required": ["type", "file"]
            },
            {
                "properties": {
                    "type": { "type": "string", "const": "tree" },
                    "path": { "type": "string" },
                    "levels": { "type": "integer" }
                },
                "required": ["type", "path"]
            },
            {
                "properties": {
                    "type": { "type": "string", "const": "ls" },
                    "path": { "type": "string" },
                    "long_format": { "type": "boolean" },
                    "all": { "type": "boolean" }
                },
                "required": ["type", "path"]
            },
            {
                "properties": {
                    "type": { "type": "string", "const": "glob" },
                    "pattern": { "type": "string" },
                    "path": { "type": "string" },
                    "type_filter": { "type": "string", "enum": ["file", "directory", "all"] }
                },
                "required": ["type", "pattern", "path"]
            }
        ]
    })
}

fn build_tool_definitions(max_commands: u8) -> String {
    let mut props = serde_json::Map::new();
    for i in 1..=max_commands.max(1) {
        props.insert(format!("command{i}"), command_schema(i));
    }

    json!([
        {
            "type": "function",
            "function": {
                "name": "restricted_exec",
                "description": "Execute restricted commands in parallel.",
                "parameters": {
                    "type": "object",
                    "properties": props,
                    "required": ["command1"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "answer",
                "description": "Final answer with relevant files and line ranges.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string", "description": "The final answer in XML format." }
                    },
                    "required": ["answer"]
                }
            }
        }
    ])
    .to_string()
}

// ─── 响应解析 ─────────────────────────────────────────────

fn find_json_object_end(raw: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in raw.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(idx + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_tool_call(text: &str) -> Option<ParsedToolCall> {
    let text = text.replace("</s>", "");
    let marker = "[TOOL_CALLS]";
    let args_marker = "[ARGS]";
    let marker_start = text.find(marker)?;
    let name_start = marker_start + marker.len();
    let args_start_rel = text[name_start..].find(args_marker)?;
    let name = text[name_start..name_start + args_start_rel].trim();
    if name.is_empty() {
        return None;
    }

    let raw = text[name_start + args_start_rel + args_marker.len()..].trim();
    let end = find_json_object_end(raw).unwrap_or(raw.len());
    let args = serde_json::from_str(&raw[..end]).ok()?;
    Some(ParsedToolCall {
        thinking: text[..marker_start].trim().to_string(),
        name: name.to_string(),
        args,
    })
}

/// LLM 可能直接返回顶层 rg/readfile/tree/ls/glob（不带 restricted_exec 包裹），
/// 归一化为 `restricted_exec: { command1: <原参数> }`。
fn normalize_top_level_tool_call(mut call: ParsedToolCall) -> ParsedToolCall {
    if !matches!(
        call.name.as_str(),
        "rg" | "readfile" | "tree" | "ls" | "glob"
    ) {
        return call;
    }

    if let Some(command) = call.args.as_object_mut() {
        command.insert("type".to_string(), Value::String(call.name.clone()));
    }
    call.args = serde_json::json!({ "command1": call.args });
    call.name = "restricted_exec".to_string();
    call
}

fn parse_response(data: &[u8]) -> anyhow::Result<Option<ParsedToolCall>> {
    let mut raw_text = String::new();
    let mut extracted_text = String::new();
    for frame in connect_frame_decode(data) {
        let text_candidate = String::from_utf8_lossy(&frame);
        if text_candidate.starts_with('{') {
            if let Ok(err_obj) = serde_json::from_str::<Value>(&text_candidate) {
                if let Some(error) = err_obj.get("error") {
                    let code = error
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let message = error.get("message").and_then(Value::as_str).unwrap_or("");
                    return Err(anyhow::anyhow!("[Error] {code}: {message}"));
                }
            }
        }

        // 流式响应可能把工具名与 JSON 拆到多个 Connect 帧，必须完整收集后再解析。
        raw_text.push_str(&text_candidate.replace('\u{fffd}', ""));

        for s in extract_strings(&frame) {
            if !s.is_empty() {
                extracted_text.push_str(&s);
            }
        }
    }

    Ok(parse_tool_call(&extracted_text)
        .or_else(|| parse_tool_call(&raw_text))
        .map(normalize_top_level_tool_call))
}

fn parse_plain_response(data: &[u8]) -> String {
    connect_frame_decode(data)
        .into_iter()
        .flat_map(|frame| extract_strings(&frame))
        .filter(|s| s.len() > 10)
        .collect::<Vec<_>>()
        .join("")
}

fn should_retry_unparsed_response(text: &str, already_retried: bool, turns_left: usize) -> bool {
    !already_retried && turns_left >= 1 && !text.trim().is_empty()
}

fn unparsed_response_retry_prompt(text: &str) -> String {
    if text.contains("[TOOL_CALLS]") {
        "Your previous response looked like a tool call but its JSON was incomplete or invalid. Retry now with exactly one complete tool call. If you already have enough context, call `[TOOL_CALLS]answer[ARGS]` with valid XML; otherwise call `[TOOL_CALLS]restricted_exec[ARGS]` with complete JSON only.".to_string()
    } else {
        "Your previous response did not use the required tool-call protocol. Retry now with exactly one valid tool call: either `[TOOL_CALLS]restricted_exec[ARGS]` for more search commands, or `[TOOL_CALLS]answer[ARGS]` with the final XML answer.".to_string()
    }
}

fn unparsed_response_diagnostic(text: &str) -> String {
    let marker = "[TOOL_CALLS]";
    let args_marker = "[ARGS]";
    let tool_name = text.find(marker).and_then(|marker_start| {
        let rest = &text[marker_start + marker.len()..];
        rest.find(args_marker)
            .map(|end| rest[..end].trim())
            .filter(|value| !value.is_empty())
    });
    let json_complete = text
        .find(args_marker)
        .and_then(|start| find_json_object_end(text[start + args_marker.len()..].trim()))
        .is_some();
    format!(
        "marker={}, tool={}, chars={}, json_complete={}",
        text.contains(marker),
        tool_name.unwrap_or("unknown"),
        text.chars().count(),
        json_complete
    )
}

// ─── Answer 解析 ──────────────────────────────────────────

fn resolve_answer_path(vpath: &str, root: &Path) -> Option<(String, PathBuf)> {
    let mut normalized = vpath.trim().replace('\\', "/");
    if normalized.starts_with("/codebase") {
        normalized = normalized
            .trim_start_matches("/codebase")
            .trim_start_matches('/')
            .to_string();
    }

    let candidate = if Path::new(&normalized).is_absolute() {
        PathBuf::from(&normalized)
    } else {
        if crate::fastcontext::executor::has_parent_dir(Path::new(&normalized)) {
            return None;
        }
        root.join(&normalized)
    };
    let absolute = candidate.canonicalize().unwrap_or(candidate);
    if !absolute.starts_with(root) {
        return None;
    }
    let rel = absolute.strip_prefix(root).ok().map(normalize_path)?;
    Some((rel, absolute))
}

fn parse_answer(
    xml_text: &str,
    project_root: &Path,
    ignore_matcher: &Gitignore,
) -> anyhow::Result<Vec<FastContextFile>> {
    let file_re =
        Regex::new(r#"(?s)<file\s+path=["']([^"']+)["']>(.*?)</file>"#).expect("valid regex");
    let range_re = Regex::new(r"<range>(\d+)-(\d+)</range>").expect("valid regex");
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf());
    let mut files = Vec::new();

    for cap in file_re.captures_iter(xml_text) {
        let vpath = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
        let body = cap.get(2).map(|m| m.as_str()).unwrap_or_default();
        let Some((rel_path, full_path)) = resolve_answer_path(vpath, &root) else {
            continue;
        };
        if is_ignored_path(ignore_matcher, &full_path) {
            continue;
        }

        let ranges = range_re
            .captures_iter(body)
            .filter_map(|range_cap| {
                let start = range_cap.get(1)?.as_str().parse::<usize>().ok()?;
                let end = range_cap.get(2)?.as_str().parse::<usize>().ok()?;
                Some([start.max(1), end.max(start)])
            })
            .collect::<Vec<_>>();

        files.push(FastContextFile {
            path: Some(rel_path),
            full_path: Some(normalize_path(&full_path)),
            ranges,
        });
    }

    Ok(files)
}

// ─── Repo map ─────────────────────────────────────────────

fn get_repo_map(
    project_root: &Path,
    target_depth: u8,
    exclude_paths: &[String],
    ignore_matcher: &Gitignore,
) -> RepoMap {
    for depth in (1..=target_depth.max(1)).rev() {
        let tree = build_tree(
            project_root,
            "/codebase",
            depth,
            exclude_paths,
            ignore_matcher,
        );
        let size_bytes = tree.len();
        if size_bytes <= MAX_TREE_BYTES {
            return RepoMap {
                tree,
                depth,
                size_bytes,
                fell_back: depth < target_depth,
            };
        }
    }

    let tree = list_root(project_root, exclude_paths, ignore_matcher);
    RepoMap {
        size_bytes: tree.len(),
        tree,
        depth: 0,
        fell_back: true,
    }
}

/// 构建项目简介：README 首段 + manifest 顶层信息
/// 目的：让 LLM 第一轮就知道项目用什么技术栈、入口在哪，省一轮 ls/tree 探查
fn build_project_summary(root: &Path, ignore_matcher: &Gitignore) -> String {
    let mut sections = Vec::new();

    // README 首 30 行（截断）
    for candidate in ["README.md", "README.MD", "Readme.md", "readme.md", "README"] {
        let path = root.join(candidate);
        if is_ignored_path(ignore_matcher, &path) {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(&path) {
            let head = content.lines().take(30).collect::<Vec<_>>().join("\n");
            if !head.trim().is_empty() {
                sections.push(format!(
                    "### README ({candidate}, first 30 lines)\n```\n{head}\n```"
                ));
                break;
            }
        }
    }

    // Cargo.toml workspace / package 顶层
    let cargo_toml = root.join("Cargo.toml");
    if !is_ignored_path(ignore_matcher, &cargo_toml) {
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            let head = content.lines().take(40).collect::<Vec<_>>().join("\n");
            if !head.trim().is_empty() {
                sections.push(format!(
                    "### Cargo.toml (first 40 lines)\n```toml\n{head}\n```"
                ));
            }
        }
    }

    // package.json 顶层
    let package_json = root.join("package.json");
    if !is_ignored_path(ignore_matcher, &package_json) {
        if let Ok(content) = std::fs::read_to_string(&package_json) {
            let head = content.lines().take(40).collect::<Vec<_>>().join("\n");
            if !head.trim().is_empty() {
                sections.push(format!(
                    "### package.json (first 40 lines)\n```json\n{head}\n```"
                ));
            }
        }
    }

    // pyproject.toml
    let pyproject_toml = root.join("pyproject.toml");
    if !is_ignored_path(ignore_matcher, &pyproject_toml) {
        if let Ok(content) = std::fs::read_to_string(&pyproject_toml) {
            let head = content.lines().take(30).collect::<Vec<_>>().join("\n");
            if !head.trim().is_empty() {
                sections.push(format!(
                    "### pyproject.toml (first 30 lines)\n```toml\n{head}\n```"
                ));
            }
        }
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nProject Summary (auto-extracted):\n{}",
            sections.join("\n\n")
        )
    }
}

fn trim_messages(messages: &mut Vec<ChatMessage>) {
    if messages.len() <= 4 {
        return;
    }
    let mut trimmed = Vec::new();
    trimmed.extend_from_slice(&messages[..2]);
    trimmed.push(ChatMessage::new(
        1,
        "[Prior search rounds omitted to reduce payload. Provide your best answer based on available context.]".to_string(),
    ));
    trimmed.extend_from_slice(&messages[messages.len() - 2..]);
    *messages = trimmed;
}

fn build_meta(
    repo_map: &RepoMap,
    native: bool,
    raw_response: Option<String>,
    stats: &SearchStats,
) -> Value {
    let mut meta = json!({
        "treeDepth": repo_map.depth,
        "treeSizeKB": ((repo_map.size_bytes as f64 / 1024.0) * 10.0).round() / 10.0,
        "fellBack": repo_map.fell_back,
        "native": native,
        "stats": stats.to_json()
    });
    if let Some(raw) = raw_response {
        meta["raw_response"] = Value::String(raw);
    }
    meta
}

// ─── 搜索主循环 ───────────────────────────────────────────

/// 使用 Rust 原生实现执行 fast-context 检索。
pub async fn search(opts: SearchOptions) -> anyhow::Result<SearchResult> {
    let started_at = Instant::now();
    log::info!(
        "[fast-context] 开始搜索: project_root={}, query_len={}, tree_depth={}, max_turns={}, max_results={}, max_commands={}, timeout_ms={}, exclude_count={}",
        opts.project_root.display(),
        opts.query.chars().count(),
        opts.tree_depth,
        opts.max_turns,
        opts.max_results,
        opts.max_commands,
        opts.timeout_ms,
        opts.exclude_paths.len()
    );

    let project_root = opts
        .project_root
        .canonicalize()
        .with_context(|| format!("无法解析项目路径: {}", opts.project_root.display()))?;
    if !project_root.is_dir() {
        return Err(anyhow::anyhow!("项目路径不是目录: {}", project_root.display()));
    }
    log::info!("[fast-context] 项目路径已解析: {}", project_root.display());

    let detected_key = detect_api_key(opts.api_key.as_deref())
        .context("未找到 Devin / Windsurf API Key，请在设置中填写或登录客户端后重试")?;
    log::info!(
        "[fast-context] Devin / Windsurf API Key 已解析: source={}, label={}, masked={}, length={}",
        detected_key.source.as_str(),
        detected_key.source.label(),
        mask_api_key(&detected_key.api_key),
        detected_key.api_key.chars().count()
    );
    let api_key = detected_key.api_key;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(opts.timeout_ms + 5000))
        .build()
        .context("创建 fast-context HTTP 客户端失败")?;

    log::info!("[fast-context] 开始获取 Devin / Windsurf JWT");
    let jwt = fetch_jwt(&client, &api_key).await?;
    log::info!(
        "[fast-context] Devin / Windsurf JWT 获取成功: length={}",
        jwt.len()
    );
    log::info!("[fast-context] 开始检查 Fast Context 限流状态");
    if !check_rate_limit(&client, &api_key, &jwt).await? {
        log::warn!("[fast-context] Fast Context 限流检查未通过");
        return Err(anyhow::anyhow!("RATE_LIMITED: Fast Context 当前限流，请稍后重试"));
    }
    log::info!("[fast-context] Fast Context 限流检查通过");

    // 一次解析项目 ignore 文件，并在 repo map 与全部本地检索命令中复用。
    let ignore_matcher = build_fast_context_ignore(&project_root);
    let repo_map = get_repo_map(
        &project_root,
        opts.tree_depth,
        &opts.exclude_paths,
        &ignore_matcher,
    );
    log::info!(
        "[fast-context] repo map 已生成: depth={}, size_bytes={}, fell_back={}",
        repo_map.depth,
        repo_map.size_bytes,
        repo_map.fell_back
    );
    // Repo Map 智能化：附加 README / manifest 摘要，提升 LLM 首轮命中率
    let project_summary = build_project_summary(&project_root, &ignore_matcher);
    // 并发版 ToolExecutor：Arc<Mutex<状态>> 让多条 restricted_exec 命令可并行，并统一应用默认排除目录。
    let executor = Arc::new(ToolExecutor::new(
        project_root.clone(),
        opts.exclude_paths.clone(),
        ignore_matcher,
    ));
    let tool_defs = build_tool_definitions(opts.max_commands);
    let system_prompt = build_system_prompt(opts.max_turns, opts.max_commands, opts.max_results);
    // 中文 query 提示：当中文字符占比超过 30%，user prompt 内追加翻译提醒
    let language_hint = if chinese_ratio(&opts.query) > 0.30 {
        "\n\nLanguage note: The Problem Statement above is in Chinese. The codebase identifiers are most likely in English — translate domain terms into English keywords before searching (e.g. 截图→screenshot/capture, 剪贴板→clipboard, 配置→config, 服务→service, 控制器→controller)."
    } else {
        ""
    };
    let user_content = format!(
        "Problem Statement: {}\n\nRepo Map (tree -L {} /codebase):\n```text\n{}\n```{}{}",
        opts.query, repo_map.depth, repo_map.tree, project_summary, language_hint
    );

    let mut messages = vec![
        ChatMessage::new(5, system_prompt),
        ChatMessage::new(1, user_content),
    ];
    let total_api_calls = opts.max_turns as usize + 1;
    let mut compensated_turns = 0usize;
    let mut force_answer_injected = false;

    let mut turn = 0usize;
    let mut empty_answer_retried = false;
    let mut unparsed_response_retried = false;
    while turn < total_api_calls + compensated_turns {
        log::info!(
            "[fast-context] 搜索轮次开始: turn={}, messages={}, compensated_turns={}, force_answer_injected={}",
            turn + 1,
            messages.len(),
            compensated_turns,
            force_answer_injected
        );
        let proto = build_request(&api_key, &jwt, &messages, &tool_defs)?;
        let response = match streaming_request(&client, &proto, opts.timeout_ms, 2).await {
            Ok(response) => response,
            Err(err)
                if matches!(err.code.as_str(), "PAYLOAD_TOO_LARGE" | "TIMEOUT")
                    && messages.len() > 4 =>
            {
                log::warn!(
                    "[fast-context] 流式请求失败并触发上下文裁剪: code={}, status={:?}, messages={}",
                    err.code,
                    err.status,
                    messages.len()
                );
                trim_messages(&mut messages);
                let retry_proto = build_request(&api_key, &jwt, &messages, &tool_defs)?;
                streaming_request(&client, &retry_proto, opts.timeout_ms, 0)
                    .await
                    .map_err(|retry_err| anyhow::anyhow!("{} (context_trimmed=true)", retry_err))?
            }
            Err(err) => return Err(anyhow::anyhow!(err)),
        };
        log::debug!(
            "[fast-context] 搜索轮次响应已收到: turn={}, bytes={}",
            turn + 1,
            response.len()
        );

        let Some(tool_call) = parse_response(&response)? else {
            let text = parse_plain_response(&response);
            if text.trim().is_empty() {
                return Err(anyhow::anyhow!("fast-context 未返回可解析响应"));
            }
            if text.starts_with("[Error]") {
                return Err(anyhow::anyhow!(text));
            }
            let turns_left = (total_api_calls + compensated_turns).saturating_sub(turn + 1);
            if should_retry_unparsed_response(&text, unparsed_response_retried, turns_left) {
                log::warn!(
                    "[fast-context] 未解析到合法工具调用，触发补偿重试: turn={}, turns_left={}, contains_tool_marker={}",
                    turn + 1,
                    turns_left,
                    text.contains("[TOOL_CALLS]")
                );
                unparsed_response_retried = true;
                messages.push(ChatMessage::new(1, unparsed_response_retry_prompt(&text)));
                turn += 1;
                continue;
            }
            log::warn!(
                "[fast-context] 未解析到合法工具调用，搜索退化失败: length={}",
                text.len()
            );
            return Err(anyhow::anyhow!(
                "fast-context 未获得合法工具调用: {}",
                unparsed_response_diagnostic(&text)
            ));
        };

        match tool_call.name.as_str() {
            "answer" => {
                let answer_xml = tool_call
                    .args
                    .get("answer")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let files = parse_answer(answer_xml, &project_root, &executor.ignore_matcher)?;
                log::info!(
                    "[fast-context] answer 解析完成: files={}, elapsed_ms={}",
                    files.len(),
                    started_at.elapsed().as_millis()
                );
                // 空 answer 自动重试：LLM 偶发直接返回 0 文件，但还有 turn 余量时给一次重试
                // 触发条件：解析得到 0 个文件、未重试过、且剩余 turn 至少 1 个
                let effective_used = turn.saturating_sub(compensated_turns) + 1;
                let turns_left = (opts.max_turns as usize).saturating_sub(effective_used);
                if files.is_empty() && !empty_answer_retried && turns_left >= 1 {
                    log::warn!(
                        "[fast-context] 检测到空 ANSWER，触发自动重试: turn={}, turns_left={}",
                        turn + 1,
                        turns_left
                    );
                    empty_answer_retried = true;
                    // 用 user role 注入更具体的搜索指令，让 LLM 必须先 rg 再 answer
                    messages.push(ChatMessage::new(
                        1,
                        "Your previous answer was empty. Re-attempt the search: first issue a restricted_exec call with 2-3 broad rg searches against the most likely source directories (e.g. src/), then read the top matches, and finally provide a non-empty ANSWER with concrete file paths.".to_string(),
                    ));
                    turn += 1;
                    continue;
                }
                let stats = executor.snapshot_stats().await;
                return Ok(SearchResult {
                    files,
                    rg_patterns: executor.collected_rg_patterns().await,
                    file_cache: executor.snapshot_read_cache().await,
                    stats: stats.clone(),
                    meta: build_meta(&repo_map, true, None, &stats),
                    answer_received: true,
                });
            }
            "restricted_exec" => {
                let call_id = uuid::Uuid::new_v4().to_string();
                let args_json = serde_json::to_string(&tool_call.args)
                    .context("序列化 fast-context 工具参数失败")?;
                let valid_commands = count_valid_commands(&tool_call.args);
                // 检测重复命令（指纹与上一次相同），帮助诊断 LLM 浪费
                let dup_count = executor.count_repeat_commands(&tool_call.args).await;
                if dup_count > 0 {
                    log::warn!(
                        "[fast-context] 检测到重复命令: turn={}, dup_count={}",
                        turn + 1,
                        dup_count
                    );
                }
                log::info!(
                    "[fast-context] restricted_exec 调用: turn={}, valid_commands={}, dup_count={}",
                    turn + 1,
                    valid_commands,
                    dup_count
                );
                let batch = ToolExecutor::exec_tool_call(executor.clone(), &tool_call.args).await;
                let results = batch.output;
                log::debug!(
                    "[fast-context] restricted_exec 返回: turn={}, output_len={}",
                    turn + 1,
                    results.len()
                );

                if batch.stats.commands_useful == 0 && compensated_turns < MAX_COMPENSATIONS {
                    log::warn!(
                        "[fast-context] 本轮未产生有效上下文，补偿搜索轮次: turn={}, invalid={}, path_missing={}",
                        turn + 1,
                        batch.stats.commands_invalid,
                        batch.stats.path_missing
                    );
                    compensated_turns += 1;
                }

                messages.push(ChatMessage {
                    role: 2,
                    content: tool_call.thinking,
                    tool_call_id: Some(call_id.clone()),
                    tool_name: Some("restricted_exec".to_string()),
                    tool_args_json: Some(args_json),
                    ref_call_id: None,
                });
                messages.push(ChatMessage {
                    role: 4,
                    content: results,
                    tool_call_id: None,
                    tool_name: None,
                    tool_args_json: None,
                    ref_call_id: Some(call_id),
                });

                let effective_turn = turn.saturating_sub(compensated_turns);
                if effective_turn >= opts.max_turns.saturating_sub(1) as usize
                    && !force_answer_injected
                {
                    log::info!(
                        "[fast-context] 搜索轮次即将耗尽，已注入强制 answer 提示: turn={}",
                        turn + 1
                    );
                    messages.push(ChatMessage::new(1, FINAL_FORCE_ANSWER.to_string()));
                    force_answer_injected = true;
                }
            }
            other => return Err(anyhow::anyhow!("fast-context 返回未知工具调用: {}", other)),
        }
        turn += 1;
    }

    log::warn!(
        "[fast-context] 已达到最大轮次但未获得 answer: elapsed_ms={}",
        started_at.elapsed().as_millis()
    );
    Err(anyhow::anyhow!("fast-context 已达到最大轮次但未获得 answer"))
}

// ─── 格式化输出 ───────────────────────────────────────────

/// 解析 fast-context 返回的文件项为真实路径（优先 full_path，否则 root.join(path)）。
/// 做路径安全校验：拒绝项目外路径。
fn resolve_fast_context_file(root: &Path, file: &FastContextFile) -> anyhow::Result<Option<PathBuf>> {
    let candidate = if let Some(full_path) = file.full_path.as_deref() {
        PathBuf::from(full_path)
    } else if let Some(path) = file.path.as_deref() {
        root.join(path)
    } else {
        return Ok(None);
    };
    let absolute = candidate.canonicalize().unwrap_or(candidate);
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !absolute.starts_with(&root) {
        return Err(anyhow::anyhow!(
            "fast-context 返回了项目外路径: {}",
            absolute.display()
        ));
    }
    Ok(Some(absolute))
}

/// 从已知文件内容中切片指定行范围（格式 `L{line_no}:{line}`）。
fn extract_line_range(content: &str, start: usize, end: usize) -> String {
    let mut out = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        if line_no >= start && line_no <= end {
            out.push(format!("L{}:{}", line_no, line));
        }
        if line_no > end {
            break;
        }
    }
    out.join("\n")
}

fn read_line_range(path: &Path, start: usize, end: usize) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("读取文件失败: {}", path.display()))?;
    Ok(extract_line_range(&content, start, end))
}

/// 把 SearchResult 格式化为 pretty 文本（包含命中文件的代码片段（优先用搜索期间 readfile 缓存，避免重复读盘）、
/// grep keywords、统计与 config 诊断。
pub fn format_result(result: &SearchResult, opts: &SearchOptions) -> String {
    let root = PathBuf::from(&opts.project_root);
    let mut parts = Vec::new();
    let mut code_sections = 0usize;

    parts.push("The following code sections were retrieved:".to_string());
    parts.push(String::new());

    for file in &result.files {
        let Some(path) = resolve_fast_context_file(&root, file).ok().flatten() else {
            continue;
        };
        if !path.exists() || !path.is_file() {
            continue;
        }

        let display = crate::fastcontext::executor::normalize_path(&path);
        let ranges = if file.ranges.is_empty() {
            vec![[1, 80]]
        } else {
            file.ranges.clone()
        };

        for range in ranges {
            let start = range[0].max(1);
            let end = range[1].max(start).min(start.saturating_add(220));
            // 优先用 ToolExecutor 中已读取的文件内容（fast-context 阶段 readfile 命中）
            let cache_key = crate::fastcontext::executor::normalize_path(&path);
            let snippet = if let Some(content) = result.file_cache.get(&cache_key) {
                extract_line_range(content, start, end)
            } else {
                read_line_range(&path, start, end).unwrap_or_default()
            };
            if snippet.trim().is_empty() {
                continue;
            }
            parts.push(format!("Path: {}", display));
            parts.push(format!("Lines: L{}-L{}", start, end));
            parts.push(snippet);
            parts.push(String::new());
            code_sections += 1;
        }
    }

    if code_sections == 0 && result.answer_received {
        parts.push("No relevant files found.".to_string());
    }
    if !result.rg_patterns.is_empty() {
        parts.push(format!("grep keywords: {}", result.rg_patterns.join(", ")));
    }
    parts.push(format!(
        "[fast-context stats] commands_seen={}, commands_executed={}, commands_useful={}, commands_invalid={}, repaired={}, path_missing={}, path_repaired={}, cache_hits={}, useful_command_rate={}%, invalid_command_rate={}%",
        result.stats.commands_seen,
        result.stats.commands_executed,
        result.stats.commands_useful,
        result.stats.commands_invalid,
        result.stats.commands_repaired,
        result.stats.path_missing,
        result.stats.path_repaired,
        result.stats.cache_hits,
        result.stats.useful_rate(),
        result.stats.invalid_rate()
    ));
    if !result.meta.is_null() {
        parts.push(format!("[fast-context config] {}", result.meta));
    }

    parts.join("\n")
}

/// 把 SearchResult 序列化为结构化 JSON（供 CLI `--json` 模式）。
pub fn search_result_json(result: &SearchResult, opts: &SearchOptions) -> Value {
    json!({
        "files": result.files.iter().map(|f| json!({
            "path": f.path,
            "full_path": f.full_path,
            "ranges": f.ranges,
        })).collect::<Vec<_>>(),
        "rg_patterns": result.rg_patterns,
        "stats": result.stats.to_json(),
        "answer_received": result.answer_received,
        "meta": result.meta,
        "request": {
            "max_turns": opts.max_turns,
            "max_results": opts.max_results,
            "max_commands": opts.max_commands,
            "timeout_ms": opts.timeout_ms,
            "tree_depth": opts.tree_depth,
            "exclude_paths": opts.exclude_paths,
        }
    })
}

use anyhow::Context;
use std::time::Duration;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_call_extracts_json_and_ignores_tail() {
        let parsed = parse_tool_call(
            "thinking\n[TOOL_CALLS]restricted_exec[ARGS]{\"command1\":{\"type\":\"rg\",\"pattern\":\"SouTool\",\"path\":\"/codebase/src\"}}</s>",
        )
        .expect("应识别 restricted_exec 调用");

        assert_eq!(parsed.thinking, "thinking");
        assert_eq!(parsed.name, "restricted_exec");
        assert_eq!(parsed.args["command1"]["pattern"].as_str(), Some("SouTool"));
    }

    #[test]
    fn malformed_tool_call_is_retryable_unparsed_response() {
        let text = r#"thinking
[TOOL_CALLS]restricted_exec[ARGS]{"command1":{"type":"rg","pattern":"gesture","path":"/codebase/src"}"#;

        assert!(
            parse_tool_call(text).is_none(),
            "半截工具调用不应被解析为合法工具调用"
        );
        assert!(
            should_retry_unparsed_response(text, false, 1),
            "包含半截工具调用且仍有轮次时应触发补偿重试"
        );
        assert!(
            !should_retry_unparsed_response(text, true, 1),
            "已重试过则不应再次补偿"
        );
    }

    #[test]
    fn unparsed_response_diagnostic_does_not_echo_arguments() {
        let text = "thinking\n[TOOL_CALLS]restricted_exec[ARGS]{...some json...}";
        let diagnostic = unparsed_response_diagnostic(text);
        assert!(diagnostic.contains("tool=restricted_exec"), "应包含工具名: {diagnostic}");
        assert!(!diagnostic.contains("some json"), "不应回显参数内容");
    }

    #[test]
    fn parse_response_surfaces_connect_error_json() {
        let frame = crate::fastcontext::proto::connect_frame_encode(
            br#"{"error":{"code":"unauthenticated","message":"bad token"}}"#,
            false,
        )
        .expect("error 帧应可编码");
        let error = parse_response(&frame).expect_err("Connect error 帧应返回错误");
        assert!(error.to_string().contains("unauthenticated"));
        assert!(error.to_string().contains("bad token"));
    }

    #[test]
    fn parse_response_joins_tool_call_split_across_connect_frames() {
        let first = crate::fastcontext::proto::connect_frame_encode(
            br#"[TOOL_CALLS]restricted_exec[ARGS]{"command1":{"type":"rg","pattern":"Sou"#,
            false,
        )
        .expect("首帧应可编码");
        let second = crate::fastcontext::proto::connect_frame_encode(br#"Tool","path":"/codebase/src"}}"#, false)
            .expect("尾帧应可编码");
        let mut response = first;
        response.extend(second);

        let parsed = parse_response(&response)
            .expect("跨帧响应应可读取")
            .expect("跨帧工具调用应可解析");
        assert_eq!(parsed.name, "restricted_exec");
        assert_eq!(parsed.args["command1"]["pattern"], "SouTool");
    }

    #[test]
    fn parse_response_wraps_top_level_readfile_as_restricted_exec() {
        let frame = crate::fastcontext::proto::connect_frame_encode(
            br#"[TOOL_CALLS]readfile[ARGS]{"file":"/codebase/src/lib.rs","start_line":2}"#,
            false,
        )
        .expect("readfile 帧应可编码");

        let parsed = parse_response(&frame)
            .expect("readfile 响应应可读取")
            .expect("顶层 readfile 应可规范化");
        assert_eq!(parsed.name, "restricted_exec");
        assert_eq!(parsed.args["command1"]["type"], "readfile");
        assert_eq!(parsed.args["command1"]["file"], "/codebase/src/lib.rs");
    }

    #[test]
    fn chinese_ratio_detects_chinese_dominant_text() {
        // 纯英文：0
        assert!(chinese_ratio("Find ImageCodec class") < 0.05);
        // 纯中文：接近 1
        assert!(chinese_ratio("找到图像编码器类的实现位置") > 0.9);
        // 中英混合（约一半中文）
        let ratio = chinese_ratio("找到 ImageCodec 类的实现");
        assert!(
            ratio > 0.30 && ratio < 0.80,
            "中英混合中文占比应在 30%~80%，实际 {}",
            ratio
        );
        // 空字符串安全
        assert_eq!(chinese_ratio(""), 0.0);
    }

    #[test]
    fn parse_answer_keeps_safe_paths_and_rejects_escape() {
        let root = std::env::temp_dir().join(format!("fc-answer-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "x").unwrap();
        let ignore = build_fast_context_ignore(&root);
        let xml = r#"<ANSWER>
          <file path="/codebase/src/lib.rs">
            <range>1-5</range>
            <range>10-20</range>
          </file>
          <file path="/etc/passwd">
            <range>1-2</range>
          </file>
        </ANSWER>"#;
        let files = parse_answer(xml, &root, &ignore).expect("answer 应可解析");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path.as_deref(), Some("src/lib.rs"));
        assert_eq!(files[0].ranges, vec![[1, 5], [10, 20]]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn trim_messages_keeps_head_tail() {
        let mut msgs = vec![
            ChatMessage::new(5, "sys"),
            ChatMessage::new(1, "user"),
            ChatMessage::new(2, "a"),
            ChatMessage::new(4, "r1"),
            ChatMessage::new(2, "b"),
            ChatMessage::new(4, "r2"),
        ];
        trim_messages(&mut msgs);
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].content, "sys");
        assert_eq!(msgs[1].content, "user");
        assert_eq!(msgs[3].content, "b");
        assert_eq!(msgs[4].content, "r2");
    }

    #[test]
    fn format_result_renders_pretty_with_code_sections() {
        // 用真实临时文件 + file_cache 验证代码片段输出
        let root = std::env::temp_dir().join(format!("fc-format-test-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let file = root.join("src/lib.rs");
        std::fs::write(&file, "pub fn auth() {}\n// impl\npub fn other() {}\n").unwrap();

        let mut file_cache = std::collections::HashMap::new();
        file_cache.insert(
            crate::fastcontext::executor::normalize_path(&file),
            "pub fn auth() {}\n// impl\npub fn other() {}\n".to_string(),
        );

        let result = SearchResult {
            files: vec![FastContextFile {
                path: Some("src/lib.rs".to_string()),
                full_path: Some(crate::fastcontext::executor::normalize_path(&file)),
                ranges: vec![[1, 2]],
            }],
            rg_patterns: vec!["auth".to_string()],
            file_cache,
            stats: SearchStats {
                commands_seen: 4,
                commands_useful: 3,
                ..SearchStats::default()
            },
            meta: json!({"treeDepth": 3, "treeSizeKB": 12.5, "fellBack": false}),
            answer_received: true,
        };
        let opts = SearchOptions {
            query: "q".to_string(),
            project_root: root.clone(),
            max_turns: 3,
            max_results: 10,
            max_commands: 8,
            ..Default::default()
        };
        let text = format_result(&result, &opts);
        assert!(text.contains("The following code sections were retrieved:"));
        assert!(text.contains("Path: "));
        assert!(text.contains("src/lib.rs"));
        assert!(text.contains("Lines: L1-L2"));
        assert!(text.contains("L1:pub fn auth() {}"), "应包含代码片段: {}", text);
        assert!(text.contains("grep keywords: auth"));
        assert!(text.contains("[fast-context stats]"));
        assert!(text.contains("useful_command_rate=75%"), "应包含统计: {}", text);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_result_json_shape() {
        let result = SearchResult {
            files: vec![FastContextFile {
                path: Some("src/a.rs".to_string()),
                full_path: Some("/proj/src/a.rs".to_string()),
                ranges: vec![[1, 5]],
            }],
            rg_patterns: vec!["auth".to_string()],
            file_cache: std::collections::HashMap::new(),
            stats: SearchStats::default(),
            meta: json!({"treeDepth": 3}),
            answer_received: true,
        };
        let opts = SearchOptions {
            query: "q".to_string(),
            project_root: PathBuf::from("/proj"),
            ..Default::default()
        };
        let v = search_result_json(&result, &opts);
        assert_eq!(v["files"][0]["path"], "src/a.rs");
        assert_eq!(v["request"]["max_turns"], 3);
    }
}
