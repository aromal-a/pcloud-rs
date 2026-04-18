//! Per-category rate-limit policy attached to a [`crate::ConfigProfile`].
//!
//! Audit finding: expensive daemon operations (bulk public-link listing,
//! integrity run-once, snapshot create) were not rate-limited at the IPC
//! layer, so a chatty client could exhaust daemon work budgets. The
//! policy in this module exposes three coarse categories and one per-
//! category token-bucket budget each. The daemon's
//! `pcloud_daemon::rate_limit` module consumes these values at IPC
//! dispatch time.
//!
//! All three buckets are **per session** (per connected IPC peer). The
//! defaults are conservative but not crippling:
//!
//! - [`RateCategory::Cheap`] — unlimited (status, userinfo, field
//!   selectors). Capacity and refill are therefore ignored; the policy
//!   ships with `0/0` placeholders.
//! - [`RateCategory::Medium`] — 30 requests / minute per session
//!   (sync-list, list-links, single-item show).
//! - [`RateCategory::Expensive`] — 6 requests / minute per session
//!   (snapshot create, integrity run-once, bulk public-link
//!   operations, tree-link create, change-crypto-password).
//!
//! Operators can override any category via the `[rate_limit]` config
//! section; zero capacity disables the bucket for that category. See
//! `docs/book/src/reference/config.md` for the operator-facing
//! documentation.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Burst capacity for the [`RateCategory::AuthAttempt`] bucket.
///
/// 10 attempts before the bucket is exhausted; replenishes at
/// [`AUTH_ATTEMPT_REFILL_PER_SEC`] tokens/second.
pub const AUTH_ATTEMPT_CAPACITY: u32 = 10;

/// Refill rate for the [`RateCategory::AuthAttempt`] bucket: 5 tokens
/// per minute, expressed as tokens/second.
pub const AUTH_ATTEMPT_REFILL_PER_SEC: f64 = 5.0 / 60.0;

/// Coarse category used to assign a request to a rate-limit bucket.
///
/// The daemon dispatcher classifies every decoded `Request` into one of
/// these four buckets before calling the backend; the classifier is
/// deliberately conservative — anything it does not recognize falls into
/// [`RateCategory::Medium`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateCategory {
    /// No rate limit applied. Reserved for status probes, `GetUserInfo`,
    /// field selectors, and other cheap read-only ops.
    Cheap,
    /// Moderate per-session burst (default: 30/min). Applied to
    /// list-style endpoints and single-item reads.
    Medium,
    /// Strict per-session burst (default: 6/min). Applied to resource-
    /// intensive operations: snapshot create, integrity run-once, bulk
    /// public-link operations, tree-link create, crypto password change.
    Expensive,
    /// Auth-specific burst (default: 10 attempts, refill 5/min). Applied
    /// to credential submission, TFA, crypto unlock, and account password
    /// change operations to limit brute-force exposure through the IPC.
    AuthAttempt,
}

/// Per-category bucket parameters. Capacity is the burst size; refill is
/// tokens/second (serialized as `refill_per_sec`).
///
/// Setting `capacity = 0` disables rate-limiting for the category (the
/// policy degrades to "always allow"). This is the documented way for
/// operators to turn off a bucket without removing the config block.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateBucket {
    /// Maximum burst size in tokens. Default depends on category.
    pub capacity: u32,
    /// Sustained refill rate in tokens per second.
    pub refill_per_sec: f64,
}

impl RateBucket {
    /// Disabled bucket (used as the default for [`RateCategory::Cheap`]).
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            capacity: 0,
            refill_per_sec: 0.0,
        }
    }

    /// Build a per-minute bucket: capacity of `n` tokens refilling back to
    /// full over 60 seconds.
    #[must_use]
    pub fn per_minute(n: u32) -> Self {
        Self {
            capacity: n,
            refill_per_sec: f64::from(n) / 60.0,
        }
    }

    /// `true` when this bucket enforces a limit.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.capacity > 0 && self.refill_per_sec.is_finite() && self.refill_per_sec > 0.0
    }
}

