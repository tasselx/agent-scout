# Changelog

> **[English](CHANGELOG.md) | [简体中文](CHANGELOG.zh-CN.md)**

All notable changes to this project are documented here.

## [0.5.0] - 2026-08-12

### Added

- **Web docs options** — new `agent-scout webdocs` subcommand and `get_web_docs_options` MCP tool listing attachable documentation sources (`GetWebDocsOptions`; e.g. cloudflare, duckdb, bun), each with an llms.txt-style `docsUrl` or `docsSearchDomain` plus optional `synonyms` / `isFeatured`.
- **Web docs options docs** — usage introduced in README / QUICKSTART (bilingual) and the `agent-scout-search` skill (SKILL.md + agents/openai.yaml).

## [0.4.0] - 2026-08-11

### Added

- **AI code search (fast-context)** — new `agent-scout fc` subcommand and `fast_context_search` MCP tool for natural-language semantic search over a local codebase, powered by Windsurf's Devstral model (`GetDevstralStream`).
- **fast-context rewritten in Rust** — async Rust implementation (tokio + reqwest): multi-turn tool-call loop, command-shape normalization for malformed LLM arguments, path fallback to the nearest existing parent, gitignore support (`.gitignore` / `.codeiumignore` / `.windsurfignore` / `.devinignore`), command fingerprint caching + cross-turn duplicate detection, readfile content cache, SearchStats diagnostics.
- **Pretty output** — `fc` text output now includes the matched code snippets (reusing files already read during search), plus grep keywords and stats/config diagnostics; `fc --json --pretty` pretty-prints structured JSON.
- **Chinese query hints** — queries with >30% Chinese characters get a translation hint appended to the prompt.
- **Empty-answer auto retry & unparsed-response retry** — the search loop recovers from empty `<ANSWER>` or malformed tool-call responses when turns remain.
- **Bilingual docs** — README / QUICKSTART now default to English with a language switch to 简体中文 (`README.zh-CN.md`, `QUICKSTART.zh-CN.md`); CHANGELOG follows the same pattern.
- **fast-context docs** — usage introduced in README, QUICKSTART, and the `agent-scout-search` skill (SKILL.md + agents/openai.yaml).

### Changed

- New dependencies: `tokio`, `reqwest`, `anyhow`, `futures-util`, `uuid`, `dirs`, `log`, `ignore`, `globset` (dev: `tempfile`).

### Fixed

- Path-validation false negatives on macOS caused by the `/var → /private/var` symlink (project root is canonicalized in `ToolExecutor::new`).

### Acknowledgements

- The fast-context capability is based on [fast-context-mcp](https://github.com/SammySnake-d/fast-context-mcp) (Node.js), rewritten natively in Rust with engineering enhancements.
