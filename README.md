# agent-scout

**web search + image caption + audio transcription + AI code search** CLI tool + MCP server + Agent Skill,
backed by Windsurf/Devin server-side APIs (`GetWebSearchResults` / `GetImageCaption` / `GetTranscription` / `GetDevstralStream`), written in **Rust**.

> This is an **open-source general-purpose web search + image caption + transcription + code search tool** that does not depend on any specific client — it ships in three forms:
> **CLI** (direct invocation from scripts/terminal), **MCP server** (pluggable into any MCP client), **Agent Skill** (for AI agents to call).
> It authenticates against Windsurf/Devin, and additionally bundles **automatic local-install credential extraction** and **self-cleaning error logs**.

> **[简体中文](README.zh-CN.md) | English**

## Cross-platform support

`agent-scout` is written in pure Rust — **native support for macOS / Linux / Windows**, single-binary distribution, zero runtime dependencies:

| Platform | Build | Auth source |
|----------|-------|-------------|
| macOS | `cargo build --release` | Devin `~/Library/Application Support/Devin/.../state.vscdb` |
| Linux | `cargo build --release` | Devin `~/.config/Devin/.../state.vscdb`, CLI `credentials.toml` (incl. WSL) |
| Windows | `cargo build --release` | Devin `%APPDATA%\Devin\...\state.vscdb` |

> Credential auto-extraction is routed per-platform internally (see the "Authentication" section); everything else behaves identically across platforms.

## Three usage forms

| Form | Description | Quick entry |
|------|-------------|-------------|
| **CLI** | Search / caption / transcribe / code-search directly from terminal or scripts; stdout emits JSON / text | `agent-scout "query" --limit 3` |
| **MCP server** | Plug into Cursor / Claude Desktop / any MCP host | `agent-scout --mcp` |
| **Agent Skill** | For AI agents (Codex / InsCode, etc.) to call automatically | `skills/agent-scout-search/` |

> 📖 For a quick start, see **[QUICKSTART.md](QUICKSTART.md)**.

## Works out of the box

From downloading the binary to your first search, **no configuration required**:

1. **Download/build** a single executable (no Node runtime, no dependency installation)
2. **Run** `agent-scout "query"` / `agent-scout caption image` / `agent-scout transcribe audio` — automatically detects your local Devin/Windsurf login credentials
3. **Done** — JSON search results / image description / transcribed text are returned directly

**Fastest way: one-line install** (downloads the latest release binary from GitHub Releases; no Rust toolchain needed):

```bash
AGENT_SCOUT_REPO=tasselx/agent-scout curl -fsSL \
  https://raw.githubusercontent.com/tasselx/agent-scout/main/install.sh | bash
```

The script auto-detects platform/arch, installs the binary to `~/.local/bin` (override with
`AGENT_SCOUT_PREFIX`), and prints PATH instructions. You can also run `./install.sh` directly
(see the header comments). Release artifacts ship with `SHA256SUMS` checksums for verification
on the Releases page.

Zero-config flow:

```
agent-scout binary ──▶ auto-extract local key ──▶ server-side search / caption / transcribe / code search ──▶ JSON hits / description / transcript / file ranges
    no API key config         zero manual steps            multi-host auto retry
```

All three forms (CLI / MCP / Skill) follow the same zero-config principle: as long as you have logged into Devin/Windsurf locally, it works directly.

## Features

- 🔍 **Web search**: `GetWebSearchResults` server-side search returning structured `{title, url, snippet, source}` results
- 🖼️ **Image caption**: `GetImageCaption` server-side vision analysis — describe an image or answer questions about it
- 🎙️ **Audio transcription**: `GetTranscription` server-side speech-to-text (Whisper), auto-detected formats (wav/mp3/ogg/opus/webm/m4a/flac)
- 🤖 **AI code search (fast-context)**: `GetDevstralStream` semantic search over a local codebase — natural-language query → relevant file paths + line ranges + grep keywords
- 🎯 **Domain filtering**: `--domain` restricts search scope (e.g. `github.com`)
- 🪄 **Zero-config auth**: automatically detects the local Devin/Windsurf login key, no manual config
- 🧩 **MCP server**: `--mcp` runs over stdio, supporting both `Content-Length` + NDJSON framing
- 🧠 **Agent Skill**: ships the `agent-scout-search` skill for agents
- 📝 **Error logs**: per-day log files with automatic cleanup of stale entries
- 🚀 **Pure Rust**: cross-platform, zero Node runtime dependency, single-binary distribution

