//! Tier-2 HA configuration block.
//!
//! The `[ha]` table selects the active-passive daemon handoff mode
//! documented in `docs/enterprise/ha.md` §4.2. Defaults are
//! backwards-compatible: HA is **disabled** unless an operator opts in
//! by setting `enabled = true`.

use serde::{Deserialize, Serialize};

/// Handoff posture for a daemon when it fails to acquire the Tier-2
/// file-lock lease. Serialises lowercase in JSON / TOML to match the
/// operator-facing docs (`mode = "passive"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HaContendedMode {
    /// Refuse to start when the lease is already held. Emits a clear
    /// error naming the primary so systemd / an operator can
    /// investigate before retrying. This is the safest default once
    /// HA is enabled.
    #[default]
    Refuse,
    /// Bind the IPC socket and reject every incoming request with
    /// `pcloud_ipc::ResponseStatus::Unavailable` + a message that
    /// names the primary. Poll the lease every
    /// [`crate::ha::HaPolicy::passive_poll_interval_secs`] and
    /// promote to primary if the lease is released.
    Passive,
}

/// Top-level `[ha]` config block. Opt-in; when `enabled = false` the
/// daemon behaves exactly as if this module did not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HaPolicy {
    /// Turn the Tier-2 file-lock lease on. Default: `false`.
    /// When `false`, the other fields in this block are ignored.
    #[serde(default)]
    pub enabled: bool,
    /// Posture when the lease is already held at startup. Default:
    /// [`HaContendedMode::Refuse`].
    #[serde(default)]
    pub mode: HaContendedMode,
    /// Heartbeat cadence for the primary in seconds. The primary
    /// re-writes the lease file metadata at this interval so
    /// observers can see a rolling `last_heartbeat_unix`. Default:
    /// `30`. Values below 1 are clamped to 1 at runtime.
    #[serde(default = "default_heartbeat")]
    pub heartbeat_interval_secs: u64,
    /// Poll cadence for a passive daemon in seconds. Default: `10`.
    /// Lower values detect a released lease faster at the cost of
    /// more filesystem churn.
    #[serde(default = "default_poll")]
    pub passive_poll_interval_secs: u64,
}

fn default_heartbeat() -> u64 {
    30
}

fn default_poll() -> u64 {
    10
}

impl Default for HaPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: HaContendedMode::Refuse,
            heartbeat_interval_secs: default_heartbeat(),
            passive_poll_interval_secs: default_poll(),
        }
    }
}

impl HaPolicy {
    /// Secure defaults: HA disabled. Matches
    /// [`HaPolicy::default`] — provided as a named constructor so
    /// call-sites in `ConfigProfile::secure_defaults` read uniformly
    /// with the other `secure_defaults` helpers in this crate.
    #[must_use]
    pub fn secure_defaults() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled_refuse_mode() {
        let p = HaPolicy::default();
        assert!(!p.enabled);
        assert_eq!(p.mode, HaContendedMode::Refuse);
        assert_eq!(p.heartbeat_interval_secs, 30);
        assert_eq!(p.passive_poll_interval_secs, 10);
    }

    #[test]
    fn serde_roundtrip() {
        let p = HaPolicy {
            enabled: true,
            mode: HaContendedMode::Passive,
            heartbeat_interval_secs: 15,
            passive_poll_interval_secs: 5,
        };
        let json = serde_json::to_string(&p).expect("encode");
        assert!(json.contains("\"passive\""));
        let back: HaPolicy = serde_json::from_str(&json).expect("decode");
        assert_eq!(p, back);
    }

    #[test]
    fn backcompat_missing_block_decodes() {
        // Older envelopes lack `[ha]`. Ensure `#[serde(default)]` on
        // the ConfigProfile side falls back to this block's default.
        let p: HaPolicy = serde_json::from_str("{}").expect("decode empty");
        assert!(!p.enabled);
    }
}
