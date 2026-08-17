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
use agent_scout::ops;
use agent_scout::search::SearchOptions;

fn print_usage(stream: &mut dyn Write) {
    let _ = stream.write_all(
        b"agent-scout: Windsurf/Devin web search CLI (+ MCP companion)\n\
          usage:\n\
          \x20 agent-scout <query> [--limit N] [--domain d] [--mode m] [--api-key k]\n\
          \x20 agent-scout caption <image-path> [--question \"...\"] [--mime m] [--json] [--api-key k]\n\
          \x20 agent-scout transcribe <audio-path> [--timeout N] [--json] [--api-key k]\n\
          \x20 agent-scout fc <query> [--path DIR] [--turns N] [--depth N] [--max-results N]\n\
          \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20 [--exclude a,b] [--json] [--api-key k]   (AI semantic code search)\n\
          \x20 agent-scout webdocs [--json] [--api-key k]       (list attachable web docs options)\n\
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
        if token == "--limit" || token == "--domain" || token == "--mode"
            || token == "--api-key" || token == "--question" || token == "--mime"
            || token == "--timeout" || token == "--path" || token == "--turns"
            || token == "--depth" || token == "--max-results" || token == "--exclude" {
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
            match ops::web_search(home, &key, &query, &opts) {
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

fn run_caption(
    positionals: &[String],
    flags: &std::collections::HashMap<String, Option<String>>,
    home: &std::path::Path,
) -> i32 {
    let image_path = positionals.first().cloned().unwrap_or_default();
    if image_path.is_empty() {
        eprintln!("agent-scout caption: image path is required");
        return 2;
    }
    let cli_key = flags.get("api-key").cloned().flatten().unwrap_or_default();
    let question = flags.get("question").cloned().flatten().unwrap_or_default();
    let mime = flags.get("mime").cloned().flatten().unwrap_or_default();
    let as_json = flags.contains_key("json");
    let pretty = flags.contains_key("pretty");
    match ops::image_caption(home, &cli_key, &image_path, "", &question, &mime) {
        Ok(caption) => {
            agent_scout::log::log_info(
                home,
                &format!("caption success: path={:?} chars={}", image_path, caption.chars().count()),
            );
            if as_json {
                println!("{}", ops::caption_json(&caption, pretty));
            } else {
                println!("{}", caption);
            }
            0
        }
        Err(e) => {
            eprintln!("agent-scout caption: {}", e);
            agent_scout::log::log_error(home, &format!("caption failed: path={:?} error={}", image_path, e));
            1
        }
    }
}

fn run_transcribe(
    positionals: &[String],
    flags: &std::collections::HashMap<String, Option<String>>,
    home: &std::path::Path,
) -> i32 {
    let audio_path = positionals.first().cloned().unwrap_or_default();
    if audio_path.is_empty() {
        eprintln!("agent-scout transcribe: audio path is required");
        return 2;
    }
    let cli_key = flags.get("api-key").cloned().flatten().unwrap_or_default();
    let timeout = flags
        .get("timeout")
        .cloned()
        .flatten()
        .and_then(|t| t.parse::<u64>().ok())
        .filter(|n| *n > 0);
    let as_json = flags.contains_key("json");
    let pretty = flags.contains_key("pretty");
    match ops::audio_transcribe(home, &cli_key, &audio_path, "", timeout) {
        Ok(text) => {
            agent_scout::log::log_info(
                home,
                &format!("transcribe success: path={:?} chars={}", audio_path, text.chars().count()),
            );
            if as_json {
                println!("{}", ops::transcript_json(&text, pretty));
            } else {
                println!("{}", text);
            }
            0
        }
        Err(e) => {
            eprintln!("agent-scout transcribe: {}", e);
            agent_scout::log::log_error(home, &format!("transcribe failed: path={:?} error={}", audio_path, e));
            1
        }
    }
}

fn run_fc(
    positionals: &[String],
    flags: &std::collections::HashMap<String, Option<String>>,
    home: &std::path::Path,
) -> i32 {
    let query = positionals.join(" ");
    if query.trim().is_empty() {
        eprintln!("agent-scout fc: query is required");
        return 2;
    }
    let cli_key = flags.get("api-key").cloned().flatten().unwrap_or_default();

    let mut opts = agent_scout::fastcontext::SearchOptions::default();
    opts.query = query;
    if let Some(p) = flags.get("path").cloned().flatten() {
        if !p.is_empty() {
            opts.project_root = std::path::PathBuf::from(p);
        }
    }
    if let Some(n) = flags.get("turns").cloned().flatten() {
        if let Ok(v) = n.parse::<u8>() {
            if (1..=5).contains(&v) {
                opts.max_turns = v;
            }
        }
    }
    if let Some(n) = flags.get("depth").cloned().flatten() {
        if let Ok(v) = n.parse::<u8>() {
            if (1..=6).contains(&v) {
                opts.tree_depth = v;
            }
        }
    }
    if let Some(n) = flags.get("max-results").cloned().flatten() {
        if let Ok(v) = n.parse::<u8>() {
            if (1..=30).contains(&v) {
                opts.max_results = v;
            }
        }
    }
    if let Some(e) = flags.get("exclude").cloned().flatten() {
        if !e.is_empty() {
            opts.exclude_paths = e.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }

    if !opts.project_root.is_dir() {
        eprintln!("agent-scout fc: project path does not exist: {}", opts.project_root.display());
        return 2;
    }

    let result = match ops::fast_context(home, &cli_key, opts.clone()) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("agent-scout fc: {}", e);
            agent_scout::log::log_error(home, &format!("fastcontext failed: query={:?} error={}", opts.query, e));
            return 1;
        }
    };

    let as_json = flags.contains_key("json");
    let pretty = flags.contains_key("pretty");
    if as_json {
        let payload = agent_scout::fastcontext::search::search_result_json(&result, &opts);
        println!("{}", ops::render_json(&payload, pretty));
    } else {
        println!("{}", agent_scout::fastcontext::search::format_result(&result, &opts));
    }

    agent_scout::log::log_info(home, &format!("fastcontext success: query={:?}", opts.query));
    0
}

fn run_webdocs(
    flags: &std::collections::HashMap<String, Option<String>>,
    home: &std::path::Path,
) -> i32 {
    let cli_key = flags.get("api-key").cloned().flatten().unwrap_or_default();
    match ops::web_docs(home, &cli_key) {
        Ok(options) => {
            println!("{}", ops::webdocs_json(&options, flags.contains_key("pretty")));
            agent_scout::log::log_info(
                home,
                &format!("webdocs success: options={}", options.len()),
            );
            0
        }
        Err(e) => {
            eprintln!("agent-scout webdocs: {}", e);
            agent_scout::log::log_error(home, &format!("webdocs failed: error={}", e));
            1
        }
    }
}

pub fn main_entry() -> i32 {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&argv);
    let home = ops::home_dir();

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

    if args.positionals.first().map(String::as_str) == Some("caption") {
        return run_caption(&args.positionals[1..], &args.flags, &home);
    }

    if args.positionals.first().map(String::as_str) == Some("transcribe") {
        return run_transcribe(&args.positionals[1..], &args.flags, &home);
    }

    if args.positionals.first().map(String::as_str) == Some("fc")
        || args.positionals.first().map(String::as_str) == Some("fastcontext")
    {
        return run_fc(&args.positionals[1..], &args.flags, &home);
    }

    if args.positionals.first().map(String::as_str) == Some("webdocs") {
        return run_webdocs(&args.flags, &home);
    }

    if args.positionals.is_empty() {
        print_usage(&mut std::io::stderr());
        return 2;
    }

    let query = args.positionals.join(" ");
    let cli_key = args.flags.get("api-key").cloned().flatten().unwrap_or_default();

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

    match ops::web_search(&home, &cli_key, &query, &opts) {
        Ok(hits) => {
            println!("{}", ops::hits_json(&hits, args.flags.contains_key("pretty")));
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