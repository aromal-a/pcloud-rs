#![allow(clippy::pedantic)]
//! Feature-gated integration tests for the observability metric wiring
//! landed against `bd-1du` (observability-daemon-integration).
//!
//! These tests are only compiled under `--features metrics`. They:
//! - drive synthetic events through the dispatch loop,
//! - assert metric counters/gauges increment correctly,
//! - assert the Prometheus exporter renders the expected families and
//!   labels,
//! - assert the upstream label sanitizer still guards against bad input
//!   (e.g. no quotes/newlines/backslashes leak into labels).
//!
//! The tests never invoke a real pCloud server. They use
//! `Environment::Development` and synthetic auth tokens (see
//! `crypto_change_password.rs` for the same pattern).

#![cfg(feature = "metrics")]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_auth::AuthCommand;
use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::{bootstrap_with_config, dispatch};
use pcloud_ipc::{Method, Request};
use pcloud_model::ids::UserId;
use pcloud_observability::metrics::{AuthResult, CryptoLockState, TransferDirection};
use pcloud_secret::secret_string::SecretString;

fn fresh_shell(label: &str) -> pcloud_daemon::RuntimeShell {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock should be after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "pcloud-daemon-metrics-{label}-{}-{nonce}",
        std::process::id()
    ));
    let config = ConfigProfile::secure_defaults(root, Environment::Development);
    let mut runtime = bootstrap_with_config(config).expect("bootstrap ok");
    runtime
        .auth
        .apply(AuthCommand::LoginWithToken {
            token: SecretString::new("metrics-test-token".to_owned()),
        })
        .expect("login token");
    runtime
        .auth
        .apply(AuthCommand::MarkAuthenticated {
            user_id: Some(UserId::new(1)),
            auth_token: SecretString::new("metrics-test-token".to_owned()),
        })
        .expect("mark authenticated");
    runtime
}

#[test]
fn dispatch_records_method_status_and_latency() {
    let mut runtime = fresh_shell("dispatch");

    let _ = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetHealth,
        },
    );
    let _ = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetHealth,
        },
    );
    let _ = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetStatus,
        },
    );

    let families = &runtime.observability.families;
    let health_count = families
        .request_count
        .get(&("GetHealth".to_owned(), "ok".to_owned()))
        .copied()
        .unwrap_or(0);
    assert_eq!(health_count, 2, "two GetHealth dispatches must be counted");

    let status_count = families
        .request_count
        .get(&("GetStatus".to_owned(), "ok".to_owned()))
        .copied()
        .unwrap_or(0);
    assert_eq!(status_count, 1);

    let snap = runtime
        .observability
        .health_report()
        .metrics_snapshot
        .unwrap();
    assert!(snap.contains("pcloud_request_count{method=\"GetHealth\",status=\"ok\"} 2"));
    assert!(snap.contains("pcloud_request_latency_seconds_count{method=\"GetHealth\"} 2"));
    assert!(snap.contains("pcloud_request_latency_seconds_count{method=\"GetStatus\"} 1"));
}

#[test]
fn sync_root_gauge_tracks_add_and_remove_via_direct_family_updates() {
    // The full sync-root add/remove path requires a real remote-folder
    // validation against a network API (not available in unit tests).
    // For a deterministic test we drive the helper directly — the same
    // helper that runtime hooks call after each successful persist.
    let mut runtime = fresh_shell("syncroots");
    runtime
        .store
        .repositories
        .sync_graph
        .tracked_sync_roots
        .push(pcloud_store::repositories::sync_graph::SyncRootRecord {
            sync_id: pcloud_model::ids::SyncId::new(1),
            local_path: "/tmp/a".into(),
            remote_path: "/a".into(),
            paused: false,
            sync_type: pcloud_model::sync::SyncType::Full,
        });
    runtime.metric_sync_root_count();
    assert_eq!(runtime.observability.families.sync_root_count, 1);

    runtime
        .store
        .repositories
        .sync_graph
        .tracked_sync_roots
        .push(pcloud_store::repositories::sync_graph::SyncRootRecord {
            sync_id: pcloud_model::ids::SyncId::new(2),
            local_path: "/tmp/b".into(),
            remote_path: "/b".into(),
            paused: false,
            sync_type: pcloud_model::sync::SyncType::Full,
        });
    runtime.metric_sync_root_count();
    assert_eq!(runtime.observability.families.sync_root_count, 2);

    runtime
        .store
        .repositories
        .sync_graph
        .tracked_sync_roots
        .clear();
    runtime.metric_sync_root_count();
    assert_eq!(runtime.observability.families.sync_root_count, 0);
}

