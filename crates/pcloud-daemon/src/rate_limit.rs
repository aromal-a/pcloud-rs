//! Per-session, per-category IPC rate limiter.
//!
//! The dispatcher classifies every decoded [`pcloud_ipc::Request`] into a
//! [`pcloud_config::rate_limit::RateCategory`] before invoking the
//! backend. A per-category token bucket (configured via
//! [`pcloud_config::rate_limit::RateLimitPolicy`]) decides whether the
//! request proceeds or is rejected with [`pcloud_ipc::ResponseStatus::Conflict`]
//! and a "retry after Ns" message. Rate-limit rejection is **never** an
//! internal-error path — the caller sees a deterministic "conflict,
//! retry later" response.
//!
//! Rationale: an audit against the Rust rewrite noted that expensive
//! operations (bulk public-link listing, integrity run-once, snapshot
//! create) had no IPC-layer limiter, so a chatty or hostile client
//! could DoS the daemon by fan-out. This module closes that gap.
//!
//! Per-peer granularity is provided by [`PerPeerRateLimiter`] which
//! maintains a `HashMap<peer_uid, SessionRateLimiter>`. Each distinct
//! peer uid gets its own independent token-bucket state so one client
//! hammering the daemon cannot starve another. The underlying
//! [`SessionRateLimiter`] is a plain token-bucket container and knows
//! nothing about peer identity on its own.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use pcloud_config::rate_limit::{RateBucket, RateCategory, RateLimitPolicy};
use pcloud_ipc::{Method, Request, Response, ResponseStatus};
use pcloud_resilience::{TokenBucket, TokenBucketConfig};

/// Per-category bucket map for a single IPC session.
///
/// `Clone` is deliberately not derived — sessions own their own buckets
/// and must not share refill state across peers.
#[derive(Debug)]
pub struct SessionRateLimiter {
    enabled: bool,
    medium: Option<TokenBucket>,
    expensive: Option<TokenBucket>,
    auth_attempt: Option<TokenBucket>,
    // Cheap category is unconditionally allowed; no bucket is materialized.
}

/// Outcome of an admission check.
#[derive(Debug, Clone, PartialEq)]
pub enum RateDecision {
    /// Caller may proceed.
    Allow,
    /// Caller is over budget. `category` is the name of the offending
    /// bucket (for logging/telemetry) and `retry_after` is how long
    /// before the client should retry the exact same request.
    Reject {
        /// Offending bucket's stable category name.
        category: &'static str,
        /// Duration the client must wait before a retry will succeed.
        retry_after: Duration,
    },
}

impl SessionRateLimiter {
    /// Build a session limiter from a validated
    /// [`RateLimitPolicy`]. When `policy.enabled` is `false`, every
    /// category degrades to "always allow".
    #[must_use]
    pub fn new(policy: &RateLimitPolicy) -> Self {
        let enabled = policy.enabled;
        let medium = Self::build_bucket(policy.medium, enabled);
        let expensive = Self::build_bucket(policy.expensive, enabled);
        let auth_attempt = Self::build_bucket(policy.auth_attempt, enabled);
        Self {
            enabled,
            medium,
            expensive,
            auth_attempt,
        }
    }

    fn build_bucket(bucket: RateBucket, enabled: bool) -> Option<TokenBucket> {
        if !enabled || !bucket.is_enabled() {
            return None;
        }
        // `is_enabled()` already validates positivity; surface any
        // unexpected config error as "disabled" so dispatch never
        // panics on a malformed rate-limit block.
        let cfg = TokenBucketConfig::new(bucket.capacity, bucket.refill_per_sec).ok()?;
        Some(TokenBucket::new(cfg))
    }

    /// Check admission for the given request. Returns [`RateDecision::Allow`]
    /// when the caller is within budget (or when the bucket is disabled).
    #[must_use]
    pub fn check(&self, request: &Request) -> RateDecision {
        if !self.enabled {
            return RateDecision::Allow;
        }
        let category = categorize(request);
        let bucket = match category {
            RateCategory::Cheap => return RateDecision::Allow,
            RateCategory::Medium => self.medium.as_ref(),
            RateCategory::Expensive => self.expensive.as_ref(),
            RateCategory::AuthAttempt => self.auth_attempt.as_ref(),
        };
        let Some(bucket) = bucket else {
            return RateDecision::Allow;
        };
        // `try_acquire(1)` cannot error here — capacity is > 0 and we
        // always request a single token.
        match bucket.try_acquire(1) {
            Ok(true) => RateDecision::Allow,
            Ok(false) => RateDecision::Reject {
                category: category_label(category),
                retry_after: retry_after_for(bucket),
            },
            // A bucket configuration error during try_acquire would be
            // unreachable (n=1, capacity >= 1), but fail-open to keep
            // dispatch robust.
            Err(_) => RateDecision::Allow,
        }
    }

