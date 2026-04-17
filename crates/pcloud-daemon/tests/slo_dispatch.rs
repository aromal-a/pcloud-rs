#![allow(clippy::pedantic)]
//! Integration test for the canonical SLO IPC surface.
//!
//! Drives `Method::GetSlo` through `dispatch`, asserts the response is
//! a well-formed [`pcloud_ipc::SloReportPayload`] JSON document, and
//! verifies that the canonical SLO set is present with the stable
//! naming contract.
//!
//! No network traffic is required; the SLO registry is always available
//! on the runtime shell regardless of feature flags.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_config::{ConfigProfile, Environment};
use pcloud_daemon::{bootstrap_with_config, dispatch};
use pcloud_ipc::{Method, Request, ResponseStatus, SloReportPayload};

fn unique_root(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pcloud-daemon-slo-{tag}-{}-{nonce}",
        std::process::id()
    ))
}

fn fresh_runtime(tag: &str) -> pcloud_daemon::RuntimeShell {
    let config = ConfigProfile::secure_defaults(unique_root(tag), Environment::Development);
    bootstrap_with_config(config).expect("bootstrap runtime")
}

/// Stable contract: `Method::GetSlo` returns a JSON payload whose
/// `slos` array contains exactly the canonical 7-entry set in
/// registration order. Every entry starts out as `no_data` on a fresh
/// runtime (no observations yet).
#[test]
fn get_slo_returns_canonical_set_with_no_data_on_fresh_runtime() {
    let mut runtime = fresh_runtime("fresh");
    let response = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetSlo,
        },
    );
    assert_eq!(response.status, ResponseStatus::Ok, "{:?}", response);

    let payload: SloReportPayload =
        serde_json::from_str(&response.message).expect("valid SLO JSON");
    let names: Vec<&str> = payload.slos.iter().map(|e| e.slo_name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "ipc.request.latency.p99",
            "ipc.request.error_rate",
            "auth.login.success_rate",
            "upload.throughput_mbps.p50",
            "mount.read.latency.p99",
            "integrity_sweeper.run.p95",
            "audit.hash_chain.verify.daily_pass_rate",
        ],
        "canonical SLO name set is the stable contract",
    );
    for entry in &payload.slos {
        assert_eq!(
            entry.status, "no_data",
            "fresh runtime should report no_data for {}",
            entry.slo_name
        );
    }
    // `pass == true` on an empty registry is the documented "no data
    // means no breach" behaviour.
    assert!(payload.pass);
}

/// Observations on the shared SLO registry flow through to the JSON
/// payload: a deliberate latency breach flips the status to
/// `violation` and the aggregate `pass` bit to `false`.
#[test]
fn get_slo_reflects_observations_through_shared_registry() {
    let mut runtime = fresh_runtime("observed");
    for _ in 0..1000 {
        runtime.observability.slo.observe_ipc_latency(0.5); // 500ms
    }
    let response = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetSlo,
        },
    );
    assert_eq!(response.status, ResponseStatus::Ok);
    let payload: SloReportPayload =
        serde_json::from_str(&response.message).expect("valid SLO JSON");
    let entry = payload
        .slos
        .iter()
        .find(|e| e.slo_name == "ipc.request.latency.p99")
        .expect("ipc.request.latency.p99 present");
    assert_eq!(
        entry.status, "violation",
        "500ms latency must breach the 100ms p99 target; actual={}",
        entry.actual
    );
    assert!(!payload.pass, "aggregate pass bit must flip on violation");
}

/// Drive real dispatches (GetStatus, GetHealth) plus a simulated auth
/// login observation, then query `Method::GetSlo` and assert that the
/// IPC latency and auth SLOs have non-empty samples (`!= "no_data"`).
///
/// This proves the end-to-end wiring: `handle_request` auto-observes
/// `ipc.request.latency.p99` and `ipc.request.error_rate` on every
/// round-trip, and the auth SLO is driven by a direct
/// `observe_auth_login` (which in production fires inside
/// `auth_response` on `LoginSucceeded` / `LoginFailed` events).
#[test]
fn dispatch_plus_login_produces_non_empty_slo_samples() {
    let mut runtime = fresh_runtime("dispatch-login");

    // Drive several real IPC dispatches. Each one auto-records an IPC
    // latency + outcome observation inside `RuntimeShell::handle_request`.
    for _ in 0..5 {
        let resp = dispatch(
            &mut runtime,
            Request::Plain {
                method: Method::GetStatus,
            },
        );
        assert_eq!(resp.status, ResponseStatus::Ok);
    }
    let resp = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetHealth,
        },
    );
    assert_eq!(resp.status, ResponseStatus::Ok);

    // Simulate a successful login observation. In production this fires
    // inside the auth state machine on `AuthEvent::LoginSucceeded`.
    // We call it directly on the shared SLO Arc to prove the wiring.
    runtime.observability.slo.observe_auth_login(true);

    // Now query the SLO surface and assert the wired SLIs have data.
    let response = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetSlo,
        },
    );
    assert_eq!(response.status, ResponseStatus::Ok);
    let payload: SloReportPayload =
        serde_json::from_str(&response.message).expect("valid SLO JSON");

    // IPC latency: the 5 + 1 + 1 (GetSlo itself) dispatches must have
    // produced at least 7 latency samples. The SLO should no longer be
    // `no_data`.
    let ipc_lat = payload
        .slos
        .iter()
        .find(|e| e.slo_name == "ipc.request.latency.p99")
        .expect("ipc.request.latency.p99 present");
    assert_ne!(
        ipc_lat.status, "no_data",
        "IPC latency SLO must have data after real dispatches; got status={}, actual={}",
        ipc_lat.status, ipc_lat.actual
    );

    // IPC error rate: same dispatches feed the error-rate SLI.
    let ipc_err = payload
        .slos
        .iter()
        .find(|e| e.slo_name == "ipc.request.error_rate")
        .expect("ipc.request.error_rate present");
    assert_ne!(
        ipc_err.status, "no_data",
        "IPC error rate SLO must have data after real dispatches; got status={}, actual={}",
        ipc_err.status, ipc_err.actual
    );

    // Auth login success rate: the simulated `observe_auth_login(true)`
    // must move this SLO out of `no_data`.
    let auth_slo = payload
        .slos
        .iter()
        .find(|e| e.slo_name == "auth.login.success_rate")
        .expect("auth.login.success_rate present");
    assert_ne!(
        auth_slo.status, "no_data",
        "Auth login SLO must have data after observe_auth_login; got status={}, actual={}",
        auth_slo.status, auth_slo.actual
    );
    // With a single successful login, the success rate should be 100% => ok.
    assert_eq!(
        auth_slo.status, "ok",
        "A single successful login should meet the 99% target; actual={}",
        auth_slo.actual
    );

    // Aggregate pass should be true (all dispatches succeeded, all
    // non-IPC SLOs either have good data or are still no_data which
    // does not count as violation).
    assert!(
        payload.pass,
        "aggregate pass bit should be true when all observations are healthy"
    );
}
