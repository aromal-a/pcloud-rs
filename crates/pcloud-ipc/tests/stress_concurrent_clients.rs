#![allow(clippy::pedantic)]
//! Concurrent IPC stress test: 50 clients × 500 sequential requests each
//! against a dev-mode owner-only Unix-socket server that loops `serve_once`.
//!
//! Gated by `#[ignore]` — run explicitly with:
//!
//! ```text
//! cargo test --release -p pcloud-ipc -- --ignored stress
//! ```

// **PLATFORM:** Linux
// **GATING:** none (portable; uses Linux-only idioms — see TODO(bd-xplat)).

use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use pcloud_ipc::{
    IpcClient, IpcServer, current_effective_uid,
    methods::{Method, Request, Response, ResponseStatus},
};

const CLIENTS: usize = 50;
const REQUESTS_PER_CLIENT: usize = 500;

fn open_fd_count() -> io::Result<usize> {
    let mut count = 0;
    // TODO(bd-xplat): Linux-only — needs cfg gate or platform trait abstraction. See PLAN_CROSSPLATFORM.md §2.
    for entry in std::fs::read_dir("/proc/self/fd")? {
        let _ = entry?;
        count += 1;
    }
    Ok(count)
}

#[test]
#[ignore = "stress: 50 clients x 500 reqs, run with --release --ignored"]
fn stress_concurrent_ipc_clients_do_not_leak_or_panic() {
    let socket_path: PathBuf = std::env::temp_dir().join(format!(
        "pcloud-ipc-stress-{}-{}.sock",
        std::process::id(),
        std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
    ));
    let server = IpcServer::new(current_effective_uid());
    let bound = Arc::new(server.bind(&socket_path).expect("bind"));

    let stop = Arc::new(AtomicBool::new(false));
    let served = Arc::new(AtomicU64::new(0));

    let server_stop = Arc::clone(&stop);
    let server_served = Arc::clone(&served);
    let server_bound = Arc::clone(&bound);
    let server_thread = thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            let result = server_bound.serve_once(|request| match request {
                Request::Plain {
                    method: Method::GetHealth,
                } => Response {
                    status: ResponseStatus::Ok,
                    message: "healthy".to_owned(),
                },
                Request::Plain {
                    method: Method::GetStatus,
                } => Response {
                    status: ResponseStatus::Ok,
                    message: "ready".to_owned(),
                },
                other => Response {
                    status: ResponseStatus::InvalidRequest,
                    message: format!("unexpected: {other:?}"),
                },
            });
            if result.is_ok() {
                server_served.fetch_add(1, Ordering::Relaxed);
            }
        }
    });

    thread::sleep(Duration::from_millis(50));
    let baseline_fds = open_fd_count().unwrap_or(0);

    let start = Instant::now();
    let mut handles = Vec::with_capacity(CLIENTS);
    for client_idx in 0..CLIENTS {
        let sp = socket_path.clone();
        handles.push(thread::spawn(move || -> Result<(), String> {
            let client = IpcClient;
            for req_idx in 0..REQUESTS_PER_CLIENT {
                let method = if req_idx % 2 == 0 {
                    Method::GetHealth
                } else {
                    Method::GetStatus
                };
                let response = client
                    .send(&sp, &Request::Plain { method })
                    .map_err(|e| format!("client {client_idx} req {req_idx}: {e}"))?;
                if response.status != ResponseStatus::Ok {
                    return Err(format!(
                        "client {client_idx} req {req_idx} non-ok: {:?}",
                        response.status
                    ));
                }
            }
            Ok(())
        }));
    }

    let mut failures = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok(())) => {}
            Ok(Err(msg)) => failures.push(msg),
            Err(panic) => failures.push(format!("client thread panicked: {panic:?}")),
        }
    }

    stop.store(true, Ordering::Relaxed);
    let _ = std::os::unix::net::UnixStream::connect(&socket_path);
    let _ = server_thread.join();

    let elapsed = start.elapsed();
    let after_fds = open_fd_count().unwrap_or(0);
    drop(bound);

    assert!(
        failures.is_empty(),
        "stress workload produced {} failures; first={:?}",
        failures.len(),
        failures.first()
    );

    let total = CLIENTS * REQUESTS_PER_CLIENT;
    let served_count = served.load(Ordering::Relaxed) as usize;
    assert!(
        served_count >= total,
        "server served {served_count} < expected {total}"
    );

    let fd_drift = after_fds.saturating_sub(baseline_fds);
    assert!(
        fd_drift <= 64,
        "fd drift {fd_drift} exceeds leak ceiling (baseline={baseline_fds}, after={after_fds})"
    );

    assert!(!socket_path.exists(), "socket path should be cleaned up");

    eprintln!(
        "stress complete: {total} requests in {:.2?} ({:.0} req/s), fd drift={fd_drift}",
        elapsed,
        total as f64 / elapsed.as_secs_f64()
    );
}
