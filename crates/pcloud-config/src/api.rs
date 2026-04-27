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

/// Dynamic TLS certificate-revocation check mode.
///
/// Tracked under bead `pcloud-rs-t9o` (FedRAMP-style dynamic revocation).
///
/// The rustls client config in `pcloud-proto::tls` currently performs
/// standard webpki path validation against the Mozilla `webpki-roots`
/// bundle but does NOT consult a Certificate Revocation List (CRL) nor
/// validate stapled Online Certificate Status Protocol (OCSP) responses.
/// FedRAMP / FIPS / DoD-adjacent deployments typically require at least
/// one dynamic revocation channel.
///
/// ## Why this is a config knob and not a default-on gate
///
/// - **CRL sourcing is operator-specific.** FedRAMP-class customers
///   mount their own CRL DER file (or bundle) at a known path. pcloud-rs
///   cannot hardcode a URL or a well-known filesystem location without
///   guessing a deployment policy.
/// - **OCSP stapling is server-driven.** A client can only verify a
///   stapled OCSP response if the *server* includes one in its TLS
///   `CertificateStatus` extension. The pCloud API servers are third
///   party; whether they staple is an observational fact, not a
///   contract.
/// - **Fail-closed is dangerous without infra.** Turning on strict
///   revocation before CRLs are mounted or before stapling is confirmed
///   would cause every production client to refuse to connect.
///
/// The shipped implementation honors this knob only by validating a
/// stapled OCSP response if the server sends one AND `StapledPermissive`
/// is selected; otherwise revocation is not checked. See the rustdoc on
/// `pcloud_proto::tls` for the wire-level hook points.
///
/// Stored as a string in the envelope (`"Disabled"`, `"StapledPermissive"`,
/// `"StapledStrict"`, `"CrlFile"`). Overridden at runtime by
/// `PCLOUD_API_TLS_REVOCATION`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TlsRevocationCheck {
    /// Revocation is not checked. This is the default and matches the
    /// pre-t9o behavior. Suitable for commercial deployments that rely
    /// solely on Mozilla root-bundle trust. **Not** FedRAMP-compliant.
    Disabled,
    /// Validate a stapled OCSP response *if* the server sends one;
    /// otherwise continue (do not fail the handshake). Recommended
    /// default for environments that want belt-and-braces revocation
    /// when the server cooperates without breaking connectivity when
    /// it does not.
    StapledPermissive,
    /// Require the server to staple a valid OCSP response. Abort the
    /// handshake if no stapled response is present or the stapled
    /// response is expired / revoked. **Only enable after confirming
    /// the target API servers actually staple** — otherwise every
    /// connection attempt will fail.
    StapledStrict,
    /// Consult a locally-mounted CRL DER file at the given path. The
    /// file is loaded once at startup; operators must rotate it out of
    /// band and restart the daemon to pick up updates. Empty string
    /// disables this mode.
    CrlFile(String),
}

impl Default for TlsRevocationCheck {
    /// Defaults to [`TlsRevocationCheck::Disabled`] for backward
    /// compatibility and to avoid breaking existing deployments that
    /// have not yet wired a revocation source.
    fn default() -> Self {
        Self::Disabled
    }
}

impl TlsRevocationCheck {
    /// Returns `true` when the configured mode demands strict
    /// fail-closed behavior (handshake must abort on missing/expired
    /// revocation data).
    #[must_use]
    pub fn is_strict(&self) -> bool {
        matches!(self, Self::StapledStrict)
    }

