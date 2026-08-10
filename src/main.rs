//! agent-scout — CLI for Windsurf/Devin server-side web search.
//!
//! Usage:
//!   agent-scout <query> [--limit N] [--domain d] [--mode m] [--api-key k]
//!   agent-scout --mcp                       (run MCP stdio server)
//!   agent-scout config set [key]            (save a key, chmod 600)
//!   agent-scout config show                 (show saved key status, masked)
//!   agent-scout config test [query]         (verify the configured key)
//!   agent-scout config clear                (remove the saved key file)
//!
//! Exit codes: 0 = success, 1 = error, 2 = usage.

use std::io::Write;

use agent_scout::auth;
use agent_scout::search::{search, SearchOptions};

fn print_usage(stream: &mut dyn Write) {
    let _ = stream.write_all(
        b"agent-scout: Windsurf/Devin web search CLI (+ MCP companion)\n\
          usage:\n\
          \x20 agent-scout <query> [--limit N] [--domain d] [--mode m] [--api-key k]\n\
          \x20 agent-scout --mcp                 (run as MCP stdio server)\n\
          \x20 agent-scout config set [key]      (save a key to ~/.config/windsurf-search/api-key, chmod 600)\n\
          \x20 agent-scout config show           (show saved key status, masked)\n\
          \x20 agent-scout config test [query]   (run a real search to verify the configured key)\n\
          \x20 agent-scout config clear          (remove the saved key file)\n",
    );
}

struct Args {
    positionals: Vec<String>,
    flags: std::collections::HashMap<String, Option<String>>,
}

fn parse_args(argv: &[String]) -> Args {
    let mut positionals = Vec::new();
    let mut flags: std::collections::HashMap<String, Option<String>> = std::collections::HashMap::new();
    let mut i = 0;
    while i < argv.len() {
        let token = &argv[i];
        if token == "--limit" || token == "--domain" || token == "--mode" || token == "--api-key" {
            flags.insert(token[2..].to_string(), argv.get(i + 1).cloned());
            i += 2;
        } else if let Some(rest) = token.strip_prefix("--") {
            flags.insert(rest.to_string(), None);
            i += 1;
        } else {
            positionals.push(token.clone());
            i += 1;
        }
    }
    Args { positionals, flags }
}

fn run_config(action: &str, args: &[String], home: &std::path::Path) -> i32 {
    let key_file = auth::default_key_file_path(home);
    match action {
        "set" => {
            let key = if args.is_empty() {
                match auth::read_line_stdin("Windsurf API key: ", &mut std::io::stderr()) {
                    Ok(k) => k,
                    Err(e) => {
                        eprintln!("agent-scout config set: {}", e);
                        return 1;
                    }
                }
            } else {
                args[0].clone()
            };
            if key.is_empty() {
                eprintln!("agent-scout config set: key is required");
                return 2;
            }
            if let Err(e) = auth::save_key(&key_file, &key) {
                eprintln!("agent-scout config set: {}", e);
                return 1;
            }
            let fmt = auth::describe_key_format(&key);
            println!("saved key to {}", key_file.display());
            println!("format: {}", fmt.label);
            if !fmt.ok {
                println!("warning: this key format may not work with GetWebSearchResults");
            }
            0
        }
        "show" => {
            let key = auth::read_configured_key(&key_file);
            let fmt = auth::describe_key_format(&key);
            println!("config file: {}", key_file.display());
            println!(
                "key: {}",
                if key.is_empty() { "(not configured)".to_string() } else { auth::mask_key(&key) }
            );
            println!("format: {}", fmt.label);
            println!(
                "status: {}",
                if key.is_empty() { "missing".to_string() } else { "configured".to_string() }
            );
            if key.is_empty() {
                2
            } else {
                0
            }
        }
        "test" => {
            let query = args.first().cloned().unwrap_or_else(|| "windsurf search connectivity test".to_string());
            let key = match auth::resolve_api_key(home, "", &std::env::vars().collect::<Vec<_>>(), None) {
                Ok(k) => k,
                Err(e) => {
                    eprintln!("{}", e);
                    return 1;
                }
            };
            println!("testing key {} ({}) with query {:?}", auth::mask_key(&key), auth::describe_key_format(&key).label, query);
            let opts = SearchOptions { limit: 1, ..Default::default() };
            match search(&key, &query, &opts) {
                Ok(hits) => {
                    println!("OK: got {} result(s)", hits.len());
                    if let Some(first) = hits.first() {
                        println!("  [1] {}", first.title);
                        println!("      {}", first.url);
                    }
                    0
                }
                Err(e) => {
                    eprintln!("FAIL: {}", e);
                    1
                }
            }
        }
        "clear" => {
            match std::fs::remove_file(&key_file) {
                Ok(_) | Err(_) => {
                    println!("removed {}", key_file.display());
                    0
                }
            }
        }
        other => {
            eprintln!("agent-scout config: unknown action {:?}. use set | show | test | clear", other);
            2
        }
    }
}

pub fn main_entry() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&argv);
    let home = home::home_dir().unwrap_or_else(std::path::PathBuf::default);

    // --mcp / --serve: run the MCP stdio server.
    if args.flags.contains_key("mcp") || args.flags.contains_key("serve") {
        return match agent_scout::mcp::run() {
            Ok(_) => 0,
            Err(e) => {
                eprintln!("agent-scout mcp: {}", e);
                agent_scout::log::log_error(&home, &format!("MCP server error: {e}"));
                1
            }
        };
    }

    if args.positionals.first().map(String::as_str) == Some("config") {
        let action = args.positionals.get(1).cloned().unwrap_or_default();
        let code = run_config(&action, &args.positionals[2..], &home);
        if code != 0 {
            agent_scout::log::log_error(
                &home,
                &format!("config '{}' failed with exit {}", action, code),
            );
        }
        return code;
    }

    if args.positionals.is_empty() {
        print_usage(&mut std::io::stderr());
        return 2;
    }

    let query = args.positionals.join(" ");
    let cli_key = args.flags.get("api-key").cloned().flatten().unwrap_or_default();
    let api_key = match auth::resolve_api_key(&home, &cli_key, &std::env::vars().collect::<Vec<_>>(), None) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{}", e);
            agent_scout::log::log_error(&home, &format!("resolve api key failed: {e}"));
            return 1;
        }
    };

    let mut opts = SearchOptions::default();
    if let Some(limit) = args.flags.get("limit").cloned().flatten() {
        if let Ok(n) = limit.parse::<usize>() {
            if n > 0 {
                opts.limit = n;
            }
        }
    }
    if let Some(domain) = args.flags.get("domain").cloned().flatten() {
        if !domain.is_empty() {
            opts.domain = Some(domain);
        }
    }
    if let Some(mode) = args.flags.get("mode").cloned().flatten() {
        if let Ok(n) = mode.parse::<i64>() {
            opts.mode = Some(serde_json::json!(n));
        } else {
            opts.mode = Some(serde_json::json!(mode));
        }
    }

    match search(&api_key, &query, &opts) {
        Ok(hits) => {
            let payload = serde_json::json!({ "hits": hits });
            println!("{}", serde_json::to_string(&payload).unwrap_or_default());
            agent_scout::log::log_info(
                &home,
                &format!("search success: query={:?} hits={}", query, hits.len()),
            );
            0
        }
        Err(e) => {
            eprintln!("agent-scout: {}", e);
            agent_scout::log::log_error(&home, &format!("search failed: query={:?} error={}", query, e));
            1
        }
    }
}

fn main() {
    let code = main_entry();
    std::process::exit(code);
}