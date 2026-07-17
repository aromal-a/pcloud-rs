#![allow(clippy::pedantic)]
//! Integration tests for the expanded pcloud-web UI (P4.5+).
//!
//! Each test spins up the server on an ephemeral loopback port with an
//! empty daemon socket path, so IPC is always short-circuited to
//! "offline" and we exercise purely the HTTP/HTML/CSRF plumbing.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use pcloud_ipc::{IpcServer, Method, Request, Response, ResponseStatus};
use pcloud_secret::secret_string::SecretString;
use pcloud_web::{WebConfig, bind_for_test, generate_web_token};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Fire up the server, return (addr, web_token, join_handle).
/// Handle is aborted by the caller at end of test.
async fn start() -> (std::net::SocketAddr, String, tokio::task::JoinHandle<()>) {
    let token = generate_web_token().expect("getrandom unavailable in test");
    let cfg = WebConfig {
        socket_path: PathBuf::new(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        web_token: SecretString::new(token.clone()),
        ..WebConfig::default()
    };
    let (listener, addr, app) = bind_for_test(cfg).await.expect("bind");
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, token, handle)
}

async fn start_with_socket(
    socket_path: &Path,
) -> (std::net::SocketAddr, String, tokio::task::JoinHandle<()>) {
    let token = generate_web_token().expect("getrandom unavailable in test");
    let cfg = WebConfig {
        socket_path: socket_path.to_path_buf(),
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        web_token: SecretString::new(token.clone()),
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

/// Extract a `Set-Cookie: name=value` cookie from a raw HTTP response.
/// Returns the cookie value or panics with the full response for debugging.
fn extract_cookie(resp: &str, name: &str) -> String {
    let needle = format!("{name}=");
    for line in resp.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("set-cookie:") && lower.contains(&needle) {
            // Recover the value on the case-preserving original line.
            if let Some(start) = line.find(&needle) {
                let rest = &line[start + needle.len()..];
                let end = rest.find(';').unwrap_or(rest.len());
                return rest[..end].trim().to_string();
            }
        }
    }
    panic!("no {name} cookie in response: {resp}");
}

fn extract_csrf_cookie(resp: &str) -> String {
    extract_cookie(resp, "pcw_csrf")
}

fn successful_daemon_response(request: Request) -> Response {
    let message = match request {
        Request::Plain {
            method: Method::GetStatus,
        } => r#"{"sync_root_count":2,"mount_state":"mounted"}"#.to_string(),
        Request::Plain {
            method: Method::GetSyncRoots,
        } => r#"[{"sync_id":7,"local_path":"/tmp/local","remote_path":"/remote"}]"#.to_string(),
        Request::Plain {
            method: Method::GetPending,
        } => r#"{"pending":1}"#.to_string(),
        Request::Plain {
            method: Method::ListPublicLinks,
        } => r#"[{"link_id":42,"code":"fixture-code"}]"#.to_string(),
        Request::Plain {
            method: Method::ListNotifications,
        } => r#"[{"event":"fixture"}]"#.to_string(),
        Request::CreateFilePublicLink { .. } | Request::CreateFolderPublicLink { .. } => {
            r#"{"link_id":42}"#.to_string()
        }
        Request::ShowPublicLink { .. } => r#"{"link_id":42}"#.to_string(),
        _ => r#"{"ok":true}"#.to_string(),
    };
    Response {
        status: ResponseStatus::Ok,
        message,
    }
}

#[tokio::test]
async fn sync_list_renders_html_with_csrf_token() {
    let (addr, web_token, handle) = start().await;

    let req = format!(
        "GET /sync HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let resp = raw_request(addr, &req).await;

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
    assert_eq!(extract_cookie(&resp, "pcw_session"), web_token);
    // Form is rendered.
    assert!(resp.contains("<form method=\"post\" action=\"/sync\""));
    assert!(resp.contains("name=\"csrf_token\""));

    handle.abort();
}

#[tokio::test]
async fn hostile_host_header_is_rejected() {
    let (addr, _web_token, handle) = start().await;

    let req = "GET /health HTTP/1.1\r\n\
               Host: attacker.example\r\n\
               Connection: close\r\n\r\n";
    let resp = raw_request(addr, req).await;
    assert!(
        resp.starts_with("HTTP/1.1 400 "),
        "expected hostile Host rejection, got: {resp}"
    );

    handle.abort();
}

#[tokio::test]
async fn daemon_backed_get_routes_reject_missing_web_token() {
    let (addr, _token, handle) = start().await;

    for path in [
        "/",
        "/sync",
        "/publinks",
        "/activity",
        "/settings",
        "/api/status",
    ] {
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        let resp = raw_request(addr, &req).await;
        assert!(
            resp.starts_with("HTTP/1.1 401 "),
            "expected 401 for {path}, got: {resp}"
        );
    }

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
    let host = format!("localhost:{}", addr.port());
    let origin = format!("http://{host}");

    let body = "local_path=%2Ftmp%2Fa&remote_path=%2Fa&sync_type=full";
    let req = format!(
        "POST /sync HTTP/1.1\r\n\
         Host: {host}\r\n\
         Origin: {origin}\r\n\
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
    let get = format!(
        "GET /publinks HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let get_resp = raw_request(addr, &get).await;
    assert!(get_resp.starts_with("HTTP/1.1 200 "));
    let csrf = extract_csrf_cookie(&get_resp);

    // CREATE with matching cookie+header+web_token: must reach handler, IPC
    // fails → 502.
    let host = format!("localhost:{}", addr.port());
    let origin = format!("http://{host}");
    let body = "path=%2Ffoo.txt&expiry=&password=";
    let create = format!(
        "POST /publinks HTTP/1.1\r\n\
         Host: {host}\r\n\
         Origin: {origin}\r\n\
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
         Host: {host}\r\n\
         Origin: {origin}\r\n\
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
async fn cross_origin_mutation_is_rejected() {
    let (addr, web_token, handle) = start().await;
    let host = format!("localhost:{}", addr.port());

    let get = format!(
        "GET /sync HTTP/1.1\r\n\
         Host: {host}\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let get_resp = raw_request(addr, &get).await;
    assert!(get_resp.starts_with("HTTP/1.1 200 "));
    let csrf = extract_csrf_cookie(&get_resp);
    let session = extract_cookie(&get_resp, "pcw_session");

    let body = format!("csrf_token={csrf}&local_path=%2Ftmp%2Fa&remote_path=%2Fa&sync_type=full");
    let post = format!(
        "POST /sync HTTP/1.1\r\n\
         Host: {host}\r\n\
         Origin: http://attacker.example\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Cookie: pcw_csrf={csrf}; pcw_session={session}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    );
    let resp = raw_request(addr, &post).await;
    assert!(
        resp.starts_with("HTTP/1.1 403 "),
        "expected cross-origin rejection, got: {resp}"
    );

    handle.abort();
}

#[tokio::test]
async fn browser_like_form_post_uses_hidden_csrf_and_session_cookie() {
    let (addr, web_token, handle) = start().await;
    let host = format!("localhost:{}", addr.port());
    let origin = format!("http://{host}");

    let get = format!(
        "GET /sync HTTP/1.1\r\n\
         Host: {host}\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let get_resp = raw_request(addr, &get).await;
    assert!(get_resp.starts_with("HTTP/1.1 200 "));
    let csrf = extract_csrf_cookie(&get_resp);
    let session = extract_cookie(&get_resp, "pcw_session");

    let body = format!("csrf_token={csrf}&local_path=%2Ftmp%2Fa&remote_path=%2Fa&sync_type=full");
    let post = format!(
        "POST /sync HTTP/1.1\r\n\
         Host: {host}\r\n\
         Origin: {origin}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Cookie: pcw_csrf={csrf}; pcw_session={session}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n{body}",
        len = body.len()
    );
    let resp = raw_request(addr, &post).await;
    assert!(
        resp.starts_with("HTTP/1.1 502 "),
        "expected browser-like form to pass web/CSRF/origin gates and reach IPC, got: {resp}"
    );

    handle.abort();
}

#[tokio::test]
async fn activity_returns_json_when_accept_is_application_json() {
    let (addr, web_token, handle) = start().await;

    let json_get = format!(
        "GET /activity HTTP/1.1\r\nHost: localhost\r\n\
         Accept: application/json\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let resp = raw_request(addr, &json_get).await;
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
    let html_get = format!(
        "GET /activity HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let html_resp = raw_request(addr, &html_get).await;
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
    let (addr, web_token, handle) = start().await;
    let get = format!(
        "GET /settings HTTP/1.1\r\n\
         Host: localhost\r\n\
         X-PCloud-Web-Token: {web_token}\r\n\
         Connection: close\r\n\r\n"
    );
    let resp = raw_request(addr, &get).await;
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

#[tokio::test]
async fn online_daemon_routes_and_mutations_succeed_end_to_end() {
    let root = tempfile::tempdir().expect("temporary web runtime");
    let socket = root.path().join("runtime/pcloud.sock");
    let bound = IpcServer::new(pcloud_ipc::current_effective_uid())
        .bind(&socket)
        .expect("bind fake daemon");
    bound
        .set_accept_timeout(Some(Duration::from_millis(300)))
        .expect("set accept timeout");
    let daemon = std::thread::spawn(move || {
        let mut served = 0;
        loop {
            match bound.serve_once(successful_daemon_response) {
                Ok(()) => served += 1,
                Err(pcloud_ipc::IpcTransportError::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(error) => panic!("fake daemon failure: {error}"),
            }
        }
        served
    });

    let (addr, web_token, handle) = start_with_socket(&socket).await;
    let host = format!("localhost:{}", addr.port());
    let origin = format!("http://{host}");

    let index = raw_request(
        addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {host}\r\n\
             X-PCloud-Web-Token: {web_token}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(index.starts_with("HTTP/1.1 200 "), "{index}");
    assert!(index.contains("Online"), "{index}");
    assert!(index.contains("mounted"), "{index}");
    let csrf = extract_csrf_cookie(&index);
    let session = extract_cookie(&index, "pcw_session");

    for (path, expected) in [
        ("/api/status", "sync_root_count"),
        ("/sync", "Pending"),
        ("/publinks", "fixture-code"),
        ("/activity", "fixture"),
        ("/settings", "socket_path"),
        ("/metrics", "metrics feature"),
    ] {
        let response = raw_request(
            addr,
            &format!(
                "GET {path} HTTP/1.1\r\nHost: {host}\r\n\
                 Cookie: pcw_session={session}\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 200 ")
                || (path == "/metrics" && response.starts_with("HTTP/1.1 404 ")),
            "{path}: {response}"
        );
        assert!(response.contains(expected), "{path}: {response}");
    }

    let activity_json = raw_request(
        addr,
        &format!(
            "GET /activity HTTP/1.1\r\nHost: {host}\r\n\
             Accept: application/x-ndjson\r\nCookie: pcw_session={session}\r\n\
             Connection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(
        activity_json.starts_with("HTTP/1.1 200 "),
        "{activity_json}"
    );
    assert!(activity_json.contains("\"online\":true"), "{activity_json}");

    for sync_type in ["full", "mirror", "backup", "unknown"] {
        let body = format!(
            "local_path=%2Ftmp%2Flocal&remote_path=%2Fremote&sync_type={sync_type}&csrf_token={csrf}"
        );
        let response = raw_request(
            addr,
            &format!(
                "POST /sync HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\n\
                 Cookie: pcw_csrf={csrf}; pcw_session={session}\r\n\
                 Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
                len = body.len()
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 303 "), "{response}");
        assert!(response.contains("location: /sync"), "{response}");
    }

    let delete_sync = raw_request(
        addr,
        &format!(
            "DELETE /sync/7 HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\n\
             Cookie: pcw_csrf={csrf}; pcw_session={session}\r\n\
             X-CSRF-Token: {csrf}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(delete_sync.starts_with("HTTP/1.1 200 "), "{delete_sync}");

    for path in ["/fixture.txt", "/fixture-folder/"] {
        let body =
            format!("path={path}&expiry=4102444800&password=fixture-password&csrf_token={csrf}");
        let response = raw_request(
            addr,
            &format!(
                "POST /publinks HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\n\
                 Content-Type: application/x-www-form-urlencoded\r\n\
                 Cookie: pcw_csrf={csrf}; pcw_session={session}\r\n\
                 Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
                len = body.len()
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 303 "), "{response}");
        assert!(response.contains("location: /publinks"), "{response}");
    }

    for code in ["42", "fixture-code"] {
        let response = raw_request(
            addr,
            &format!(
                "DELETE /publinks/{code} HTTP/1.1\r\nHost: {host}\r\n\
                 Referer: {origin}/publinks\r\n\
                 Cookie: pcw_csrf={csrf}; pcw_session={session}\r\n\
                 X-CSRF-Token: {csrf}\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    }

    handle.abort();
    let served = daemon.join().expect("fake daemon thread");
    assert!(served >= 20, "expected broad IPC coverage, served {served}");
}