## Structure

```
src/
  search.rs     core search: request building, HTTP calls (multi-host retry), result normalization
  caption.rs    image caption: GetImageCaption request building, HTTP calls (multi-host retry), base64/mime
  transcribe.rs transcription: GetTranscription request building, HTTP calls (multi-host retry), base64
  auth.rs       API key resolution (CLI → env → key file → local auto-discovery), config read/write
  log.rs        error/info logs (per-day files, automatic cleanup of old logs)
  mcp.rs        MCP stdio server (NDJSON + Content-Length framing; web_search + image_caption + audio_transcribe tools)
  main.rs       CLI entry (search + caption + transcribe + config subcommands + --mcp)
  fastcontext/  AI semantic code search (fast-context): Devstral protocol client, executor, search loop
skills/
  agent-scout-search/  web search + caption + transcribe skill for agents (SKILL.md + openai.yaml)
tests/
  search_live.rs  integration test: local mock HTTP verifying the search() success path
```

## Build & test

```bash
cargo build --release                       # produces target/release/agent-scout
cargo test                                  # unit tests + integration tests
```

## Release downloads

Prebuilt binaries for three platforms are published with **GitHub Releases** — download and use directly without building:

- **macOS** (arm64 / x86_64): `agent-scout-macos-aarch64` / `agent-scout-macos-x86_64`
- **Linux** (x86_64 / aarch64): `agent-scout-linux-x86_64` / `agent-scout-linux-aarch64`
- **Windows** (x86_64): `agent-scout-windows-x86_64.exe`

> See the **Releases** page for the entry point; every Release ships a `SHA256SUMS` checksum file —
> verify locally with `shasum -a 256 -c SHA256SUMS` after downloading.
> You can also package locally with `build-release.sh` (also generates checksums).
> Packaging details: [`.github/workflows/release.yml`](.github/workflows/release.yml).

## Installing to PATH (activation)

After building, use any of the following to activate `agent-scout` on your PATH for global invocation.

**Option 1: `cargo install` (recommended; installs to `~/.cargo/bin`)**

```bash
cd /path/to/agent-scout        # enter the project directory first (--path . points at the current dir)
cargo install --path . --root "$HOME/.cargo"
# then call directly (~/.cargo/bin is usually on PATH)
agent-scout "some query" --limit 3
```

> Tip: the `.` in `--path .` is the current directory — you must run this inside the project directory;
> or use an absolute path `cargo install --path /path/to/agent-scout --root "$HOME/.cargo"` to skip the `cd`.

**Option 2: symlink into a PATH directory**

```bash
ln -sf "$(pwd)/target/release/agent-scout" ~/.local/bin/agent-scout
# ensure ~/.local/bin is on PATH
```

**Option 3: activate via environment variable key (no local Devin login needed)**

```bash
export WINDSURF_API_KEY='devin-session-token$...'
agent-scout "some query"
# or one-off:
WINDSURF_API_KEY='devin-session-token$...' agent-scout "some query"
```

### Key resolution priority

`--api-key` → `WINDSURF_API_KEY` env var → key file → local auto-discovery.

So after installation it is **usually zero-config** (auto-detects the local Devin/Windsurf key);
for machines without a login, provide a key explicitly via the environment variable or `--api-key`.

## Usage

> Below, `agent-scout` refers to the command activated on PATH; if not activated, replace with `target/release/agent-scout`.