#[test]
fn crypto_lock_unlock_updates_gauge() {
    let mut runtime = fresh_shell("crypto");

    let _ = dispatch(
        &mut runtime,
        Request::CryptoSetup {
            password: "test-pass".to_owned(),
            hint: Some("hint".to_owned()),
        },
    );
    let snap = runtime
        .observability
        .health_report()
        .metrics_snapshot
        .unwrap();
    // After setup the shell transitions to Locked (0) per CryptoShell::setup.
    assert!(
        snap.contains("pcloud_crypto_lock_state 0") || snap.contains("pcloud_crypto_lock_state 1"),
        "unexpected crypto state snapshot: {snap}"
    );

    let _ = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::LockCrypto,
        },
    );
    // After LockCrypto, the gauge must reflect the documented value mapping
    // (-1=Unsetup, 0=Locked, 1=Unlocked). Compare through the public
    // `as_value()` helper rather than discriminant casts, which would
    // conflate the enum layout with the gauge encoding.
    let actual = runtime.observability.families.crypto_lock_state;
    assert!(
        actual == CryptoLockState::Locked.as_value()
            || actual == CryptoLockState::Unsetup.as_value(),
        "expected crypto gauge to reflect Locked or Unsetup after LockCrypto, got {actual}"
    );
    // The snapshot must also include the gauge line so scrapers can read it.
    let snap = runtime
        .observability
        .health_report()
        .metrics_snapshot
        .unwrap();
    assert!(
        snap.contains(&format!("pcloud_crypto_lock_state {actual}")),
        "prometheus snapshot missing crypto gauge line: {snap}"
    );
}

#[test]
fn ipc_client_gauge_increments_and_decrements_and_clamps() {
    let mut runtime = fresh_shell("ipc");
    assert_eq!(runtime.observability.families.ipc_connected_clients, 0);
    runtime.on_ipc_client_connected();
    runtime.on_ipc_client_connected();
    assert_eq!(runtime.observability.families.ipc_connected_clients, 2);
    runtime.on_ipc_client_disconnected();
    assert_eq!(runtime.observability.families.ipc_connected_clients, 1);
    runtime.on_ipc_client_disconnected();
    runtime.on_ipc_client_disconnected(); // over-disconnect must clamp to 0
    assert_eq!(runtime.observability.families.ipc_connected_clients, 0);
}

#[test]
fn transfer_bytes_counter_accumulates_by_direction() {
    let mut runtime = fresh_shell("transfer");
    runtime
        .observability
        .families
        .add_transfer_bytes(TransferDirection::Upload, 1000);
    runtime
        .observability
        .families
        .add_transfer_bytes(TransferDirection::Upload, 500);
    runtime
        .observability
        .families
        .add_transfer_bytes(TransferDirection::Download, 4096);

    let snap = runtime
        .observability
        .health_report()
        .metrics_snapshot
        .unwrap();
    assert!(snap.contains("pcloud_transfer_bytes_total{direction=\"upload\"} 1500"));
    assert!(snap.contains("pcloud_transfer_bytes_total{direction=\"download\"} 4096"));
}