    /// Shorthand: `true` if the category has an active bucket. Intended
    /// for diagnostics / test assertions only.
    #[must_use]
    pub fn bucket_active(&self, category: RateCategory) -> bool {
        match category {
            RateCategory::Cheap => false,
            RateCategory::Medium => self.medium.is_some(),
            RateCategory::Expensive => self.expensive.is_some(),
            RateCategory::AuthAttempt => self.auth_attempt.is_some(),
        }
    }
}

/// Per-peer rate-limit registry. Each distinct peer uid gets its own
/// [`SessionRateLimiter`] with an independent token-bucket state, so one
/// client cannot starve another by saturating the daemon's shared
/// category buckets.
///
/// The registry is guarded by a `std::sync::Mutex` (serve loop is
/// single-threaded under owner-only IPC, so contention is nil; the
/// lock exists purely to make the map Send+Sync in case the dispatch
/// path is ever moved off the serve thread). The `RateLimitPolicy` is
/// cloned once at construction and reused for every new peer.
///
/// Note: eviction tied to connection-count drop is intentionally *not*
/// wired. `pcloud-ipc::transport::ConnectionGuard` already evicts peer
/// entries when their live-connection count returns to 0; this limiter
/// holds at most one entry per *uid*, not per-connection, and retains
/// token state across short reconnects to prevent a burst-reconnect
/// workaround for bucket exhaustion. Memory footprint is bounded by the
/// number of distinct authorized uids (owner-only: 1 in production).
pub struct PerPeerRateLimiter {
    policy: RateLimitPolicy,
    peers: Mutex<HashMap<u32, SessionRateLimiter>>,
}

impl PerPeerRateLimiter {
    /// Build a new per-peer registry from a validated policy.
    #[must_use]
    pub fn new(policy: &RateLimitPolicy) -> Self {
        Self {
            policy: policy.clone(),
            peers: Mutex::new(HashMap::new()),
        }
    }

    /// Admission check for a request arriving from `peer_uid`. Creates
    /// a fresh `SessionRateLimiter` for the peer on first sight.
    #[must_use]
    pub fn check(&self, peer_uid: u32, request: &Request) -> RateDecision {
        let mut guard = match self.peers.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                log::error!("pcloud-daemon: PerPeerRateLimiter mutex poisoned; recovering");
                poisoned.into_inner()
            }
        };
        let limiter = guard
            .entry(peer_uid)
            .or_insert_with(|| SessionRateLimiter::new(&self.policy));
        limiter.check(request)
    }

    /// Rebuild every per-peer limiter from a freshly loaded policy.
    /// Called from SIGHUP hot-reload.
    pub fn apply_policy(&mut self, policy: &RateLimitPolicy) {
        self.policy = policy.clone();
        let mut guard = match self.peers.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clear();
    }
}

impl core::fmt::Debug for PerPeerRateLimiter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PerPeerRateLimiter")
            .field("enabled", &self.policy.enabled)
            .finish_non_exhaustive()
    }
}

/// Convert a [`RateDecision::Reject`] into a typed IPC [`Response`] with
/// `ResponseStatus::Conflict`. Returns `None` when the caller was
/// admitted, so call sites can use a simple early-return pattern.
#[must_use]
pub fn reject_response(decision: &RateDecision) -> Option<Response> {
    match decision {
        RateDecision::Allow => None,
        RateDecision::Reject {
            category,
            retry_after,
        } => {
            // Round up to whole seconds so the client message is
            // always a positive, actionable hint.
            let secs = retry_after.as_secs_f64().ceil().max(1.0) as u64;
            Some(Response {
                status: ResponseStatus::Conflict,
                message: format!("rate limit exceeded: {category}, retry after {secs}s"),
            })
        }
    }
}

/// Stable category label used in log messages and reject responses.
#[must_use]
pub fn category_label(category: RateCategory) -> &'static str {
    match category {
        RateCategory::Cheap => "cheap",
        RateCategory::Medium => "medium",
        RateCategory::Expensive => "expensive",
        RateCategory::AuthAttempt => "auth_attempt",
    }
}

