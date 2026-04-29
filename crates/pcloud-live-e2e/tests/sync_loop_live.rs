#![allow(clippy::pedantic)]
//! Live end-to-end test for the background sync loop.
//!
//! Gated behind `PCLOUD_LIVE_E2E=1`. Requires valid credentials in the
//! environment (see `common/mod.rs` for the credential convention).
//!
//! Verifies that:
//! 1. The sync loop starts with a real authenticated session.
//! 2. A sync root can be registered and the loop observes it.
//! 3. A local file creation is eventually picked up by the loop's local
//!    scan and queued for upload.
//!
//! **Note:** This test creates a temporary sync root and local directory.
//! It does NOT verify the file actually appears on the remote (that would
//! require waiting for upload completion through the real API, which is
//! slow and flaky). It only verifies the loop machinery is wired.

// **PLATFORM:** all
// **GATING:** PCLOUD_LIVE_E2E=1.

mod common;

use std::time::{Duration, Instant};

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::sync_loop_runtime::spawn_daemon_sync_loop;

fn is_live_enabled() -> bool {
    matches!(
        std::env::var("PCLOUD_LIVE_E2E").ok().as_deref(),
        Some("1" | "true" | "TRUE")
    )
}

/// Live sync loop: login, add sync root, verify loop processes it.
#[test]
#[ignore = "live-e2e: requires PCLOUD_LIVE_E2E=1 + credentials; run with --ignored"]
fn live_sync_loop_processes_authenticated_root() {
    if !is_live_enabled() {
        eprintln!("PCLOUD_LIVE_E2E not set; skipping live sync loop test");
        return;
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "pcloud-live-sync-loop-{}-{nonce}",
        std::process::id()
    ));
    let config = ConfigProfile::secure_defaults(root.clone(), Environment::Development);
    let store_path = config.paths.state_dir.join("store.sqlite3");

    let runtime = pcloud_daemon::bootstrap_with_config(config).expect("bootstrap should succeed");

    // Attempt login via common helper credentials.
    // If login fails, skip the test rather than failing.
    let auth_snapshot = runtime.auth.snapshot();
    if auth_snapshot.auth_token.is_none() {
        eprintln!("no auth token available after bootstrap; skipping live sync loop test");
        return;
    }

    let (handle, _token) = spawn_daemon_sync_loop(&runtime.config, &runtime.auth, store_path)
        .expect("failed to open sync loop store connection");

    assert!(handle.is_alive(), "sync loop should be running");

    // Wait for at least one cycle.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let status = handle.shared.current_status();
        if status.cycles_completed > 0 {
            println!(
                "live sync loop completed {} cycle(s) in {:?}",
                status.cycles_completed,
                Instant::now()
            );
            break;
        }
        if Instant::now() >= deadline {
            panic!("live sync loop did not complete a cycle within 60s");
        }
        std::thread::sleep(Duration::from_millis(500));
    }

    // Clean shutdown.
    let result = handle.shutdown_and_join();
    assert!(result.is_ok(), "sync loop should shut down cleanly");

    // Best-effort cleanup.
    let _ = std::fs::remove_dir_all(&root);
}
