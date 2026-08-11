# Quick Start

> **[English](QUICKSTART.md) | [简体中文](QUICKSTART.zh-CN.md)**

`agent-scout` is an open-source, general-purpose **web search + image caption + audio transcription + AI code search** tool with three usage forms:
**CLI** command line, **MCP server** (pluggable into any MCP client), **Agent Skill** (for AI agents to call).
It is backed by Windsurf/Devin server-side APIs (`GetWebSearchResults` / `GetImageCaption` / `GetTranscription` / `GetDevstralStream`), written in **pure Rust**,
with **macOS / Linux / Windows support** and **zero-config usage** — as long as you have logged into Devin or Windsurf locally, your credentials are auto-detected. The AI code search (`fc`) finds relevant files and line ranges in a local codebase from a natural-language query.

> Full command, auth, MCP, and skill documentation: **[README.md](README.md)**.
> Below, `$BIN` refers to the `agent-scout` command activated on PATH; if not activated, replace with `target/release/agent-scout`.

## 💡 Works out of the box

Get the binary and search / caption / transcribe directly, **zero config**:

```bash
$BIN "tauri window drag region" --limit 3   # search
$BIN caption ~/Pictures/photo.png           # image caption
$BIN transcribe ~/Recordings/meeting.wav    # transcribe
```

As long as you have used Devin / Windsurf locally, the program auto-detects your credentials and completes the search, caption, or transcription — no manual API key setup. (All three forms — CLI, MCP, Skill — follow this zero-config principle.)

**Don't want to build? One-line install** (downloads the latest binary from GitHub Releases; no Rust toolchain needed):

```bash
AGENT_SCOUT_REPO=tasselx/agent-scout curl -fsSL \
  https://raw.githubusercontent.com/tasselx/agent-scout/main/install.sh | bash
```

The script auto-detects platform/arch, installs to `~/.local/bin`, and prints PATH instructions; Release artifacts ship with `SHA256SUMS` checksums for verification. If you have a Rust toolchain and prefer to build yourself, see the next section.

## 1. Build & activate

You need the Rust toolchain (`cargo`):

```bash
# build release
cargo build --release
# the executable: target/release/agent-scout
BIN=agent-scout
```

### Install to PATH (global invocation)

**Option A: `cargo install` (recommended; installs to `~/.cargo/bin`)**

```bash
cd /path/to/agent-scout        # enter the project directory first (--path . points at the current dir)
cargo install --path . --root "$HOME/.cargo"
# then call agent-scout directly
agent-scout "some query" --limit 3
```

> Tip: you can also use an absolute path `cargo install --path /path/to/agent-scout --root "$HOME/.cargo"` to skip the `cd`.

**Option B: symlink**

```bash
ln -sf "$(pwd)/target/release/agent-scout" ~/.local/bin/agent-scout
```

**Option C: activate via environment variable key (no local login)**

```bash
export WINDSURF_API_KEY='devin-session-token$...'
# or one-off: WINDSURF_API_KEY='...' agent-scout "query"
```

> Pick any one of the three. After option A/B, it is usually **zero-config** (auto-detects the local Devin/Windsurf key).

## 2. Search directly (no config needed)

```bash
$BIN "tauri window drag region" --limit 3
# key auto-extracted from the locally logged-in Devin/Windsurf; no manual setup
# stdout emits JSON:
# { "hits": [ { "title": "...", "url": "...", "snippet": "...", "source": "windsurf" } ] }
```

Common flags:

| Flag | Description |
|------|-------------|
| `--limit N` | Result count (1–10, default 5) |
| `--domain d` | Domain filter, e.g. `github.com` |
| `--mode m` | Upstream search mode |
| `--pretty` | Pretty-print JSON output (indented, human-readable) |
| `--api-key k` | Explicit key (overrides auto-detection) |

### 💡 Key point: the default key lookup is the least work

**Configure nothing** — as long as you have logged into Devin / Windsurf locally, `agent-scout` auto-detects the key from your local install (`state.vscdb` / `credentials.toml`) and searches directly:

```bash
$BIN "rust async runtime" --limit 3
```

You only need to specify a key manually in these cases:

1. **Never logged into Devin/Windsurf on this machine** → use `--api-key` or the `WINDSURF_API_KEY` env var;
2. **You want a specific account / different key** → override with `--api-key`;
3. **Key expired (401)** → re-login to Devin, or `$BIN config set 'devin-session-token$...'`.

> Default behavior: `--api-key` → `WINDSURF_API_KEY` env var → key file → local auto-detection.
> When none of the first three are provided, it falls through to local detection — so day-to-day use is often just `$BIN "query"`.

### Process results with jq

Search stdout is pure JSON, so you can extract fields directly with `jq`:

```bash
$BIN "tauri" --limit 5 | jq '.hits[] | {title, url}'   # titles and links only
$BIN "rust async" | jq '.hits | length'                # count hits
$BIN "tokio" --limit 10 > results.json                 # save to file
```

## 2b. Image caption (describe / analyze local images)

The same zero-config principle applies to image caption: `agent-scout caption <image-path>` reads a local image, base64-encodes it, sends it to the server-side `GetImageCaption`, and prints the model's vision analysis to stdout.

```bash
# describe an image
$BIN caption ~/Pictures/screenshot.png

# ask a question / give an instruction about the image
$BIN caption ~/Pictures/photo.jpg --question "Who is in the photo?"

# specify the mime type (default guessed from extension, e.g. png/jpg/webp/gif)
$BIN caption ~/tmp/image --mime image/webp

# output JSON (easier for scripts)
$BIN caption ~/tmp/image --json
```

Common flags:

| Flag | Description |
|------|-------------|
| `--question "..."` | Question / instruction about the image |
| `--mime m` | MIME type, e.g. `image/png` (default guessed from extension) |
| `--json` | Emit `{"caption": "..."}` for scripting (default is plain text) |
| `--pretty` | Pretty-print JSON output with `--json` |
| `--api-key k` | Explicit key (overrides auto-detection) |

stdout is a plain-text description (no JSON wrapper):

```text
The interface is built with React and Tailwind CSS, with a left sidebar navigation, a search bar at the top...
```

> Scripting: add `--json` to get `{"caption": "..."}`, then extract with `jq -r '.caption'`, e.g.
> `$BIN caption ~/Pictures/photo.png --json | jq -r '.caption'`.

## 2c. Transcribe (audio to text)

The same zero-config principle applies to transcription: `agent-scout transcribe <audio-path>` reads local audio, base64-encodes it, sends it to the server-side `GetTranscription` (backed by OpenAI Whisper), and prints the transcript to stdout. The format is auto-detected by the backend (wav/mp3/ogg/opus/webm/m4a/flac), no need to specify.

```bash
# transcribe an audio clip
$BIN transcribe ~/Recordings/meeting.wav

# output JSON (easier for scripts)
$BIN transcribe ~/Recordings/meeting.mp3 --json

# raise the timeout (transcription is slow; default 60s)
$BIN transcribe ~/Recordings/long.ogg --timeout 120
```

Common flags:

| Flag | Description |
|------|-------------|
| `--timeout N` | Timeout in seconds (default 60) |
| `--json` | Emit `{"transcribedText": "..."}` for scripting (default is plain text) |
| `--pretty` | Pretty-print JSON output with `--json` |
| `--api-key k` | Explicit key (overrides auto-detection) |

stdout is a plain-text transcript (no JSON wrapper):

```text
OK, the meeting begins. First, let's review last week's progress...
```

> Scripting: add `--json` to get `{"transcribedText": "..."}`, then extract with `jq -r '.transcribedText'`, e.g.
> `$BIN transcribe ~/Recordings/meeting.wav --json | jq -r '.transcribedText'`.

## 2d. Fast context: AI semantic code search

`agent-scout fc` searches a **local codebase** with a natural-language query — an AI-driven semantic search powered by Windsurf's Devstral model (`GetDevstralStream`). Instead of keyword matching, the model runs multiple rounds of local commands (rg / readfile / tree / ls / glob) and returns the relevant file paths + line ranges + grep keywords, with code snippets in the pretty output.

```bash
# search the current directory
$BIN fc "where is the authentication logic?"

# search a specific project
$BIN fc "数据库连接池的实现" --path /path/to/project

# deeper search, more results, exclude heavy dirs
$BIN fc "auth flow" --path . --turns 4 --max-results 15 --exclude node_modules,dist,target

# structured output for scripting
$BIN fc "auth" --path . --json | jq '.files[].path'
```

