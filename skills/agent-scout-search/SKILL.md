---
name: agent-scout-search
description: >
  Tools powered by the agent-scout binary (server-side Windsurf/Devin backend):
  (1) web search via GetWebSearchResults — current web facts, documentation
  lookups, generic web research, or answers that need up-to-date info beyond
  the model's training data; (2) image captioning / vision analysis via
  GetImageCaption — describe or answer questions about a local image file;
  (3) audio transcription via GetTranscription — transcribe a local audio
  file to text; and (4) fast-context AI semantic code search via
  GetDevstralStream — find relevant files, line ranges, and grep keywords in
  a local codebase from a natural-language query.
  Triggers on queries like "search the web for X", "look up X", "what is the
  latest on X", "what does this image show / describe this picture",
  "transcribe this audio / what does this recording say", or "find where X is
  implemented / where is the authentication logic / which files handle X in
  this codebase".
  Returns JSON hits with title, url, snippet, and source, the vision model's
  caption text, the transcription text, or the fast-context result (file
  paths + line ranges + code snippets + grep keywords). Zero-configuration:
  automatically discovers the local Devin/Windsurf credential, so no API key
  management is needed.
---

# agent-scout Web Search, Vision, Transcription & Code Search

Search the web, caption/analyze images, transcribe audio, and run AI semantic code search on a local codebase using the
`agent-scout` binary (Rust implementation of Windsurf/Devin server-side
`GetWebSearchResults`, `GetImageCaption`, `GetTranscription`, and `GetDevstralStream`). Auto-discovers
the local Devin/Windsurf login credential — no key configuration required.

## Quick start

Run the CLI and parse the JSON output:

```bash
agent-scout "your search query" --limit 5
```

stdout is pure JSON:

```json
{"hits":[{"title":"...","url":"https://...","snippet":"...","source":"windsurf"}]}
```

## Common options

| flag | meaning |
|------|---------|
| `--limit N` | result count, 1–10, default 5 |
| `--domain d` | restrict to a domain, e.g. `github.com` |
| `--mode m` | upstream search mode |
| `--pretty` | pretty-print the JSON output |
| `--api-key k` | explicit key (overrides auto-discovery) |

## Procedure

1. `agent-scout "query"` — run the search, capture stdout.
2. Parse the JSON with `jq` or a Python one-liner to read `hits[].title`,
   `hits[].url`, `hits[].snippet`.
3. If a command fails, first check stderr (diagnostics go there, not stdout).

## Examples

```bash
# Basic search
agent-scout "tauri window drag region" --limit 3

# Domain-restricted
agent-scout "tokio" --limit 2 --domain github.com

# One-off inline parse
agent-scout "rust async runtime" --limit 3 | jq '.hits[] | {title, url}'
```

## Exit codes

- `0` = success
- `1` = error (e.g. no credential, upstream 4xx/5xx)
- `2` = usage error

## Image captioning (识图)

Describe or analyze a local image file using the same Windsurf/Devin backend
(`GetImageCaption`). The `caption` subcommand reads the image, base64-encodes
it, and prints the model's analysis to stdout.

```bash
# Basic caption
agent-scout caption /path/to/photo.png

# Ask a specific question about the image
agent-scout caption /path/to/photo.png --question "What UI framework is this?"

# Force a mime type (guessed from extension by default)
agent-scout caption /path/to/image --mime image/webp

# JSON output for scripting
agent-scout caption /path/to/photo.png --json
```

stdout is plain text (the caption); add `--json` to get `{"caption": "..."}`. Options:

| flag | meaning |
|------|---------|
| `--question "..."` | question / instruction to the vision model |
| `--mime m` | mime type, e.g. `image/png` (default guessed from extension) |
| `--json` | output `{"caption": "..."}` instead of plain text |
| `--pretty` | pretty-print the JSON output (with `--json`) |
| `--api-key k` | explicit key (overrides auto-discovery) |

## Audio transcription (转写)

Transcribe a local audio file using the same Windsurf/Devin backend
(`GetTranscription`, backed by OpenAI Whisper). The `transcribe` subcommand
reads the audio file, base64-encodes it, and prints the transcript to stdout.
Format is auto-detected by the backend (wav/mp3/ogg/opus/webm/m4a/flac).

```bash
# Basic transcription
agent-scout transcribe /path/to/recording.wav

# JSON output for scripting
agent-scout transcribe /path/to/recording.mp3 --json

# Longer timeout (transcription can be slow, default 60s)
agent-scout transcribe /path/to/recording.ogg --timeout 120
```

stdout is plain text (the transcript); add `--json` to get `{"transcribedText": "..."}`. Options:

| flag | meaning |
|------|---------|
| `--timeout N` | timeout in seconds (default 60) |
| `--json` | output `{"transcribedText": "..."}` instead of plain text |
| `--pretty` | pretty-print the JSON output (with `--json`) |
| `--api-key k` | explicit key (overrides auto-discovery) |

## Fast-context code search (代码检索)

Find relevant files, line ranges, and grep keywords in a **local codebase**
from a natural-language query, using the same Windsurf/Devin backend
(`GetDevstralStream`, AI-driven semantic code search). The model runs multiple
rounds of local commands (rg / readfile / tree / ls / glob) and returns the
matching file paths + inclusive line ranges, with code snippets in the pretty
output.

```bash
# Search the current directory
agent-scout fc "where is the authentication logic?"

# Search a specific project root
agent-scout fc "auth flow" --path /path/to/project

# Deep search with more results, excluding heavy dirs
agent-scout fc "auth flow" --path . --turns 4 --max-results 15 \
  --exclude node_modules,dist,target

# Structured JSON output for scripting
agent-scout fc "auth" --path . --json | jq '.files[].path'
```

stdout is the pretty text (file paths + line ranges + code snippets + grep
keywords + stats/config diagnostics); add `--json` to get structured output.
Options:

| flag | meaning |
|------|---------|
| `--path DIR` | project root to search (default: current directory) |
| `--turns N` | search rounds, 1–5, default 3 (more = deeper but slower & more quota) |
| `--depth N` | repo-map tree depth, 1–6, default 3 (lower for huge monorepos) |
| `--max-results N` | max files to return, 1–30, default 10 |
| `--exclude a,b` | dirs/patterns to exclude, e.g. `node_modules,dist,.git` |
| `--json` | structured JSON output instead of pretty text |
| `--pretty` | pretty-print the JSON output (with `--json`) |
| `--api-key k` | explicit key (overrides auto-discovery) |

> Note: fast-context consumes Windsurf account quota (one model call per turn;
> 4 calls by default per query) — prefer one fc call over repeated ones, and
> tune `--turns` / `--depth` / `--max-results` / `--exclude` to the task.

## Troubleshooting

- **"no API key found"** → the machine has no Devin/Windsurf login. Provide
  `--api-key 'devin-session-token$...'` or set `WINDSURF_API_KEY`.
- **HTTP 401** → the session token expired. Re-login to Devin/Windsurf, or
  supply a fresh key via `--api-key`.
- **Binary not found** → build and install first:

  ```bash
  cd /path/to/agent-scout && cargo build --release
  cargo install --path . --root "$HOME/.cargo"
  ```

For full details see QUICKSTART.md / README.md in the agent-scout repo.