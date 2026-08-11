//! agent-scout — MCP server + CLI for Windsurf/Devin server-side web search.
//! Core logic is pure Rust; see `search` and `auth` modules.

pub mod auth;
pub mod caption;
pub mod log;
pub mod mcp;
pub mod search;
pub mod transcribe;

pub use search::{Hit, SearchOptions};

/// Package version string, kept in sync with Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");