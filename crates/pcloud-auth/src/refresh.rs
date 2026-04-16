//! Refresh coordinator: glue between `SessionManager`, `RefreshPolicy`,
//! and an injected `Clock`.
//!
//! The coordinator owns no I/O. It exposes two primitives:
//! * [`RefreshCoordinator::evaluate`] — classify the current session
//!   state into a `RefreshDecision` (Idle/Refresh/Expired/NoSession/Ok).
//! * [`RefreshCoordinator::run_refresh`] — single-flight wrapper around
//!   a caller-provided refresh closure. Concurrent callers collapse to
//!   one in-flight attempt, matching enterprise-grade expectations.
//!
//! Auth-expired from a downstream API call is surfaced by
//! [`RefreshCoordinator::handle_auth_expired`], which either triggers a
//! single re-auth attempt via a retained-credentials closure or returns
//! `LifecycleError::AuthExpired` so the caller can propagate a clean
//! error to the user.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;

use pcloud_secret::secret_string::SecretString;

use crate::lifecycle::{Clock, LifecycleError, RefreshGuard, RefreshPolicy};
use crate::manager::SessionManager;

/// Result of classifying the current session against a policy+clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshDecision {
    /// No authenticated session; nothing to do.
    NoSession,
    /// Session is healthy; no refresh required.
    Ok,
    /// Threshold crossed; proactive refresh should fire.
    Refresh,
    /// Idle timeout exceeded; session should be revoked.
    IdleLogout,
    /// Hard expiry passed; re-authentication is required.
    Expired,
}

/// Outcome returned by [`RefreshCoordinator::run_refresh`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Refresh ran and updated the session.
    Refreshed,
    /// Another caller was already refreshing; this caller yielded.
    AlreadyInFlight,
    /// No authenticated session.
    NoSession,
}

/// Session-lifecycle coordinator.
#[derive(Debug)]
pub struct RefreshCoordinator {
    policy: RefreshPolicy,
    clock: Arc<dyn Clock>,
    guard: Arc<RefreshGuard>,
}

impl RefreshCoordinator {
    /// Build a coordinator from policy + clock. The policy is sanitized
    /// (see [`RefreshPolicy::sanitized`]) before being stored.
    #[must_use]
    pub fn new(policy: RefreshPolicy, clock: Arc<dyn Clock>) -> Self {
        Self {
            policy: policy.sanitized(),
            clock,
            guard: Arc::new(RefreshGuard::new()),
        }
    }

    /// Return the effective, sanitized refresh policy in use.
    #[must_use]
    pub fn policy(&self) -> &RefreshPolicy {
        &self.policy
    }

    /// Shared handle to the in-process refresh guard, used to serialize
    /// concurrent refresh attempts across the daemon.
    #[must_use]
    pub fn guard(&self) -> Arc<RefreshGuard> {
        Arc::clone(&self.guard)
    }

    /// Classify current session against policy + wall clock.
    #[must_use]
    pub fn evaluate(&self, session: &SessionManager) -> RefreshDecision {
        let Some(lc) = session.lifecycle() else {
            return RefreshDecision::NoSession;
        };
        let now = self.clock.now_secs();
        if lc.is_expired(now) {
            return RefreshDecision::Expired;
        }
        if lc.is_idle_expired(now, &self.policy) {
            return RefreshDecision::IdleLogout;
        }
        if lc.should_refresh(now, &self.policy) {
            return RefreshDecision::Refresh;
        }
        RefreshDecision::Ok
    }

    /// Run the caller-supplied refresh function under single-flight.
    ///
    /// `refresh_fn` must return a fresh `SecretString` auth token on
    /// success. On success, the session's token is replaced and its
    /// timers are reset. On concurrent call, returns
    /// [`RefreshOutcome::AlreadyInFlight`] without invoking the closure.
    pub fn run_refresh<F, E>(
        &self,
        session: &mut SessionManager,
        refresh_fn: F,
    ) -> Result<RefreshOutcome, E>
    where
        F: FnOnce() -> Result<SecretString, E>,
        E: From<LifecycleError>,
    {
        if session.lifecycle().is_none() {
            return Ok(RefreshOutcome::NoSession);
        }
        let Some(_ticket) = self.guard.try_begin() else {
            return Ok(RefreshOutcome::AlreadyInFlight);
        };

        let new_token = refresh_fn()?;
        let now = self.clock.now_secs();
        session
            .record_refresh(now, &self.policy, new_token)
            .map_err(|_| LifecycleError::NoSession)?;
        Ok(RefreshOutcome::Refreshed)
    }

