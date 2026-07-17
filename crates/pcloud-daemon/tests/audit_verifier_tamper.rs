#![allow(clippy::pedantic)]
//! Integration test for the scheduled audit-chain verifier.
//!
//! Drives the verifier through a mock runner, proves the happy path
//! via `Method::GetAuditVerifierStatus`, then injects a tampered
//! outcome and asserts the failure event surfaces correctly in the
//! IPC status payload.
//!
//! No network traffic or real SQLite tamper required; the
//! `VerifierRunner` trait abstracts the store walk.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pcloud_config::{ConfigProfile, Environment, audit_verifier::AuditVerifierConfig};
use pcloud_daemon::audit_verifier_service::{
    AuditVerifierShell, StoreVerifierRunner, VerifierOutcome, VerifierRunner,
};
use pcloud_daemon::{bootstrap_with_config, dispatch};
use pcloud_ipc::{AuditVerifierStatusPayload, Method, Request, ResponseStatus};
use pcloud_observability::slo::Slo;

fn unique_root(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nonce = SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pcloud-daemon-audit-verifier-{tag}-{}-{nonce}",
        std::process::id()
    ))
}

fn fresh_runtime(tag: &str) -> pcloud_daemon::RuntimeShell {
    let config = ConfigProfile::secure_defaults(unique_root(tag), Environment::Development);
    bootstrap_with_config(config).expect("bootstrap runtime")
}

/// Mock runner that returns a pre-configured sequence of outcomes.
struct MockRunner {
    outcomes: Mutex<Vec<(VerifierOutcome, Option<i64>)>>,
}

impl MockRunner {
    fn single_pass(chain_length: u64, last_id: i64) -> Self {
        Self {
            outcomes: Mutex::new(vec![(
                VerifierOutcome::Pass { chain_length },
                Some(last_id),
            )]),
        }
    }

    fn single_fail(detail: &str) -> Self {
        Self {
            outcomes: Mutex::new(vec![(
                VerifierOutcome::Fail {
                    chain_length: 0,
                    detail: detail.to_owned(),
                },
                None,
            )]),
        }
    }
}

impl VerifierRunner for MockRunner {
    fn run(&self, _start_from: Option<i64>) -> (VerifierOutcome, Option<i64>) {
        let mut g = self.outcomes.lock().expect("mock runner poisoned");
        g.pop()
            .unwrap_or((VerifierOutcome::Pass { chain_length: 0 }, None))
    }
}

/// Fresh runtime reports `never_run` before any tick fires.
#[test]
fn fresh_runtime_reports_never_run() {
    let mut runtime = fresh_runtime("never-run");
    let response = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetAuditVerifierStatus,
        },
    );
    assert_eq!(response.status, ResponseStatus::Ok, "{:?}", response);

    let payload: AuditVerifierStatusPayload =
        serde_json::from_str(&response.message).expect("valid JSON");
    assert!(payload.enabled, "default config enables the verifier");
    assert_eq!(payload.last_result, "never_run");
    assert_eq!(payload.total_passes, 0);
    assert_eq!(payload.total_failures, 0);
}

/// After a successful `run_once`, the status reports `pass` with the
/// correct chain length.
#[test]
fn run_once_pass_surfaces_via_ipc() {
    let mut runtime = fresh_runtime("pass");
    let runner = MockRunner::single_pass(42, 42);
    let slo = Slo::new();

    let outcome = runtime.audit_verifier.run_once(&runner, &slo);
    assert!(
        matches!(outcome, VerifierOutcome::Pass { chain_length: 42 }),
        "unexpected: {outcome:?}"
    );

    let response = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetAuditVerifierStatus,
        },
    );
    assert_eq!(response.status, ResponseStatus::Ok);

    let payload: AuditVerifierStatusPayload =
        serde_json::from_str(&response.message).expect("valid JSON");
    assert_eq!(payload.last_result, "pass");
    assert_eq!(payload.chain_length, 42);
    assert_eq!(payload.total_passes, 1);
    assert_eq!(payload.total_failures, 0);
    assert!(payload.last_run_ts > 0, "timestamp must be set");
}

