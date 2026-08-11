# agent-scout

**web search + 识图 + 音频转写** 命令行工具 + MCP server + Agent Skill，
基于 Windsurf/Devin 服务端接口（`GetWebSearchResults` / `GetImageCaption` / `GetTranscription`），用 **Rust** 编写。

> 这是一个**开源的通用 web 搜索 + 识图 + 转写工具**，不依赖特定客户端——提供三种使用形态：
> **CLI**（脚本/终端直接调用）、**MCP server**（接入任意 MCP 客户端）、**Agent Skill**（供 AI agent 调用）。
> 后端走 Windsurf/Devin 认证，并额外内置 **本地安装认证自动提取** 与 **自动清理的错误日志**。

## 跨平台支持

`agent-scout` 使用纯 Rust 编写，**macOS / Linux / Windows 三平台原生支持**，单二进制分发、零运行时依赖：

| 平台 | 构建 | 认证来源 |
|------|------|----------|
| macOS | `cargo build --release` | Devin `~/Library/Application Support/Devin/.../state.vscdb` |
| Linux | `cargo build --release` | Devin `~/.config/Devin/.../state.vscdb`、CLI `credentials.toml`（含 WSL） |
| Windows | `cargo build --release` | Devin `%APPDATA%\Devin\...\state.vscdb` |

> 认证自动提取逻辑内部按平台路由（见「认证」章节），其余功能三平台一致。

## 三种使用形态

| 形态 | 说明 | 快速入口 |
|------|------|----------|
| **CLI** | 终端/脚本直接搜索/识图/转写，stdout 输出 JSON / 文本 | `agent-scout "查询" --limit 3` |
| **MCP server** | 接入 Cursor / Claude Desktop / 任意 MCP host | `agent-scout --mcp` |
| **Agent Skill** | 供 AI agent（Codex / InsCode 等）自动调用 | `skills/agent-scout-search/` |

> 📖 快速上手请看 **[QUICKSTART.md](QUICKSTART.md)**。

## 开箱即用

从拿到二进制到完成首次搜索，**无需任何配置**：

1. **下载/构建** 单个可执行文件（无 Node 运行时、无依赖安装）
2. **运行** `agent-scout "查询"` / `agent-scout caption 图片` / `agent-scout transcribe 音频` —— 自动识别本机 Devin/Windsurf 登录凭证
3. **完成** 直接返回 JSON 搜索结果 / 图片描述 / 转写文本

**最快方式：一键安装**（从 GitHub Releases 下载最新版二进制，无需 Rust 工具链）：

```bash
AGENT_SCOUT_REPO=tasselx/agent-scout curl -fsSL \
  https://raw.githubusercontent.com/tasselx/agent-scout/main/install.sh | bash
```

脚本会自动检测当前平台/架构，下载对应二进制到 `~/.local/bin`（可用
`AGENT_SCOUT_PREFIX` 覆盖目录），并把 PATH 提示打印出来。也可以本地直接跑
`./install.sh`（详见脚本头部注释）。发布产物均附带 `SHA256SUMS` 校验和，
可在 Releases 页核对。

零配置流程示意：

```
agent-scout 二进制 ──▶ 自动提取本机 key ──▶ 服务端搜索 / 识图 / 转写 ──▶ JSON hits / 描述 / 转写文本
    无需 API key 配置        零手动步骤           自动重试多 host
```

三种形态（CLI / MCP / Skill）都遵循同一零配置原则：只要本机登录过 Devin/Windsurf，即可直接使用。

## 功能特性

