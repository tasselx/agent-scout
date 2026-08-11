# 更新日志（Changelog）

> **[English](CHANGELOG.md) | [简体中文](CHANGELOG.zh-CN.md)**

本项目的所有重要变更均记录在此。

## [0.4.0] - 2026-08-11

### 新增

- **AI 代码搜索（fast-context）**——新增 `agent-scout fc` 子命令与 `fast_context_search` MCP 工具，对本地代码库做自然语言语义搜索，由 Windsurf 的 Devstral 模型驱动（`GetDevstralStream`）。
- **fast-context 用 Rust 原生重写**——对齐 sanshu 实现（async tokio + reqwest）：多轮工具调用循环、LLM 畸形参数的命令形状归一化、路径回退到最近存在的父目录、gitignore 支持（`.gitignore` / `.codeiumignore` / `.windsurfignore` / `.devinignore`）、命令指纹缓存 + 跨轮重复检测、readfile 内容缓存、SearchStats 诊断。
- **Pretty 输出**——`fc` 文本输出现在包含命中的代码片段（复用搜索期间已读过的文件内容），以及 grep 关键词与统计/config 诊断；`fc --json --pretty` 美化结构化 JSON。
- **中文查询提示**——中文占比超过 30% 的查询会在 prompt 中追加翻译提示。
- **空 answer 自动重试与未解析响应重试**——搜索循环在还有轮次时能自动恢复空 `<ANSWER>` 或畸形工具调用响应。
- **文档双语化**——README / QUICKSTART 默认英文，并提供简体中文切换（`README.zh-CN.md`、`QUICKSTART.zh-CN.md`）；CHANGELOG 遵循同样的模式。
- **fast-context 文档**——在 README、QUICKSTART 与 `agent-scout-search` skill（SKILL.md + agents/openai.yaml）中引入使用说明。

### 变更

- 新增依赖：`tokio`、`reqwest`、`anyhow`、`futures-util`、`uuid`、`dirs`、`log`、`ignore`、`globset`（dev：`tempfile`）。

### 修复

- macOS 上 `/var → /private/var` 符号链接导致的路径校验误判（`ToolExecutor::new` 中对项目根目录做了 canonicalize）。

### 致谢

- fast-context 能力参考了 [fast-context-mcp](https://github.com/SammySnake-d/fast-context-mcp)（Node.js 实现），用 Rust 原生重写并补充工程增强。
