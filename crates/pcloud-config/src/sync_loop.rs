//! Background sync-loop configuration.
//!
//! Controls the autonomous background sync loop that drives incremental
//! synchronization of all active sync roots. The loop is the single most
//! important runtime component for making the daemon a *real* sync client
//! rather than a purely IPC-reactive shell.
//!
//! ## Defaults
//!
//! - `enabled = true` — the loop starts automatically at bootstrap.
//! - `poll_interval_secs = 30` — one full cycle every 30 seconds.
//! - `batch_size = 100` — at most 100 files per transfer batch.
//! - `max_concurrent_transfers = 4` — parallel upload/download slots.
//!
//! ## Validation
//!
//! [`SyncLoopConfig::validate`] enforces:
//! - `poll_interval_secs` in `[5, 3600]`.
//! - `batch_size` in `[1, 10_000]`.
//! - `max_concurrent_transfers` in `[1, 64]`.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Minimum poll interval in seconds.
pub const MIN_POLL_INTERVAL_SECS: u64 = 5;
/// Maximum poll interval in seconds.
pub const MAX_POLL_INTERVAL_SECS: u64 = 3600;
/// Default poll interval in seconds.
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 30;
/// Default batch size.
pub const DEFAULT_BATCH_SIZE: usize = 100;
/// Default concurrent transfer limit.
pub const DEFAULT_MAX_CONCURRENT_TRANSFERS: usize = 4;

/// Default full-scan interval in seconds (5 minutes).
pub const DEFAULT_FULL_SCAN_INTERVAL_SECS: u64 = 300;

/// Configuration for the background sync loop.
///
/// Persisted as the `[sync]` section of the config profile envelope.
///
/// # Serde invariant
///
/// All fields have `#[serde(default)]` or struct-level `Default`, so
/// older envelopes that lack a `[sync]` section deserialize cleanly
/// via the struct default.
///
/// # Example
///
/// ```
/// use pcloud_config::sync_loop::SyncLoopConfig;
/// let cfg = SyncLoopConfig::default();
/// assert!(cfg.enabled);
/// assert_eq!(cfg.poll_interval_secs, 30);
/// assert!(cfg.propagate_deletes);
/// assert_eq!(cfg.full_scan_interval_secs, 300);
/// assert_eq!(cfg.conflict_policy, "rename_both");
/// assert_eq!(cfg.upload_chunk_size, 10 * 1024 * 1024);
/// cfg.validate().unwrap();
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncLoopConfig {
    /// Whether the background sync loop is enabled. When `false` the
    /// daemon starts without spawning the sync thread.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Seconds between successive full sync cycles. Clamped to
    /// `[5, 3600]` by validation.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// Maximum number of files to include in a single transfer batch.
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
    /// Maximum number of concurrent upload/download operations.
    #[serde(default = "default_max_concurrent_transfers")]
    pub max_concurrent_transfers: usize,
    /// Whether to propagate deletions during sync. When `false`, delete
    /// operations are never emitted regardless of `SyncType` — ultra-safe
    /// mode for environments where accidental data loss is unacceptable.
    /// Default: `true`.
    #[serde(default = "default_propagate_deletes")]
    pub propagate_deletes: bool,
    /// Seconds between full local filesystem tree walks. Between full
    /// scans the engine relies on filesystem watcher events (when
    /// available) or skips local scanning entirely. Clamped to
    /// `[30, 86400]` by validation. Default: `300` (5 minutes).
    #[serde(default = "default_full_scan_interval")]
    pub full_scan_interval_secs: u64,
    /// Policy applied when the planner detects a local/remote conflict.
    /// One of `"newest_wins"`, `"rename_both"`, `"error"`,
    /// `"prefer_local"`, `"prefer_remote"`, `"manual_review"`.
    /// Default: `"rename_both"`.
    #[serde(default = "default_conflict_policy")]
    pub conflict_policy: String,
    /// Upload chunk size in bytes for the chunked upload path.
    /// Files larger than this threshold are uploaded in multiple
    /// `upload_write` round-trips. Default: 10 MiB.
    #[serde(default = "default_upload_chunk_size")]
    pub upload_chunk_size: usize,
    /// **audit-06 M-4.1 (opt-in).** When `true`, the daemon's sync loop
    /// consults the platform power-source reader at the start of each
    /// cycle and skips the cycle while the host reports running on
    /// battery. Default: `false` so existing deployments observe no
    /// behavioural change. Linux uses `/sys/class/power_supply` (no
    /// extra deps); other platforms currently treat the state as
    /// `Unknown` (i.e. "do not pause") because the engine crate
    /// intentionally does not pull `battery`/`starship-battery` —
    /// platform-specific delegation can be wired by the daemon if
    /// required. See `pcloud_engine` (the `power` module) for the
    /// reader trait — the path is not a published intra-doc link
    /// because `pcloud_engine` is a sibling crate not re-exported
    /// from `pcloud-config`.
    #[serde(default)]
    pub pause_on_battery: bool,
    /// **T2.1.c (plan-side only).** Minimum local-file size, in bytes,
    /// at which the planner attempts a differential ("rsync-style")
    /// upload. Files strictly smaller than this threshold always go
    /// down the full-upload path because the rolling-hash + signature
    /// overhead exceeds the bandwidth saved on small payloads.
    ///
    /// Default: `4 * 1024 * 1024` (4 MiB) — matches the
    /// [`DEFAULT_DIFFERENTIAL_THRESHOLD_BYTES`] constant.
    ///
    /// Note: as of T2.1.c the field controls *planning* only. The
    /// engine computes a delta and stores it on the planned operation
    /// for later execution; actual upload-via-`upload_writefromfile`
    /// is gated on upstream byte-range API parity (T2.1 follow-up).
    #[serde(default = "default_differential_threshold_bytes")]
    pub differential_threshold_bytes: u64,
}