/// Classify an inbound [`Request`] into a rate-limit bucket.
///
/// Policy:
///
/// - **Cheap**: status / health probes, userinfo, field selectors
///   (`ValueGet`/`ValueHas`), session status.
/// - **AuthAttempt**: credential submissions, TFA, crypto unlock, account
///   password change — strict per-session burst to limit brute-force.
/// - **Expensive**: snapshot create, integrity run-once, integrity
///   skip, bulk public-link listing, tree-link create, change-crypto-
///   password, send-crypto-change-user-private, audit verify-chain.
/// - **Medium**: everything else that crosses the IPC boundary (the
///   conservative default).
#[must_use]
pub fn categorize(request: &Request) -> RateCategory {
    match request {
        Request::Plain { method } => categorize_plain(*method),
        // Auth-attempt: credential submissions and crypto unlock.
        Request::PasswordSubmission { .. } => RateCategory::AuthAttempt,
        Request::AuthTokenSubmission { .. } => RateCategory::AuthAttempt,
        Request::TwoFactorCodeSubmission { .. } => RateCategory::AuthAttempt,
        Request::CryptoUnlock { .. } => RateCategory::AuthAttempt,
        Request::CryptoSetup { .. } => RateCategory::AuthAttempt,
        Request::AccountChangePassword { .. } => RateCategory::AuthAttempt,
        // Expensive: bulk/heavy operations.
        Request::BackupSnapshot { .. } => RateCategory::Expensive,
        Request::AuditVerifyChain { .. } => RateCategory::Expensive,
        Request::CreateTreePublicLink { .. } => RateCategory::Expensive,
        // Sibling path-based variant performs N sequential path->id
        // resolutions before the tree-link create, so it is at least as
        // expensive as the id-based form.
        Request::CreateTreePublicLinkFromPaths { .. } => RateCategory::Expensive,
        // Backup mutations: backend-touching lifecycle calls
        // (CreateBackup/StopDevice) and local-only device registration
        // cleanup (DeleteBackupDevice). All three are classified
        // `Expensive` to share a bucket with other mutating lifecycle
        // operations and to dampen abusive retries against the real
        // backup endpoints.
        Request::CreateBackup { .. }
        | Request::StopDevice { .. }
        | Request::DeleteBackupDevice => RateCategory::Expensive,
        // Server-side copy from a remote fileid drives an upload_create
        // + multi-chunk upload_write sequence and is bulk/heavy once
        // the real wiring lands (bd-1du). Classify now so the stub and
        // the eventual implementation share the same bucket.
        Request::UploadWriteFromFile { .. } => RateCategory::Expensive,
        Request::IntegrityRunOnce | Request::IntegritySkip { .. } => RateCategory::Expensive,
        Request::CryptoChangePassword { .. } | Request::CryptoChangePasswordUnlocked { .. } => {
            RateCategory::Expensive
        }
        // Dual-crypto-backend setup (fresh or post-password-change).
        // Mutates the crypto profile; classify as Expensive so it
        // shares a bucket with other mutating lifecycle operations and
        // to dampen abusive retries against `crypto_setuserkeys`.
        Request::CryptoSetupV2 { .. } => RateCategory::Expensive,
        // Hot-path sym-key fetches used during normal sealed-file
        // operation. Not `Cheap` (they drive a backend RTT + an RSA
        // unwrap) and not `Expensive` (they are per-file/per-folder
        // primitives on the read path). Classify as `Medium`.
        Request::CryptoGetFolderKey { .. } | Request::CryptoGetFileKey { .. } => {
            RateCategory::Medium
        }
        // Cheap field selectors / session probes.
        Request::ValueGet { .. } | Request::ValueHas { .. } => RateCategory::Cheap,
        Request::SessionStatus => RateCategory::Cheap,
        // Everything else defaults to Medium.
        _ => RateCategory::Medium,
    }
}

fn categorize_plain(method: Method) -> RateCategory {
    match method {
        // Cheap: status / health / userinfo / pending counters.
        Method::GetStatus
        | Method::GetHealth
        | Method::Health
        | Method::GetPending
        | Method::GetUserInfo
        | Method::SessionStatus
        | Method::GetCryptoStatus => RateCategory::Cheap,
        // Auth-attempt: login begin triggers the auth state machine.
        Method::LoginBegin
        | Method::SubmitPassword
        | Method::SubmitTwoFactorCode
        | Method::UnlockCrypto => RateCategory::AuthAttempt,
        // Expensive: bulk listings, heavy mutations, and privileged
        // lifecycle control. Shutdown is placed in the expensive bucket
        // (low capacity) so a chatty or hostile client cannot spam the
        // daemon into restarting. A legitimate operator rarely needs
        // more than a handful of shutdown signals per minute.
        Method::ListPublicLinks
        | Method::ListUploadLinks
        | Method::IntegrityStatus
        | Method::SendCryptoChangeUserPrivate
        | Method::Shutdown => RateCategory::Expensive,
        // Everything else (pause/resume, list-shares, ...)
        // sits in the medium bucket.
        _ => RateCategory::Medium,
    }
}