/// Tampered chain: inject a failure outcome and verify the broken-chain
/// detail surfaces through `GetAuditVerifierStatus`.
#[test]
fn tampered_chain_surfaces_failure_via_ipc() {
    let mut runtime = fresh_runtime("tamper");
    let tamper_detail = "audit chain broken at id=7: entry_hash mismatch";
    let runner = MockRunner::single_fail(tamper_detail);
    let slo = Slo::new();

    let outcome = runtime.audit_verifier.run_once(&runner, &slo);
    assert!(
        matches!(outcome, VerifierOutcome::Fail { .. }),
        "expected Fail, got: {outcome:?}"
    );

    let response = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetAuditVerifierStatus,
        },
    );
    assert_eq!(response.status, ResponseStatus::Ok);

    let payload: AuditVerifierStatusPayload =
        serde_json::from_str(&response.message).expect("valid JSON");
    assert_eq!(payload.last_result, "fail");
    assert_eq!(payload.total_failures, 1);
    assert_eq!(payload.total_passes, 0);
    assert!(
        payload.last_error.contains("entry_hash mismatch"),
        "detail must surface: got {:?}",
        payload.last_error
    );
    assert!(payload.last_run_ts > 0);
}

/// Pass-then-fail sequence: both counters increment correctly.
#[test]
fn pass_then_fail_increments_both_counters() {
    let mut runtime = fresh_runtime("seq");
    let slo = Slo::new();

    // Pass first
    let pass_runner = MockRunner::single_pass(10, 10);
    runtime.audit_verifier.run_once(&pass_runner, &slo);

    // Then fail
    let fail_runner = MockRunner::single_fail("tamper at id=11");
    runtime.audit_verifier.run_once(&fail_runner, &slo);

    let response = dispatch(
        &mut runtime,
        Request::Plain {
            method: Method::GetAuditVerifierStatus,
        },
    );
    let payload: AuditVerifierStatusPayload =
        serde_json::from_str(&response.message).expect("valid JSON");
    assert_eq!(payload.total_passes, 1);
    assert_eq!(payload.total_failures, 1);
    // Last result should be the most recent (fail)
    assert_eq!(payload.last_result, "fail");
}

#[test]
fn store_runner_covers_real_empty_chain_and_invalid_database() {
    let runtime = fresh_runtime("store-runner");
    let runner = StoreVerifierRunner::new(runtime.store.db_path.clone());
    let (outcome, latest) = runner.run(None);
    assert!(matches!(outcome, VerifierOutcome::Pass { .. }));
    assert!(latest.is_none());

    let missing = StoreVerifierRunner::new(unique_root("missing-db").join("missing.sqlite3"));
    let (outcome, latest) = missing.run(Some(i64::MAX));
    assert!(matches!(outcome, VerifierOutcome::Fail { .. }));
    assert!(latest.is_none());
}

#[test]
fn cron_scheduler_ticks_checkpoints_is_idempotent_and_shuts_down() {
    let root = unique_root("scheduled");
    let checkpoint = root.join("checkpoint/audit.json");
    let mut shell = AuditVerifierShell::from_config(AuditVerifierConfig {
        enabled: true,
        schedule_cron: "* * * * * * *".to_owned(),
        checkpoint_path: Some(checkpoint.clone()),
    })
    .expect("valid every-second schedule");
    let runner: Arc<dyn VerifierRunner> = Arc::new(MockRunner::single_pass(3, 3));
    let slo = Arc::new(Slo::new());
    shell
        .start_schedule(Arc::clone(&runner), Arc::clone(&slo))
        .expect("start scheduler");
    shell
        .start_schedule(runner, slo)
        .expect("second start is idempotent");

    let deadline = Instant::now() + Duration::from_secs(3);
    while shell.scheduled_run_count() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(shell.scheduled_run_count() > 0, "scheduler did not tick");
    assert!(shell.total_passes() > 0);
    assert_eq!(shell.total_failures(), 0);
    assert!(checkpoint.exists());
    let checkpoint_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&checkpoint).expect("checkpoint bytes"))
            .expect("checkpoint JSON");
    assert_eq!(checkpoint_json["last_verified_id"], 3);
    shell.shutdown();
    shell.shutdown();

    let mut disabled = AuditVerifierShell::disabled();
    assert!(
        disabled
            .start_schedule(
                Arc::new(MockRunner::single_pass(0, 0)),
                Arc::new(Slo::new()),
            )
            .is_err()
    );
    disabled.shutdown();
}