- 🔍 **Web 搜索**：`GetWebSearchResults` 服务端搜索，返回 `{title, url, snippet, source}` 结构化结果
- 🖼️ **识图（image caption）**：`GetImageCaption` 服务端视觉分析，描述图片或回答关于图片的问题
- 🎙️ **音频转写（transcribe）**：`GetTranscription` 服务端语音转文字（Whisper），格式自动检测（wav/mp3/ogg/opus/webm/m4a/flac）
- 🎯 **域名过滤**：`--domain` 限定搜索范围（如 `github.com`）
- 🪄 **零配置认证**：自动识别本机 Devin/Windsurf 登录 key，无需手动配置
- 🧩 **MCP server**：`--mcp` 以 stdio 运行，支持 `Content-Length` + NDJSON 双帧协议
- 🧠 **Agent Skill**：附带 `agent-scout-search` skill 供 agent 调用
- 📝 **错误日志**：按天分文件记录，超期自动清理
- 🚀 **纯 Rust**：跨平台、零 Node 运行时依赖，单二进制分发

## 结构

```
src/
  search.rs     核心搜索：请求体构造、HTTP 调用（多 host 重试）、结果归一化
  caption.rs    识图：GetImageCaption 请求体构造、HTTP 调用（多 host 重试）、base64 读取/mime 猜测
  transcribe.rs 转写：GetTranscription 请求体构造、HTTP 调用（多 host 重试）、base64 读取
  auth.rs       API key 解析（CLI → env → key 文件 → 本地自动发现）、config 读写
  log.rs        错误/信息日志（按天分文件，自动清理旧日志）
  mcp.rs        MCP stdio server（NDJSON + Content-Length 双帧协议，web_search + image_caption + audio_transcribe 工具）
  main.rs       CLI 入口（查询 + caption + transcribe + config 子命令 + --mcp）
skills/
  agent-scout-search/  供 agent 调用的 web 搜索 + 识图 + 转写 skill（SKILL.md + openai.yaml）
tests/
  search_live.rs  集成测试：本地 mock HTTP 验证 search() 成功路径
```

## 构建 & 测试

```bash
cargo build --release                       # 生成 target/release/agent-scout
cargo test                                  # 29 单元测试 + 3 集成测试
```

## Release 下载

三平台预编译二进制随 **GitHub Releases** 发布，免构建直接下载使用：

- **macOS** (arm64 / x86_64)：`agent-scout-macos-aarch64` / `agent-scout-macos-x86_64`
- **Linux** (x86_64 / aarch64)：`agent-scout-linux-x86_64` / `agent-scout-linux-aarch64`
- **Windows** (x86_64)：`agent-scout-windows-x86_64.exe`

> 发布入口见仓库 **Releases** 页；每个 Release 附带 `SHA256SUMS` 校验和文件，
> 下载后可自行 `shasum -a 256 -c SHA256SUMS` 校验。
> 也可用项目内 `build-release.sh` 本地打包（同样生成校验和）。
> 打包细节见 [`.github/workflows/release.yml`](.github/workflows/release.yml)。

## 安装到 PATH（激活）

编译后可用以下任一方式把 `agent-scout` 激活到 PATH，实现全局直接调用。

**方式 1：`cargo install`（推荐，自动装到 `~/.cargo/bin`）**

```bash
cd /path/to/agent-scout        # 先进入项目目录（--path . 指向当前目录）
cargo install --path . --root "$HOME/.cargo"
# 之后可直接调用（~/.cargo/bin 通常已在 PATH）
agent-scout "some query" --limit 3
```

> 提示：`--path .` 的 `.` 是当前目录，必须先在项目目录下执行；
> 或用绝对路径 `cargo install --path /path/to/agent-scout --root "$HOME/.cargo"` 免去 `cd`。

**方式 2：软链接到 PATH 目录**

```bash
ln -sf "$(pwd)/target/release/agent-scout" ~/.local/bin/agent-scout
# 需确保 ~/.local/bin 在 PATH 中
```

**方式 3：key 通过环境变量激活（不依赖本机 Devin 登录）**

```bash
export WINDSURF_API_KEY='devin-session-token$...'
agent-scout "some query"
# 或单次使用：
WINDSURF_API_KEY='devin-session-token$...' agent-scout "some query"
```

### key 解析优先级

`--api-key` → `WINDSURF_API_KEY` 环境变量 → key 文件 → 本地自动识别。