/// Compute a "retry after" hint for a bucket that just rejected a
/// request. Uses `peek_wait_for`, which does **not** consume a token —
/// the client has already been rejected by `try_acquire`, so burning a
/// second reservation here would double-count the rejection (audit-04
/// §7-opus M-2 / FIX-PLAN P3).
///
/// We cap the reported wait so the message stays legible for
/// very slow refills (e.g. 1 token / 10 min).
fn retry_after_for(bucket: &TokenBucket) -> Duration {
    match bucket.peek_wait_for(1) {
        Ok(d) => d.min(Duration::from_secs(600)),
        Err(_) => Duration::from_secs(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_config::rate_limit::{RateBucket, RateLimitPolicy};
    use pcloud_ipc::Method;

    fn policy_for_test(medium: u32, expensive: u32) -> RateLimitPolicy {
        RateLimitPolicy {
            enabled: true,
            cheap: RateBucket::disabled(),
            medium: if medium == 0 {
                RateBucket::disabled()
            } else {
                RateBucket {
                    capacity: medium,
                    // Use a slow refill so tests can observe the
                    // bucket draining deterministically across
                    // successive `try_acquire(1)` calls.
                    refill_per_sec: 0.1,
                }
            },
            expensive: if expensive == 0 {
                RateBucket::disabled()
            } else {
                RateBucket {
                    capacity: expensive,
                    refill_per_sec: 0.1,
                }
            },
            auth_attempt: RateBucket {
                capacity: 10,
                refill_per_sec: 0.1,
            },
        }
    }

    #[test]
    fn categorize_cheap_plain_methods() {
        let r = Request::Plain {
            method: Method::GetStatus,
        };
        assert_eq!(categorize(&r), RateCategory::Cheap);
        let r = Request::Plain {
            method: Method::GetUserInfo,
        };
        assert_eq!(categorize(&r), RateCategory::Cheap);
    }

    #[test]
    fn categorize_expensive_plain_methods() {
        let r = Request::Plain {
            method: Method::ListPublicLinks,
        };
        assert_eq!(categorize(&r), RateCategory::Expensive);
        let r = Request::Plain {
            method: Method::IntegrityStatus,
        };
        assert_eq!(categorize(&r), RateCategory::Expensive);
    }

    #[test]
    fn categorize_expensive_structured_variants() {
        let r = Request::IntegrityRunOnce;
        assert_eq!(categorize(&r), RateCategory::Expensive);
    }

    #[test]
    fn categorize_medium_default() {
        let r = Request::Plain {
            method: Method::ListIncomingShares,
        };
        assert_eq!(categorize(&r), RateCategory::Medium);
        let r = Request::Plain {
            method: Method::PauseSync,
        };
        assert_eq!(categorize(&r), RateCategory::Medium);
    }

    #[test]
    fn categorize_auth_attempt_plain_methods() {
        let r = Request::Plain {
            method: Method::LoginBegin,
        };
        assert_eq!(categorize(&r), RateCategory::AuthAttempt);
        let r = Request::Plain {
            method: Method::SubmitPassword,
        };
        assert_eq!(categorize(&r), RateCategory::AuthAttempt);
        let r = Request::Plain {
            method: Method::UnlockCrypto,
        };
        assert_eq!(categorize(&r), RateCategory::AuthAttempt);
    }

    #[test]
    fn categorize_auth_attempt_structured_variants() {
        let r = Request::PasswordSubmission {
            username: "user@example.com".to_owned(),
            value: pcloud_ipc::RedactedString::new("pass"),
        };
        assert_eq!(categorize(&r), RateCategory::AuthAttempt);
        let r = Request::CryptoUnlock {
            password: pcloud_ipc::RedactedString::new("pass"),
        };
        assert_eq!(categorize(&r), RateCategory::AuthAttempt);
    }

    #[test]
    fn categorize_structured_variants() {
        let r = Request::ValueGet {
            name: "x".to_owned(),
            kind: pcloud_ipc::ValueKvKind::Bool,
        };
        assert_eq!(categorize(&r), RateCategory::Cheap);
        let r = Request::AuditVerifyChain {
            range: pcloud_ipc::AuditVerifyRange::default(),
        };
        assert_eq!(categorize(&r), RateCategory::Expensive);
    }

    #[test]
    fn burst_up_to_capacity_is_accepted() {
        let policy = policy_for_test(3, 2);
        let limiter = SessionRateLimiter::new(&policy);
        // Use a genuinely Medium method (not an AuthAttempt one).
        let req = Request::Plain {
            method: Method::ListIncomingShares,
        };
        for _ in 0..3 {
            assert_eq!(limiter.check(&req), RateDecision::Allow);
        }
    }

    #[test]
    fn over_capacity_is_rejected_with_conflict_response() {
        let policy = policy_for_test(2, 1);
        let limiter = SessionRateLimiter::new(&policy);
        // Use a genuinely Medium method (not an AuthAttempt one).
        let req = Request::Plain {
            method: Method::ListIncomingShares,
        };
        assert_eq!(limiter.check(&req), RateDecision::Allow);
        assert_eq!(limiter.check(&req), RateDecision::Allow);
        let decision = limiter.check(&req);
        match &decision {
            RateDecision::Reject {
                category,
                retry_after,
            } => {
                assert_eq!(*category, "medium");
                assert!(retry_after.as_secs() >= 1);
            }
            other => panic!("expected reject, got {other:?}"),
        }
        let resp = reject_response(&decision).expect("should produce response");
        assert!(matches!(resp.status, ResponseStatus::Conflict));
        assert!(resp.message.starts_with("rate limit exceeded: medium"));
        assert!(resp.message.contains("retry after"));
    }

    #[test]
    fn expensive_category_uses_expensive_bucket() {
        let policy = policy_for_test(100, 1);
        let limiter = SessionRateLimiter::new(&policy);
        let req = Request::Plain {
            method: Method::ListPublicLinks,
        };
        assert_eq!(limiter.check(&req), RateDecision::Allow);
        // Bucket is exhausted immediately.
        assert!(matches!(
            limiter.check(&req),
            RateDecision::Reject {
                category: "expensive",
                ..
            }
        ));
    }

    #[test]
    fn cheap_category_always_allowed_even_when_bucket_disabled() {
        let policy = policy_for_test(1, 1);
        let limiter = SessionRateLimiter::new(&policy);
        let req = Request::Plain {
            method: Method::GetStatus,
        };
        // Hammer the cheap path 100x; must never be rejected.
        for _ in 0..100 {
            assert_eq!(limiter.check(&req), RateDecision::Allow);
        }
    }

    #[test]
    fn token_refill_restores_admission() {
        // Capacity 1, refills at 1 tok/sec via medium bucket.
        let policy = RateLimitPolicy {
            enabled: true,
            cheap: RateBucket::disabled(),
            medium: RateBucket {
                capacity: 1,
                refill_per_sec: 1000.0, // fast refill for the test
            },
            expensive: RateBucket::disabled(),
            auth_attempt: RateBucket::disabled(),
        };
        let limiter = SessionRateLimiter::new(&policy);
        // Use a genuinely Medium method (LoginBegin is now AuthAttempt).
        let req = Request::Plain {
            method: Method::ListIncomingShares,
        };
        assert_eq!(limiter.check(&req), RateDecision::Allow);
        // Second immediate check may reject (bucket drained).
        let _ = limiter.check(&req);
        // Sleep a bit; refill at 1000/s should top the bucket back up.
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(limiter.check(&req), RateDecision::Allow);
    }

    #[test]
    fn master_switch_disables_every_bucket() {
        let mut policy = policy_for_test(1, 1);
        policy.enabled = false;
        let limiter = SessionRateLimiter::new(&policy);
        let req = Request::Plain {
            method: Method::ListPublicLinks,
        };
        for _ in 0..50 {
            assert_eq!(limiter.check(&req), RateDecision::Allow);
        }
    }

    #[test]
    fn zero_capacity_disables_single_bucket() {
        let policy = policy_for_test(0, 2);
        let limiter = SessionRateLimiter::new(&policy);
        assert!(!limiter.bucket_active(RateCategory::Medium));
        assert!(limiter.bucket_active(RateCategory::Expensive));

        let req = Request::Plain {
            method: Method::LoginBegin,
        };
        for _ in 0..10 {
            assert_eq!(limiter.check(&req), RateDecision::Allow);
        }
    }

    #[test]
    fn reject_response_is_none_for_allow() {
        assert!(reject_response(&RateDecision::Allow).is_none());
    }
}
