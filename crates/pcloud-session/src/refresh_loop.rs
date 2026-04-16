//! Session refresh loop (sub-task 3).
//!
//! This module provides a single, testable, synchronous primitive —
//! [`tick`] — that represents one iteration of the session supervisor's
//! periodic check. A concrete async runner (tokio task, std::thread
//! ticker, etc.) can call [`tick`] on its preferred cadence; the
//! decision logic, single-flight guarantees, and audit emission are
//! driven by the [`SessionSupervisor`] and by the pure-function logic
//! below.
//!
//! ## Why a pure `tick`, not a spawned tokio task here
//!
//! The daemon's IPC path (`serve.rs`) is currently synchronous; there
//! is no top-level tokio runtime yet. Inverting control — injecting
//! `tick` into whichever runtime the daemon adopts — keeps this crate's
//! public surface stable and lets the unit tests in
//! `pcloud-daemon` drive refresh decisions deterministically via
//! `pcloud_auth::TestClock`. Once the daemon gains a tokio runtime, a
//! thin `async fn refresh_loop_task(interval: Duration, ...)` wrapper
//! that awaits `tokio::time::sleep` and calls `tick` on each wake is
//! trivial to add without touching this module's test contract.
//!
//! ## Security / persistence constraints (CLAUDE.md "Secrets")
//!
//! * **NO password persistence.** pCloud's refresh uses the current
//!   auth token itself (`userinfo?getauth=1`). The token is held only
//!   in the live `SessionManager` and never written to disk except via
//!   the owner-only `auth_vault` when `durable_auth_tokens_enabled` is
//!   opted in. If the refresh succeeds, the new token replaces the old
//!   one (old `SecretString::Drop` zeroizes). If the refresh fails with
//!   `AuthExpired`, the session is revoked and the caller must
//!   re-authenticate.
//! * **Idle-logout emits an audit event.** The audit details string
//!   carries no secret material — only the lifecycle decision name and
//!   the user-id (when known).

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_auth::{AuthEvent, RefreshDecision, RefreshTokenError, SessionManager};
use thiserror::Error;

use pcloud_backends::auth_backend::AuthRuntime;

use crate::session_lifecycle::{SessionLifecycleError, SessionSupervisor};

/// Errors surfaced from one [`tick`] of the refresh loop.
#[derive(Debug, Error)]
pub enum RefreshLoopError {
    /// Wraps an error bubbled up from the session lifecycle layer
    /// (policy evaluation, idle-logout bookkeeping, etc.).
    #[error(transparent)]
    Lifecycle(#[from] SessionLifecycleError),
    /// The refresh orchestrator returned a non-expiry failure that the
    /// loop cannot interpret locally; carries the upstream message.
    #[error("refresh orchestrator failure: {0}")]
    Orchestrator(String),
}

/// The outcome of a single refresh-loop tick. Returned so the embedding
/// runner can decide how to log / meter the step without needing a
/// mutable observability handle inside the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    /// No session attached; nothing to do.
    NoSession,
    /// Session is healthy; no action taken.
    Ok,
    /// Proactive refresh ran and a new token was installed.
    Refreshed,
    /// Another caller already held the single-flight guard; this tick
    /// yielded.
    AlreadyInFlight,
    /// Idle-logout threshold crossed. The session has been revoked and
    /// the caller should emit the pre-formatted audit detail string.
    IdleLogout {
        /// Pre-formatted audit detail string for the runner to persist.
        audit_details: String,
    },
    /// Hard expiry crossed. The session has been revoked; caller must
    /// re-authenticate.
    Expired,
    /// Refresh attempt surfaced a server-classified expiry. Session is
    /// revoked.
    AuthExpired {
        /// pCloud `result` code returned by the refresh call.
        result: u64,
    },
    /// Refresh attempt hit a temporary failure; session is intact.
    TemporaryFailure {
        /// Human-readable failure reason captured from the transport.
        reason: String,
    },
}

