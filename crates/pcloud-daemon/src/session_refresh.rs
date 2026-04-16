//! Session refresh integration for the daemon serve loop.
//!
//! The daemon's IPC serve loop (`serve.rs`) calls
//! [`pcloud_session::refresh_loop::tick`] on every iteration (after
//! each request or accept-timeout). This module provides configuration
//! mapping helpers and integration tests for the refresh path.
//!
//! ## Why inline in the serve loop, not a background thread?
//!
//! `RuntimeShell` is intentionally `!Sync` (see `runtime.rs` doc
//! header). Running the refresh tick on the same thread that owns the
//! runtime avoids introducing `Mutex<SessionManager>` and the locking
//! discipline that would entail. The accept-timeout on the listener
//! socket ensures the tick fires even during idle periods.
//!
//! ## Configuration
//!
//! Two `[auth]` knobs control the refresh cadence:
//!
//! | Key | Default | Effect |
//! |-----|---------|--------|
//! | `refresh_check_interval_secs` | 300 | Listener accept timeout; controls max idle gap between ticks |
//! | `refresh_margin_secs` | 600 | Seconds before expiry to start proactive refresh |
//!
//! Setting `refresh_check_interval_secs = 0` disables the accept-timeout
//! entirely; the tick will then only fire on each incoming IPC request.
//!
//! ## Security
//!
//! No secret material is logged. The per-tick token clone inside
//! `refresh_loop::tick` is dropped (and zeroized) at the end of each
//! tick invocation.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::time::Duration;

use pcloud_auth::RefreshPolicy;
use pcloud_config::auth::AuthPolicy;

/// Derive a [`RefreshPolicy`] from the operator-visible `[auth]` config
/// block. The `refresh_margin_secs` is converted to a threshold
/// fraction: `threshold = 1.0 - (margin / lifetime)`.
///
/// Falls back to [`RefreshPolicy::default`] when the margin exceeds
/// the token lifetime (to avoid a nonsensical threshold <= 0).
#[must_use]
pub fn policy_from_config(auth: &AuthPolicy) -> RefreshPolicy {
    let base = RefreshPolicy::default();
    let lifetime_secs = base.lifetime.as_secs();
    let margin = auth.refresh_margin_secs.min(lifetime_secs);
    let threshold = if lifetime_secs > 0 {
        1.0 - (margin as f32 / lifetime_secs as f32)
    } else {
        0.8
    };
    RefreshPolicy {
        lifetime: Duration::from_secs(lifetime_secs),
        refresh_threshold: threshold,
        max_idle: base.max_idle,
    }
    .sanitized()
}

/// Compute the accept-timeout to set on the IPC listener. Returns
/// `None` when the background refresh is disabled
/// (`refresh_check_interval_secs == 0`).
#[must_use]
pub fn accept_timeout(auth: &AuthPolicy) -> Option<Duration> {
    let secs = auth.refresh_check_interval_secs;
    if secs == 0 {
        return None;
    }
    // Cap at 60 s so shutdown remains responsive even with large
    // check intervals.
    Some(Duration::from_secs(secs.min(60)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use pcloud_auth::{AuthCommand, Clock, RefreshPolicy, SessionManager, TestClock};
    use pcloud_config::{ConfigProfile, api::ApiMode};
    use pcloud_model::ids::UserId;
    use pcloud_secret::secret_string::SecretString;

    use pcloud_backends::auth_backend::AuthRuntime;
    use pcloud_session::refresh_loop::{self, TickOutcome};
    use pcloud_session::session_lifecycle::SessionSupervisor;

    fn dev_runtime() -> AuthRuntime {
        let mut config = ConfigProfile::secure_defaults(
            std::path::PathBuf::from("/tmp/pcloud-session-refresh-test"),
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
    fn policy_from_default_config() {
        let auth = AuthPolicy::default();
        let policy = policy_from_config(&auth);
        // Default: 600s margin on 3600s lifetime => threshold ~0.833
        let expected = 1.0 - (600.0 / 3600.0);
        assert!(
            (policy.refresh_threshold - expected as f32).abs() < 0.01,
            "threshold should be ~{expected}, got {}",
            policy.refresh_threshold
        );
    }

    #[test]
    fn policy_clamps_excessive_margin() {
        let auth = AuthPolicy {
            refresh_margin_secs: 999_999, // exceeds lifetime
            ..AuthPolicy::default()
        };
        let policy = policy_from_config(&auth);
        // Clamped to lifetime => threshold = 0.0, then sanitized to 0.0
        // (which is valid — means "refresh immediately").
        assert!(
            policy.refresh_threshold >= 0.0 && policy.refresh_threshold <= 1.0,
            "threshold must be in [0,1], got {}",
            policy.refresh_threshold
        );
    }

    #[test]
    fn accept_timeout_returns_none_when_disabled() {
        let auth = AuthPolicy {
            refresh_check_interval_secs: 0,
            ..AuthPolicy::default()
        };
        assert!(accept_timeout(&auth).is_none());
    }

    #[test]
    fn accept_timeout_caps_at_60s() {
        let auth = AuthPolicy {
            refresh_check_interval_secs: 3600,
            ..AuthPolicy::default()
        };
        assert_eq!(accept_timeout(&auth), Some(Duration::from_secs(60)));
    }

    #[test]
    fn refresh_fires_when_within_window() {
        let clock = Arc::new(TestClock::new(0));
        let policy = RefreshPolicy {
            lifetime: Duration::from_secs(1000),
            refresh_threshold: 0.8,
            max_idle: None,
        };
        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = authed_session(0, &policy);

        // Before threshold: tick should be Ok.
        clock.advance(Duration::from_secs(100));
        let outcome = refresh_loop::tick(&sup, &runtime, &mut session).unwrap();
        assert_eq!(outcome, TickOutcome::Ok);

        // Past threshold (>800s): tick should refresh.
        clock.advance(Duration::from_secs(750));
        let outcome = refresh_loop::tick(&sup, &runtime, &mut session).unwrap();
        assert_eq!(outcome, TickOutcome::Refreshed);
    }

    #[test]
    fn integration_always_refresh_with_zero_margin() {
        // margin = lifetime => threshold = 0.0 => always within refresh window.
        let clock = Arc::new(TestClock::new(0));
        let auth = AuthPolicy {
            refresh_margin_secs: 3600, // == lifetime
            ..AuthPolicy::default()
        };
        let policy = policy_from_config(&auth);
        assert!(
            policy.refresh_threshold < 0.01,
            "threshold should be ~0, got {}",
            policy.refresh_threshold
        );

        let sup = SessionSupervisor::with_clock(policy.clone(), clock.clone() as Arc<dyn Clock>);
        let runtime = dev_runtime();
        let mut session = authed_session(0, &policy);

        // Even at t=1 (essentially immediately), refresh should fire.
        clock.advance(Duration::from_secs(1));
        let outcome = refresh_loop::tick(&sup, &runtime, &mut session).unwrap();
        assert_eq!(outcome, TickOutcome::Refreshed);
    }
}
