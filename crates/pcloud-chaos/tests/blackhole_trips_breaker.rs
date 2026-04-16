#![allow(clippy::pedantic)]
//! Scenario 3: blackhole at connect → circuit breaker trips, retries back off.
//!
//! We point at an RFC5737 TEST-NET-1 address that is documented as
//! unroutable and will either hang or produce a connect error. We wrap every
//! attempt in a short tokio timeout (200 ms) and feed each
//! timeout/connect-error into a `CircuitBreaker` with
//! `failure_threshold = 3`. After three consecutive failures the breaker
//! must report `Open` and reject further attempts with
//! `CircuitBreakerError::Open`. We also assert the retry schedule produces
//! a monotonic non-decreasing backoff — the daemon must never burn CPU in
//! a tight retry loop against a dead endpoint.
//!
//! Runs in default `cargo test`. Budget: < 5 s.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::time::{Duration, Instant};

use pcloud_resilience::circuit_breaker::{
    BreakerState, CircuitBreaker, CircuitBreakerConfig, CircuitBreakerError,
};
use pcloud_resilience::retry::{BackoffSchedule, RetryDecision, RetryPolicy};

#[tokio::test]
async fn chaos_blackhole_trips_breaker() {
    // Allow override for local labs; default is RFC5737 TEST-NET-1.
    let addr =
        std::env::var("PCLOUD_BACKEND_ADDR").unwrap_or_else(|_| "10.255.255.1:443".to_string());

    let config = CircuitBreakerConfig::new(3, Duration::from_secs(30));
    let breaker = Arc::new(CircuitBreaker::new(config));

    // Exponential backoff: 20 ms, 40 ms, 80 ms (cap) — total well under budget.
    let policy = RetryPolicy::new(
        4,
        BackoffSchedule::Exponential {
            base: Duration::from_millis(20),
            factor: 2.0,
            max: Duration::from_millis(80),
        },
    );

    let mut last_wait = Duration::ZERO;
    let mut attempts = 0u32;

    for attempt in 1..=3u32 {
        attempts += 1;

        // First three attempts: breaker is Closed, admission must succeed.
        breaker
            .try_acquire()
            .expect("breaker should admit while Closed");

        let started = Instant::now();
        let res = tokio::time::timeout(
            Duration::from_millis(200),
            tokio::net::TcpStream::connect(&addr),
        )
        .await;

        // Any of {outer timeout, connect error} counts as a failure.
        // Any successful connect would falsify the scenario assumption.
        let failed = match res {
            Err(_elapsed) => true,
            Ok(Err(_io_err)) => true,
            Ok(Ok(_stream)) => false,
        };
        assert!(
            failed,
            "blackhole address {addr} should not yield a connected stream"
        );
        breaker.record_failure();

        // Predicted: per-attempt budget is respected, no hang.
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(500),
            "per-request budget blown: {elapsed:?}"
        );

        // Predicted: retry backoff is monotonic non-decreasing.
        if let RetryDecision::Retry { wait } = policy.next(attempt) {
            assert!(
                wait >= last_wait,
                "retry back-off regressed: {wait:?} < {last_wait:?}"
            );
            last_wait = wait;
        }
    }

    assert_eq!(attempts, 3, "expected 3 attempts before trip");
    assert_eq!(
        breaker.state(),
        BreakerState::Open,
        "breaker must Open after failure_threshold consecutive failures"
    );

    // Predicted: next attempt rejected fast with Open.
    let err = breaker.try_acquire().expect_err("breaker must reject");
    assert_eq!(err, CircuitBreakerError::Open);
}
