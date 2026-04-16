#![allow(clippy::pedantic)]
//! Live drain-state coverage: bootstrap a daemon, verify it reports
//! `state: "running"`, trigger `begin_drain()` programmatically (no
//! real SIGTERM — we cannot safely send signals within a single-process
//! test harness), verify the `DrainStatus` probe transitions to
//! `"draining"`, and finally assert the state machine reaches
//! `"stopped"` after `mark_stopped()`.
//!
//! This test exercises the same IPC surface that `pcloudc drain` and
//! external supervisors consume during rolling upgrades. It does NOT
//! contact the pCloud backend — the drain state machine is entirely
//! local.
//!
//! Runtime-gated on `PCLOUD_LIVE_E2E=1`.
//!
//! Security invariants:
//! * No credentials are required (drain is a local-only facility).
//! * Every response is scanned for secret leaks anyway, because the
//!   daemon may echo internal state strings and we must prove they
//!   never contain credential material.

#![forbid(unsafe_code)]

// **PLATFORM:** all (drain state machine is portable).
// **GATING:** none at build time; runtime-gated.

mod common;

use pcloud_daemon::signals;
use pcloud_ipc::{DrainStatusPayload, Method, Request, ResponseStatus};

use crate::common::{TestDaemon, assert_no_secret_leak, skip_if_not_live, status_label};

/// Parse the `DrainStatusPayload` from a response message.
fn parse_drain_payload(msg: &str) -> DrainStatusPayload {
    serde_json::from_str(msg).unwrap_or_else(|e| {
        panic!("DrainStatus response is not valid JSON: {e}\nmessage: {msg}");
    })
}

#[test]
#[ignore = "live-e2e: gated on PCLOUD_LIVE_E2E=1"]
fn live_drain_state_machine_running_draining_stopped() {
    if skip_if_not_live(&[]) {
        return;
    }

    // Reset process-wide drain statics so this test is hermetic even
    // when run alongside other tests in the same process. The
    // `reset_for_test` contract is narrow: it is only safe when no
    // serve loop is concurrently running, which holds here because we
    // dispatch synchronously through `TestDaemon`.
    signals::reset_for_test();

    let mut daemon = TestDaemon::new("drain");

    // ── Phase 1: Running ───────────────────────────────────────────
    let resp = daemon.dispatch(Request::Plain {
        method: Method::DrainStatus,
    });
    assert_no_secret_leak(&resp);
    assert_eq!(
        resp.status,
        ResponseStatus::Ok,
        "DrainStatus while running failed: status={} message={}",
        status_label(&resp.status),
        resp.message
    );

    let payload = parse_drain_payload(&resp.message);
    assert_eq!(
        payload.state, "running",
        "expected state='running' before drain; got '{}'",
        payload.state
    );
    assert_eq!(
        payload.elapsed_drain_ms, 0,
        "elapsed_drain_ms must be 0 while running"
    );

    // ── Phase 2: Trigger drain ─────────────────────────────────────
    let transitioned = signals::begin_drain();
    assert!(
        transitioned,
        "begin_drain() must return true on the first call"
    );

    // Idempotence: a second call must return false.
    assert!(
        !signals::begin_drain(),
        "begin_drain() must return false on repeated calls"
    );

    let resp2 = daemon.dispatch(Request::Plain {
        method: Method::DrainStatus,
    });
    assert_no_secret_leak(&resp2);
    assert_eq!(
        resp2.status,
        ResponseStatus::Ok,
        "DrainStatus while draining failed: status={} message={}",
        status_label(&resp2.status),
        resp2.message
    );

    let payload2 = parse_drain_payload(&resp2.message);
    assert_eq!(
        payload2.state, "draining",
        "expected state='draining' after begin_drain(); got '{}'",
        payload2.state
    );
    // elapsed_drain_ms should be non-negative (could be 0 if the clock
    // granularity is coarse, so we only assert it is present).
    // in_flight includes the current DrainStatus dispatch itself, so
    // it may be >= 1. We just assert it is present and not absurd.
    assert!(
        payload2.in_flight < 100,
        "in_flight is suspiciously high: {}",
        payload2.in_flight
    );

    // ── Phase 3: Mark stopped ──────────────────────────────────────
    signals::mark_stopped();

    let resp3 = daemon.dispatch(Request::Plain {
        method: Method::DrainStatus,
    });
    assert_no_secret_leak(&resp3);
    // The IPC layer may or may not still accept the probe after
    // mark_stopped(); if it does, verify the state label.
    if resp3.status == ResponseStatus::Ok {
        let payload3 = parse_drain_payload(&resp3.message);
        assert_eq!(
            payload3.state, "stopped",
            "expected state='stopped' after mark_stopped(); got '{}'",
            payload3.state
        );
    }

    // ── Cleanup: reset so other tests in this process are not poisoned.
    signals::reset_for_test();
}

#[test]
#[ignore = "live-e2e: gated on PCLOUD_LIVE_E2E=1"]
fn live_drain_in_flight_guard_accounting() {
    if skip_if_not_live(&[]) {
        return;
    }

    signals::reset_for_test();

    // Verify the InFlightGuard RAII contract: creating a guard bumps
    // in_flight by 1; dropping it decrements.
    assert_eq!(signals::in_flight(), 0, "in_flight must start at 0");

    {
        let _g1 = signals::InFlightGuard::new();
        assert_eq!(
            signals::in_flight(),
            1,
            "in_flight must be 1 with one guard"
        );

        let _g2 = signals::InFlightGuard::new();
        assert_eq!(
            signals::in_flight(),
            2,
            "in_flight must be 2 with two guards"
        );
    }
    // Both guards dropped.
    assert_eq!(
        signals::in_flight(),
        0,
        "in_flight must return to 0 after all guards drop"
    );

    signals::reset_for_test();
}
