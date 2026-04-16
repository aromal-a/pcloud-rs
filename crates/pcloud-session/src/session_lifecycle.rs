//! Daemon-side glue for session lifecycle management.
//!
//! Composes [`pcloud_auth::RefreshCoordinator`] with a system clock and
//! a [`pcloud_auth::RefreshPolicy`] that can be tuned by the operator.
//! A single `SessionSupervisor` owns the coordinator and a weak link to
//! a closure that knows how to re-authenticate (token refresh today,
//! password re-login when credentials are retained in memory).
//!
//! This module deliberately does **not** persist passwords. The
//! `reauth_fn` closure is provided by the caller at session start so
//! secrets live inside that closure's capture and are dropped (and
//! zeroized) together with the supervisor.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;

use pcloud_auth::{
    Clock, LifecycleError, RefreshCoordinator, RefreshDecision, RefreshOutcome, RefreshPolicy,
    SessionManager, SystemClock,
};
use pcloud_secret::secret_string::SecretString;
use thiserror::Error;

/// Errors surfaced from the daemon-side lifecycle layer.
#[derive(Debug, Error)]
pub enum SessionLifecycleError {
    /// Wraps a [`LifecycleError`] surfaced by the underlying
    /// [`RefreshCoordinator`] (e.g. `AuthExpired`, policy violation).
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    /// The refresh backend returned a non-expiry failure; the string
    /// is the upstream transport/protocol error message.
    #[error("refresh backend failed: {0}")]
    Refresh(String),
}

/// Operator-tunable lifecycle configuration. Falls back to secure
/// defaults (1h lifetime, 80% refresh, no idle logout).
#[derive(Debug, Clone, Default)]
pub struct SessionLifecycleConfig {
    /// Refresh policy (lifetime, threshold fraction, optional idle
    /// logout) the supervisor will enforce on every `evaluate`/`tick`.
    pub policy: RefreshPolicy,
}

/// Daemon-owned session supervisor.
#[derive(Debug)]
pub struct SessionSupervisor {
    coordinator: RefreshCoordinator,
    clock: Arc<dyn Clock>,
}

impl SessionSupervisor {
    /// Build a supervisor that uses the process wall clock
    /// ([`SystemClock`]). Prefer [`SessionSupervisor::with_clock`] in
    /// tests so timings can be driven deterministically.
    #[must_use]
    pub fn new(policy: RefreshPolicy) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        Self {
            coordinator: RefreshCoordinator::new(policy, Arc::clone(&clock)),
            clock,
        }
    }

    /// Build a supervisor with a caller-supplied [`Clock`]. Used by the
    /// unit tests (via `TestClock`) to exercise threshold firing, idle
    /// logout, and single-flight collapse without real sleeps.
    #[must_use]
    pub fn with_clock(policy: RefreshPolicy, clock: Arc<dyn Clock>) -> Self {
        Self {
            coordinator: RefreshCoordinator::new(policy, Arc::clone(&clock)),
            clock,
        }
    }

    /// Current wall-clock seconds as observed by the supervisor's
    /// injected `Clock`. Public so [`crate::refresh_loop::tick`] and
    /// auth glue can derive consistent timestamps (e.g. when attaching
    /// a new lifecycle after a successful login).
    #[must_use]
    pub fn now_secs(&self) -> u64 {
        self.clock.now_secs()
    }

    /// Borrow the inner [`RefreshCoordinator`] so callers can access
    /// the single-flight guard or run raw coordinator operations that
    /// the supervisor does not re-expose.
    #[must_use]
    pub fn coordinator(&self) -> &RefreshCoordinator {
        &self.coordinator
    }

    /// Observe whether a proactive refresh is currently holding the
    /// single-flight slot. Used by `pcloud_daemon::runtime::RuntimeShell`
    /// to render `Method::SessionStatus` without contending with the
    /// refresh path (the guard uses an internal `Mutex<bool>`).
    #[must_use]
    pub fn refresh_in_flight(&self) -> bool {
        self.coordinator.guard().is_in_flight()
    }

    /// Expose the [`RefreshPolicy`] so callers can attach new session
    /// lifecycles with the same timing contract the supervisor will
    /// enforce on subsequent `tick`/`evaluate` calls.
    #[must_use]
    pub fn policy(&self) -> &RefreshPolicy {
        self.coordinator.policy()
    }

    /// Classify the current session. Callers typically invoke this on a
    /// ticker or before each outbound API call.
    pub fn evaluate(&self, session: &SessionManager) -> RefreshDecision {
        self.coordinator.evaluate(session)
    }

    /// Proactive refresh. Calls `refresh_fn` under single-flight and
    /// swaps the session token on success.
    pub fn run_refresh<F>(
        &self,
        session: &mut SessionManager,
        refresh_fn: F,
    ) -> Result<RefreshOutcome, SessionLifecycleError>
    where
        F: FnOnce() -> Result<SecretString, SessionLifecycleError>,
    {
        self.coordinator.run_refresh(session, refresh_fn)
    }

    /// Handle a 401/auth-expired signal. If credentials are retained
    /// (i.e. `attach_lifecycle(..., credentials_retained=true)`) and a
    /// re-auth closure is supplied, runs a single re-auth attempt.
    /// Otherwise revokes the session and surfaces
    /// [`LifecycleError::AuthExpired`].
    pub fn handle_auth_expired<F>(
        &self,
        session: &mut SessionManager,
        reauth_fn: Option<F>,
    ) -> Result<RefreshOutcome, SessionLifecycleError>
    where
        F: FnOnce() -> Result<SecretString, SessionLifecycleError>,
    {
        self.coordinator.handle_auth_expired(session, reauth_fn)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pcloud_auth::{AuthCommand, TestClock};
    use pcloud_model::ids::UserId;
    use pcloud_secret::{ExposeSecret, secret_string::SecretString};

    use super::*;

    fn authed(now: u64, policy: &RefreshPolicy, retained: bool) -> SessionManager {
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
    fn supervisor_runs_refresh_at_threshold() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(100),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let mut session = authed(0, &policy, false);
        clock.advance(Duration::from_secs(81));
        assert_eq!(sup.evaluate(&session), RefreshDecision::Refresh);

        let out = sup
            .run_refresh(&mut session, || Ok(SecretString::new("fresh")))
            .unwrap();
        assert_eq!(out, RefreshOutcome::Refreshed);
        assert_eq!(
            session
                .snapshot()
                .auth_token
                .as_ref()
                .unwrap()
                .expose_secret(),
            "fresh"
        );
    }

    #[test]
    fn supervisor_surfaces_auth_expired_without_retained_creds() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy::default();
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let mut session = authed(0, &policy, false);

        let err = sup
            .handle_auth_expired::<fn() -> Result<SecretString, SessionLifecycleError>>(
                &mut session,
                None,
            )
            .unwrap_err();
        assert!(matches!(
            err,
            SessionLifecycleError::Lifecycle(LifecycleError::AuthExpired)
        ));
        assert!(session.snapshot().auth_token.is_none());
    }

    #[test]
    fn supervisor_reauths_when_credentials_retained() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy::default();
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let mut session = authed(0, &policy, true);
        let out = sup
            .handle_auth_expired(&mut session, Some(|| Ok(SecretString::new("reborn"))))
            .unwrap();
        assert_eq!(out, RefreshOutcome::Refreshed);
        assert_eq!(
            session
                .snapshot()
                .auth_token
                .as_ref()
                .unwrap()
                .expose_secret(),
            "reborn"
        );
    }
}
