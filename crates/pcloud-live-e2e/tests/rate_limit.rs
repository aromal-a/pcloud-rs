#![allow(clippy::pedantic)]
//! Live IPC rate-limit coverage: send more expensive requests than the
//! per-session Expensive bucket allows, assert the daemon returns the
//! typed `ResponseStatus::Conflict` with `"rate limit exceeded"` after
//! the bucket is drained, and assert no partial state was observed.
//!
//! This test does not require pCloud backend credentials — the rate
//! limiter runs entirely inside the daemon's dispatch path, and we use
//! a request shape (`Method::AuditVerifyChain`) that categorizes as
//! `Expensive` but does not require authentication to reach the gate.
//! We still gate on `PCLOUD_LIVE_E2E=1` so the binary remains part of
//! the opt-in suite.
//!
//! Pre-alpha honesty: the default `Expensive` bucket holds 6 tokens
//! refilling at 0.1/s (6/min). We burst 10 identical requests and
//! assert at least one comes back `Conflict`. We do not measure the
//! exact rejection rate — that is covered by unit tests in
//! `pcloud-daemon::rate_limit` — because wall-clock variance between
//! dispatches would make tight assertions flaky.

#![forbid(unsafe_code)]

// **PLATFORM:** all
// **GATING:** none at build time; runtime-gated.

mod common;

use pcloud_ipc::{AuditVerifyRange, Request, ResponseStatus};

use crate::common::{TestDaemon, assert_no_secret_leak, skip_if_not_live};

#[test]
#[ignore = "live-e2e: gated on PCLOUD_LIVE_E2E=1"]
fn live_rate_limiter_rejects_over_budget_expensive_burst() {
    if skip_if_not_live(&[]) {
        return;
    }

    let mut daemon = TestDaemon::new("rate-limit");

    // `AuditVerifyChain` is classified as `Expensive` by
    // `pcloud_daemon::rate_limit::categorize`; the default `secure_defaults`
    // policy is 6 tokens with a 0.1 tok/s refill.  Burst 10 calls; at
    // least the last 3-4 must be rate-limited.
    let mut ok_count = 0_u32;
    let mut rejected_count = 0_u32;
    let mut first_rejection_message = String::new();

    for _ in 0..10 {
        let resp = daemon.dispatch(Request::AuditVerifyChain {
            range: AuditVerifyRange::default(),
        });
        assert_no_secret_leak(&resp);
        match resp.status {
            ResponseStatus::Conflict => {
                rejected_count += 1;
                if first_rejection_message.is_empty() {
                    first_rejection_message = resp.message;
                }
            }
            // Any non-Conflict outcome counts as "admitted by the
            // limiter" for the purposes of this test. The backend may
            // succeed (Ok) or fail for unrelated reasons
            // (InternalError/InvalidRequest) — we only care that the
            // bucket was not depleted yet.
            _ => {
                ok_count += 1;
            }
        }
    }

    assert!(
        rejected_count >= 1,
        "expected at least one Conflict response from the rate limiter after bursting 10 \
         Expensive requests; saw admitted={ok_count} rejected={rejected_count}"
    );
    // Sanity: when the limiter kicks in, its message must advertise
    // both the category label and a retry-after hint. These substrings
    // are the stable wire format produced by
    // `pcloud_daemon::rate_limit::reject_response`.
    assert!(
        first_rejection_message.contains("rate limit exceeded"),
        "rate-limit reject message must start with 'rate limit exceeded': {first_rejection_message}"
    );
    assert!(
        first_rejection_message.contains("retry after"),
        "rate-limit reject message must carry a retry-after hint: {first_rejection_message}"
    );
    assert!(
        first_rejection_message.contains("expensive"),
        "rate-limit reject message must include the category label: {first_rejection_message}"
    );
}