Common flags:

| Flag | Default | Description |
|------|---------|-------------|
| `--path DIR` | current dir | Project root to search |
| `--turns N` | 3 (1-5) | Search rounds; more = deeper but slower & more quota |
| `--depth N` | 3 (1-6) | Repo-map tree depth; lower for huge monorepos, higher for small projects |
| `--max-results N` | 10 (1-30) | Max files to return |
| `--exclude a,b` | — | Dirs/patterns to exclude, e.g. `node_modules,dist,.git` |
| `--json` | off | Emit structured JSON instead of the pretty text |
| `--pretty` | off | Pretty-print the JSON output |
| `--api-key k` | auto | Explicit key (overrides auto-detection) |

The pretty output includes the matched code snippets (reusing files already read during search, so no extra IO), plus grep keywords and stats/config diagnostics:

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

For scripting, use `--json`:

```bash
# Extract the matched file paths
$BIN fc "auth" --path . --json | jq '.files[].path'

# Inspect the first hit (path + ranges)
$BIN fc "auth" --path . --json | jq '.files[0]'

# Pretty-printed JSON
$BIN fc "auth" --path . --json --pretty

# Feed the rg keywords back into a real grep
$BIN fc "auth" --path . --json | jq -r '.rg_patterns[]' | while read -r p; do rg "$p" src; done
```

Example `--json` payload:

```json
{
  "files": [
    { "path": "src/auth/handler.py", "full_path": "/proj/src/auth/handler.py", "ranges": [[10, 60], [120, 180]] },
    { "path": "src/middleware/jwt.py", "full_path": "/proj/src/middleware/jwt.py", "ranges": [[1, 40]] }
  ],
  "rg_patterns": ["authenticate", "jwt.*verify", "session.*token"],
  "stats": { "commandsSeen": 15, "commandsUseful": 13, "commandsInvalid": 0, "cacheHits": 0 },
  "meta": { "treeDepth": 3, "treeSizeKB": 3.8, "fellBack": false }
}
```

The same capability is exposed as the `fast_context_search` MCP tool (see section 4) — e.g. with `project_path` pointing at the repo root, `max_turns`/`max_results` tuning the depth, and `exclude_paths` skipping heavy dirs:

```json
{
  "name": "fast_context_search",
  "arguments": {
    "query": "where is the database pool initialized",
    "project_path": "/path/to/project",
    "max_turns": 3,
    "max_results": 10,
    "exclude_paths": ["node_modules", "dist", "target"]
  }
}
```

Zero-config applies here too — the local Devin/Windsurf key is auto-detected.

> Note: fast-context consumes Windsurf account quota (one model call per turn; 4 calls by default per query), so it is not meant for high-frequency batch use.

## 3. Verify connectivity

```bash
$BIN config test
# testing key devin-sessio…eqTd_g (devin-session-token (session token)) with query "connectivity"
# OK: got 1 result(s)
```

## 4. Run as an MCP server

```bash
$BIN --mcp
```

Uses the standard Model Context Protocol (stdio transport) and **works with any client that supports stdio MCP servers** —
Cursor, Claude Desktop, VS Code, Zed, Windsurf, custom MCP hosts, etc., on macOS / Linux / Windows.

Exposes the `web_search` tool (`query` / `limit` / `domain` / `mode`), the `image_caption` tool
(`image_path` / `image_base64` / `mime` / `question`), the `audio_transcribe` tool
(`audio_path` / `audio_base64` / `timeout`), and the `fast_context_search` tool
(`query` / `project_path` / `tree_depth` / `max_turns` / `max_results` / `exclude_paths`),
supporting both `Content-Length` and NDJSON framing. Plug into an MCP client:

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

> Use `"agent-scout"` for `command` when it is activated on PATH (easiest); otherwise use an absolute path, e.g.
> `"command": "/Users/you/agent-scout/target/release/agent-scout"`.

## 5. Key management & authentication

Key resolution priority: `--api-key` → environment variable → key file → **local login auto-detection**.

```bash
# view current key status
$BIN config show

# save a key manually (chmod 600, written to ~/.config/windsurf-search/api-key)
$BIN config set 'devin-session-token$...'

# remove the saved key
$BIN config clear
```

