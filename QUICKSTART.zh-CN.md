# 快速开始（Quick Start）

> **[English](QUICKSTART.md) | [简体中文](QUICKSTART.zh-CN.md)**

`agent-scout` 是一个开源的通用 **web 搜索 + 识图 + 音频转写 + AI 代码搜索**工具，提供三种使用形态：
**CLI** 命令行、**MCP server**（接入任意 MCP 客户端）、**Agent Skill**（供 AI agent 调用）。
基于 Windsurf/Devin 服务端接口（`GetWebSearchResults` / `GetImageCaption` / `GetTranscription` / `GetDevstralStream`），**纯 Rust** 编写，
**macOS / Linux / Windows 三平台支持**，**零配置即用**——只要本机用 Devin 或 Windsurf 登录过，就会自动识别登录凭证。AI 代码搜索（`fc`）用自然语言查询即可在本地代码库中定位相关文件与行号。

> 完整的命令、认证、MCP、skill 说明见 **[README.zh-CN.md](README.zh-CN.md)**。
> 下文 `$BIN` 表示已激活到 PATH 的可执行命令 `agent-scout`；未激活时替换为 `target/release/agent-scout`。

## 💡 开箱即用

拿到二进制即可直接搜索 / 识图 / 转写，**零配置**：

```bash
$BIN "tauri window drag region" --limit 3   # 搜索
$BIN caption ~/Pictures/photo.png           # 识图
$BIN transcribe ~/Recordings/meeting.wav    # 转写
```

只要本机用过 Devin / Windsurf 登录，程序会自动识别登录凭证完成搜索、识图或转写，无需手动设置 API key。
（CLI、MCP、Skill 三种形态都遵循这一零配置原则。）

**不想编译？一键安装**（从 GitHub Releases 下载最新二进制，无需 Rust 工具链）：

```bash
AGENT_SCOUT_REPO=tasselx/agent-scout curl -fsSL \
  https://raw.githubusercontent.com/tasselx/agent-scout/main/install.sh | bash
```

脚本自动检测平台/架构，安装到 `~/.local/bin` 并提示 PATH 配置；Release 产物均附带
`SHA256SUMS` 校验和可核对。需要 Rust 工具链自己构建时，见下一节。

## 1. 构建 & 激活

需要 Rust 工具链（`cargo`）：

```bash
# 编译 release
cargo build --release
# 生成的可执行文件：target/release/agent-scout
BIN=agent-scout
```

### 安装到 PATH（全局直接调用）

**方式 A：`cargo install`（推荐，装到 `~/.cargo/bin`）**

```bash
cd /path/to/agent-scout        # 先进入项目目录（--path . 指向当前目录）
cargo install --path . --root "$HOME/.cargo"
# 之后可直接用 agent-scout 命令
agent-scout "some query" --limit 3
```

> 提示：也可用绝对路径 `cargo install --path /path/to/agent-scout --root "$HOME/.cargo"` 免去 `cd`。

**方式 B：软链接**

```bash
ln -sf "$(pwd)/target/release/agent-scout" ~/.local/bin/agent-scout
```

**方式 C：key 用环境变量激活（无本机登录时）**

```bash
export WINDSURF_API_KEY='devin-session-token$...'
# 或单次：WINDSURF_API_KEY='...' agent-scout "query"
```

> 三种方式任选其一即可。方式 A/B 安装后通常**零配置**直接可用（自动识别本机 Devin/Windsurf key）。

## 2. 直接搜索（无需任何配置）

```bash
$BIN "tauri window drag region" --limit 3
# 自动从本机登录的 Devin/Windsurf 提取 key，无需手动设置
# stdout 输出 JSON:
# { "hits": [ { "title": "...", "url": "...", "snippet": "...", "source": "windsurf" } ] }
```

常用参数：

| 参数 | 说明 |
|------|------|
| `--limit N` | 结果数（1–10，默认 5） |
| `--domain d` | 域名过滤，如 `github.com` |
| `--mode m` | 上游搜索模式 |
| `--pretty` | 美化输出 JSON（缩进格式化，便于人读） |
| `--api-key k` | 显式指定 key（覆盖自动识别） |

### 💡 关键：默认读取 key 最省事

