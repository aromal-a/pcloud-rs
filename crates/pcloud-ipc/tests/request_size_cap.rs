#![allow(clippy::pedantic)]
#![cfg(unix)]
//! Tests for the per-request IPC frame size cap (P0.8).
//!
//! These tests verify that a peer cannot cause unbounded allocation by
//! sending an inflated length prefix, and that legitimate-size requests
//! still succeed.

// **PLATFORM:** Unix (uses `std::os::unix::net::UnixStream` directly to
// hand-craft malformed frames; the size-cap invariant itself is
// cross-platform and additionally covered by unit tests).
// **GATING:** `#[cfg(unix)]` at file level.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::{thread, time::Duration};

use pcloud_ipc::{
    IpcClient, IpcError, IpcServer, MAX_REQUEST_BYTES, Method, Request, Response, ResponseStatus,
    auth::current_effective_uid, protocol,
};

fn unique_socket_path(tag: &str) -> std::path::PathBuf {
    // Use `/tmp` directly: macOS SUN_LEN=104 cannot accommodate the
    // per-user tempdir `/var/folders/.../T/` prefix (49 chars).
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::path::PathBuf::from("/tmp").join(format!(
        "pipc-{}-{}-{}.sock",
        std::process::id(),
        nonce % 1_000_000_000,
        tag,
    ))
}

/// An oversized declared length (10 MiB) must be rejected before any
/// allocation is made, the connection must be closed, and the server
/// must stay alive for the next client. Reading from the socket after
/// the server drops it must return EOF (zero bytes).
#[test]
fn oversized_frame_is_rejected_and_connection_dropped() {
    let socket_path = unique_socket_path("oversized-frame");
    let server = IpcServer::new(current_effective_uid());
    let bound = server.bind(&socket_path).expect("socket should bind");

    let handle = thread::spawn(move || {
        // Handle the oversized attempt (connection will be dropped).
        bound
            .serve_once(|_| Response {
                status: ResponseStatus::Ok,
                message: "should not run".to_owned(),
            })
            .expect("oversized frame should be isolated without server error");
        // Confirm the server still accepts a follow-up legit client.
        bound
            .serve_once(|request| match request {
                Request::Plain {
                    method: Method::GetHealth,
                } => Response {
                    status: ResponseStatus::Ok,
                    message: "healthy".to_owned(),
                },
                _ => Response {
                    status: ResponseStatus::InvalidRequest,
                    message: "unexpected request".to_owned(),
                },
            })
            .expect("follow-up request should still be served");
    });

    thread::sleep(Duration::from_millis(20));

    // Construct a header with a declared payload length of 10 MiB.
    // The server MUST reject this before calling Vec::with_capacity on
    // the attacker-declared size.
    let declared: u32 = 10 * 1024 * 1024;
    let mut header = [0u8; 8];
    header[0..4].copy_from_slice(&declared.to_le_bytes());
    // version + message_type are irrelevant; the size gate fires first.
    header[4..6].copy_from_slice(&1u16.to_le_bytes());
    header[6..8].copy_from_slice(&1u16.to_le_bytes());

    // Sanity: the size we're about to declare is well above the cap.
    assert!(declared as usize > MAX_REQUEST_BYTES);

    let mut stream = UnixStream::connect(&socket_path).expect("client should connect");
    stream
        .write_all(&header)
        .expect("oversized header should write");
    let _ = stream.shutdown(std::net::Shutdown::Write);

    // Server must close without sending a response: read_to_end yields
    // zero bytes. This also implicitly proves no allocation of 10 MiB
    // was attempted (the server would have hung on read_exact otherwise).
    let mut response_bytes = Vec::new();
    stream
        .read_to_end(&mut response_bytes)
        .expect("server-closed stream should read cleanly");
    assert!(
        response_bytes.is_empty(),
        "server must drop oversized-frame connection without writing a response; got {} bytes",
        response_bytes.len()
    );

    // Follow-up legit client confirms server is still healthy.
    let client = IpcClient;
    let response = client
        .send(
            &socket_path,
            &Request::Plain {
                method: Method::GetHealth,
            },
        )
        .expect("follow-up client send should succeed");
    assert_eq!(response.status, ResponseStatus::Ok);
    assert_eq!(response.message, "healthy");

    handle.join().expect("server thread should exit");
}

/// A normal-sized request must be accepted and served.
#[test]
fn legit_size_request_succeeds() {
    let socket_path = unique_socket_path("legit-size");
    let server = IpcServer::new(current_effective_uid());
    let bound = server.bind(&socket_path).expect("socket should bind");

    let handle = thread::spawn(move || {
        bound
            .serve_once(|request| match request {
                Request::Plain {
                    method: Method::GetHealth,
                } => Response {
                    status: ResponseStatus::Ok,
                    message: "healthy".to_owned(),
                },
                _ => Response {
                    status: ResponseStatus::InvalidRequest,
                    message: "unexpected request".to_owned(),
                },
            })
            .expect("legit request should be served");
    });

    thread::sleep(Duration::from_millis(20));

    let client = IpcClient;
    let response = client
        .send(
            &socket_path,
            &Request::Plain {
                method: Method::GetHealth,
            },
        )
        .expect("client send should succeed");
    assert_eq!(response.status, ResponseStatus::Ok);
    assert_eq!(response.message, "healthy");

    // Also sanity-check a request encoded via the protocol layer is well
    // under the cap, matching the in-line comment on MAX_REQUEST_BYTES.
    let bytes = protocol::encode_request_bare(&Request::Plain {
        method: Method::GetHealth,
    })
    .expect("request encodes");
    assert!(bytes.len() < 64 * 1024);

    handle.join().expect("server thread should exit");
}

/// Unit-level confirmation that the error variant carries both the
/// declared and max sizes and that its Display output is informative.
#[test]
fn request_too_large_error_carries_sizes() {
    let err = IpcError::RequestTooLarge {
        declared: 10 * 1024 * 1024,
        max: MAX_REQUEST_BYTES,
    };
    let rendered = err.to_string();
    assert!(rendered.contains("10485760"), "got: {rendered}");
    assert!(
        rendered.contains(&MAX_REQUEST_BYTES.to_string()),
        "got: {rendered}"
    );
}
