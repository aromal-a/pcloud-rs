//! Session lifecycle tracking: proactive refresh, idle logout, and
//! single-flight refresh coordination.
//!
//! This module is additive to the existing `SessionManager` API. It
//! exposes a `Clock` trait (defaulting to `SystemClock`) plus a
//! `RefreshPolicy` that describes when the daemon should proactively
//! refresh the auth token and when idle sessions should be terminated.
//!
//! The module performs **no I/O**. It is a pure state machine around
//! timing decisions so tests can inject a `TestClock` and reason about
//! refresh race safety, threshold firing, and idle logout.
//!
//! Security notes:
//! * `credentials_retained` is a boolean flag only. This crate never
//!   stores passwords; the daemon owns credential lifetime decisions.
//! * Explicit `revoke` zeroizes the auth token via `SecretString::Drop`.

// **PLATFORM:** all
// **GATING:** none (portable).

use core::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_secret::secret_string::SecretString;
use thiserror::Error;

/// Monotonic-ish clock abstraction (wall-clock seconds since UNIX epoch).
///
/// A trait is used so tests can inject a deterministic clock without
/// depending on the not-yet-landed `pcloud-resilience` crate.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Return current time as seconds since UNIX epoch.
    fn now_secs(&self) -> u64;
}

/// Default system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Test clock that advances only when instructed.
#[derive(Debug, Default)]
pub struct TestClock {
    now: Mutex<u64>,
}

impl TestClock {
    /// Construct a test clock anchored at `start_secs`.
    #[must_use]
    pub fn new(start_secs: u64) -> Self {
        Self {
            now: Mutex::new(start_secs),
        }
    }

    /// Advance the test clock by `by`. Saturates at `u64::MAX`.
    pub fn advance(&self, by: Duration) {
        let mut guard = self.now.lock().expect("test clock mutex poisoned");
        *guard = guard.saturating_add(by.as_secs());
    }

    /// Set the test clock to an absolute UNIX-seconds value.
    pub fn set(&self, secs: u64) {
        let mut guard = self.now.lock().expect("test clock mutex poisoned");
        *guard = secs;
    }
}

impl Clock for TestClock {
    fn now_secs(&self) -> u64 {
        *self.now.lock().expect("test clock mutex poisoned")
    }
}

/// Refresh and idle policy. All durations are wall-clock seconds.
#[derive(Debug, Clone)]
pub struct RefreshPolicy {
    /// Session lifetime (seconds). Default 1h, matching pCloud auth.
    pub lifetime: Duration,
    /// Fraction of lifetime at which proactive refresh fires.
    /// Defaults to 0.8 (80%).
    pub refresh_threshold: f32,
    /// Optional idle logout. `None` disables idle logout (default).
    pub max_idle: Option<Duration>,
}

impl Default for RefreshPolicy {
    fn default() -> Self {
        Self {
            lifetime: Duration::from_secs(3600),
            refresh_threshold: 0.8,
            max_idle: None,
        }
    }
}

impl RefreshPolicy {
    /// Validate bounds and coerce to sane values.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        if !(0.0..=1.0).contains(&self.refresh_threshold) {
            self.refresh_threshold = 0.8;
        }
        if self.lifetime.is_zero() {
            self.lifetime = Duration::from_secs(3600);
        }
        self
    }

    /// Number of seconds after session start at which refresh should
    /// fire (rounded down).
    #[must_use]
    pub fn refresh_after_secs(&self) -> u64 {
        let secs = self.lifetime.as_secs() as f64 * self.refresh_threshold as f64;
        secs as u64
    }
}

/// Timing fields attached to an authenticated session.
#[derive(Debug, Clone)]
pub struct SessionLifecycle {
    /// UNIX seconds when the session was established (or last refresh).
    pub established_at: u64,
    /// UNIX seconds when the session expires.
    pub expires_at: u64,
    /// UNIX seconds of last observed activity.
    pub last_used_at: u64,
    /// Whether credentials (e.g. password) remain retained in memory
    /// so a 401-triggered re-auth attempt is possible without user
    /// interaction. This is a flag only; this crate never stores the
    /// password.
    pub credentials_retained: bool,
}