Environment variables: `WINDSURF_API_KEY` (or legacy `WINDSURFAPI_CODEIUM_API_KEY`).

### Auto-detection sources

| Source | Platform | Path |
|--------|----------|------|
| Devin CLI credentials | Linux/WSL | `~/.local/share/devin/credentials.toml` |
| Devin `state.vscdb` | macOS | `~/Library/Application Support/Devin/User/globalStorage/state.vscdb` |
| Devin `state.vscdb` | Windows | `%APPDATA%\Devin\User\globalStorage\state.vscdb` |
| Devin `state.vscdb` | Linux | `~/.config/Devin/User/globalStorage/state.vscdb` |
| Devin / Windsurf same-named locations | All platforms | compatibility fallback |

## 6. Error logs (auto-cleanup)

The tool writes **per-day** error/info logs:

```
~/.config/windsurf-search/logs/agent-scout-YYYY-MM-DD.log
```

- Writes `[ERROR]` on key read failures, search/caption/transcribe failures, config errors
- Writes `[INFO]` on successful search/caption/transcribe
- **Auto cleanup**: on each write, logs older than 7 days are deleted, keeping at most 30 files

```bash
# view logs
tail -f ~/.config/windsurf-search/logs/*.log
```

## 7. Tests

```bash
./test-search.sh            # full test (incl. real search)
./test-search.sh --quick    # offline-only tests
cargo test                  # unit + integration tests
```

## 8. Skills usage (for agents)

The repo ships a standard skill for AI agents to call web search, image caption, and transcription directly.
It follows the standard Agent Skill spec (`SKILL.md` + YAML frontmatter) and **works with any agent that loads skills this way** —
InsCode, Codex, and compatible frameworks, on macOS / Linux / Windows.

```
skills/agent-scout-search/
├── SKILL.md           # usage guide (frontmatter + steps; search, caption & transcribe)
└── agents/openai.yaml # UI metadata
```

**skill name**: `agent-scout-search`

Install into an agent's skills directory (InsCode global example):

```bash
ln -sfn "$(pwd)/skills/agent-scout-search" ~/.config/inscode/skills/agent-scout-search
```

For Codex, link to `~/.codex/skills/agent-scout-search`. After installation, agents auto-match the skill when they need web search.

## 9. CLI command reference

| Command | Purpose |
|---------|---------|
| `agent-scout "<query>" [--limit N] [--domain d] [--mode m] [--pretty] [--api-key k]` | Web search; stdout emits JSON hits (`--pretty` beautifies) |
| `agent-scout caption <image> [--question "..." --mime m --json --pretty] [--api-key k]` | Image caption: describe/analyze a local image (`--json` emits `{"caption": "..."}`, `--pretty` beautifies JSON) |
| `agent-scout transcribe <audio> [--timeout N --json --pretty] [--api-key k]` | Transcription: speech-to-text (`--json` emits `{"transcribedText": "..."}`, `--pretty` beautifies JSON) |
| `agent-scout fc <query> [--path DIR] [--turns N] [--depth N] [--max-results N] [--exclude a,b] [--json] [--api-key k]` | AI semantic code search over a local codebase (fast-context) |
| `agent-scout --mcp` | Run as MCP stdio server |
| `agent-scout config set [key]` | Save a key (chmod 600); interactive input when no key given |
| `agent-scout config show` | Show current key status (masked) |
| `agent-scout config test [query]` | Connectivity test (real search to verify the key) |
| `agent-scout config clear` | Delete the saved key file |

> **Uninstall**: remove the command from PATH.
> - Installed via `cargo install`: `rm "$(which agent-scout)"` (or `cargo uninstall --root "$HOME/.cargo" agent-scout`)
> - Symlinked: `rm ~/.local/bin/agent-scout`
> - Local files to clean up: `~/.config/windsurf-search/` (key, logs)

## Exit codes

`0` = success, `1` = error, `2` = usage error. Diagnostics go to stderr; search stdout is pure JSON, caption/transcribe stdout is plain text.

## Security notes

- Never commit real tokens; `config show` only displays a masked value.
- Auto-detected keys are used in memory only, never persisted to disk, never printed in full.
- Session tokens expire — on a 401, re-login or `config set`.