    /// Returns `true` when revocation checking is effectively a no-op.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        matches!(self, Self::Disabled)
            || matches!(self, Self::CrlFile(path) if path.trim().is_empty())
    }
}

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
    /// TLS certificate revocation check mode (bead `pcloud-rs-t9o`).
    ///
    /// Default (and pre-t9o behavior): [`TlsRevocationCheck::Disabled`].
    /// Off by default because (a) FedRAMP CRL paths are deployment-
    /// specific, (b) stapled OCSP requires server participation, and
    /// (c) fail-closed without infra would break all production
    /// handshakes. See the [`TlsRevocationCheck`] rustdoc for the
    /// rationale and supported modes.
    ///
    /// `#[serde(default)]` so older config envelopes (without this
    /// field) continue to load without migration.
    #[serde(default)]
    pub tls_revocation_check: TlsRevocationCheck,
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
                tls_revocation_check: TlsRevocationCheck::default(),
            },
            Environment::Production => Self {
                mode: ApiMode::secure_default_for(environment),
                host: "bineapi.pcloud.com".to_owned(),
                port: 443,
                server_name: "bineapi.pcloud.com".to_owned(),
                connect_timeout_ms: 5_000,
                read_timeout_ms: 15_000,
                tls_revocation_check: TlsRevocationCheck::default(),
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
                // audit-06 H-6.1: per-endpoint composition rule.
                // `connect_timeout` must not exceed `read_timeout` —
                // otherwise the connect deadline would dwarf the
                // per-frame read deadline, defeating the slowloris guard.
                validate_timeout_composition(
                    std::time::Duration::from_millis(self.connect_timeout_ms),
                    std::time::Duration::from_millis(self.read_timeout_ms),
                    None,
                )?;
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

/// Validate the composition `connect ≤ read ≤ total` for a transport
/// timeout triple.
///
/// audit-06 H-6.1 — a misordered triple (e.g. `total_timeout < read_timeout`)
/// causes the per-syscall read deadline to fire before the total deadline can
/// arm, producing spurious timeout errors. Rejecting at config-load time with
/// [`ConfigError::InvalidTimeoutComposition`] keeps the failure visible at the
/// boundary where the operator can correct it instead of inside the hot path.
///
/// `total` is optional: if a config layer does not (yet) carry a total-request
/// timeout, the composition is enforced only on the `connect ≤ read` pair.
/// Both `connect` and `read` are still rejected if zero — that case is the
/// caller's `validate()` responsibility for the surrounding struct.
///
/// # Errors
///
/// - `connect > read` — slowloris guard inversion.
/// - `read > total` (when `total` is supplied) — read loop fires before
///   the total deadline can arm.
///
/// # Example
///
/// ```
/// use std::time::Duration;
/// use pcloud_config::api::validate_timeout_composition;
///
/// // OK: 5s connect, 15s read, 60s total.
/// validate_timeout_composition(
///     Duration::from_secs(5),
///     Duration::from_secs(15),
///     Some(Duration::from_secs(60)),
/// )
/// .expect("well-ordered triple");
///
/// // Rejected: read deadline exceeds total.
/// assert!(
///     validate_timeout_composition(
///         Duration::from_secs(5),
///         Duration::from_secs(120),
///         Some(Duration::from_secs(60)),
///     )
///     .is_err()
/// );
/// ```
pub fn validate_timeout_composition(
    connect: std::time::Duration,
    read: std::time::Duration,
    total: Option<std::time::Duration>,
) -> Result<(), ConfigError> {
    if connect > read {
        return Err(ConfigError::InvalidTimeoutComposition(
            "connect_timeout must not exceed read_timeout",
        ));
    }
    if let Some(total) = total
        && read > total
    {
        return Err(ConfigError::InvalidTimeoutComposition(
            "read_timeout must not exceed total_request_timeout",
        ));
    }
    Ok(())
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
    if let Some((host, port)) = trimmed.rsplit_once(':') {
        if let Ok(port) = port.parse::<u16>() {
            return (host.to_owned(), Some(port));
        }
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
    fn tls_revocation_default_is_disabled() {
        // Bead pcloud-rs-t9o: backward-compatible default. Existing
        // envelopes that were written before t9o must still load
        // without a migration and deserialize to `Disabled`.
        let endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        assert!(matches!(
            endpoint.tls_revocation_check,
            super::TlsRevocationCheck::Disabled
        ));
        assert!(endpoint.tls_revocation_check.is_disabled());
        assert!(!endpoint.tls_revocation_check.is_strict());
    }

    #[test]
    fn tls_revocation_strict_is_strict() {
        let strict = super::TlsRevocationCheck::StapledStrict;
        assert!(strict.is_strict());
        assert!(!strict.is_disabled());
    }

    #[test]
    fn tls_revocation_crl_empty_path_is_disabled() {
        // Empty-path CRL mode must behave as Disabled so an operator
        // who forgets to set the path does not get a silent downgrade
        // surprise.
        let empty = super::TlsRevocationCheck::CrlFile(String::new());
        assert!(empty.is_disabled());
    }

    #[test]
    fn production_tls_is_allowed() {
        let endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        assert!(matches!(endpoint.mode, ApiMode::Tls));

        endpoint
            .validate(Environment::Production)
            .expect("tls must be permitted in production");
    }

    // ── audit-06 H-6.1: timeout composition validation ───────────────────

    #[test]
    fn timeout_composition_accepts_well_ordered_triple() {
        use std::time::Duration;
        super::validate_timeout_composition(
            Duration::from_secs(5),
            Duration::from_secs(15),
            Some(Duration::from_secs(60)),
        )
        .expect("connect <= read <= total must pass");
    }

    #[test]
    fn timeout_composition_accepts_equal_pairs() {
        use std::time::Duration;
        super::validate_timeout_composition(
            Duration::from_secs(15),
            Duration::from_secs(15),
            Some(Duration::from_secs(15)),
        )
        .expect("equality is allowed");
    }

    #[test]
    fn timeout_composition_rejects_connect_gt_read() {
        use std::time::Duration;
        let err = super::validate_timeout_composition(
            Duration::from_secs(30),
            Duration::from_secs(15),
            None,
        )
        .expect_err("connect > read must be rejected");
        assert!(matches!(
            err,
            ConfigError::InvalidTimeoutComposition(msg) if msg.contains("connect_timeout")
        ));
    }

    #[test]
    fn timeout_composition_rejects_read_gt_total() {
        use std::time::Duration;
        let err = super::validate_timeout_composition(
            Duration::from_secs(5),
            Duration::from_secs(120),
            Some(Duration::from_secs(60)),
        )
        .expect_err("read > total must be rejected");
        assert!(matches!(
            err,
            ConfigError::InvalidTimeoutComposition(msg) if msg.contains("read_timeout")
        ));
    }

    #[test]
    fn timeout_composition_total_optional_skips_upper_check() {
        use std::time::Duration;
        // No total => only connect <= read is enforced.
        super::validate_timeout_composition(Duration::from_secs(5), Duration::from_secs(120), None)
            .expect("missing total must skip the read<=total check");
    }

    #[test]
    fn validate_endpoint_rejects_inverted_connect_read_pair() {
        // Building a misconfigured endpoint via the public surface and
        // validating it must surface the typed composition error.
        let mut endpoint = ApiEndpoint::secure_defaults(Environment::Production);
        endpoint.connect_timeout_ms = 30_000; // 30s connect
        endpoint.read_timeout_ms = 5_000; //  5s read   (inverted)
        let err = endpoint
            .validate(Environment::Production)
            .expect_err("inverted timeout pair must be rejected at validate()");
        assert!(matches!(err, ConfigError::InvalidTimeoutComposition(_)));
    }
}