**什么都不用配**——只要本机用过 Devin / Windsurf 登录，`agent-scout` 会自动从本地安装
（`state.vscdb` / `credentials.toml`）识别 key，直接搜索即可：

```bash
$BIN "rust async runtime" --limit 3
```

只有以下情况才需要手动指定 key：

1. **本机从未登录过** Devin/Windsurf → 用 `--api-key` 或 `WINDSURF_API_KEY` 环境变量；
2. **希望使用特定账号 / 不同 key** → 用 `--api-key` 覆盖；
3. **key 过期（返回 401）** → 重新登录 Devin，或 `$BIN config set 'devin-session-token$...'`。

> 默认行为：`--api-key` → `WINDSURF_API_KEY` 环境变量 → key 文件 → 本地自动识别。
> 前三种都未提供时，自动跳到本地识别——所以日常使用往往只需 `$BIN "查询词"` 一条命令。

### 管道处理结果（jq）

搜索 stdout 是纯 JSON，可直接用 `jq` 提取字段：

```bash
$BIN "tauri" --limit 5 | jq '.hits[] | {title, url}'   # 只取标题和链接
$BIN "rust async" | jq '.hits | length'                # 统计命中数
$BIN "tokio" --limit 10 > results.json                 # 保存到文件
```

## 2b. 识图（描述 / 分析本地图片）

同一套零配置原则也适用于识图：`agent-scout caption <图片路径>` 会读取本地图片、
base64 编码后发给服务端 `GetImageCaption`，把模型的视觉分析打印到 stdout。

```bash
# 描述一张图片
$BIN caption ~/Pictures/screenshot.png

# 针对图片提问 / 下指令
$BIN caption ~/Pictures/photo.jpg --question "照片里有哪些人？"

# 指定 mime 类型（默认按扩展名猜测，如 png/jpg/webp/gif）
$BIN caption ~/tmp/image --mime image/webp

# 输出 JSON（便于脚本解析）
$BIN caption ~/tmp/image --json
```

常用参数：

| 参数 | 说明 |
|------|------|
| `--question "..."` | 对图片的问题 / 指令 |
| `--mime m` | MIME 类型，如 `image/png`（默认按扩展名猜测） |
| `--json` | 输出 `{"caption": "..."}` 便于脚本解析（默认纯文本） |
| `--pretty` | 配合 `--json` 美化输出 JSON |
| `--api-key k` | 显式指定 key（覆盖自动识别） |

stdout 输出为纯文本描述（无 JSON 包裹）：

```text
界面采用了 React 与 Tailwind CSS 构建，左侧为侧边导航栏，顶部为搜索栏……
```

> 脚本解析：加 `--json` 得到 `{"caption": "..."}`，可用 `jq -r '.caption'` 取值，如
> `$BIN caption ~/Pictures/photo.png --json | jq -r '.caption'`。

## 2c. 转写（音频转文字）

同一套零配置原则也适用于转写：`agent-scout transcribe <音频路径>` 会读取本地音频、
base64 编码后发给服务端 `GetTranscription`（后端为 OpenAI Whisper），把转写文本打印到 stdout。
格式由后端自动检测（wav/mp3/ogg/opus/webm/m4a/flac），无需指定。

```bash
# 转写一段音频
$BIN transcribe ~/Recordings/meeting.wav

# 输出 JSON（便于脚本解析）
$BIN transcribe ~/Recordings/meeting.mp3 --json

# 加大超时（转写较慢，默认 60 秒）
$BIN transcribe ~/Recordings/long.ogg --timeout 120
```

常用参数：

| 参数 | 说明 |
|------|------|
| `--timeout N` | 超时秒数（默认 60） |
| `--json` | 输出 `{"transcribedText": "..."}` 便于脚本解析（默认纯文本） |
| `--pretty` | 配合 `--json` 美化输出 JSON |
| `--api-key k` | 显式指定 key（覆盖自动识别） |

stdout 输出为纯文本转写（无 JSON 包裹）：

```text
好的，会议开始。首先回顾一下上周的进展……
```

> 脚本解析：加 `--json` 得到 `{"transcribedText": "..."}`，可用 `jq -r '.transcribedText'` 取值，如
> `$BIN transcribe ~/Recordings/meeting.wav --json | jq -r '.transcribedText'`。