impl Default for SyncLoopConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            poll_interval_secs: default_poll_interval(),
            batch_size: default_batch_size(),
            max_concurrent_transfers: default_max_concurrent_transfers(),
            propagate_deletes: default_propagate_deletes(),
            full_scan_interval_secs: default_full_scan_interval(),
            conflict_policy: default_conflict_policy(),
            upload_chunk_size: default_upload_chunk_size(),
            pause_on_battery: false,
            differential_threshold_bytes: default_differential_threshold_bytes(),
        }
    }
}

fn default_enabled() -> bool {
    true
}
fn default_poll_interval() -> u64 {
    DEFAULT_POLL_INTERVAL_SECS
}
fn default_batch_size() -> usize {
    DEFAULT_BATCH_SIZE
}
fn default_max_concurrent_transfers() -> usize {
    DEFAULT_MAX_CONCURRENT_TRANSFERS
}
fn default_propagate_deletes() -> bool {
    true
}
fn default_full_scan_interval() -> u64 {
    DEFAULT_FULL_SCAN_INTERVAL_SECS
}
fn default_conflict_policy() -> String {
    "rename_both".to_owned()
}
/// Default upload chunk size: 10 MiB.
pub const DEFAULT_UPLOAD_CHUNK_SIZE: usize = 10 * 1024 * 1024;
fn default_upload_chunk_size() -> usize {
    DEFAULT_UPLOAD_CHUNK_SIZE
}

/// Default differential-sync threshold: 4 MiB.
///
/// Files strictly smaller than this size skip the rolling-hash /
/// signature path because the per-block hashing overhead exceeds the
/// bandwidth saved by a partial upload. The constant is exposed so the
/// engine planner and tests can share the same default without
/// re-hardcoding.
pub const DEFAULT_DIFFERENTIAL_THRESHOLD_BYTES: u64 = 4 * 1024 * 1024;
fn default_differential_threshold_bytes() -> u64 {
    DEFAULT_DIFFERENTIAL_THRESHOLD_BYTES
}

