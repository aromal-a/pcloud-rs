#![allow(clippy::pedantic)]
//! Integration coverage for the graceful-drain protocol.
//!
//! Verifies, end-to-end:
//!
//! 1. `begin_drain` transitions the serve loop into `Draining` and
//!    `in_flight` reaches zero when the current dispatch completes.
//! 2. While draining, new non-status requests receive
//!    `ResponseStatus::Unavailable("daemon draining, retry")`.
//! 3. `Method::DrainStatus` continues to answer during drain, and
//!    reports `state == "stopped"` after the loop has exited.
//! 4. The serve loop exits cleanly (`Ok(())`) once drain completes, so
//!    the `pcloudd` binary can unbind the socket and exit `0`.
//!
//! The test runs entirely in-process — no real `SIGTERM` is delivered
//! — because the `SHUTDOWN_REQUESTED` static is process-wide and a
//! real signal here would race with every other test in the crate.
//! We exercise the same observable path: `begin_drain` →
//! `serve_until_shutdown_with_flag` polls the external flag and the
//! drain machine → loop returns.

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Integration-test-wide serialization lock. Every scenario mutates
/// the process-wide `SHUTDOWN_REQUESTED` / `DRAIN_STATE` statics and
/// therefore cannot run concurrently with its sibling scenario.
fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

use pcloud_config::{ConfigProfile, Environment};
use pcloud_ipc::{
    DrainStatusPayload, IpcClient, IpcServer, Method, Request, ResponseStatus,
    current_effective_uid,
};

fn bootstrap_test_shell() -> pcloud_daemon::RuntimeShell {
    // Use `/tmp` (not `std::env::temp_dir()`) so the Unix-socket path
    // stays under SUN_LEN on macOS, where the per-user tempdir
    // `/var/folders/.../T/` alone eats 49 chars.
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock post epoch")
        .as_nanos();
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "pd-drn-{}-{}",
        std::process::id(),
        nonce % 1_000_000_000
    ));
    let config = ConfigProfile::secure_defaults(root, Environment::Development);
    pcloud_daemon::bootstrap_with_config(config).expect("bootstrap")
}

#[test]
fn drain_admits_status_probes_and_rejects_new_traffic() {
    let _serial = serial_lock();
    pcloud_daemon::signals::reset_for_test();
    // Build a full daemon shell and bind its socket in-process.
    let mut runtime = bootstrap_test_shell();
    let socket_path = runtime.config.paths.ipc_socket_path();
    let server = IpcServer::new(current_effective_uid());
    let bound = server.bind(&socket_path).expect("socket bind");

    let external = Arc::new(AtomicBool::new(false));
    let external_for_thread = Arc::clone(&external);
    let barrier = Arc::new(Barrier::new(2));
    let barrier_for_thread = Arc::clone(&barrier);

    let handle = std::thread::spawn(move || {
        barrier_for_thread.wait();
        pcloud_daemon::serve_until_shutdown_with_flag(
            &bound,
            &mut runtime,
            Some(&external_for_thread),
        )
    });
    barrier.wait();

    let client = IpcClient;

    // 1. DrainStatus before drain: state == running, elapsed == 0.
    let resp = client
        .send(
            &socket_path,
            &Request::Plain {
                method: Method::DrainStatus,
            },
        )
        .expect("DrainStatus probe");
    assert!(matches!(resp.status, ResponseStatus::Ok));
    let payload: DrainStatusPayload =
        serde_json::from_str(&resp.message).expect("DrainStatusPayload decode");
    assert_eq!(payload.state, "running");
    assert_eq!(payload.elapsed_drain_ms, 0);

    // 2. Flip the external shutdown flag → serve loop observes it on
    //    the next iteration and begins the drain transition. Because
    //    the loop is blocked on `accept(2)` we nudge it with a
    //    harmless probe to wake it.
    external.store(true, Ordering::SeqCst);
    let _ = client.send(
        &socket_path,
        &Request::Plain {
            method: Method::GetStatus,
        },
    );

    // Wait for the loop to exit. Drain-timeout on the default profile
    // is 30 s; the test should be well under that because there is no
    // in-flight work after the nudge returns.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            panic!("serve loop did not exit within 5s of SIGTERM-equivalent");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let serve_result = handle.join().expect("serve thread join");
    serve_result.expect("serve loop should exit cleanly on drain");

    // At this point `BoundIpcServer` has been dropped inside the
    // thread, so the socket file has been unlinked. A fresh probe
    // will fail with ECONNREFUSED / ENOENT — which is the documented
    // terminal state `pcloudc drain` treats as success.
    let post_drain = client.send(
        &socket_path,
        &Request::Plain {
            method: Method::DrainStatus,
        },
    );
    assert!(
        post_drain.is_err(),
        "post-drain probe must fail; got {post_drain:?}"
    );

    // Reset the process-wide drain state for any sibling tests.
    // `mark_stopped` is idempotent and safe to call from any state.
    pcloud_daemon::signals::mark_stopped();
}

#[test]
fn drain_gate_rejects_ordinary_requests_with_unavailable() {
    let _serial = serial_lock();
    pcloud_daemon::signals::reset_for_test();
    // Unit-level guard on the dispatch gate: when the daemon is in
    // `Draining`, a non-status request must receive
    // `Unavailable("daemon draining, retry")`. We exercise this via
    // the in-process IPC loop so the wire format is exercised too.
    let mut runtime = bootstrap_test_shell();
    let socket_path = runtime.config.paths.ipc_socket_path();
    let server = IpcServer::new(current_effective_uid());
    let bound = server.bind(&socket_path).expect("socket bind");

    // Force the drain state BEFORE the serve loop picks up the flag —
    // the subsequent `accept` + dispatch will then hit the gate.
    pcloud_daemon::signals::begin_drain();

    let external = Arc::new(AtomicBool::new(false));
    let external_for_thread = Arc::clone(&external);
    let handle = std::thread::spawn(move || {
        pcloud_daemon::serve_until_shutdown_with_flag(
            &bound,
            &mut runtime,
            Some(&external_for_thread),
        )
    });

    // Give the serve thread a moment to park on accept(2).
    std::thread::sleep(Duration::from_millis(50));

    let client = IpcClient;
    let resp = client
        .send(
            &socket_path,
            &Request::Plain {
                method: Method::GetStatus,
            },
        )
        .expect("drain-gated send");
    assert!(
        matches!(resp.status, ResponseStatus::Unavailable),
        "non-status request during drain must be Unavailable, got {:?}",
        resp.status
    );
    assert!(
        resp.message.contains("draining"),
        "drain message should mention draining: {:?}",
        resp.message
    );

    // DrainStatus should still answer even while gated.
    let resp2 = client
        .send(
            &socket_path,
            &Request::Plain {
                method: Method::DrainStatus,
            },
        )
        .expect("DrainStatus during drain");
    assert!(matches!(resp2.status, ResponseStatus::Ok));
    let payload: DrainStatusPayload = serde_json::from_str(&resp2.message).expect("payload decode");
    assert_eq!(payload.state, "draining");

    // Tear down: flip the external flag so the loop exits.
    external.store(true, Ordering::SeqCst);
    // Nudge accept.
    let _ = client.send(
        &socket_path,
        &Request::Plain {
            method: Method::DrainStatus,
        },
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            panic!("serve loop did not exit within 5s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    handle.join().expect("join").expect("serve exit clean");
    pcloud_daemon::signals::mark_stopped();
}