#[test]
fn auth_result_counter_increments_on_events() {
    let mut runtime = fresh_shell("auth");
    runtime
        .observability
        .families
        .record_auth(AuthResult::Success);
    runtime
        .observability
        .families
        .record_auth(AuthResult::Failure);
    runtime
        .observability
        .families
        .record_auth(AuthResult::Failure);
    let snap = runtime
        .observability
        .health_report()
        .metrics_snapshot
        .unwrap();
    assert!(snap.contains("pcloud_auth_attempts_total{result=\"success\"} 1"));
    assert!(snap.contains("pcloud_auth_attempts_total{result=\"failure\"} 2"));
}

#[test]
fn label_sanitizer_blocks_injection_from_status_and_method_labels() {
    // The handle_request entry point only ever supplies `&'static` method
    // names from the internal lookup, so injection is structurally
    // impossible from that side. Still, the upstream sanitizer must
    // neutralize adversarial strings fed directly to observe_request
    // (belt-and-braces guarantee for any future ad-hoc callers).
    let mut runtime = fresh_shell("sanitize");
    runtime.observability.families.observe_request(
        "Get\"Bad\nMethod\\x",
        "ok\" label=\"evil",
        0.001,
    );
    let snap = runtime
        .observability
        .health_report()
        .metrics_snapshot
        .unwrap();
    assert!(!snap.contains("\"Bad"));
    assert!(!snap.contains("evil"));
    // Newlines are required between exposition lines; what we MUST reject is
    // a newline appearing *inside* a quoted label value (which would break
    // scrapers and enable log-injection). Check each individual line for
    // balanced quotes rather than asserting on the whole snapshot.
    for line in snap.lines() {
        let quote_count = line.chars().filter(|c| *c == '"').count();
        assert!(
            quote_count % 2 == 0,
            "unbalanced quotes on line (label value may span a newline): {line}"
        );
    }
    // Also assert the raw injected newline from the method label did not
    // survive verbatim anywhere in a label-value position.
    assert!(
        !snap.contains("Method\n"),
        "raw newline from injected method label leaked: {snap}"
    );
}

#[test]
fn prometheus_export_contains_expected_metric_families_after_workload() {
    let mut runtime = fresh_shell("workload");
    let _ = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetStatus,
        },
    );
    let _ = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetHealth,
        },
    );
    runtime
        .observability
        .families
        .record_auth(AuthResult::Success);
    runtime
        .observability
        .families
        .add_transfer_bytes(TransferDirection::Download, 64);
    runtime.on_ipc_client_connected();
    runtime.metric_sync_root_count();

    let snap = runtime
        .observability
        .health_report()
        .metrics_snapshot
        .unwrap();
    for expected in [
        "# TYPE pcloud_request_count counter",
        "# TYPE pcloud_request_latency_seconds histogram",
        "# TYPE pcloud_auth_attempts_total counter",
        "# TYPE pcloud_transfer_bytes_total counter",
        "# TYPE pcloud_crypto_lock_state gauge",
        "# TYPE pcloud_sync_root_count gauge",
        "# TYPE pcloud_ipc_connected_clients gauge",
        "# TYPE pcloud_panic_count counter",
    ] {
        assert!(
            snap.contains(expected),
            "prometheus snapshot missing family: {expected}\n---\n{snap}"
        );
    }
}

#[test]
fn panic_hook_increments_panic_metric_on_refresh() {
    pcloud_daemon::install_panic_metrics_hook();
    let mut runtime = fresh_shell("panic");

    // Simulate a panic in a separate thread so the installed hook runs
    // without tearing down the test process.
    let _ = std::thread::spawn(|| {
        let _ = std::panic::catch_unwind(|| {
            panic!("synthetic panic for metrics test");
        });
    })
    .join();

    // A dispatch call folds the global panic counter into the metric.
    let _ = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetStatus,
        },
    );
    assert!(
        runtime.observability.families.panic_count >= 1,
        "panic metric should have been incremented by the hook"
    );
}