impl SyncLoopConfig {
    /// Validate the sync loop config bounds.
    ///
    /// # Errors
    ///
    /// Returns a static description of the first violation.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.poll_interval_secs < MIN_POLL_INTERVAL_SECS
            || self.poll_interval_secs > MAX_POLL_INTERVAL_SECS
        {
            return Err("sync.poll_interval_secs must be between 5 and 3600");
        }
        if self.batch_size == 0 || self.batch_size > 10_000 {
            return Err("sync.batch_size must be between 1 and 10000");
        }
        if self.max_concurrent_transfers == 0 || self.max_concurrent_transfers > 64 {
            return Err("sync.max_concurrent_transfers must be between 1 and 64");
        }
        if self.full_scan_interval_secs < 30 || self.full_scan_interval_secs > 86400 {
            return Err("sync.full_scan_interval_secs must be between 30 and 86400");
        }
        let valid_policies = [
            "newest_wins",
            "rename_both",
            "error",
            "prefer_local",
            "prefer_remote",
            "manual_review",
        ];
        if !valid_policies.contains(&self.conflict_policy.as_str()) {
            return Err(
                "sync.conflict_policy must be one of: newest_wins, rename_both, error, prefer_local, prefer_remote, manual_review",
            );
        }
        if self.upload_chunk_size < 256 * 1024 || self.upload_chunk_size > 256 * 1024 * 1024 {
            return Err("sync.upload_chunk_size must be between 256KiB and 256MiB");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid() {
        let cfg = SyncLoopConfig::default();
        cfg.validate().unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.poll_interval_secs, 30);
        assert_eq!(cfg.batch_size, 100);
        assert_eq!(cfg.max_concurrent_transfers, 4);
        assert!(cfg.propagate_deletes);
        assert_eq!(cfg.full_scan_interval_secs, 300);
        assert_eq!(cfg.conflict_policy, "rename_both");
        assert_eq!(cfg.upload_chunk_size, 10 * 1024 * 1024);
    }

    #[test]
    fn rejects_zero_poll_interval() {
        let cfg = SyncLoopConfig {
            poll_interval_secs: 0,
            ..SyncLoopConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_excessive_poll_interval() {
        let cfg = SyncLoopConfig {
            poll_interval_secs: 7200,
            ..SyncLoopConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_zero_batch_size() {
        let cfg = SyncLoopConfig {
            batch_size: 0,
            ..SyncLoopConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_zero_concurrent_transfers() {
        let cfg = SyncLoopConfig {
            max_concurrent_transfers: 0,
            ..SyncLoopConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_low_full_scan_interval() {
        let cfg = SyncLoopConfig {
            full_scan_interval_secs: 10,
            ..SyncLoopConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_excessive_full_scan_interval() {
        let cfg = SyncLoopConfig {
            full_scan_interval_secs: 100_000,
            ..SyncLoopConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn propagate_deletes_false_is_valid() {
        let cfg = SyncLoopConfig {
            propagate_deletes: false,
            ..SyncLoopConfig::default()
        };
        cfg.validate().unwrap();
    }

    #[test]
    fn serde_roundtrip() {
        let cfg = SyncLoopConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: SyncLoopConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn missing_section_deserializes_to_default() {
        let back: SyncLoopConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(back, SyncLoopConfig::default());
    }

    #[test]
    fn pause_on_battery_default_is_false() {
        let cfg = SyncLoopConfig::default();
        assert!(!cfg.pause_on_battery);
    }

    #[test]
    fn differential_threshold_default_is_four_mib() {
        let cfg = SyncLoopConfig::default();
        assert_eq!(cfg.differential_threshold_bytes, 4 * 1024 * 1024);
    }

    #[test]
    fn differential_threshold_missing_field_deserializes_to_default() {
        // Older envelopes that pre-date T2.1.c omit the field
        // entirely; serde must fall back to the default constant.
        let json = serde_json::to_string(&SyncLoopConfig::default()).unwrap();
        assert!(json.contains("differential_threshold_bytes"));
        let back: SyncLoopConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(
            back.differential_threshold_bytes,
            DEFAULT_DIFFERENTIAL_THRESHOLD_BYTES
        );
    }

    #[test]
    fn pause_on_battery_roundtrips() {
        let cfg = SyncLoopConfig {
            pause_on_battery: true,
            ..SyncLoopConfig::default()
        };
        cfg.validate().unwrap();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: SyncLoopConfig = serde_json::from_str(&json).unwrap();
        assert!(back.pause_on_battery);
    }
}
