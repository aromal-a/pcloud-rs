//! API endpoint binding (transport mode, host/port, TLS SNI, timeouts).
//!
//! Persists in the envelope's `profile.api` object. The transport mode is
//! gated by [`crate::Environment`]: [`Environment::Production`] rejects
//! [`ApiMode::Plaintext`] unconditionally
//! ([`ApiEndpoint::validate`]).

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

use crate::{ConfigError, Environment};

/// Transport mode for the API binding.
///
/// Stored as a string in the envelope (`"Development"`, `"Plaintext"`,
/// `"Tls"`). Overridden at runtime by `PCLOUD_API_MODE` (values
/// `dev`/`development`, `plain`/`plaintext`/`tcp`, `tls`/`ssl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiMode {
    /// Relaxed development mode. No TLS enforcement; host/port validation
    /// is skipped. Only valid in [`Environment::Development`] /
    /// [`Environment::Test`].
    Development,
    /// Cleartext TCP. Rejected by [`ApiEndpoint::validate`] under
    /// [`Environment::Production`]. Useful only for local loopback
    /// interop testing.
    Plaintext,
    /// TLS with SNI set to [`ApiEndpoint::server_name`]. Required for
    /// production.
    Tls,
}

/// Fully resolved API endpoint binding.
///
/// Persists in `profile.api`. Field-by-field env-var overrides (applied
/// after deserialization by [`crate::env::apply_env_overrides`]):
///
/// | Env var                          | Field                 |
/// |----------------------------------|-----------------------|
/// | `PCLOUD_API_MODE`                | `mode`                |
/// | `PCLOUD_API_HOST`                | `host`                |
/// | `PCLOUD_API_PORT`                | `port`                |
/// | `PCLOUD_API_SERVER_NAME`         | `server_name`         |
/// | `PCLOUD_API_CONNECT_TIMEOUT_MS`  | `connect_timeout_ms`  |
/// | `PCLOUD_API_READ_TIMEOUT_MS`     | `read_timeout_ms`     |
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiEndpoint {
    /// Transport mode selecting plaintext, TLS, or relaxed-development
    /// framing. Default: `Development` (dev/test profiles) or `Tls`
    /// (production). Valid values: `Development`, `Plaintext`, `Tls`.
    /// **Security:** [`Environment::Production`] rejects `Plaintext` —
    /// TLS is mandatory in production. Example: `mode = "Tls"`.
    pub mode: ApiMode,
    /// DNS host or IP literal of the API endpoint. Default:
    /// `"bineapi.pcloud.com"`. Valid values: any non-empty string
    /// (validated non-empty in plaintext/TLS modes). **Security:** used as
    /// both the TCP `connect()` target and, via [`Self::server_name`],
    /// the TLS certificate verification name — a wrong value can silently
    /// route traffic to the wrong endpoint. Example:
    /// `host = "bineapi-eu.pcloud.com"`.
    pub host: String,
    /// TCP port the client dials. Default: `443`. Valid values: `1..=65535`
    /// (port `0` is rejected by [`Self::validate`] in plaintext/TLS modes).
    /// **Security:** production plaintext interop ports (e.g. `8398`)
    /// require `mode = Plaintext`, which itself is refused in production.
    /// Example: `port = 443`.
    pub port: u16,
    /// TLS SNI / X.509 certificate-verification name presented to the
    /// server. Default: `"bineapi.pcloud.com"`. Valid values: any
    /// non-empty string in TLS mode; ignored in `Plaintext` / `Development`.
    /// **Security:** decoupled from [`Self::host`] so an operator can point
    /// at a staging IP while still validating the production certificate
    /// name — setting it to an attacker-controlled value disables MITM
    /// protection. Example: `server_name = "bineapi.pcloud.com"`.
    pub server_name: String,
    /// TCP connect timeout in milliseconds. Default: `5_000`. Valid values:
    /// any `u64 > 0` in plaintext/TLS modes; `0` is rejected. **Security:**
    /// acts as a denial-of-service bound — too high and a stalled TCP
    /// handshake pins a worker indefinitely. Example:
    /// `connect_timeout_ms = 5000`.
    pub connect_timeout_ms: u64,
    /// Per-read timeout in milliseconds for an established connection.
    /// Default: `15_000`. Valid values: any `u64 > 0` in plaintext/TLS
    /// modes; `0` is rejected. **Security:** caps the time the parser
    /// will sit waiting for the next frame; avoids a slowloris-class hang
    /// on a legitimate-looking but frozen peer. Example:
    /// `read_timeout_ms = 15000`.
    pub read_timeout_ms: u64,
}