impl SessionLifecycle {
    /// Create new lifecycle starting at `now` with the given policy.
    #[must_use]
    pub fn new(now_secs: u64, policy: &RefreshPolicy, credentials_retained: bool) -> Self {
        Self {
            established_at: now_secs,
            expires_at: now_secs.saturating_add(policy.lifetime.as_secs()),
            last_used_at: now_secs,
            credentials_retained,
        }
    }

    /// Mark activity at `now_secs`. Resets idle timer.
    pub fn mark_used(&mut self, now_secs: u64) {
        self.last_used_at = now_secs;
    }

    /// Proactive refresh decision: true if `now >= established + lifetime * threshold`.
    #[must_use]
    pub fn should_refresh(&self, now_secs: u64, policy: &RefreshPolicy) -> bool {
        let refresh_at = self
            .established_at
            .saturating_add(policy.refresh_after_secs());
        now_secs >= refresh_at
    }

    /// Hard expiry: `now >= expires_at`.
    #[must_use]
    pub fn is_expired(&self, now_secs: u64) -> bool {
        now_secs >= self.expires_at
    }

    /// Idle logout check. Returns false if policy has no `max_idle`.
    #[must_use]
    pub fn is_idle_expired(&self, now_secs: u64, policy: &RefreshPolicy) -> bool {
        match policy.max_idle {
            Some(max) => now_secs.saturating_sub(self.last_used_at) >= max.as_secs(),
            None => false,
        }
    }

    /// Update lifecycle fields after a successful refresh.
    pub fn record_refresh(&mut self, now_secs: u64, policy: &RefreshPolicy) {
        self.established_at = now_secs;
        self.expires_at = now_secs.saturating_add(policy.lifetime.as_secs());
        self.last_used_at = now_secs;
    }
}

/// Errors from lifecycle-level decisions.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LifecycleError {
    /// A refresh is already in flight (single-flight guard).
    ///
    /// * **Cause**: another caller is currently holding a
    ///   [`RefreshTicket`]; the guard ensures concurrent refresh
    ///   requests collapse onto a single network round-trip.
    /// * **Recoverability**: the other refresh will either succeed or
    ///   fail; either way, this caller can observe the outcome via
    ///   the next [`crate::refresh::RefreshCoordinator::evaluate`].
    /// * **Retry guidance**: do not spin. Wait for the next lifecycle
    ///   tick (typically milliseconds) or observe the session state
    ///   transition.
    #[error("auth refresh already in flight")]
    RefreshInFlight,
    /// The session has hard-expired; caller must re-authenticate.
    ///
    /// * **Cause**: `now >= expires_at`, or the server classified the
    ///   current token as unrecoverably expired.
    /// * **Recoverability**: not recoverable by refresh. The session
    ///   is revoked and the token is zeroized.
    /// * **Retry guidance**: run an interactive login flow.
    #[error("auth session expired; re-authentication required")]
    AuthExpired,
    /// No active session yet.
    ///
    /// * **Cause**: a lifecycle operation ran before any
    ///   [`SessionLifecycle`] was attached to the session (e.g.
    ///   evaluating refresh while `LoggedOut`).
    /// * **Recoverability**: benign — surface it to the caller so it
    ///   can run login first.
    /// * **Retry guidance**: only retry after a successful login.
    #[error("no active session")]
    NoSession,
}

/// Single-flight refresh guard. Shared across the daemon so multiple
/// concurrent callers trying to refresh simultaneously collapse into
/// one attempt.
#[derive(Debug, Default)]
pub struct RefreshGuard {
    in_flight: AtomicBool,
}

impl RefreshGuard {
    /// Construct an empty guard with no in-flight refresh.
    #[must_use]
    pub fn new() -> Self {
        Self {
            in_flight: AtomicBool::new(false),
        }
    }

    /// Attempt to acquire the refresh slot. Returns `Some(ticket)` if
    /// acquired, `None` if another refresh is already in flight.
    /// Uses an atomic flag instead of a mutex so panic poison can never
    /// make the guard look permanently busy.
    ///
    /// The ticket releases the slot on `Drop`.
    pub fn try_begin(self: &Arc<Self>) -> Option<RefreshTicket> {
        self.in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| RefreshTicket {
                owner: Arc::clone(self),
            })
    }

    /// Returns `true` when a refresh ticket is currently held. For
    /// tests and observability only.
    pub fn is_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::Acquire)
    }
}

