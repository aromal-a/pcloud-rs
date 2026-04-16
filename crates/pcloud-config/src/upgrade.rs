//! Upgrade / daemon-handoff configuration attached to a
//! [`crate::ConfigProfile`].
//!
//! This section controls the simpler-than-socket-activation handoff
//! protocol used during rolling binary upgrades:
//!
//! - a new daemon instance, on bootstrap, may observe that an existing
//!   daemon still holds the Tier-2 HA lease
//!   (`pcloud_daemon::ha_lease`) or the IPC socket lock,
//! - instead of bailing out immediately, it waits up to
//!   [`UpgradePolicy::handoff_timeout_secs`] for the old daemon to drain
//!   and release its locks,
//! - if the timeout expires, the new daemon returns a typed bootstrap
//!   error so the supervisor can retry or surface a clear diagnostic.
//!
//! The field is optional on disk: older envelopes load with
//! [`UpgradePolicy::default`] (30 s), matching the documented drain
//! SLO in `docs/book/src/operations/upgrade.md`.
//!
//! # Security posture
//!
//! The handoff protocol is **not** a security boundary: it relies on
//! the same UID-scoped owner-only IPC socket and the Tier-2 HA lease
//! for cross-process cooperation. No secret material flows through
//! these settings.

// **PLATFORM:** all
// **GATING:** none (portable declaration).

use serde::{Deserialize, Serialize};

/// Default handoff timeout, in seconds. Matches the default drain SLO
/// documented in `docs/book/src/operations/upgrade.md`.
pub const DEFAULT_HANDOFF_TIMEOUT_SECS: u32 = 30;

/// Default drain timeout honoured by the serve loop. A second SIGTERM
/// or the expiration of this timer forces an exit.
pub const DEFAULT_DRAIN_TIMEOUT_SECS: u32 = 30;

/// Validated `[upgrade]` section. All knobs are operator-facing and
/// non-secret.
///
/// ```
/// use pcloud_config::upgrade::UpgradePolicy;
/// let p = UpgradePolicy::default();
/// assert_eq!(p.handoff_timeout_secs, 30);
/// assert_eq!(p.drain_timeout_secs, 30);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradePolicy {
    /// Seconds a new daemon waits for the previous daemon's lease and
    /// socket to release during a rolling upgrade. Zero disables the
    /// wait entirely (fail fast); values above 600 are capped by the
    /// validator.
    #[serde(default = "default_handoff_timeout_secs")]
    pub handoff_timeout_secs: u32,
    /// Seconds the serve loop waits for in-flight requests to complete
    /// after SIGTERM before forcing a shutdown. Zero exits as soon as
    /// no dispatch is executing; values above 600 are capped by the
    /// validator.
    #[serde(default = "default_drain_timeout_secs")]
    pub drain_timeout_secs: u32,
}

impl Default for UpgradePolicy {
    fn default() -> Self {
        Self {
            handoff_timeout_secs: DEFAULT_HANDOFF_TIMEOUT_SECS,
            drain_timeout_secs: DEFAULT_DRAIN_TIMEOUT_SECS,
        }
    }
}

impl UpgradePolicy {
    /// Secure defaults — identical to [`UpgradePolicy::default`] today
    /// because no field has a stricter posture than the disk default.
    /// Kept as a distinct constructor so future secure-defaults
    /// divergence has one callsite to update.
    #[must_use]
    pub fn secure_defaults() -> Self {
        Self::default()
    }

    /// Clamp absurd values to the documented maxima. The upper bound
    /// (600 s = 10 min) is generous enough to cover the largest
    /// in-flight upload staging flush we have observed under load and
    /// stops pathological configs from parking the serve loop for
    /// hours. Never returns an error — callers expect a validated
    /// profile after this point.
    pub fn normalise(&mut self) {
        const MAX: u32 = 600;
        if self.handoff_timeout_secs > MAX {
            self.handoff_timeout_secs = MAX;
        }
        if self.drain_timeout_secs > MAX {
            self.drain_timeout_secs = MAX;
        }
    }
}

fn default_handoff_timeout_secs() -> u32 {
    DEFAULT_HANDOFF_TIMEOUT_SECS
}

fn default_drain_timeout_secs() -> u32 {
    DEFAULT_DRAIN_TIMEOUT_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_documentation() {
        let p = UpgradePolicy::default();
        assert_eq!(p.handoff_timeout_secs, 30);
        assert_eq!(p.drain_timeout_secs, 30);
    }

    #[test]
    fn serde_round_trip() {
        let p = UpgradePolicy {
            handoff_timeout_secs: 45,
            drain_timeout_secs: 60,
        };
        let json = serde_json::to_string(&p).expect("encode");
        let decoded: UpgradePolicy = serde_json::from_str(&json).expect("decode");
        assert_eq!(decoded, p);
    }

    #[test]
    fn serde_accepts_missing_fields_via_default() {
        let decoded: UpgradePolicy = serde_json::from_str("{}").expect("decode empty");
        assert_eq!(decoded, UpgradePolicy::default());
    }

    #[test]
    fn normalise_caps_absurd_values() {
        let mut p = UpgradePolicy {
            handoff_timeout_secs: 10_000,
            drain_timeout_secs: 10_000,
        };
        p.normalise();
        assert_eq!(p.handoff_timeout_secs, 600);
        assert_eq!(p.drain_timeout_secs, 600);
    }
}
