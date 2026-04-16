//! Product feature flags attached to a [`crate::ConfigProfile`].
//!
//! Flags are conservative by default: every non-essential capability is
//! **off** unless explicitly opted in. None of these flags weaken security
//! directly, but a few (notably [`FeatureFlags::durable_auth_tokens_enabled`])
//! gate features that write long-lived secret material to disk.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use crate::audit_verifier::AuditVerifierConfig;
use crate::integrity_sweeper::IntegritySweeperConfig;

/// Boolean feature toggles for the daemon/SDK.
///
/// Persists in the envelope's `profile.features` object. Every field is
/// required by the schema (no serde defaults). Override mapping:
///
/// | Env var                       | Field                           |
/// |-------------------------------|---------------------------------|
/// | `PCLOUD_DURABLE_AUTH_TOKENS`  | `durable_auth_tokens_enabled`   |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureFlags {
    /// Enable peer-to-peer transfers. Default: `false`. Valid values:
    /// `true`, `false`. **Security:** reserved for future work; no current
    /// code path reads this flag, so toggling it has no runtime effect
    /// today. Keep `false` until the feature lands. Example:
    /// `p2p_enabled = false`.
    pub p2p_enabled: bool,
    /// Enable crypto-folder support (AES-256-GCM sector encryption,
    /// metadata filename encoding, crypto share temppass flow). Default:
    /// `true` in [`crate::ConfigProfile::secure_defaults`]. Valid values:
    /// `true`, `false`. **Security:** disabling this leaves previously
    /// encrypted folders unreachable on the client — do not flip to
    /// `false` to "just see the files" because encrypted blobs will not
    /// be decrypted or emitted through FUSE. Example:
    /// `crypto_enabled = true`.
    pub crypto_enabled: bool,
    /// Enable on-disk persistence of auth tokens via the owner-only vault
    /// at [`crate::paths::ManagedPaths::auth_token_vault_path`]. Default:
    /// `false` — tokens live only in memory unless the user opts in.
    /// Valid values: `true`, `false`. **Security:** when `true`, a
    /// long-lived API token is written to `config_dir/auth_token` (mode
    /// `0600`, parent `0700`). Passwords are *never* persisted regardless
    /// of this flag. Override via `PCLOUD_DURABLE_AUTH_TOKENS=1`. Example:
    /// `durable_auth_tokens_enabled = false`.
    pub durable_auth_tokens_enabled: bool,
    /// Background integrity-sweeper configuration block. Persists as
    /// the `[features.integrity_sweeper]` table. `#[serde(default)]`
    /// preserves backward compatibility with envelopes that predate
    /// this field; the default ([`IntegritySweeperConfig::default`])
    /// is **off and safe** — no background scrubbing happens until an
    /// operator explicitly opts in. Tracked under `bd-1du.4.6.1`.
    #[serde(default)]
    pub integrity_sweeper: IntegritySweeperConfig,
    /// Scheduled audit-chain verifier configuration. Persists as
    /// `[features.audit_verifier]`. `#[serde(default)]` preserves backward
    /// compatibility with envelopes predating this block; the default
    /// ([`AuditVerifierConfig::default`]) is **on** because periodic
    /// self-verification of the tamper-evident audit chain is a
    /// non-negotiable audit requirement (see runbook playbook "Responding
    /// to audit chain broken"). The runtime side lives in
    /// `pcloud_daemon::audit_verifier_service`.
    #[serde(default)]
    pub audit_verifier: AuditVerifierConfig,
}