## 2d. Fast context：AI 语义代码搜索

`agent-scout fc` 用自然语言查询对**本地代码库**做语义搜索——由 Windsurf 的 Devstral 模型（`GetDevstralStream`）驱动的 AI 检索。它不是关键词匹配，而是模型多轮调用本地命令（rg / readfile / tree / ls / glob），最终返回相关文件路径 + 行号范围 + grep 关键词，pretty 输出还附带代码片段。

```bash
# 搜索当前目录
$BIN fc "认证逻辑在哪里？"

# 搜索指定项目
$BIN fc "数据库连接池的实现" --path /path/to/project

# 更深搜索、更多结果、排除大目录
$BIN fc "auth flow" --path . --turns 4 --max-results 15 --exclude node_modules,dist,target

# 结构化输出（脚本解析）
$BIN fc "auth" --path . --json | jq '.files[].path'
```

常用参数：

| 参数 | 默认 | 说明 |
|------|------|------|
| `--path DIR` | 当前目录 | 要搜索的项目根目录 |
| `--turns N` | 3 (1-5) | 搜索轮次；越多越深，但更慢、更耗配额 |
| `--depth N` | 3 (1-6) | repo map 目录树深度；超大仓库用更低值，小项目用更高值 |
| `--max-results N` | 10 (1-30) | 最多返回文件数 |
| `--exclude a,b` | — | 排除的目录/模式，如 `node_modules,dist,.git` |
| `--json` | 关 | 输出结构化 JSON（替代 pretty 文本） |
| `--pretty` | 关 | 美化 JSON 输出 |
| `--api-key k` | 自动 | 显式指定 key（覆盖自动识别） |

pretty 输出包含命中的代码片段（复用搜索期间已读过的文件内容，零额外 IO），以及 grep 关键词和统计/config 诊断：

```text
The following code sections were retrieved:

Path: /path/to/project/src/auth/handler.py
Lines: L10-L60
L10:class AuthHandler:
L11:    def login(self, ...):
...

grep keywords: authenticate, jwt.*verify, session.*token

[fast-context stats] commands_seen=15, commands_executed=15, commands_useful=13, ...
[fast-context config] {"treeDepth":3,"treeSizeKB":3.8,"fellBack":false,...}
```

同样的能力也以 `fast_context_search` MCP 工具形式提供（见第 4 节）。这里同样零配置——本机 Devin/Windsurf key 自动识别。

> 注意：fast-context 会消耗 Windsurf 账户配额（每轮一次模型调用，默认每查询 4 次），不适合高频批量调用。

## 3. 验证连接

```bash
$BIN config test
# testing key devin-sessio…eqTd_g (devin-session-token (session token)) with query "connectivity"
# OK: got 1 result(s)
```

## 4. 作为 MCP 服务运行

```bash
$BIN --mcp
```

采用标准 Model Context Protocol（stdio 传输），**支持任何接入 stdio MCP server 的客户端**——
Cursor、Claude Desktop、VS Code、Zed、Windsurf、自研 MCP host 等，macOS / Linux / Windows 通用。

暴露 `web_search` 工具（`query` / `limit` / `domain` / `mode`）、`image_caption` 工具
（`image_path` / `image_base64` / `mime` / `question`）与 `audio_transcribe` 工具
（`audio_path` / `audio_base64` / `timeout`），
支持 `Content-Length` 与 NDJSON 两种帧协议。接入 MCP 客户端：

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

> `command` 用 `"agent-scout"` 即可（已激活到 PATH 时最省事）；未激活 PATH 则用绝对路径，如
> `"command": "/Users/you/agent-scout/target/release/agent-scout"`。

## 5. key 管理与认证

key 解析优先级：`--api-key` → 环境变量 → key 文件 → **本地登录自动识别**。

```bash
# 查看当前 key 状态
$BIN config show

# 手动保存 key（chmod 600，写入 ~/.config/windsurf-search/api-key）
$BIN config set 'devin-session-token$...'

# 清除已保存 key
$BIN config clear
```

环境变量：`WINDSURF_API_KEY`（或旧版 `WINDSURFAPI_CODEIUM_API_KEY`）。

### 自动识别来源

