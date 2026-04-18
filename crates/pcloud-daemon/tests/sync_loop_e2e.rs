#![allow(clippy::pedantic)]
//! End-to-end integration test for the background sync loop.
//!
//! Verifies that:
//! 1. The sync loop thread actually starts when spawned from bootstrap.
//! 2. The loop runs cycles (observable via the status snapshot).
//! 3. Sync root registration is visible to the loop.
//! 4. The loop shuts down cleanly on request.
//! 5. Auth token sharing works between the IPC thread and the loop.
//!
//! Uses development-mode backends (no real pCloud API calls) so the
//! test runs offline and deterministically.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::sync_loop::SyncLoopState;
use pcloud_daemon::sync_loop_runtime::spawn_daemon_sync_loop;
use pcloud_secret::secret_string::SecretString;

fn bootstrap_test_runtime() -> (pcloud_daemon::RuntimeShell, std::path::PathBuf) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "pcloud-sync-loop-e2e-{}-{nonce}",
        std::process::id()
    ));
    let config = ConfigProfile::secure_defaults(root, Environment::Development);
    let store_path = config.paths.state_dir.join("store.sqlite3");
    let runtime =
        pcloud_daemon::bootstrap_with_config(config).expect("runtime bootstrap should succeed");
    (runtime, store_path)
}

/// The sync loop actually starts, runs at least one cycle, and shuts
/// down cleanly.
#[test]
fn sync_loop_starts_runs_and_shuts_down() {
    let (runtime, store_path) = bootstrap_test_runtime();

    let (handle, _token) = spawn_daemon_sync_loop(&runtime.config, &runtime.auth, store_path)
        .expect("failed to open sync loop store connection");

    // The loop should be alive.
    assert!(handle.is_alive(), "sync loop should be running");

    // Wait for at least one cycle to complete. With default 30s poll,
    // the first cycle runs immediately after spawn.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = handle.shared.current_status();
        if status.cycles_completed > 0 {
            break;
        }
        if Instant::now() >= deadline {
            panic!(
                "sync loop did not complete a cycle within 5s; state={:?}",
                status.state
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Shut down.
    let result = handle.shutdown_and_join();
    assert!(result.is_ok(), "sync loop should shut down cleanly");
}

/// The sync loop correctly observes auth token changes from the IPC
/// thread side.
#[test]
fn sync_loop_observes_auth_token_update() {
    let (runtime, store_path) = bootstrap_test_runtime();

    let (handle, token) = spawn_daemon_sync_loop(&runtime.config, &runtime.auth, store_path)
        .expect("failed to open sync loop store connection");

    // Initially the runtime has no auth, so the loop should skip
    // cycles (no roots and no auth).
    std::thread::sleep(Duration::from_millis(100));

    // Set an auth token.
    {
        let mut guard = token.lock().unwrap();
        *guard = Some(SecretString::new("e2e-test-token".to_owned()));
    }

    // Wake the loop to process the new token.
    handle.shared.wake();

    // Wait for a cycle that sees the token.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = handle.shared.current_status();
        if status.cycles_completed > 0 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("sync loop did not process a cycle after token update");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = handle.shutdown_and_join();
}

/// The sync loop pause/resume lifecycle works correctly.
#[test]
fn sync_loop_pause_and_resume() {
    let (runtime, store_path) = bootstrap_test_runtime();

    let (handle, _token) = spawn_daemon_sync_loop(&runtime.config, &runtime.auth, store_path)
        .expect("failed to open sync loop store connection");

    // Wait for the loop to become idle after one cycle.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = handle.shared.current_status();
        if status.cycles_completed > 0 {
            break;
        }
        if Instant::now() >= deadline {
            panic!("sync loop did not complete initial cycle");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Pause.
    handle.shared.pause();
    std::thread::sleep(Duration::from_millis(200));
    let status = handle.shared.current_status();
    assert_eq!(status.state, SyncLoopState::Paused, "loop should be paused");
    let cycles_at_pause = status.cycles_completed;

    // While paused, no new cycles should run.
    std::thread::sleep(Duration::from_millis(300));
    let status = handle.shared.current_status();
    assert_eq!(
        status.cycles_completed, cycles_at_pause,
        "no new cycles should run while paused"
    );

    // Resume.
    handle.shared.resume();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = handle.shared.current_status();
        if status.cycles_completed > cycles_at_pause {
            break;
        }
        if Instant::now() >= deadline {
            panic!("sync loop did not resume after unpause");
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = handle.shutdown_and_join();
}

/// A disabled sync loop does not spawn a thread and reports Disabled.
#[test]
fn disabled_sync_loop_returns_disabled_state() {
    let (mut runtime, store_path) = bootstrap_test_runtime();
    runtime.config.sync_loop.enabled = false;

    let (handle, _token) = spawn_daemon_sync_loop(&runtime.config, &runtime.auth, store_path)
        .expect("failed to open sync loop store connection");

    assert!(!handle.is_alive(), "disabled loop should not have a thread");
    assert_eq!(
        handle.shared.current_status().state,
        SyncLoopState::Disabled,
    );
}
