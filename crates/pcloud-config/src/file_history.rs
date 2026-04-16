//! Revision-history configuration attached to a [`crate::ConfigProfile`].
//!
//! Populates the pluggable `RevisionProvider` wired by the daemon. The
//! section is intentionally optional on disk so older envelopes still
//! load unchanged — the default produces a `NullRevisionProvider`,
//! which makes `pcloudc log / diff / restore` return a structured
//! "not configured" response with an actionable remediation pointer.
//!
//! # Security posture
//!
//! - In [`crate::Environment::Production`] the URL must be `https://`;
//!   plaintext URLs are refused with [`crate::ConfigError::InvalidApiEndpoint`].
//! - Non-production profiles accept `http://` URLs so integration tests
//!   can target mock servers, but still validate URL well-formedness.
//! - No secret material is carried in this section. The URL is treated
//!   as operator-visible infrastructure metadata, not a credential.

// **PLATFORM:** all (portable declaration; runtime wiring lives in the
// daemon crate).
// **GATING:** none.

use serde::{Deserialize, Serialize};

use crate::{ConfigError, Environment};

/// Revision-history / file-log provider configuration.
///
/// Persists in the envelope as `profile.file_history`. Optional on disk
/// — absent or empty documents default to [`FileHistoryConfig::disabled`]
/// and yield a null provider at runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileHistoryConfig {
    /// URL of the revision endpoint the HTTP provider POSTs to.
    ///
    /// When `None`, the daemon wires a `NullRevisionProvider` which
    /// returns a structured "not configured" error on every call.
    /// When `Some(_)`, the daemon wires an `HttpRevisionProvider`
    /// targeting the URL (feature `file-history-http` on the proto
    /// crate).
    #[serde(default)]
    pub revision_url: Option<String>,
}

impl FileHistoryConfig {
    /// Explicit constructor matching the serde default (no endpoint
    /// configured). Useful for call sites that want to document intent.
    #[must_use]
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Validate the configuration against the active [`Environment`].
    ///
    /// Returns [`ConfigError::InvalidApiEndpoint`] when the URL is
    /// syntactically malformed, or when the active environment is
    /// [`Environment::Production`] and the URL is plaintext `http://`.
    pub fn validate(&self, env: Environment) -> Result<(), ConfigError> {
        let Some(url) = self.revision_url.as_deref() else {
            return Ok(());
        };
        let trimmed = url.trim();
        if trimmed.is_empty() {
            return Err(ConfigError::InvalidApiEndpoint(
                "file_history.revision_url must not be empty when set",
            ));
        }
        let is_https = trimmed.starts_with("https://");
        let is_http = trimmed.starts_with("http://");
        if !is_https && !is_http {
            return Err(ConfigError::InvalidApiEndpoint(
                "file_history.revision_url must start with http:// or https://",
            ));
        }
        if matches!(env, Environment::Production) && !is_https {
            return Err(ConfigError::InvalidApiEndpoint(
                "file_history.revision_url must be https:// in Production",
            ));
        }
        Ok(())
    }

    /// `true` when an endpoint URL is configured.
    #[must_use]
    pub fn is_configured(&self) -> bool {
        self.revision_url
            .as_deref()
            .map(str::trim)
            .is_some_and(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        let cfg = FileHistoryConfig::default();
        assert!(!cfg.is_configured());
        assert_eq!(cfg.revision_url, None);
        cfg.validate(Environment::Production).unwrap();
    }

    #[test]
    fn https_accepted_everywhere() {
        let cfg = FileHistoryConfig {
            revision_url: Some("https://example.com/listrevisions".into()),
        };
        cfg.validate(Environment::Development).unwrap();
        cfg.validate(Environment::Production).unwrap();
    }

    #[test]
    fn plaintext_rejected_in_production() {
        let cfg = FileHistoryConfig {
            revision_url: Some("http://local/listrevisions".into()),
        };
        cfg.validate(Environment::Development).unwrap();
        let err = cfg.validate(Environment::Production).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidApiEndpoint(_)));
    }

    #[test]
    fn malformed_url_rejected() {
        let cfg = FileHistoryConfig {
            revision_url: Some("ftp://nope".into()),
        };
        let err = cfg.validate(Environment::Development).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidApiEndpoint(_)));
    }

    #[test]
    fn empty_string_rejected_when_explicitly_set() {
        let cfg = FileHistoryConfig {
            revision_url: Some("   ".into()),
        };
        let err = cfg.validate(Environment::Development).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidApiEndpoint(_)));
    }

    #[test]
    fn round_trips_through_serde_omitting_when_none() {
        let cfg = FileHistoryConfig::default();
        let json = serde_json::to_value(&cfg).unwrap();
        // `revision_url` serializes as `null` by default.
        assert!(json.get("revision_url").is_some());
        let roundtrip: FileHistoryConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg, roundtrip);
    }
}
