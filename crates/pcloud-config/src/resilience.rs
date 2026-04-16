//! Resilience policy attached to a [`crate::ConfigProfile`].
//!
//! Controls the opt-in rate limiter, circuit breaker, and retry policy used
//! by `pcloud-proto`'s `ResilientTransport`. The default is a conservative
//! **enabled** posture: modest per-endpoint burst, short circuit breaker,
//! and a small number of retries with exponential backoff.
//!
//! Existing direct-dispatch transports are untouched: the policy is only
//! consulted when a caller explicitly opts in by wrapping a transport.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration block for the resilience wrapper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResiliencePolicy {
    /// Master switch for the resilience wrapper. Default: `true`. Valid
    /// values: `true`, `false`. **Security:** disabling removes the
    /// client-side rate limit and circuit breaker, which can cause the
    /// daemon to hammer a failing endpoint; prefer tuning the fields
    /// below over turning it off wholesale. Example: `enabled = true`.
    pub enabled: bool,

    /// Per-endpoint token-bucket capacity (burst size). Default: `16`.
    /// Valid values: `u32 >= 1`. **Security:** caps the instantaneous
    /// request rate per endpoint so a buggy consumer cannot trip
    /// server-side abuse heuristics and get the account rate-limited
    /// globally. Example: `rate_limit_capacity = 16`.
    pub rate_limit_capacity: u32,
    /// Per-endpoint token-bucket refill rate in tokens/second. Default:
    /// `8.0`. Valid values: positive finite `f64`. **Security:** sets
    /// the sustained request rate (after the burst is drained).
    /// Example: `rate_limit_refill_per_sec = 8.0`.
    pub rate_limit_refill_per_sec: f64,

    /// Consecutive failures required to trip the breaker to Open.
    /// Default: `5`. Valid values: `u32 >= 1`. **Security:** lower
    /// values fail faster when an endpoint degrades; higher values are
    /// more tolerant of transient blips. Example:
    /// `breaker_failure_threshold = 5`.
    pub breaker_failure_threshold: u32,
    /// Duration the breaker stays Open before admitting a probe, in
    /// milliseconds. Default: `30_000`. Valid values: any `u64`.
    /// **Security:** the recovery window — too short and the breaker
    /// thrashes; too long and transient outages look permanent.
    /// Example: `breaker_reset_timeout_ms = 30000`.
    pub breaker_reset_timeout_ms: u64,

    /// Total attempts including the first call. Default: `3`. Valid
    /// values: `u32 >= 1`; `1` disables retries entirely. **Security:**
    /// bounds the amplification factor a single request can generate
    /// against a failing endpoint. Example: `retry_max_attempts = 3`.
    pub retry_max_attempts: u32,
    /// Initial retry delay in milliseconds (before exponential backoff).
    /// Default: `100`. Valid values: any `u64`. **Security:** too low
    /// causes burst retries on transient errors. Example:
    /// `retry_base_delay_ms = 100`.
    pub retry_base_delay_ms: u64,
    /// Exponential backoff factor. Default: `2.0`. Valid values: finite
    /// `f64 >= 1.0`. **Security:** `1.0` disables exponential growth
    /// (constant delay); `< 1.0` is nonsensical and will be rejected by
    /// the transport wrapper. Example: `retry_factor = 2.0`.
    pub retry_factor: f64,
    /// Upper bound on a single retry delay in milliseconds. Default:
    /// `5_000`. Valid values: any `u64`. **Security:** caps the
    /// exponential growth so retries never wait minutes on a long-lived
    /// outage. Example: `retry_max_delay_ms = 5000`.
    pub retry_max_delay_ms: u64,
    /// Deterministic jitter seed applied via equal-jitter. Default:
    /// `0x00C0_FFEE_F00D`. Valid values: any `u64`. **Security:** keeps
    /// tests reproducible while still spreading retry storms across
    /// clients that share the seed. Example: `retry_jitter_seed = 0`.
    pub retry_jitter_seed: u64,
}

impl Default for ResiliencePolicy {
    fn default() -> Self {
        Self::secure_defaults()
    }
}

impl ResiliencePolicy {
    /// Conservative but enabled defaults. Safe to apply to every endpoint.
    #[must_use]
    pub const fn secure_defaults() -> Self {
        Self {
            enabled: true,
            rate_limit_capacity: 16,
            rate_limit_refill_per_sec: 8.0,
            breaker_failure_threshold: 5,
            breaker_reset_timeout_ms: 30_000,
            retry_max_attempts: 3,
            retry_base_delay_ms: 100,
            retry_factor: 2.0,
            retry_max_delay_ms: 5_000,
            retry_jitter_seed: 0x00C0_FFEE_F00D,
        }
    }

    /// Convert to a Duration for the breaker reset timeout.
    #[must_use]
    pub const fn breaker_reset_timeout(&self) -> Duration {
        Duration::from_millis(self.breaker_reset_timeout_ms)
    }

    /// Convert to a Duration for the retry base delay.
    #[must_use]
    pub const fn retry_base_delay(&self) -> Duration {
        Duration::from_millis(self.retry_base_delay_ms)
    }

    /// Convert to a Duration for the retry max delay.
    #[must_use]
    pub const fn retry_max_delay(&self) -> Duration {
        Duration::from_millis(self.retry_max_delay_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_enabled_and_conservative() {
        let p = ResiliencePolicy::default();
        assert!(p.enabled);
        assert!(p.rate_limit_capacity >= 1);
        assert!(p.rate_limit_refill_per_sec > 0.0);
        assert!(p.breaker_failure_threshold >= 1);
        assert!(p.retry_max_attempts >= 1);
        assert!(p.retry_factor >= 1.0);
    }

    #[test]
    fn duration_helpers_round_trip() {
        let p = ResiliencePolicy::default();
        assert_eq!(
            p.breaker_reset_timeout(),
            Duration::from_millis(p.breaker_reset_timeout_ms)
        );
        assert_eq!(
            p.retry_base_delay(),
            Duration::from_millis(p.retry_base_delay_ms)
        );
        assert_eq!(
            p.retry_max_delay(),
            Duration::from_millis(p.retry_max_delay_ms)
        );
    }

    #[test]
    fn serde_roundtrip() {
        let p = ResiliencePolicy::default();
        let j = serde_json::to_string(&p).unwrap();
        let back: ResiliencePolicy = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }
}
