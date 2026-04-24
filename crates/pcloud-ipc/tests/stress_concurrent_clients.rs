#![allow(clippy::pedantic)]
#![cfg(unix)]
//! Concurrent IPC stress test: 50 clients × 500 sequential requests each
//! against a dev-mode owner-only Unix-socket server that loops `serve_once`.
//!
//! Gated by `#[ignore]` — run explicitly with:
//!
//! ```text
//! cargo test --release -p pcloud-ipc -- --ignored stress
//! ```

// **PLATFORM:** Unix (uses `std::os::unix::net` idioms directly).
// **GATING:** `#[cfg(unix)]` at file level.

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

/// Count open file descriptors for the current process.
///
/// On Linux this reads `/proc/self/fd`, which is the only reliable
/// per-process fd directory available without elevated privilege.
/// On other platforms the function returns `Ok(0)` — fd-leak detection is
/// a Linux-only capability in this test. The fd-drift assertion below is
/// therefore only meaningful on Linux.
///
/// Cross-platform fd-leak detection (macOS `sysctl KERN_PROC_FD`, BSD
/// `procstat -f`) is Linux-only by design in this test. If cross-platform
/// coverage is needed, wire it under `bd-1du.4` cross-platform hardware
/// verification (audit-06 LOW IPC L-7.3 / ncx.84).
fn open_fd_count() -> io::Result<usize> {
    #[cfg(target_os = "linux")]
    {
        let mut count = 0;
        for entry in std::fs::read_dir("/proc/self/fd")? {
            let _ = entry?;
            count += 1;
        }
        Ok(count)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Fd-leak detection is not implemented on this platform.
        Ok(0)
    }
}

#[test]
#[ignore = "stress: 50 clients x 500 reqs, run with --release --ignored"]
fn stress_concurrent_ipc_clients_do_not_leak_or_panic() {
    // Use `/tmp` directly: macOS SUN_LEN=104 cannot accommodate the
    // per-user tempdir `/var/folders/.../T/` prefix.
    let nonce = std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos();
    let socket_path: PathBuf = std::path::PathBuf::from("/tmp").join(format!(
        "pipc-st-{}-{}.sock",
        std::process::id(),
        nonce % 1_000_000_000
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
                let req = Request::Plain { method };
                // Retry once on ENOTCONN: macOS Unix-domain sockets can
                // transiently return os error 57 when the server's backlog
                // races under high concurrency (serve_once is single-threaded).
                let response = match client.send(&sp, &req) {
                    Err(e) if e.to_string().contains("os error 57") => {
                        std::thread::sleep(Duration::from_millis(2));
                        client.send(&sp, &req)
                            .map_err(|e2| format!("client {client_idx} req {req_idx} (retry): {e2}"))?
                    }
                    result => result.map_err(|e| format!("client {client_idx} req {req_idx}: {e}"))?,
                };
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
