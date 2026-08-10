//! Integration test: exercise the full `search()` success path against a
//! local mock HTTP server (no network, no real token needed).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use agent_scout::search::{search, SearchOptions};

/// Start a tiny mock HTTP server that returns a fixed JSON payload for any
/// POST, and echoes the request path + body to a shared log for assertions.
fn start_mock() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log_clone = log.clone();

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let log = log_clone.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                let _ = reader.read_line(&mut request_line);
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" || line == "\n" {
                        break;
                    }
                    if line.to_ascii_lowercase().starts_with("content-length:") {
                        content_length = line
                            .split(':')
                            .nth(1)
                            .and_then(|s| s.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; content_length];
                let _ = reader.read_exact(&mut body);
                let body = String::from_utf8_lossy(&body).to_string();
                log.lock().unwrap().push(format!("{}|{}", request_line.trim(), body.trim()));

                let payload = r#"{"results":[
                    {"title":"Rust Async Runtime","url":"https://example.com/rust-async","snippet":"Tokio is an async runtime.","sourceUrl":"ignored"},
                    {"title":"No url, dropped","text":"x"},
                    {"name":"Second Hit","sourceUrl":"https://example.com/2","text":"snippet2"}
                ]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    payload.len(),
                    payload
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    (format!("http://{}", addr), log)
}

#[test]
fn search_success_path_against_mock() {
    let (host, log) = start_mock();
    let opts = SearchOptions {
        limit: 5,
        domain: Some("github.com".into()),
        mode: Some(serde_json::json!(2)),
        hosts: Some(vec![host.clone()]),
        timeout_secs: Some(5),
        ..Default::default()
    };
    let hits = search("devin-session-token$test", "rust async runtime", &opts).unwrap();

    // Normalization: drops the incomplete record, keeps the two valid ones.
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].title, "Rust Async Runtime");
    assert_eq!(hits[0].url, "https://example.com/rust-async");
    assert_eq!(hits[0].snippet, "Tokio is an async runtime.");
    assert_eq!(hits[0].source, "windsurf");
    assert_eq!(hits[1].title, "Second Hit");
    assert_eq!(hits[1].url, "https://example.com/2");

    // Verify the request reached the right endpoint with correct headers/body.
    let requests = log.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert!(req.starts_with("POST /exa.api_server_pb.ApiServerService/GetWebSearchResults"));
    let body_start = req.find('{').unwrap();
    let body: serde_json::Value = serde_json::from_str(&req[body_start..]).unwrap();
    assert_eq!(body["metadata"]["apiKey"], "devin-session-token$test");
    assert_eq!(body["query"], "rust async runtime");
    assert_eq!(body["limit"], 5);
    assert_eq!(body["domain"], "github.com");
    assert_eq!(body["mode"], 2);
}

#[test]
fn search_uses_first_successful_host() {
    let (host, log) = start_mock();
    let opts = SearchOptions {
        limit: 1,
        hosts: Some(vec![host.clone(), "http://127.0.0.1:1".to_string()]),
        timeout_secs: Some(5),
        ..Default::default()
    };
    let hits = search("k", "q", &opts).unwrap();
    assert_eq!(hits.len(), 2, "mock returns 2 valid hits regardless of limit");
    let reqs = log.lock().unwrap();
    assert_eq!(reqs.len(), 1, "should only hit the first (working) host");
}

#[test]
fn search_reports_http_error_without_panicking() {
    // A closed port -> connection refused -> transport error, not a panic.
    let opts = SearchOptions {
        limit: 1,
        hosts: Some(vec!["http://127.0.0.1:1".to_string()]),
        timeout_secs: Some(5),
        ..Default::default()
    };
    let result = search("k", "q", &opts);
    assert!(result.is_err());
}