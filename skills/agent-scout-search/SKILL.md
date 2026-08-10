---
name: agent-scout-search
description: >
  Perform web searches via the agent-scout tool (server-side web search through
  the Devin era of the Windsurf/Codeium backend, GetWebSearchResults). Use when
  the user needs current web facts, documentation lookups, generic web
  research, or answers that require up-to-date information beyond the model's
  training data. Triggers on queries like "search the web for X", "look up X",
  "what is the latest on X", or any request needing real-time web results.
  Returns JSON hits with title, url, snippet, and source. Zero-configuration:
  automatically discovers the local Devin/Windsurf credential, so no API key
  management is needed.
---

# agent-scout Web Search

Search the web using the `agent-scout` binary (Rust implementation of
Windsurf/Devin server-side web search). Auto-discovers the local
Devin/Windsurf login credential — no key configuration required.

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