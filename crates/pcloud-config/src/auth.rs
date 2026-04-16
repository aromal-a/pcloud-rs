//! Authentication-subsystem configuration attached to a
//! [`crate::ConfigProfile`].
//!
//! Currently exposes exactly one knob: [`AuthPolicy::backend`], which
//! selects the platform-native auth-token vault backend used by the
//! daemon. The section is intentionally optional on disk — older
//! envelopes (v1/v2) that predate it still load cleanly via
//! `#[serde(default)]` and pick up the [`VaultBackend::Auto`] default.
//!
//! # Security posture
//!
//! - The vault backend never holds a plaintext token on the heap; every
//!   backend wraps its output in `pcloud_secret::secret_string::SecretString`
//!   so zeroization on drop applies.
//! - Durable auth-token persistence remains opt-in (see
//!   [`crate::features::FeatureFlags::durable_auth_tokens_enabled`]) —
//!   choosing a backend here is a no-op unless that flag is `true`.
//! - `Auto` falls back to the universal [`VaultBackend::File`] only when
//!   the platform-native backend fails to initialise. Explicit values
//!   are honoured verbatim and never fall back silently.

// **PLATFORM:** all (portable declaration; runtime selection is
// platform-aware and lives in `pcloud-daemon::bootstrap`).
// **GATING:** none.

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// Named auth-token vault backends.
///
/// Persists in the envelope as `profile.auth.backend` (string). Accepted
/// values: `"auto"`, `"file"`, `"keychain"`, `"dpapi"`, `"secret-service"`
/// (case-insensitive on the env-var override path; serde uses the
/// kebab-case values verbatim).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum VaultBackend {
    /// Pick the platform-appropriate backend at runtime, falling back to
    /// [`VaultBackend::File`] if the platform-native backend fails to
    /// initialise.
    ///
    /// Current mapping:
    ///
    /// | Platform        | Chosen backend      |
    /// |-----------------|---------------------|
    /// | macOS           | [`VaultBackend::Keychain`] |
    /// | Windows         | [`VaultBackend::Dpapi`]    |
    /// | Linux           | [`VaultBackend::SecretService`] (with file fallback) |
    /// | FreeBSD/OpenBSD/NetBSD | [`VaultBackend::File`] |
    /// | Other Unix      | [`VaultBackend::File`] |
    #[default]
    Auto,
    /// Universal file-backed vault at `<config_dir>/auth_token` with
    /// mode `0600`. Available on every platform.
    File,
    /// macOS login-keychain backend (`security-framework`). Available
    /// only when the build target is `macos`. Attempting to select this
    /// backend on any other platform is a hard error at bootstrap time.
    Keychain,
    /// Windows DPAPI backend (`CryptProtectData` / `CryptUnprotectData`).
    /// Available only when the build target is `windows`. Attempting to
    /// select this backend on any other platform is a hard error at
    /// bootstrap time.
    Dpapi,
    /// Freedesktop Secret Service backend (`secret-service` crate).
    /// Available only when the build target is `linux`. Attempting to
    /// select this backend on any other platform is a hard error at
    /// bootstrap time.
    #[serde(rename = "secret-service")]
    SecretService,
}

impl VaultBackend {
    /// Stable kebab-case name for diagnostics / logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::File => "file",
            Self::Keychain => "keychain",
            Self::Dpapi => "dpapi",
            Self::SecretService => "secret-service",
        }
    }

    /// Parse the env-var / CLI string form. Accepts kebab-case and a few
    /// tolerant aliases (`"secretservice"`, `"ss"`, `"mac"`, `"win"`).
    /// Empty / whitespace-only values are rejected here; callers should
    /// filter those out upstream via `optional_env`.
    pub fn parse(name: &'static str, value: &str) -> Result<Self, ConfigError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Ok(Self::Auto),
            "file" => Ok(Self::File),
            "keychain" | "mac" | "macos" => Ok(Self::Keychain),
            "dpapi" | "win" | "windows" => Ok(Self::Dpapi),
            "secret-service" | "secretservice" | "ss" => Ok(Self::SecretService),
            _ => Err(ConfigError::InvalidEnvironmentValue {
                name,
                value: value.to_owned(),
            }),
        }
    }
}