因此：安装了之后**通常零配置**直接可用（自动识别本机 Devin/Windsurf key）；
如需在无登录的机器上使用，用环境变量或 `--api-key` 显式提供 key。

## 使用

> 以下用 `agent-scout` 表示已激活到 PATH 的命令；未激活时替换为 `target/release/agent-scout`。

```bash
# 查询（stdout 输出 JSON hits）—— 无需手动配置，自动发现本地 key
agent-scout "tauri window drag region" --limit 5

# 手动指定 key
agent-scout "rust async" --limit 3 --api-key 'devin-session-token$...'

# 运行 MCP stdio server（同样自动发现 key）
agent-scout --mcp

# 识图：描述一张本地图片
agent-scout caption ~/Pictures/photo.png

# 识图：针对图片提问
agent-scout caption ~/Pictures/screen.png --question "这是什么 UI 框架的界面？"

# 识图：输出 JSON（便于脚本解析）
agent-scout caption ~/Pictures/photo.png --json

# 转写：将本地音频转成文字
agent-scout transcribe ~/Recordings/meeting.wav

# 转写：输出 JSON（便于脚本解析）
agent-scout transcribe ~/Recordings/meeting.mp3 --json

# 配置 / 查看 / 测试 / 清除 api key
agent-scout config set 'devin-session-token$...'
agent-scout config show
agent-scout config test 'connectivity check'
agent-scout config clear
```

### CLI 命令速查

| 命令 | 作用 |
|------|------|
| `agent-scout "<查询>" [--limit N] [--domain d] [--mode m] [--api-key k]` | 执行 web 搜索，stdout 输出 JSON hits |
| `agent-scout caption <图片路径> [--question "..." --mime m --json] [--api-key k]` | 识图：描述或分析本地图片，stdout 输出描述文本（`--json` 输出 `{"caption": "..."}`） |
| `agent-scout transcribe <音频路径> [--timeout N --json] [--api-key k]` | 转写：语音转文字，stdout 输出转写文本（`--json` 输出 `{"transcribedText": "..."}`） |
| `agent-scout --mcp` | 以 MCP stdio server 运行 |
| `agent-scout config set [key]` | 保存 key（chmod 600）；无 key 时交互输入 |
| `agent-scout config show` | 查看当前 key 状态（掩码显示） |
| `agent-scout config test [query]` | 连通性测试（真实搜索验证 key） |
| `agent-scout config clear` | 删除已保存的 key 文件 |

## 认证（零配置即用）

key 解析优先级：

1. `--api-key <token>`
2. `WINDSURF_API_KEY` / `WINDSURFAPI_CODEIUM_API_KEY` 环境变量
3. key 文件（`~/.config/windsurf-search/api-key`、`~/.windsurf-search/api-key`、`~/.piwin/windsurf-api-key`）
4. **本地安装自动提取**（新增）

只要本机用 Devin 或 Windsurf 登录过，第 4 层会自动从本地安装提取 key，无需任何手动配置。

key 形如 `devin-session-token$...`。

### 自动提取来源

按顺序查找以下来源，返回第一个有效 key：

| 来源 | 类型 | 平台 | 路径 |
|------|------|------|------|
| Devin CLI 凭据 | TOML | Linux/WSL | `~/.local/share/devin/credentials.toml` |
| Devin `state.vscdb` | SQLite | macOS | `~/Library/Application Support/Devin/User/globalStorage/state.vscdb` |
| Devin `state.vscdb` | SQLite | Windows | `%APPDATA%\Devin\User\globalStorage\state.vscdb` |
| Devin `state.vscdb` | SQLite | Linux | `~/.config/Devin/User/globalStorage/state.vscdb` |
| Deviv / Windsurf 同名位置 | SQLite | 全平台 | 兼容回退（历史 app 名） |