| 来源 | 平台 | 路径 |
|------|------|------|
| Devin CLI 凭据 | Linux/WSL | `~/.local/share/devin/credentials.toml` |
| Devin `state.vscdb` | macOS | `~/Library/Application Support/Devin/User/globalStorage/state.vscdb` |
| Devin `state.vscdb` | Windows | `%APPDATA%\Devin\User\globalStorage\state.vscdb` |
| Devin `state.vscdb` | Linux | `~/.config/Devin/User/globalStorage/state.vscdb` |
| Deviv / Windsurf 同名位置 | 全平台 | 兼容回退 |

## 6. 错误日志（自动清理）

工具会写入**按天分文件**的错误/信息日志：

```
~/.config/windsurf-search/logs/agent-scout-YYYY-MM-DD.log
```

- 每次运行读取 key 失败、搜索/识图/转写失败、config 出错时写入 `[ERROR]`
- 搜索/识图/转写成功时写入 `[INFO]`
- **自动清理**：每次写入时自动删除超过 7 天的旧日志，并最多保留 30 个文件

```bash
# 查看日志
tail -f ~/.config/windsurf-search/logs/*.log
```

## 7. 测试

```bash
./test-search.sh            # 完整测试（含真实搜索）
./test-search.sh --quick    # 只测离线部分
cargo test                  # 单元 + 集成测试
```

## 8. Skills 使用（供 agent 调用）

仓库附带一个规范 skill，供 AI agent 通过 skill 直接调用 web 搜索、识图与转写。
采用标准 Agent Skill 规范（`SKILL.md` + YAML frontmatter），**理论上支持任何按此规范加载 skills 的 agent**——
InsCode、Codex 及兼容框架，macOS / Linux / Windows 通用。

```
skills/agent-scout-search/
├── SKILL.md           # 使用指引（frontmatter + 步骤，含搜索、识图与转写）
└── agents/openai.yaml # UI 元数据
```

**skill 名**：`agent-scout-search`

安装到 agent 的 skills 目录（以 InsCode 全局为例）：

```bash
ln -sfn "$(pwd)/skills/agent-scout-search" ~/.config/inscode/skills/agent-scout-search
```

Codex 则链接到 `~/.codex/skills/agent-scout-search`。安装后 agent 在需要联网搜索时会自动匹配该 skill。

## 9. CLI 命令速查

| 命令 | 作用 |
|------|------|
| `agent-scout "<查询>" [--limit N] [--domain d] [--mode m] [--pretty] [--api-key k]` | 执行 web 搜索，stdout 输出 JSON hits（`--pretty` 美化输出） |
| `agent-scout caption <图片路径> [--question "..." --mime m --json --pretty] [--api-key k]` | 识图：描述/分析本地图片（`--json` 输出 `{"caption": "..."}`，`--pretty` 美化 JSON） |
| `agent-scout transcribe <音频路径> [--timeout N --json --pretty] [--api-key k]` | 转写：语音转文字（`--json` 输出 `{"transcribedText": "..."}`，`--pretty` 美化 JSON） |
| `agent-scout --mcp` | 以 MCP stdio server 运行 |
| `agent-scout config set [key]` | 保存 key（chmod 600）；无 key 时交互输入 |
| `agent-scout config show` | 查看当前 key 状态（掩码显示） |
| `agent-scout config test [query]` | 连通性测试（真实搜索验证 key） |
| `agent-scout config clear` | 删除已保存的 key 文件 |

> **卸载**：移除 PATH 上的命令即可。
> - `cargo install` 装的：`rm "$(which agent-scout)"`（或 `cargo uninstall --root "$HOME/.cargo" agent-scout`）
> - 软链接的：`rm ~/.local/bin/agent-scout`
> - 需清理的本地文件：`~/.config/windsurf-search/`（key、日志）

## 退出码

`0`=成功，`1`=错误，`2`=用法错误。诊断信息走 stderr；搜索的 stdout 为纯 JSON，识图（`caption`）/转写（`transcribe`）的 stdout 为纯文本。

## 安全提示

- 不要提交真实 token；`config show` 只显示掩码。
- 自动识别的 key 仅在内存使用，不落盘、不打印完整值。
- session token 会过期，返回 401 时重新登录或 `config set`。