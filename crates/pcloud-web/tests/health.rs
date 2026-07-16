#![allow(clippy::pedantic)]
//! Integration test: start the MVP web server on an ephemeral loopback
//! port and hit `/health`. The daemon IPC socket is deliberately left
//! unset so this test never touches the real daemon.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use pcloud_secret::secret_string::SecretString;
use pcloud_web::{WebConfig, bind_for_test};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn health_endpoint_returns_200_ok() {
    let cfg = WebConfig {
        socket_path: PathBuf::new(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        ..WebConfig::default()
    };
    let (listener, addr, app) = bind_for_test(cfg).await.expect("bind");

    let server = tokio::spawn(async move {
        // Ignore the error: test will kill the task when it drops.
        let _ = axum::serve(listener, app).await;
    });

    // Minimal hand-rolled HTTP/1.1 GET — avoids pulling a full http
    // client into dev-deps for a single assertion.
    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read");
    let text = String::from_utf8_lossy(&buf);

    assert!(
        text.starts_with("HTTP/1.1 200 "),
        "unexpected response head: {text:?}"
    );
    assert!(text.contains("\r\n\r\nok"), "missing body: {text:?}");

    server.abort();
}

#[tokio::test]
async fn index_sends_csp_and_reports_offline_without_socket() {
    let web_token = "test-index-token";
    let cfg = WebConfig {
        socket_path: PathBuf::new(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        web_token: SecretString::new(web_token.to_owned()),
        ..WebConfig::default()
    };
    let (listener, addr, app) = bind_for_test(cfg).await.expect("bind");
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let mut stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
    let request = format!(
        "GET / HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read");
    let text = String::from_utf8_lossy(&buf);

    assert!(text.starts_with("HTTP/1.1 200 "));
    assert!(
        text.to_ascii_lowercase()
            .contains("content-security-policy:"),
        "missing CSP header: {text:?}"
    );
    assert!(text.contains("Offline"));

    server.abort();
}