```bash
# Search (stdout emits JSON hits) — no manual config, key auto-discovered locally
agent-scout "tauri window drag region" --limit 5

# Specify a key manually
agent-scout "rust async" --limit 3 --api-key 'devin-session-token$...'

# Run the MCP stdio server (key also auto-discovered)
agent-scout --mcp

# Image caption: describe a local image
agent-scout caption ~/Pictures/photo.png

# Image caption: ask a question about the image
agent-scout caption ~/Pictures/screen.png --question "Which UI framework is this?"

# Image caption: output JSON (easier for scripts)
agent-scout caption ~/Pictures/photo.png --json

# Transcribe: convert local audio to text
agent-scout transcribe ~/Recordings/meeting.wav

# Transcribe: output JSON (easier for scripts)
agent-scout transcribe ~/Recordings/meeting.mp3 --json

# Manage / inspect / test / clear the api key
agent-scout config set 'devin-session-token$...'
agent-scout config show
agent-scout config test 'connectivity check'
agent-scout config clear
```

### Piping & scripting

Search results are plain JSON, so they can be fed straight into `jq` and similar tools; caption/transcribe can be scripted too with `--json`:

```bash
# Titles and links only
agent-scout "tauri" --limit 5 | jq '.hits[] | {title, url}'

# Count hits
agent-scout "rust async" | jq '.hits | length'

# Extract a caption field
agent-scout caption ~/Pictures/photo.png --json | jq -r '.caption'

# Extract a transcript field
agent-scout transcribe ~/Recordings/meeting.wav --json | jq -r '.transcribedText'

# Pretty output (indented for human reading)
agent-scout "tauri" --limit 5 --pretty
agent-scout caption ~/Pictures/photo.png --json --pretty
agent-scout transcribe ~/Recordings/meeting.wav --json --pretty

# Save to file
agent-scout "tokio" --limit 10 > results.json
```

> Tip: `--json` only changes the output format, not the exit code; `--pretty` applies to JSON output only (search is JSON by default; caption/transcribe need `--json`).
> On parse failures, diagnostics go to stderr; stdout stays clean.

### CLI command reference

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

### Exit codes

`0` = success, `1` = error (e.g. no credentials, upstream 4xx/5xx), `2` = usage error.
Diagnostics go to stderr; search stdout is pure JSON, caption/transcribe stdout is plain text (add `--json` for JSON).

## Fast context: AI semantic code search

Beyond web search, `agent-scout` ships **fast-context** — an AI-driven semantic code search over a **local codebase**, powered by Windsurf's Devstral model (`GetDevstralStream`, the same reverse-engineered protocol family used by the other tools).

Instead of matching keywords, it works like this:

```
your query + repo map (tree) + project summary
        │
        ▼
Windsurf Devstral model (multi-turn tool-call loop)
        │  generates rg / readfile / tree / ls / glob commands
        ▼
executed locally in parallel (with gitignore, path fallback, caching)
        │  results fed back, repeated for N turns
        ▼
<ANSWER> XML → relevant file paths + line ranges + grep keywords
```

### CLI usage

```bash
agent-scout fc "where is the authentication logic?" --path /path/to/project
```

Flags:

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

Progress logs go to stderr (prefixed `[fc]`); results go to stdout. The pretty text output includes the matched **code snippets** (reusing files already read during search, so no extra IO), plus grep keywords and stats/config diagnostics:

