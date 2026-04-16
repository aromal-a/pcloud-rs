#![allow(clippy::pedantic)]
//! Demonstrates an end-to-end IPC round-trip against an in-process daemon.
//!
//! Rather than hard-coding a socket path (which requires a running daemon and
//! matching UID), this example brings up a `BoundIpcServer` on a temporary
//! socket, answers a single request, then connects as a client and prints the
//! round-tripped `Response`. The same transport is used by the real
//! daemon + CLI.
//!
//! Run with: `cargo run -p pcloud-ipc --example client_ping`

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;
use std::thread;

use pcloud_ipc::{
    IpcClient, IpcServer, Method, Request, Response, ResponseStatus, current_effective_uid,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Temp socket under /tmp — the transport will chmod it 0600 and chmod the
    // parent 0700 on creation.
    let tmp = std::env::temp_dir().join(format!("pcloud-ipc-example-{}", std::process::id()));
    std::fs::create_dir_all(&tmp)?;
    let socket_path: PathBuf = tmp.join("ping.sock");

    let server = IpcServer::new(current_effective_uid());
    let bound = server.bind(&socket_path)?;
    let server_socket = bound.socket_path().to_path_buf();

    // Handle exactly one connection on a background thread.
    let handle = thread::spawn(move || {
        let _ = bound.serve_once(|req| match req {
            Request::Plain {
                method: Method::GetStatus,
            } => Response {
                status: ResponseStatus::Ok,
                message: "pong".into(),
            },
            other => Response {
                status: ResponseStatus::InvalidRequest,
                message: format!("unexpected request: {other:?}"),
            },
        });
    });

    let client = IpcClient;
    let response = client.send(
        &server_socket,
        &Request::Plain {
            method: Method::GetStatus,
        },
    )?;

    println!("status:  {:?}", response.status);
    println!("message: {}", response.message);

    handle.join().expect("server thread panicked");
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}