/// Run one iteration of the session refresh loop.
///
/// Expected to be invoked on a caller-owned cadence (e.g. every 60s,
/// configurable via [`pcloud_auth::RefreshPolicy`]). The function is
/// deterministic with respect to the supervisor's injected clock, so
/// tests using `TestClock` can drive threshold firing, idle logout,
/// and single-flight collapse directly without sleeping.
///
/// Single-flight is guaranteed by [`pcloud_auth::RefreshGuard`] inside
/// the supervisor: two concurrent ticks will never both invoke
/// `auth_runtime.refresh_token`.
pub fn tick(
    supervisor: &SessionSupervisor,
    auth_runtime: &AuthRuntime,
    session: &mut SessionManager,
) -> Result<TickOutcome, RefreshLoopError> {
    match supervisor.evaluate(session) {
        RefreshDecision::NoSession => Ok(TickOutcome::NoSession),
        RefreshDecision::Ok => Ok(TickOutcome::Ok),
        RefreshDecision::Expired => {
            session.revoke();
            Ok(TickOutcome::Expired)
        }
        RefreshDecision::IdleLogout => {
            // Sub-task 3 contract: idle logout emits an audit event.
            // The caller is responsible for actually persisting the
            // event via `RuntimeShell::record_audit_event`; we return
            // the formatted details so the loop runner stays free of
            // store/StoreError coupling.
            let user = session
                .snapshot()
                .authenticated_user
                .map(|uid| uid.get().to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            let details = format!("idle_logout user_id={user}");
            session.revoke();
            Ok(TickOutcome::IdleLogout {
                audit_details: details,
            })
        }
        RefreshDecision::Refresh => {
            // Collapse concurrent ticks: if another caller is already
            // holding the single-flight guard we yield immediately
            // without invoking the transport. The guard lives inside
            // `SessionSupervisor::coordinator`; we peek at it via the
            // `refresh_in_flight` helper to avoid double-acquire.
            if supervisor.refresh_in_flight() {
                return Ok(TickOutcome::AlreadyInFlight);
            }
            // Acquire the slot for the duration of this tick. The
            // ticket releases on drop at the end of the match arm.
            let Some(_ticket) = supervisor.coordinator().guard().try_begin() else {
                return Ok(TickOutcome::AlreadyInFlight);
            };

            // Capture the current token out of the session before we
            // call the transport. `clone_secret()` is the audit-visible
            // duplication path; the copy lives only until the call
            // returns.
            let Some(current) = session
                .snapshot()
                .auth_token
                .as_ref()
                .map(|t| t.clone_secret())
            else {
                return Ok(TickOutcome::NoSession);
            };

            // Invoke `refresh_token` against the real session. The
            // orchestrator installs the new token itself via
            // `replace_auth_token` on success and revokes on
            // `AuthExpired`. We then rewind the lifecycle timers so
            // the next tick does not immediately re-fire.
            match auth_runtime.refresh_token(session, &current) {
                Ok(AuthEvent::TokenRefreshed { .. }) => {
                    // Rewind timers against the supervisor's policy so
                    // the next tick does not immediately re-fire.
                    let now = supervisor.now_secs();
                    let policy = supervisor.policy().clone();
                    // The token is already replaced inside `session` by
                    // `refresh_token`; we just update the timers.
                    if let Some(token) = session
                        .snapshot()
                        .auth_token
                        .as_ref()
                        .map(|t| t.clone_secret())
                    {
                        let _ = session.record_refresh(now, &policy, token);
                    }
                    Ok(TickOutcome::Refreshed)
                }
                Ok(_) => Ok(TickOutcome::Ok),
                Err(RefreshTokenError::AuthExpired(result)) => {
                    Ok(TickOutcome::AuthExpired { result })
                }
                Err(RefreshTokenError::TemporaryFailure(reason)) => {
                    Ok(TickOutcome::TemporaryFailure { reason })
                }
                Err(RefreshTokenError::NotAuthenticated) => Ok(TickOutcome::NoSession),
                Err(other) => Err(RefreshLoopError::Orchestrator(other.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use pcloud_auth::{AuthCommand, Clock, RefreshPolicy, SessionManager, TestClock};
    use pcloud_config::{ConfigProfile, api::ApiMode};
    use pcloud_model::ids::UserId;
    use pcloud_secret::{ExposeSecret, secret_string::SecretString};

    use pcloud_backends::auth_backend::AuthRuntime;

    use crate::session_lifecycle::SessionSupervisor;

    use super::*;

    fn dev_runtime() -> AuthRuntime {
        let mut config = ConfigProfile::secure_defaults(
            std::path::PathBuf::from("/tmp/pcloud-refresh-loop-test"),
            pcloud_config::Environment::Development,
        );
        config.api.mode = ApiMode::Development;
        AuthRuntime::from_config(&config)
    }

    fn authed_session(now: u64, policy: &RefreshPolicy) -> SessionManager {
        let mut s = SessionManager::new();
        s.apply(AuthCommand::LoginWithToken {
            token: SecretString::new("auth-token-42"),
        })
        .unwrap();
        s.apply(AuthCommand::MarkAuthenticated {
            user_id: Some(UserId::new(42)),
            auth_token: SecretString::new("auth-token-42"),
        })
        .unwrap();
        s.attach_lifecycle(now, policy, false);
        s
    }

    #[test]
    fn tick_noop_when_session_is_healthy() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = authed_session(0, &policy);

        assert_eq!(tick(&sup, &runtime, &mut session).unwrap(), TickOutcome::Ok);
    }

    #[test]
    fn tick_fires_refresh_at_threshold_and_installs_fresh_token() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = authed_session(0, &policy);

        // Advance past the 80% threshold but before hard expiry.
        clock.advance(Duration::from_secs(850));
        let outcome = tick(&sup, &runtime, &mut session).unwrap();
        assert_eq!(outcome, TickOutcome::Refreshed);
        // DevelopmentAuthTransport refresh path returns a fresh token
        // encoded by its userinfo handler; exact value is irrelevant —
        // only that the session stays authenticated and timers reset.
        assert_eq!(
            session.snapshot().state,
            pcloud_auth::SessionState::Authenticated
        );
        let lc = session
            .lifecycle()
            .expect("lifecycle attached after refresh");
        assert_eq!(lc.established_at, 850);
        assert_eq!(lc.expires_at, 1850);
    }

    #[test]
    fn tick_is_single_flight() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = authed_session(0, &policy);

        clock.advance(Duration::from_secs(850));
        // Hold the single-flight slot manually to simulate a concurrent
        // refresh on another thread.
        let guard = sup.coordinator().guard();
        let ticket = guard.try_begin().expect("slot available");
        assert!(sup.refresh_in_flight());

        // With another refresh in flight, `tick` must yield without
        // invoking the transport. We assert both the reported outcome
        // and that the session token is unchanged (i.e. no concurrent
        // refresh actually ran).
        let before = session
            .snapshot()
            .auth_token
            .as_ref()
            .map(|t| t.expose_secret().to_owned());
        let outcome = tick(&sup, &runtime, &mut session).unwrap();
        assert_eq!(outcome, TickOutcome::AlreadyInFlight);
        let after = session
            .snapshot()
            .auth_token
            .as_ref()
            .map(|t| t.expose_secret().to_owned());
        assert_eq!(before, after, "token must not change while guard held");

        drop(ticket);
        assert!(!sup.refresh_in_flight());
    }

    #[test]
    fn tick_idle_logout_revokes_and_emits_audit_details() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(1_000_000),
            refresh_threshold: 0.8,
            max_idle: Some(Duration::from_secs(300)),
        };
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = authed_session(0, &policy);

        clock.advance(Duration::from_secs(301));
        let outcome = tick(&sup, &runtime, &mut session).unwrap();
        match outcome {
            TickOutcome::IdleLogout { audit_details } => {
                assert!(audit_details.contains("idle_logout"));
                assert!(audit_details.contains("user_id=42"));
            }
            other => panic!("expected IdleLogout, got {other:?}"),
        }
        assert!(session.snapshot().auth_token.is_none());
        assert_eq!(
            session.snapshot().state,
            pcloud_auth::SessionState::LoggedOut
        );
    }

    #[test]
    fn tick_hard_expiry_revokes() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(100),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = authed_session(0, &policy);

        clock.advance(Duration::from_secs(200));
        assert_eq!(
            tick(&sup, &runtime, &mut session).unwrap(),
            TickOutcome::Expired
        );
        assert!(session.snapshot().auth_token.is_none());
    }

    #[test]
    fn tick_returns_no_session_when_unauthenticated() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy::default();
        let sup = SessionSupervisor::with_clock(policy, clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = SessionManager::new();
        assert_eq!(
            tick(&sup, &runtime, &mut session).unwrap(),
            TickOutcome::NoSession
        );
    }
}
