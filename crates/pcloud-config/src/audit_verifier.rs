//! Scheduled audit-chain verifier configuration.
//!
//! Pure configuration scaffolding for the daemon's periodic self-verification
//! of the tamper-evident `audit_events` hash chain. The runtime side lives
//! in `pcloud_daemon::audit_verifier_service`.
//!
//! ## Security posture
//!
//! - **On by default.** Unlike the integrity sweeper, the audit-chain
//!   verifier performs a read-only walk of an already-persisted table. The
//!   cost is bounded and the signal is load-bearing for tamper detection,
//!   so `enabled = true` is the secure default.
//! - **Fail-closed cron parse.** The daemon refuses to start the scheduler
//!   when `schedule_cron` cannot be parsed, rather than silently never
//!   running.
//! - **No secret material.** Only row counts and id ranges are persisted;
//!   HMAC key material (`PCLOUD_AUDIT_HMAC_KEY`) is pulled from the
//!   environment on each run and never written to disk.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Configuration block for the scheduled audit-chain verifier.
///
/// Persists as `[features.audit_verifier]`. `#[serde(default)]` on every
/// field preserves backward compatibility with envelopes predating this
/// block; the defaults returned by [`AuditVerifierConfig::default`] turn the
/// verifier **on** at 03:00 daily because periodic self-verification is a
/// non-negotiable audit requirement (see runbook §"Responding to audit
/// chain broken").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditVerifierConfig {
    /// Master switch. Default: `true`. Valid values: `true`, `false`.
    /// While `false`, the daemon still honours on-demand `pcloudc audit
    /// verify` calls but does not schedule a background walk.
    ///
    /// **Security:** disabling the scheduled verifier removes the only
    /// automatic tamper-detection path. Operators who opt out must
    /// arrange out-of-band periodic verification to preserve the
    /// non-repudiation property of the audit chain.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Cron expression that drives the verifier thread. Default:
    /// `"0 0 3 * * *"` (03:00 daily, standard 6-field seconds-minutes-
    /// hours-dom-month-dow form). Accepts any expression the `cron`
    /// crate understands.
    ///
    /// **Security:** an invalid cron expression is rejected at
    /// `AuditVerifierShell::from_config` time; the scheduler refuses to
    /// start and the parse error is surfaced to the operator. The
    /// verifier never silently runs on an unparseable schedule.
    #[serde(default = "default_schedule_cron")]
    pub schedule_cron: String,
    /// Optional filesystem path where the verifier persists its
    /// last-known-good checkpoint (`{last_run_ts, last_verified_id}`).
    /// When `None`, every run walks the entire chain from the genesis
    /// row. When `Some`, the verifier resumes from `last_verified_id + 1`
    /// on subsequent runs. The file is `0600`; parent directory is
    /// `0700`.
    ///
    /// **Security:** the checkpoint never contains HMAC key material.
    /// It only records row ids so a truncated history cannot be papered
    /// over by deleting the checkpoint (next run re-walks the full
    /// chain and re-detects the mismatch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_path: Option<PathBuf>,
}

impl Default for AuditVerifierConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            schedule_cron: default_schedule_cron(),
            checkpoint_path: None,
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_schedule_cron() -> String {
    // 03:00 daily, 6-field cron (sec min hour dom mon dow) per the
    // `cron` crate. Operators wanting a different cadence must set the
    // field explicitly; the daemon refuses to start on an unparseable
    // override.
    "0 0 3 * * *".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_on_and_03_00_daily() {
        let cfg = AuditVerifierConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.schedule_cron, "0 0 3 * * *");
        assert!(cfg.checkpoint_path.is_none());
    }

    #[test]
    fn defaults_roundtrip_through_serde() {
        let cfg = AuditVerifierConfig::default();
        let j = serde_json::to_string(&cfg).expect("serialize ok");
        let back: AuditVerifierConfig = serde_json::from_str(&j).expect("deserialize ok");
        assert_eq!(cfg, back);
    }

    #[test]
    fn missing_block_falls_back_to_default() {
        // An empty object must deserialize into the default.
        let cfg: AuditVerifierConfig = serde_json::from_str("{}").expect("empty ok");
        assert_eq!(cfg, AuditVerifierConfig::default());
    }
}
