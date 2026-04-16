//! Plugin / extension loader policy attached to a
//! [`crate::ConfigProfile`].
//!
//! The default posture is "plugins disabled, no capabilities granted". A
//! runtime that wants to load third-party extensions must explicitly flip
//! [`ExtensionPolicy::plugins_enabled`] **and** grant each required
//! capability. Capability flags without `plugins_enabled=true` are
//! rejected by [`ExtensionPolicy::validate`] so operators cannot set
//! "allow everything, loader off" by mistake.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// An ed25519 public key authorized to sign plugin manifests, encoded as
/// exactly 32 raw bytes.
pub type TrustedPluginKey = [u8; 32];

/// Plugin loader policy.
///
/// Persists in `profile.extensions`. Env-var overrides:
///
/// | Env var                              | Field                               |
/// |--------------------------------------|-------------------------------------|
/// | `PCLOUD_PLUGINS_ENABLED`             | `plugins_enabled`                   |
/// | `PCLOUD_PLUGIN_ALLOW_NETWORK`        | `allow_network_capability`          |
/// | `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL`   | `allow_sync_control_capability`     |
/// | `PCLOUD_PLUGIN_ALLOW_CRYPTO`         | `allow_crypto_capability`           |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionPolicy {
    /// Master switch — when `false`, the loader skips the plugin
    /// directory entirely and no plugin can be instantiated. Default:
    /// `false`. Valid values: `true`, `false`. **Security:** this is the
    /// first gate; setting any capability flag below without also
    /// enabling this produces [`ConfigError::InvalidExtensionPolicy`].
    /// Override via `PCLOUD_PLUGINS_ENABLED`. Example:
    /// `plugins_enabled = false`.
    pub plugins_enabled: bool,
    /// Absolute path containing trusted plugin binaries / manifests.
    /// Default: `<root>/plugins` via
    /// [`crate::ConfigProfile::secure_defaults`]. Valid values: any
    /// absolute path ([`ExtensionPolicy::validate`] rejects relative
    /// paths). **Security:** the directory itself must be owner-writable
    /// only; the loader does not re-check its mode, so the path should
    /// already live under the owner-only `config_dir`. Example:
    /// `plugin_dir = "/home/alice/.config/pcloud/pcloud-rs/plugins"`.
    pub plugin_dir: PathBuf,
    /// Permit plugins that declare a `network` capability to open
    /// outbound sockets. Default: `false`. Valid values: `true`, `false`
    /// (rejected unless `plugins_enabled = true`). **Security:** grants
    /// a plugin the ability to exfiltrate data — never enable on
    /// untrusted manifests. Override via
    /// `PCLOUD_PLUGIN_ALLOW_NETWORK`. Example:
    /// `allow_network_capability = false`.
    pub allow_network_capability: bool,
    /// Permit plugins that declare a `sync_control` capability to issue
    /// sync-lifecycle commands (add/remove syncs, start/stop). Default:
    /// `false`. Valid values: `true`, `false` (rejected unless
    /// `plugins_enabled = true`). **Security:** a malicious plugin with
    /// this capability can delete sync roots. Override via
    /// `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL`. Example:
    /// `allow_sync_control_capability = false`.
    pub allow_sync_control_capability: bool,
    /// Permit plugins that declare a `crypto` capability to interact
    /// with crypto-folder primitives (key unlock, fingerprint, etc.).
    /// Default: `false`. Valid values: `true`, `false` (rejected unless
    /// `plugins_enabled = true`). **Security:** the most sensitive
    /// capability — keys live in `SecretBytes`, but a granted plugin
    /// runs in-process and could read them. Override via
    /// `PCLOUD_PLUGIN_ALLOW_CRYPTO`. Example:
    /// `allow_crypto_capability = false`.
    pub allow_crypto_capability: bool,
    /// Ed25519 public keys (32 raw bytes each) authorized to sign
    /// plugin manifests. Default: empty. Valid values: array of 32-byte
    /// arrays. **Security:** when non-empty, the loader **requires** a
    /// valid ed25519 signature over the canonical manifest bytes from
    /// one of these keys; when empty the loader operates in "dev mode"
    /// and logs a warning. A non-empty list also requires
    /// `plugins_enabled = true`. Example:
    /// `trusted_plugin_keys = [[0x1a, 0x2b, ...]]`.
    #[serde(default)]
    pub trusted_plugin_keys: Vec<TrustedPluginKey>,
}

impl ExtensionPolicy {
    /// Construct the secure default posture: plugins disabled, no
    /// capabilities granted, no trusted keys, plugin directory pinned
    /// to `plugin_dir`.
    #[must_use]
    pub fn secure_defaults(plugin_dir: PathBuf) -> Self {
        Self {
            plugins_enabled: false,
            plugin_dir,
            allow_network_capability: false,
            allow_sync_control_capability: false,
            allow_crypto_capability: false,
            trusted_plugin_keys: Vec::new(),
        }
    }

    /// Reject internally inconsistent policies.
    ///
    /// - `plugin_dir` must be absolute.
    /// - Capability flags require `plugins_enabled = true`.
    /// - A non-empty `trusted_plugin_keys` list requires
    ///   `plugins_enabled = true`.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.plugin_dir.is_absolute() {
            return Err(ConfigError::InvalidExtensionPolicy(
                "plugin_dir must be absolute",
            ));
        }

        if !self.plugins_enabled
            && (self.allow_network_capability
                || self.allow_sync_control_capability
                || self.allow_crypto_capability)
        {
            return Err(ConfigError::InvalidExtensionPolicy(
                "capability grants require plugins_enabled=true",
            ));
        }

        if !self.plugins_enabled && !self.trusted_plugin_keys.is_empty() {
            return Err(ConfigError::InvalidExtensionPolicy(
                "trusted_plugin_keys requires plugins_enabled=true",
            ));
        }

        Ok(())
    }

    /// Returns true when a trusted-key list is configured and signature
    /// verification is therefore mandatory.
    #[must_use]
    pub fn requires_plugin_signature(&self) -> bool {
        !self.trusted_plugin_keys.is_empty()
    }
}