/// Top-level rate-limit policy for the daemon IPC dispatcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitPolicy {
    /// Master switch. Default: `true`. When `false`, every category
    /// degrades to "always allow".
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Bucket for the [`RateCategory::Cheap`] category.
    #[serde(default = "RateBucket::disabled")]
    pub cheap: RateBucket,

    /// Bucket for the [`RateCategory::Medium`] category. Default: 30/min.
    #[serde(default = "default_medium")]
    pub medium: RateBucket,

    /// Bucket for the [`RateCategory::Expensive`] category. Default: 6/min.
    #[serde(default = "default_expensive")]
    pub expensive: RateBucket,

    /// Bucket for the [`RateCategory::AuthAttempt`] category.
    /// Default: 10 attempts, refill 5/min.
    #[serde(default = "default_auth_attempt")]
    pub auth_attempt: RateBucket,
}

fn default_enabled() -> bool {
    true
}

fn default_medium() -> RateBucket {
    RateBucket::per_minute(30)
}

fn default_expensive() -> RateBucket {
    RateBucket::per_minute(6)
}

fn default_auth_attempt() -> RateBucket {
    RateBucket {
        capacity: AUTH_ATTEMPT_CAPACITY,
        refill_per_sec: AUTH_ATTEMPT_REFILL_PER_SEC,
    }
}

impl Default for RateLimitPolicy {
    fn default() -> Self {
        Self::secure_defaults()
    }
}

impl RateLimitPolicy {
    /// Conservative-but-enabled default policy.
    #[must_use]
    pub fn secure_defaults() -> Self {
        Self {
            enabled: true,
            cheap: RateBucket::disabled(),
            medium: default_medium(),
            expensive: default_expensive(),
            auth_attempt: default_auth_attempt(),
        }
    }

    /// Return the bucket for the given category.
    #[must_use]
    pub fn bucket(&self, category: RateCategory) -> RateBucket {
        match category {
            RateCategory::Cheap => self.cheap,
            RateCategory::Medium => self.medium,
            RateCategory::Expensive => self.expensive,
            RateCategory::AuthAttempt => self.auth_attempt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_audit_target() {
        let p = RateLimitPolicy::default();
        assert!(p.enabled);
        // Cheap is uncapped (disabled bucket).
        assert!(!p.cheap.is_enabled());
        // 30/min medium.
        assert_eq!(p.medium.capacity, 30);
        assert!((p.medium.refill_per_sec - 0.5).abs() < 1e-6);
        // 6/min expensive.
        assert_eq!(p.expensive.capacity, 6);
        assert!((p.expensive.refill_per_sec - 0.1).abs() < 1e-6);
    }

    #[test]
    fn bucket_selection_by_category() {
        let p = RateLimitPolicy::default();
        assert_eq!(p.bucket(RateCategory::Cheap), p.cheap);
        assert_eq!(p.bucket(RateCategory::Medium), p.medium);
        assert_eq!(p.bucket(RateCategory::Expensive), p.expensive);
    }

    #[test]
    fn disabled_bucket_is_not_enforced() {
        let b = RateBucket::disabled();
        assert!(!b.is_enabled());
    }

    #[test]
    fn per_minute_helper_derives_refill() {
        let b = RateBucket::per_minute(60);
        assert_eq!(b.capacity, 60);
        assert!((b.refill_per_sec - 1.0).abs() < 1e-6);
    }

    #[test]
    fn serde_roundtrip_preserves_values() {
        let p = RateLimitPolicy::default();
        let j = serde_json::to_string(&p).unwrap();
        let back: RateLimitPolicy = serde_json::from_str(&j).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn serde_defaults_fill_missing_fields() {
        // An empty object must deserialize into secure_defaults (via
        // per-field #[serde(default = "...")] attributes).
        let back: RateLimitPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(back, RateLimitPolicy::secure_defaults());
    }
}