/// Auth-subsystem policy block.
///
/// # Example envelope fragment
///
/// ```json
/// "auth": { "backend": "auto" }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPolicy {
    /// Selected auth-token vault backend. Defaults to
    /// [`VaultBackend::Auto`] so the daemon picks the platform-native
    /// backend for the current host.
    #[serde(default)]
    pub backend: VaultBackend,

    /// How often (in seconds) the background session-refresh thread
    /// wakes to check whether the current auth token needs proactive
    /// renewal. Default: 300 (5 min). Set to 0 to disable the
    /// background refresh loop entirely (not recommended for
    /// long-running daemons).
    #[serde(default = "default_refresh_check_interval_secs")]
    pub refresh_check_interval_secs: u64,

    /// How many seconds before token expiry the daemon should attempt
    /// a proactive refresh. Translated into the
    /// `pcloud_auth::RefreshPolicy` `refresh_threshold` fraction at
    /// bootstrap time as `1.0 - (margin / lifetime)`. Default: 600
    /// (10 min before expiry, given a 1h token lifetime this yields a
    /// threshold of ~0.83).
    #[serde(default = "default_refresh_margin_secs")]
    pub refresh_margin_secs: u64,
}

const fn default_refresh_check_interval_secs() -> u64 {
    300
}
const fn default_refresh_margin_secs() -> u64 {
    600
}

impl Default for AuthPolicy {
    fn default() -> Self {
        Self {
            backend: VaultBackend::Auto,
            refresh_check_interval_secs: default_refresh_check_interval_secs(),
            refresh_margin_secs: default_refresh_margin_secs(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_auto() {
        let p = AuthPolicy::default();
        assert_eq!(p.backend, VaultBackend::Auto);
    }

    #[test]
    fn serde_roundtrip_kebab_case() {
        let p = AuthPolicy {
            backend: VaultBackend::SecretService,
            ..AuthPolicy::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        assert!(
            s.contains("secret-service"),
            "secret-service must serialize kebab-case: {s}"
        );
        let back: AuthPolicy = serde_json::from_str(&s).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn serde_accepts_missing_backend() {
        // `#[serde(default)]` on `backend` must tolerate `{}`.
        let p: AuthPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(p.backend, VaultBackend::Auto);
    }

    #[test]
    fn parse_accepts_canonical_names() {
        for (s, expected) in [
            ("auto", VaultBackend::Auto),
            ("file", VaultBackend::File),
            ("keychain", VaultBackend::Keychain),
            ("dpapi", VaultBackend::Dpapi),
            ("secret-service", VaultBackend::SecretService),
        ] {
            assert_eq!(
                VaultBackend::parse("PCLOUD_VAULT", s).unwrap(),
                expected,
                "canonical name {s} must parse"
            );
        }
    }

    #[test]
    fn parse_accepts_aliases_case_insensitive() {
        assert_eq!(
            VaultBackend::parse("PCLOUD_VAULT", "KEYCHAIN").unwrap(),
            VaultBackend::Keychain
        );
        assert_eq!(
            VaultBackend::parse("PCLOUD_VAULT", "SecretService").unwrap(),
            VaultBackend::SecretService
        );
        assert_eq!(
            VaultBackend::parse("PCLOUD_VAULT", "ss").unwrap(),
            VaultBackend::SecretService
        );
        assert_eq!(
            VaultBackend::parse("PCLOUD_VAULT", "windows").unwrap(),
            VaultBackend::Dpapi
        );
    }

    #[test]
    fn parse_rejects_unknown_backend() {
        let err = VaultBackend::parse("PCLOUD_VAULT", "yubikey").unwrap_err();
        match err {
            ConfigError::InvalidEnvironmentValue { name, value } => {
                assert_eq!(name, "PCLOUD_VAULT");
                assert_eq!(value, "yubikey");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn as_str_is_stable() {
        assert_eq!(VaultBackend::Auto.as_str(), "auto");
        assert_eq!(VaultBackend::File.as_str(), "file");
        assert_eq!(VaultBackend::Keychain.as_str(), "keychain");
        assert_eq!(VaultBackend::Dpapi.as_str(), "dpapi");
        assert_eq!(VaultBackend::SecretService.as_str(), "secret-service");
    }
}
