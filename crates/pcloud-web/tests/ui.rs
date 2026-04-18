#![allow(clippy::pedantic)]
//! Integration tests for the expanded pcloud-web UI (P4.5+).
//!
//! Each test spins up the server on an ephemeral loopback port with an
//! empty daemon socket path, so IPC is always short-circuited to
//! "offline" and we exercise purely the HTTP/HTML/CSRF plumbing.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use pcloud_web::{WebConfig, bind_for_test, generate_web_token};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Fire up the server, return (addr, web_token, join_handle).
/// Handle is aborted by the caller at end of test.
async fn start() -> (std::net::SocketAddr, String, tokio::task::JoinHandle<()>) {
    let token = generate_web_token().expect("getrandom unavailable in test");
    let cfg = WebConfig {
        socket_path: PathBuf::new(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        web_token: token.clone(),
        ..WebConfig::default()
    };
    let (listener, addr, app) = bind_for_test(cfg).await.expect("bind");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, token, handle)
}

async fn raw_request(addr: std::net::SocketAddr, req: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream.write_all(req.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read");
    String::from_utf8_lossy(&buf).into_owned()
}

/// Extract `Set-Cookie: pcw_csrf=<hex>`-style cookie from a raw HTTP
/// response. Returns the 32-hex-char value or panics with the full
/// response for debugging.
fn extract_csrf_cookie(resp: &str) -> String {
    for line in resp.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("set-cookie:") && lower.contains("pcw_csrf=") {
            // Recover the value on the case-preserving original line.
            if let Some(start) = line.find("pcw_csrf=") {
                let rest = &line[start + "pcw_csrf=".len()..];
                let end = rest.find(';').unwrap_or(rest.len());
                return rest[..end].trim().to_string();
            }
        }
    }
    panic!("no pcw_csrf cookie in response: {resp}");
}

#[tokio::test]
async fn sync_list_renders_html_with_csrf_token() {
    let (addr, _token, handle) = start().await;

    let resp = raw_request(
        addr,
        "GET /sync HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;

    assert!(resp.starts_with("HTTP/1.1 200 "), "unexpected: {resp}");
    assert!(
        resp.to_ascii_lowercase()
            .contains("content-security-policy:"),
        "missing CSP: {resp}"
    );
    assert!(resp.contains("Sync roots"), "missing heading: {resp}");
    // Double-submit cookie must be present.
    let token = extract_csrf_cookie(&resp);
    assert_eq!(token.len(), 32, "token not 32 hex: {token}");
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    // Form is rendered.
    assert!(resp.contains("<form method=\"post\" action=\"/sync\""));

    handle.abort();
}

#[tokio::test]
async fn sync_add_rejects_request_without_web_token() {
    // Without the X-PCloud-Web-Token header, the server must return 401.
    let (addr, _token, handle) = start().await;

    let body = "local_path=%2Ftmp%2Fa&remote_path=%2Fa&sync_type=full";
    let req = format!(
        "POST /sync HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    );
    let resp = raw_request(addr, &req).await;
    assert!(
        resp.starts_with("HTTP/1.1 401 "),
        "expected 401 (missing web token), got: {resp}"
    );

    handle.abort();
}

#[tokio::test]
async fn sync_add_rejects_request_without_csrf() {
    // With valid web token but no CSRF, the server must return 403.
    let (addr, token, handle) = start().await;

    let body = "local_path=%2Ftmp%2Fa&remote_path=%2Fa&sync_type=full";
    let req = format!(
        "POST /sync HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         X-PCloud-Web-Token: {token}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    );
    let resp = raw_request(addr, &req).await;
    assert!(
        resp.starts_with("HTTP/1.1 403 "),
        "expected 403 (missing CSRF), got: {resp}"
    );

    handle.abort();
}

#[tokio::test]
async fn publink_create_then_delete_round_trip() {
    // With no daemon wired, we test the HTTP/CSRF/token plumbing only.
    // Valid token+CSRF reaches the handler → IPC fails → 502.
    // Missing token → 401. Missing CSRF (but valid token) → 403.
    let (addr, web_token, handle) = start().await;

    // First grab a CSRF cookie from GET /publinks.
    let get_resp = raw_request(
        addr,
        "GET /publinks HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(get_resp.starts_with("HTTP/1.1 200 "));
    let csrf = extract_csrf_cookie(&get_resp);

    // CREATE with matching cookie+header+web_token: must reach handler, IPC
    // fails → 502.
    let body = "path=%2Ffoo.txt&expiry=&password=";
    let create = format!(
        "POST /publinks HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Cookie: pcw_csrf={csrf}\r\n\
         X-CSRF-Token: {csrf}\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    );
    let create_resp = raw_request(addr, &create).await;
    assert!(
        create_resp.starts_with("HTTP/1.1 502 "),
        "expected 502 after token+CSRF pass, got: {create_resp}"
    );

    // CREATE without any headers: must 401 (token missing first).
    let create_no_token = format!(
        "POST /publinks HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    );
    let no_token_resp = raw_request(addr, &create_no_token).await;
    assert!(
        no_token_resp.starts_with("HTTP/1.1 401 "),
        "expected 401, got: {no_token_resp}"
    );

    // DELETE with matching cookie+header+token: passes, IPC 502.
    let del = format!(
        "DELETE /publinks/42 HTTP/1.1\r\n\
         Host: localhost\r\n\
         Cookie: pcw_csrf={csrf}\r\n\
         X-CSRF-Token: {csrf}\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let del_resp = raw_request(addr, &del).await;
    assert!(
        del_resp.starts_with("HTTP/1.1 502 "),
        "expected 502, got: {del_resp}"
    );

    handle.abort();
}

#[tokio::test]
async fn activity_returns_json_when_accept_is_application_json() {
    let (addr, _token, handle) = start().await;

    let resp = raw_request(
        addr,
        "GET /activity HTTP/1.1\r\nHost: localhost\r\n\
         Accept: application/json\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200 "));
    assert!(
        resp.to_ascii_lowercase()
            .contains("content-type: application/json"),
        "expected json: {resp}"
    );
    // Body should start with a JSON object.
    let body = resp.split("\r\n\r\n").nth(1).unwrap_or("");
    assert!(body.trim_start().starts_with('{'), "body: {body}");

    // Default (HTML) path when no Accept header is present.
    let html_resp = raw_request(
        addr,
        "GET /activity HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        html_resp
            .to_ascii_lowercase()
            .contains("content-type: text/html"),
        "expected html: {html_resp}"
    );

    handle.abort();
}

#[tokio::test]
async fn settings_redacts_secret_fields() {
    let (addr, _token, handle) = start().await;
    let resp = raw_request(
        addr,
        "GET /settings HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200 "));
    // Secret-bearing keys must appear redacted.
    assert!(resp.contains("auth_token"));
    assert!(resp.contains("password"));
    assert!(resp.contains("crypto_passphrase"));
    assert!(
        resp.contains("&lt;redacted&gt;"),
        "expected redaction marker in: {resp}"
    );
    // No cleartext secrets (trivially true here — web never holds any —
    // but assert no placeholder strings leak).
    assert!(!resp.contains("hunter2"));
    handle.abort();
}