/// RAII ticket for a single-flight refresh.
pub struct RefreshTicket {
    owner: Arc<RefreshGuard>,
}

impl std::fmt::Debug for RefreshTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshTicket").finish_non_exhaustive()
    }
}

impl Drop for RefreshTicket {
    fn drop(&mut self) {
        self.owner.in_flight.store(false, Ordering::Release);
    }
}

/// Explicit zeroize helper for a shared auth token. The caller moves
/// the token here to relinquish it; `SecretString::Drop` zeroizes the
/// buffer.
pub fn revoke_token(token: SecretString) {
    drop(token);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_default() -> RefreshPolicy {
        RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        }
    }

    #[test]
    fn refresh_fires_at_threshold() {
        let clock = TestClock::new(1_000_000);
        let policy = policy_default();
        let lc = SessionLifecycle::new(clock.now_secs(), &policy, true);

        // 79% -> no refresh.
        clock.advance(Duration::from_secs(790));
        assert!(!lc.should_refresh(clock.now_secs(), &policy));

        // 80% -> refresh.
        clock.advance(Duration::from_secs(10));
        assert!(lc.should_refresh(clock.now_secs(), &policy));
    }

    #[test]
    fn hard_expiry_detected() {
        let clock = TestClock::new(0);
        let policy = policy_default();
        let lc = SessionLifecycle::new(clock.now_secs(), &policy, false);
        assert!(!lc.is_expired(999));
        assert!(lc.is_expired(1000));
    }

    #[test]
    fn idle_logout_disarms_on_activity() {
        let clock = TestClock::new(0);
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(10_000),
            refresh_threshold: 0.8,
            max_idle: Some(Duration::from_secs(300)),
        };
        let mut lc = SessionLifecycle::new(clock.now_secs(), &policy, false);

        clock.advance(Duration::from_secs(299));
        lc.mark_used(clock.now_secs()); // activity just before idle.
        clock.advance(Duration::from_secs(299));
        assert!(!lc.is_idle_expired(clock.now_secs(), &policy));

        clock.advance(Duration::from_secs(2));
        assert!(lc.is_idle_expired(clock.now_secs(), &policy));
    }

    #[test]
    fn idle_logout_disabled_by_default() {
        let clock = TestClock::new(0);
        let policy = policy_default();
        let lc = SessionLifecycle::new(clock.now_secs(), &policy, false);
        assert!(!lc.is_idle_expired(u64::MAX / 2, &policy));
    }

    #[test]
    fn refresh_guard_is_single_flight() {
        let guard = Arc::new(RefreshGuard::new());
        let t1 = guard.try_begin();
        assert!(t1.is_some());
        let t2 = guard.try_begin();
        assert!(t2.is_none(), "second caller must be blocked");
        drop(t1);
        let t3 = guard.try_begin();
        assert!(t3.is_some(), "slot released after ticket drop");
    }

    #[test]
    fn refresh_guard_releases_slot_during_unwind() {
        let guard = Arc::new(RefreshGuard::new());
        let result = std::panic::catch_unwind({
            let guard = Arc::clone(&guard);
            move || {
                let _ticket = guard.try_begin().expect("slot available");
                assert!(guard.is_in_flight());
                panic!("simulate panic while refresh ticket is held");
            }
        });

        assert!(result.is_err());
        let ticket = guard
            .try_begin()
            .expect("slot must not remain stuck after ticket drop during unwind");
        drop(ticket);
    }

    #[test]
    fn record_refresh_resets_timers() {
        let policy = policy_default();
        let mut lc = SessionLifecycle::new(100, &policy, true);
        assert_eq!(lc.expires_at, 1_100);
        lc.record_refresh(1_000, &policy);
        assert_eq!(lc.established_at, 1_000);
        assert_eq!(lc.expires_at, 2_000);
        assert_eq!(lc.last_used_at, 1_000);
    }

    #[test]
    fn sanitized_policy_clamps_threshold() {
        let p = RefreshPolicy {
            lifetime: Duration::from_secs(0),
            refresh_threshold: 2.0,
            max_idle: None,
        }
        .sanitized();
        assert_eq!(p.lifetime, Duration::from_secs(3600));
        assert!((p.refresh_threshold - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn refresh_after_secs_matches_threshold() {
        let p = RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        assert_eq!(p.refresh_after_secs(), 800);
    }
}