impl ApiEndpoint {
    /// Produce an endpoint pinned to `bineapi.pcloud.com:443` with
    /// environment-appropriate [`ApiMode`] and conservative 5s/15s
    /// timeouts.
    #[must_use]
    pub fn secure_defaults(environment: Environment) -> Self {
        match environment {
            Environment::Development | Environment::Test => Self {
                mode: ApiMode::secure_default_for(environment),
                host: "bineapi.pcloud.com".to_owned(),
                port: 443,
                server_name: "bineapi.pcloud.com".to_owned(),
                connect_timeout_ms: 5_000,
                read_timeout_ms: 15_000,
            },
            Environment::Production => Self {
                mode: ApiMode::secure_default_for(environment),
                host: "bineapi.pcloud.com".to_owned(),
                port: 443,
                server_name: "bineapi.pcloud.com".to_owned(),
                connect_timeout_ms: 5_000,
                read_timeout_ms: 15_000,
            },
        }
    }

    /// Reject internally inconsistent or environment-incompatible bindings.
    ///
    /// Enforces:
    /// - [`Environment::Production`] + [`ApiMode::Plaintext`] →
    ///   [`ConfigError::InvalidApiEndpoint`].
    /// - Non-empty `host` in plaintext/TLS modes.
    /// - Non-zero `port` in plaintext/TLS modes.
    /// - Non-empty `server_name` in TLS mode.
    /// - Non-zero `connect_timeout_ms` and `read_timeout_ms`.
    ///
    /// [`ApiMode::Development`] skips all host-level checks by design
    /// (used for mocks and fixtures).
    pub fn validate(&self, environment: Environment) -> Result<(), ConfigError> {
        // Production must never run with a plaintext API transport. This
        // rejection is intentionally centralized here so every consumer that
        // validates an `ApiEndpoint` (SDK, services, tests, bootstrap) is
        // forced through the same gate instead of relying on
        // `bootstrap_with_config` as a single chokepoint.
        if environment == Environment::Production && matches!(self.mode, ApiMode::Plaintext) {
            return Err(ConfigError::InvalidApiEndpoint(
                "production environment requires tls api mode",
            ));
        }

        match self.mode {
            ApiMode::Development => Ok(()),
            ApiMode::Plaintext | ApiMode::Tls => {
                if self.host.trim().is_empty() {
                    return Err(ConfigError::InvalidApiEndpoint("host must not be empty"));
                }
                if self.port == 0 {
                    return Err(ConfigError::InvalidApiEndpoint("port must not be zero"));
                }
                if matches!(self.mode, ApiMode::Tls) && self.server_name.trim().is_empty() {
                    return Err(ConfigError::InvalidApiEndpoint(
                        "server_name must not be empty in tls mode",
                    ));
                }
                if self.connect_timeout_ms == 0 {
                    return Err(ConfigError::InvalidApiEndpoint(
                        "connect_timeout_ms must be non-zero",
                    ));
                }
                if self.read_timeout_ms == 0 {
                    return Err(ConfigError::InvalidApiEndpoint(
                        "read_timeout_ms must be non-zero",
                    ));
                }
                Ok(())
            }
        }
    }