```
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

For scripting, use `--json` (e.g. `agent-scout fc "auth" --json | jq '.files[].path'`).

### MCP tool

`fast_context_search` is exposed by the MCP server (see [MCP tools](#mcp-tools)) with the same parameters: `query`, `project_path`, `tree_depth`, `max_turns`, `max_results`, `exclude_paths`. It returns the same pretty text (file paths + line ranges + code snippets + keywords + diagnostics).

> Note: fast-context consumes Windsurf account quota (one model call per turn; 4 calls by default per query), so it is not meant for high-frequency batch use.

## Authentication (zero-config)

Key resolution priority:

1. `--api-key <token>`
2. `WINDSURF_API_KEY` / `WINDSURFAPI_CODEIUM_API_KEY` environment variables
3. Key file (`~/.config/windsurf-search/api-key`, `~/.windsurf-search/api-key`, `~/.piwin/windsurf-api-key`)
4. **Local installation auto-extraction**

As long as you have logged into Devin or Windsurf locally, layer 4 auto-extracts the key from your local install — no manual configuration needed.

Keys look like `devin-session-token$...`.

### Auto-extraction sources

The following sources are checked in order; the first valid key wins:

| Source | Type | Platform | Path |
|--------|------|----------|------|
| Devin CLI credentials | TOML | Linux/WSL | `~/.local/share/devin/credentials.toml` |
| Devin `state.vscdb` | SQLite | macOS | `~/Library/Application Support/Devin/User/globalStorage/state.vscdb` |
| Devin `state.vscdb` | SQLite | Windows | `%APPDATA%\Devin\User\globalStorage\state.vscdb` |
| Devin `state.vscdb` | SQLite | Linux | `~/.config/Devin/User/globalStorage/state.vscdb` |
| Devin / Windsurf same-named locations | SQLite | All | compatibility fallback (legacy app names) |

Extraction logic: `state.vscdb` reads the `apiKey` field of the `windsurfAuthStatus` record in `ItemTable`;
`credentials.toml` parses common key fields (`api_key`/`access_token`/`token`, etc.) and falls back to `sk-`-prefixed tokens.

> Note: keys are used in memory only and never written to disk; `config show` displays a masked value.

## MCP tools

`web_search`

| arg | type | required | description |
|-----|------|----------|-------------|
| `query` | string | yes | Search query |
| `limit` | number | no | 1–10, default 5 |
| `domain` | string | no | Domain filter |
| `mode` | number | no | Upstream mode |
| `pretty` | boolean | no | Pretty-print JSON output (default false) |

Returns MCP text content, JSON: `{ "hits": [ { "title", "url", "snippet", "source": "windsurf" } ] }`

`image_caption`

| arg | type | required | description |
|-----|------|----------|-------------|
| `image_path` | string | yes* | Local image path (PNG/JPG/WebP/GIF) |
| `image_base64` | string | yes* | Raw base64 image data (`data:` prefix optional) |
| `mime` | string | no | MIME type, e.g. `image/png` (default guessed from path extension) |
| `question` | string | no | Question / instruction about the image |
| `pretty` | boolean | no | Pretty-print JSON output (default false) |

> Provide either `image_path` or `image_base64` (at least one).

Returns MCP text content, JSON: `{ "caption": "..." }`

`audio_transcribe`

| arg | type | required | description |
|-----|------|----------|-------------|
| `audio_path` | string | yes* | Local audio path (wav/mp3/ogg/opus/webm/m4a/flac) |
| `audio_base64` | string | yes* | Raw base64 audio data (`data:` prefix optional) |
| `timeout` | number | no | Timeout in seconds (default 60) |
| `pretty` | boolean | no | Pretty-print JSON output (default false) |

> Provide either `audio_path` or `audio_base64` (at least one). Format is auto-detected by the backend (Whisper).

Returns MCP text content, JSON: `{ "transcribedText": "..." }`

`fast_context_search`

| arg | type | required | description |
|-----|------|----------|-------------|
| `query` | string | yes | Natural-language codebase search query |
| `project_path` | string | no | Absolute path to the project root. Empty = current working directory. |
| `tree_depth` | number | no | Directory tree depth for the repo map (1-6, default 3) |
| `max_turns` | number | no | Search rounds (1-5, default 3) |
| `max_results` | number | no | Max files to return (1-30, default 10) |
| `exclude_paths` | array | no | Dirs/patterns to exclude from the repo map |

Returns text content with the relevant file paths, line ranges, code snippets, grep keywords, and stats/config diagnostics.

Two MCP framing protocols are supported: `Content-Length` (official SDK clients) and NDJSON (handwritten clients).

## MCP setup

The MCP server runs as a **stdio subprocess** (`agent-scout --mcp`), communicating with the MCP client over stdin/stdout — no port needed.

> **Generic MCP standard**: standard Model Context Protocol (stdio transport). Any client supporting "stdio MCP servers" works —
> Cursor, Claude Desktop, VS Code, Zed, Windsurf, custom MCP hosts, etc., regardless of platform (macOS / Linux / Windows).

### Generic config (any MCP host)

Add to your client's MCP config file:

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

> **Two `command` forms**:
> - `"agent-scout"`: easiest when activated on PATH via `cargo install` etc. (see "Installing to PATH" above);
> - absolute path: when not on PATH (e.g. `/Users/you/agent-scout/target/release/agent-scout`).

### Per-client examples

**VS Code / Cursor** (`.vscode/mcp.json` or the MCP config in settings):

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

**Claude Desktop** (`claude_desktop_config.json`):

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

### Auth notes

**No key config needed** — as long as you have logged into Devin/Windsurf locally, the MCP server auto-detects and extracts the key (see "Authentication" above).

To specify a key explicitly, inject it via the environment:

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

After configuring, restart your MCP client — you can then call the `web_search` / `image_caption` / `audio_transcribe` / `fast_context_search` tools in conversation.

## Skills usage

The repo ships a standard skill for AI agents to call web search, image caption, and transcription directly:

```
skills/agent-scout-search/
├── SKILL.md           # usage guide (frontmatter + steps; search, caption & transcribe)
└── agents/openai.yaml # UI metadata
```

- **skill name**: `agent-scout-search`
- **what it does**: per SKILL.md, run `agent-scout` for search and parse JSON hits, `agent-scout caption` to describe/analyze local images, or `agent-scout transcribe` for local audio
- **standard**: follows the Agent Skill spec (`SKILL.md` + YAML frontmatter), theoretically compatible with any agent that loads skills this way —
  InsCode, Codex, and other compliant frameworks, regardless of platform.

### Install into an agent's skills directory

Option A: symlink into InsCode global skills (auto-discovered):

```bash
ln -sfn "$(pwd)/skills/agent-scout-search" ~/.config/inscode/skills/agent-scout-search
```

Option B: copy/link into the Codex skills directory:

```bash
mkdir -p ~/.codex/skills
ln -sfn "$(pwd)/skills/agent-scout-search" ~/.codex/skills/agent-scout-search
```

After installation, agents auto-match the `agent-scout-search` skill when they need web search and follow its instructions.

## Security notes

- Never commit real tokens.
- Session tokens expire — on a 401, re-run `config set`.
- Auto-extracted keys are sensitive plaintext credentials stored locally; used in memory only, never persisted to disk, never printed in full.
- Not an official Windsurf/Devin product.

## Example output

A `agent-scout "rust async" --limit 2` run emits stdout like:

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

> Tip: pair with `jq` to extract fields quickly, e.g. `agent-scout "query" | jq '.hits[].url'`.

A `agent-scout caption ~/Pictures/screen.png --question "Which UI framework is this?"` run emits stdout like:

```text
The interface is built with React and Tailwind CSS, with a left sidebar navigation,
a search bar at the top, and a data table in the main area showing a user list with status labels.
```

Caption output is plain text (no JSON wrapper) for easy inline use; add `--json` for `{"caption": "..."}` when scripting.

A `agent-scout transcribe ~/Recordings/meeting.wav` run emits stdout like:

```text
OK, the meeting begins. First, let's review last week's progress...
```

Transcript output is plain text (no JSON wrapper); add `--json` for `{"transcribedText": "..."}` when scripting.

## Error logs

Each run writes to `~/.config/windsurf-search/logs/agent-scout-YYYY-MM-DD.log`:

- Key read failures, search/caption/transcribe failures, config errors → `[ERROR]`
- Successful search/caption/transcribe → `[INFO]`
- **Auto cleanup**: on each write, logs older than 7 days are deleted, keeping at most 30 files

```bash
tail -f ~/.config/windsurf-search/logs/*.log   # watch in real time
```

## Acknowledgements

This project is based on the open-source project **[windsurf-search-mcp](https://github.com/mimimaster/windsurf-search-mcp)** (Node.js implementation),
whose core logic was rewritten in Rust, with enhancements to auth, logging, and the skill.

The `fast-context` (AI semantic code search) capability is based on the open-source project
**[fast-context-mcp](https://github.com/SammySnake-d/fast-context-mcp)** (Node.js reverse-engineering of the Windsurf Devstral
protocol), rewritten natively in Rust, with added engineering enhancements such as command-shape normalization, path fallback,
gitignore support, command caching & stats, Chinese query hints, and empty-answer auto retry.

## License

MIT