提取逻辑：`state.vscdb` 读取 `ItemTable` 中 `windsurfAuthStatus` 记录的 `apiKey` 字段；
`credentials.toml` 解析常见 key 字段（`api_key`/`access_token`/`token` 等），并回退匹配 `sk-` 前缀 token。

> 说明：key 只在内存中临时使用，不会写入磁盘；`config show` 只展示掩码。

## MCP 工具

`web_search`

| arg | 类型 | 必填 | 说明 |
|-----|------|------|------|
| `query` | string | 是 | 搜索词 |
| `limit` | number | 否 | 1–10，默认 5 |
| `domain` | string | 否 | 域名过滤 |
| `mode` | number | 否 | 上游模式 |

返回 MCP text content，JSON：`{ "hits": [ { "title", "url", "snippet", "source": "windsurf" } ] }`

`image_caption`

| arg | 类型 | 必填 | 说明 |
|-----|------|------|------|
| `image_path` | string | 是* | 本地图片文件路径（PNG/JPG/WebP/GIF） |
| `image_base64` | string | 是* | 原始 base64 图片数据（可带 `data:` 前缀） |
| `mime` | string | 否 | MIME 类型，如 `image/png`（默认按路径扩展名猜测） |
| `question` | string | 否 | 关于图片的问题 / 指令 |

> `image_path` 与 `image_base64` 二选一，至少提供一个。

返回 MCP text content，JSON：`{ "caption": "..." }`

`audio_transcribe`

| arg | 类型 | 必填 | 说明 |
|-----|------|------|------|
| `audio_path` | string | 是* | 本地音频文件路径（wav/mp3/ogg/opus/webm/m4a/flac） |
| `audio_base64` | string | 是* | 原始 base64 音频数据（可带 `data:` 前缀） |
| `timeout` | number | 否 | 超时秒数（默认 60） |

> `audio_path` 与 `audio_base64` 二选一，至少提供一个。格式由后端自动检测（Whisper）。

返回 MCP text content，JSON：`{ "transcribedText": "..." }`

支持两种 MCP 帧协议：`Content-Length`（官方 SDK 客户端）与 NDJSON（手写客户端）。

## MCP 配置使用

MCP server 以 **stdio 子进程**方式运行（`agent-scout --mcp`），通过 stdin/stdout 与 MCP 客户端通信，无需占用端口。

> **通用 MCP 标准**：采用标准 Model Context Protocol（stdio 传输），凡是支持"stdio MCP server"的客户端均可接入——
> Cursor、Claude Desktop、VS Code、Zed、Windsurf、自研 MCP host 等，理论上不受平台限制（macOS / Linux / Windows）。

### 通用配置（任意 MCP host）

在客户端的 MCP 配置文件中加入：

```json
{
  "mcpServers": {
    "agent-scout": {
      "command": "agent-scout",
      "args": ["--mcp"]
    }
  }
}
```

> **两种 `command` 写法**：
> - `"agent-scout"`：已通过 `cargo install` 等激活到 PATH 时最省事（见上文「安装到 PATH」）；
> - 绝对路径：未激活 PATH 时用（如 `/Users/you/agent-scout/target/release/agent-scout`）。

### 各客户端配置示例

**VS Code / Cursor**（`.vscode/mcp.json` 或设置中的 MCP 配置）：

```json
{
  "servers": {
    "agent-scout": {
      "type": "stdio",
      "command": "agent-scout",
      "args": ["--mcp"]
    }
  }
}
```

**Claude Desktop**（`claude_desktop_config.json`）：

```json
{
  "mcpServers": {
    "agent-scout": {
      "command": "agent-scout",
      "args": ["--mcp"]
    }
  }
}
```

### 认证说明

**无需配置 key**——只要本机用过 Devin/Windsurf 登录，MCP server 会自动识别并提取 key（见上文「认证」章节）。

如需显式指定 key，可通过环境变量注入：

```json
{
  "mcpServers": {
    "agent-scout": {
      "command": "agent-scout",
      "args": ["--mcp"],
      "env": {
        "WINDSURF_API_KEY": "devin-session-token$..."
      }
    }
  }
}
```

