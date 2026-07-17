use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use pcloud_secret::secret_string::SecretString;
use pcloud_web::{WebConfig, WebError, serve};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn unused_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

#[tokio::test]
async fn public_serve_path_writes_token_serves_and_reports_bind_conflicts() {
    let runtime = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let saved = std::env::var_os("XDG_RUNTIME_DIR");
    // SAFETY: this integration binary contains one test, so no peer thread
    // reads the process environment while the temporary value is installed.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", runtime.path()) };

    let addr = unused_addr();
    let config = WebConfig {
        socket_path: PathBuf::new(),
        bind_addr: addr,
        web_token: SecretString::new("serve-coverage-token".to_owned()),
        ..WebConfig::default()
    };
    let server = tokio::spawn(serve(config));
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut stream = loop {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(stream) => break stream,
            Err(_error) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("serve did not bind: {error}"),
        }
    };
    stream
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200"));
    assert_eq!(
        std::fs::read_to_string(runtime.path().join("pcloud-daemon/web-token")).unwrap(),
        "serve-coverage-token"
    );
    server.abort();
    let _ = server.await;

    let occupied = TcpListener::bind("127.0.0.1:0").unwrap();
    let occupied_addr = occupied.local_addr().unwrap();
    let error = serve(WebConfig {
        bind_addr: occupied_addr,
        web_token: SecretString::new("bind-conflict-token".to_owned()),
        ..WebConfig::default()
    })
    .await
    .expect_err("occupied address must fail");
    assert!(matches!(error, WebError::Bind { addr, .. } if addr == occupied_addr));

    // SAFETY: restore the sole integration test process before exit.
    unsafe {
        match saved {
            Some(value) => std::env::set_var("XDG_RUNTIME_DIR", value),
            None => std::env::remove_var("XDG_RUNTIME_DIR"),
        }
    }
}