    /// Apply a `"host"` or `"host:port"` server hint from the
    /// `get_api_servers` response (or equivalent operator override).
    ///
    /// Empty/whitespace-only strings are ignored. A port suffix is
    /// accepted only when it parses as `u16`; otherwise the port is
    /// preserved and only the host/SNI fields are updated.
    ///
    /// **Security:** unknown hosts (not ending in `.pcloud.com` or
    /// `.pcloud.link`) are rejected to prevent a compromised preferences
    /// store from redirecting traffic to an attacker-controlled server.
    /// Returns `Err` when the host fails the allowlist check.
    pub fn apply_api_server_hint(&mut self, api_server: &str) -> Result<(), &'static str> {
        if api_server.trim().is_empty() {
            return Ok(());
        }

        let (host, port) = parse_api_server_hint(api_server);
        if !is_known_safe_host(&host) {
            return Err("api server hint rejected: host is not a known pCloud domain");
        }
        self.host = host.clone();
        self.server_name = host;
        if let Some(port) = port {
            self.port = port;
        }
        Ok(())
    }
}

/// Returns `true` when `host` is within a known-safe pCloud domain.
///
/// Only `.pcloud.com` and `.pcloud.link` subdomains are trusted as API
/// server endpoints. This is deliberately restrictive — operators that
/// need a custom endpoint must use the explicit config override path, not
/// the persisted preferences hint.
#[must_use]
pub fn is_known_safe_host(host: &str) -> bool {
    host.ends_with(".pcloud.com") || host.ends_with(".pcloud.link")
}

impl ApiMode {
    /// Return the default [`ApiMode`] for a given [`Environment`]:
    /// [`ApiMode::Development`] for Development/Test, [`ApiMode::Tls`] for
    /// Production.
    #[must_use]
    pub fn secure_default_for(environment: Environment) -> Self {
        match environment {
            Environment::Development | Environment::Test => Self::Development,
            Environment::Production => Self::Tls,
        }
    }
}

fn parse_api_server_hint(api_server: &str) -> (String, Option<u16>) {
    let trimmed = api_server.trim();
    if let Some((host, port)) = trimmed.rsplit_once(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return (host.to_owned(), Some(port));
    }
    (trimmed.to_owned(), None)
}

#[cfg(test)]
mod tests {
    use crate::{ConfigError, Environment};

    use super::{ApiEndpoint, ApiMode};

    #[test]
    fn apply_api_server_hint_updates_endpoint() {
        let mut endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        endpoint
            .apply_api_server_hint("bineapi-eu.pcloud.com:8443")
            .expect("known pCloud host should be accepted");

        assert_eq!(endpoint.host, "bineapi-eu.pcloud.com");
        assert_eq!(endpoint.server_name, "bineapi-eu.pcloud.com");
        assert_eq!(endpoint.port, 8443);
    }

    #[test]
    fn apply_api_server_hint_rejects_unknown_host() {
        let mut endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        let result = endpoint.apply_api_server_hint("evil.example.com:443");
        assert!(
            result.is_err(),
            "unknown host must be rejected to prevent SSRF/redirect"
        );
        // Endpoint must be unchanged after rejection.
        assert_eq!(endpoint.host, "bineapi.pcloud.com");
    }

    #[test]
    fn apply_api_server_hint_accepts_pcloud_link() {
        let mut endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        endpoint
            .apply_api_server_hint("cdn.pcloud.link")
            .expect("pcloud.link domain should be accepted");
        assert_eq!(endpoint.host, "cdn.pcloud.link");
    }

    #[test]
    fn production_plaintext_is_rejected() {
        let mut endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        endpoint.mode = ApiMode::Plaintext;

        let err = endpoint
            .validate(Environment::Production)
            .expect_err("plaintext must be rejected in production");
        assert!(matches!(err, ConfigError::InvalidApiEndpoint(msg) if msg.contains("tls")));
    }

    #[test]
    fn development_plaintext_is_allowed() {
        let mut endpoint = ApiEndpoint::secure_defaults(Environment::Development);
        endpoint.mode = ApiMode::Plaintext;

        endpoint
            .validate(Environment::Development)
            .expect("plaintext must be permitted in development");
    }

    #[test]
    fn production_tls_is_allowed() {
        let endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        assert!(matches!(endpoint.mode, ApiMode::Tls));

        endpoint
            .validate(Environment::Production)
            .expect("tls must be permitted in production");
    }
}