配置完成后重启 MCP 客户端，即可在对话中调用 `web_search` / `image_caption` / `audio_transcribe` 工具。

## Skills 使用

仓库附带一个规范 skill，供 AI agent 通过 skill 直接调用 web 搜索、识图与转写：

```
skills/agent-scout-search/
├── SKILL.md           # 使用指引（frontmatter + 步骤，含搜索、识图与转写）
└── agents/openai.yaml # UI 元数据
```

- **skill 名**：`agent-scout-search`
- **功能**：按 SKILL.md 指引，用 `agent-scout` 执行搜索并解析 JSON hits、用 `agent-scout caption` 描述/分析本地图片、或用 `agent-scout transcribe` 转写本地音频
- **通用标准**：采用标准 Agent Skill 规范（`SKILL.md` + YAML frontmatter），理论上支持任何按此规范加载 skills 的 agent——
  InsCode、Codex、以及兼容该规范的其他 agent 框架，不受平台限制。

### 安装到 agent 的 skills 目录

方式 A：软链接到 InsCode 全局 skills（自动发现）：

```bash
ln -sfn "$(pwd)/skills/agent-scout-search" ~/.config/inscode/skills/agent-scout-search
```

方式 B：复制/链接到 Codex skills 目录：

```bash
mkdir -p ~/.codex/skills
ln -sfn "$(pwd)/skills/agent-scout-search" ~/.codex/skills/agent-scout-search
```

安装后，agent 在需要联网搜索时会自动匹配 `agent-scout-search` skill 并按其指引执行。

## 安全说明

- 不要提交真实 token。
- session token 会过期，返回 401 时重新 `config set`。
- 自动提取的 key 是本机明文存储的敏感凭证，仅在内存使用，不落盘、不打印完整值。
- 非 Windsurf/Devin 官方产品。

## 示例输出

一次 `agent-scout "rust async" --limit 2` 的 stdout 输出形如：

```json
{
  "hits": [
    {
      "title": "Async Programming in Rust",
      "url": "https://rust-lang.github.io/async-book/",
      "snippet": "A book about asynchronous programming in Rust...",
      "source": "windsurf"
    }
  ]
}
```

> 提示：配合 `jq` 可以快速取字段，如 `agent-scout "query" | jq '.hits[].url'`。

一次 `agent-scout caption ~/Pictures/screen.png --question "这个界面用了什么框架？"` 的 stdout 输出形如：

```text
界面采用了 React 与 Tailwind CSS 构建，左侧为侧边导航栏，顶部为搜索栏，
主体区域是一个数据表格，包含用户列表及其状态标签。
```

识图输出为纯文本描述（无 JSON 包裹），便于直接拼入对话；如需脚本解析，加 `--json` 得到 `{"caption": "..."}`。

一次 `agent-scout transcribe ~/Recordings/meeting.wav` 的 stdout 输出形如：

```text
好的，会议开始。首先回顾一下上周的进展……
```

转写输出为纯文本（无 JSON 包裹）；如需脚本解析，加 `--json` 得到 `{"transcribedText": "..."}`。

## 错误日志

每次运行会在 `~/.config/windsurf-search/logs/agent-scout-YYYY-MM-DD.log` 写入日志：

- 读取 key 失败、搜索/识图/转写失败、config 出错 → `[ERROR]`
- 搜索/识图/转写成功 → `[INFO]`
- **自动清理**：每次写入时删除超过 7 天的旧日志，并最多保留 30 个文件

```bash
tail -f ~/.config/windsurf-search/logs/*.log   # 实时查看
```

## 致谢

本项目参考了开源项目 **[windsurf-search-mcp](https://github.com/mimimaster/windsurf-search-mcp)**（Node.js 实现），
核心逻辑在此基础上用 Rust 重写，并对认证、日志、Skill 等能力做了增强。

## License

MIT