    /// Handle an auth-expired (e.g. HTTP 401) signal from a downstream
    /// API call. If `reauth_fn` is `Some` and credentials are retained
    /// in-memory, it is invoked exactly once (under single-flight).
    /// Otherwise returns `LifecycleError::AuthExpired` so the caller
    /// can surface a clean error to the user.
    pub fn handle_auth_expired<F, E>(
        &self,
        session: &mut SessionManager,
        reauth_fn: Option<F>,
    ) -> Result<RefreshOutcome, E>
    where
        F: FnOnce() -> Result<SecretString, E>,
        E: From<LifecycleError>,
    {
        let credentials_retained = session
            .lifecycle()
            .map(|lc| lc.credentials_retained)
            .unwrap_or(false);

        match (credentials_retained, reauth_fn) {
            (true, Some(f)) => self.run_refresh(session, f),
            _ => {
                session.revoke();
                Err(LifecycleError::AuthExpired.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use pcloud_model::ids::UserId;
    use pcloud_secret::{ExposeSecret, secret_string::SecretString};

    use crate::lifecycle::{LifecycleError, TestClock};
    use crate::{AuthCommand, RefreshPolicy, SessionManager};

    use super::*;

    #[derive(Debug, thiserror::Error)]
    enum TestError {
        #[error("lifecycle: {0}")]
        Lifecycle(#[from] LifecycleError),
    }

    fn authed_session(now: u64, policy: &RefreshPolicy, retained: bool) -> SessionManager {
        let mut s = SessionManager::new();
        s.apply(AuthCommand::LoginWithToken {
            token: SecretString::new("t0"),
        })
        .unwrap();
        s.apply(AuthCommand::MarkAuthenticated {
            user_id: Some(UserId::new(1)),
            auth_token: SecretString::new("t0"),
        })
        .unwrap();
        s.attach_lifecycle(now, policy, retained);
        s
    }

    #[test]
    fn evaluate_fires_refresh_at_threshold() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let coord = RefreshCoordinator::new(policy.clone(), clock.clone());
        let session = authed_session(0, &policy, false);

        assert_eq!(coord.evaluate(&session), RefreshDecision::Ok);
        clock.advance(Duration::from_secs(799));
        assert_eq!(coord.evaluate(&session), RefreshDecision::Ok);
        clock.advance(Duration::from_secs(1));
        assert_eq!(coord.evaluate(&session), RefreshDecision::Refresh);
        clock.advance(Duration::from_secs(200));
        assert_eq!(coord.evaluate(&session), RefreshDecision::Expired);
    }

    #[test]
    fn run_refresh_rewinds_timers_and_swaps_token() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let coord = RefreshCoordinator::new(policy.clone(), clock.clone());
        let mut session = authed_session(0, &policy, false);
        clock.advance(Duration::from_secs(850));

        let outcome = coord
            .run_refresh(&mut session, || -> Result<_, TestError> {
                Ok(SecretString::new("t1"))
            })
            .unwrap();
        assert_eq!(outcome, RefreshOutcome::Refreshed);
        assert_eq!(
            session
                .snapshot()
                .auth_token
                .as_ref()
                .unwrap()
                .expose_secret(),
            "t1"
        );
        let lc = session.lifecycle().unwrap();
        assert_eq!(lc.established_at, 850);
        assert_eq!(lc.expires_at, 1850);
    }

    #[test]
    fn run_refresh_is_single_flight() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy::default();
        let coord = RefreshCoordinator::new(policy.clone(), clock.clone());
        let mut session = authed_session(0, &policy, false);

        // Hold a ticket manually to simulate an in-flight refresh on
        // another thread. The second call must yield AlreadyInFlight
        // and the refresh_fn must not run.
        let guard = coord.guard();
        let _ticket = guard.try_begin().expect("ticket");
        let calls = AtomicUsize::new(0);

        let outcome = coord
            .run_refresh(&mut session, || -> Result<_, TestError> {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(SecretString::new("never"))
            })
            .unwrap();
        assert_eq!(outcome, RefreshOutcome::AlreadyInFlight);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn handle_auth_expired_triggers_reauth_when_credentials_retained() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy::default();
        let coord = RefreshCoordinator::new(policy.clone(), clock.clone());
        let mut session = authed_session(0, &policy, true);

        let out = coord
            .handle_auth_expired::<_, TestError>(&mut session, Some(|| Ok(SecretString::new("t2"))))
            .unwrap();
        assert_eq!(out, RefreshOutcome::Refreshed);
        assert_eq!(
            session
                .snapshot()
                .auth_token
                .as_ref()
                .unwrap()
                .expose_secret(),
            "t2"
        );
    }

    #[test]
    fn handle_auth_expired_surfaces_clean_error_without_credentials() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy::default();
        let coord = RefreshCoordinator::new(policy.clone(), clock.clone());
        let mut session = authed_session(0, &policy, false);

        let err = coord
            .handle_auth_expired::<fn() -> Result<SecretString, TestError>, TestError>(
                &mut session,
                None,
            )
            .unwrap_err();
        assert!(matches!(
            err,
            TestError::Lifecycle(LifecycleError::AuthExpired)
        ));
        assert_eq!(
            session.snapshot().state,
            crate::SessionState::LoggedOut,
            "token must be revoked on AuthExpired",
        );
        assert!(session.snapshot().auth_token.is_none());
    }

    #[test]
    fn idle_logout_decision_surfaces() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(10_000),
            refresh_threshold: 0.8,
            max_idle: Some(Duration::from_secs(100)),
        };
        let coord = RefreshCoordinator::new(policy.clone(), clock.clone());
        let mut session = authed_session(0, &policy, false);

        // Activity keeps it alive.
        clock.advance(Duration::from_secs(99));
        session.mark_used(clock.now_secs());
        clock.advance(Duration::from_secs(99));
        assert_eq!(coord.evaluate(&session), RefreshDecision::Ok);

        // Then fall idle.
        clock.advance(Duration::from_secs(200));
        assert_eq!(coord.evaluate(&session), RefreshDecision::IdleLogout);
    }

    #[test]
    fn run_refresh_noop_without_session() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy::default();
        let coord = RefreshCoordinator::new(policy, clock);
        let mut session = SessionManager::new();
        let out = coord
            .run_refresh(&mut session, || -> Result<_, TestError> {
                Ok(SecretString::new("x"))
            })
            .unwrap();
        assert_eq!(out, RefreshOutcome::NoSession);
    }
}
