//! Transport factory policy used by the daemon bootstrap.
//!
//! This module intentionally owns only the **decision** of whether a
//! production network transport should be wrapped in
//! [`pcloud_proto::resilient_transport::ResilientTransport`]. It does not
//! touch any feature backend (FUSE, crypto, shares, backups, sync,
//! public-links, notifications, audit, plugins) — the wiring of those
//! backends is unchanged.
//!
//! # Rationale
//!
//! The `ResilientTransport` wrapper landed as opt-in. For enterprise-grade
//! operation we want production daemons to default to wrapping the network
//! transport (token bucket + circuit breaker + retries with jitter) while
//! keeping dev/test on direct dispatch so existing test determinism is
//! preserved.
//!
//! # Determinism
//!
//! The production factory uses `SystemClock` + `ThreadSleepWaiter` only.
//! Any integration test that needs to exercise rate-limit / retry timing
//! must construct a `ResilientTransport` directly with a `ManualClock`
//! and a recording waiter (see `pcloud-proto` tests), **not** through the
//! production factory.
//!
//! # Scope
//!
//! This factory only exposes a typed wrapper for the binary API transport
//! (`BinaryApiTransport`). Each feature-domain backend still constructs
//! its own transport locally; touching those call sites is out of scope
//! for this change (see `CLAUDE.md` constraints).

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;

use pcloud_config::{Environment, resilience::ResiliencePolicy};
use pcloud_proto::resilient_transport::{
    RateLimitMode, ResilientError, ResilientTransport, ThreadSleepWaiter, default_classifier,
};
use pcloud_proto::transport::{BinaryApiTransport, TransportError};
use pcloud_resilience::{RateLimitError, SystemClock};

/// Decision produced by the factory, inspectable by tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapDecision {
    /// Production: transports are wrapped in `ResilientTransport`.
    Wrap,
    /// Development/Test: transports are used bare for deterministic tests.
    Bare,
}

impl WrapDecision {
    /// Returns `true` when this decision represents the wrapped
    /// production path (`ResilientTransport` active).
    #[must_use]
    pub const fn is_wrapped(&self) -> bool {
        matches!(self, Self::Wrap)
    }
}

/// Factory that decides whether to wrap outbound transports.
///
/// The factory is cheaply cloneable and `Send + Sync`: it carries a copy
/// of the [`ResiliencePolicy`] and the target [`Environment`]. Actual
/// wrapping happens lazily when a backend hands a transport in.
#[derive(Debug, Clone)]
pub struct TransportFactory {
    environment: Environment,
    policy: ResiliencePolicy,
}

impl TransportFactory {
    /// Build a factory for the given environment and resilience policy.
    #[must_use]
    pub fn new(environment: Environment, policy: ResiliencePolicy) -> Self {
        Self {
            environment,
            policy,
        }
    }

    /// The active wrap decision for this factory.
    #[must_use]
    pub fn decision(&self) -> WrapDecision {
        match self.environment {
            Environment::Production => WrapDecision::Wrap,
            Environment::Development | Environment::Test => WrapDecision::Bare,
        }
    }

    /// The environment the factory was built for.
    #[must_use]
    pub const fn environment(&self) -> Environment {
        self.environment
    }

    /// Wrap a [`BinaryApiTransport`] in a [`ResilientTransport`] when the
    /// environment is production; return `Ok(None)` otherwise.
    ///
    /// Production wrapping uses real [`SystemClock`] + [`ThreadSleepWaiter`],
    /// as required for enterprise-grade operation. Tests that need
    /// deterministic timing should construct a `ResilientTransport`
    /// directly with a `ManualClock`.
    pub fn wrap_binary(
        &self,
        inner: BinaryApiTransport,
    ) -> Result<Option<ResilientTransport<BinaryApiTransport>>, RateLimitError> {
        match self.decision() {
            WrapDecision::Bare => Ok(None),
            WrapDecision::Wrap => {
                let wrapped = ResilientTransport::new(
                    inner,
                    &self.policy,
                    Arc::new(SystemClock),
                    Arc::new(ThreadSleepWaiter),
                    default_classifier::<TransportError>(),
                    RateLimitMode::Wait,
                )?;
                Ok(Some(wrapped))
            }
        }
    }
}

/// Surface the resilient-transport error type for callers that exercise
/// a wrapped transport returned by the factory.
pub type WrappedBinaryError = ResilientError<TransportError>;

#[cfg(test)]
mod tests {
    use super::*;
    use pcloud_proto::transport::TransportConfig;
    use std::time::Duration;

    fn dummy_transport() -> BinaryApiTransport {
        BinaryApiTransport::new(TransportConfig {
            host: "127.0.0.1".to_string(),
            port: 65535,
            server_name: "localhost".to_string(),
            use_tls: false,
            connect_timeout: Duration::from_millis(10),
            read_timeout: Duration::from_millis(10),
        })
    }

    #[test]
    fn production_factory_decides_wrap() {
        let f = TransportFactory::new(Environment::Production, ResiliencePolicy::secure_defaults());
        assert_eq!(f.decision(), WrapDecision::Wrap);
        assert!(f.decision().is_wrapped());
    }

    #[test]
    fn dev_and_test_factories_decide_bare() {
        for env in [Environment::Development, Environment::Test] {
            let f = TransportFactory::new(env, ResiliencePolicy::secure_defaults());
            assert_eq!(f.decision(), WrapDecision::Bare);
            assert!(!f.decision().is_wrapped());
        }
    }

    #[test]
    fn production_wrap_binary_returns_some() {
        let f = TransportFactory::new(Environment::Production, ResiliencePolicy::secure_defaults());
        let wrapped = f.wrap_binary(dummy_transport()).expect("policy is valid");
        assert!(wrapped.is_some());
    }

    #[test]
    fn dev_wrap_binary_returns_none() {
        let f = TransportFactory::new(
            Environment::Development,
            ResiliencePolicy::secure_defaults(),
        );
        let wrapped = f.wrap_binary(dummy_transport()).expect("policy is valid");
        assert!(wrapped.is_none());
    }

    #[test]
    fn test_env_wrap_binary_returns_none() {
        let f = TransportFactory::new(Environment::Test, ResiliencePolicy::secure_defaults());
        let wrapped = f.wrap_binary(dummy_transport()).expect("policy is valid");
        assert!(wrapped.is_none());
    }
}
