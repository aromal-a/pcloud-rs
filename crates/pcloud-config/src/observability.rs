//! Observability feature flags attached to a [`crate::ConfigProfile`].
//!
//! These flags control opt-in telemetry surfaces. All defaults are
//! intentionally conservative: nothing is exported and no traces are
//! collected unless explicitly enabled.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Opt-in telemetry toggles persisted as the `observability` block of
/// the profile envelope.
///
/// All fields default to the conservative values returned by
/// [`ObservabilityFlags::secure_defaults`]: structured logs and audit
/// export enabled, span tracing and operator metrics disabled. No flag
/// in this struct weakens transport or permission enforcement; even
/// with everything enabled, production still rejects plaintext API
/// transport and group-readable config files. Deserialized from the
/// TOML/JSON `observability` table; overrides for individual flags are
/// applied from `PCLOUD_OBSERVABILITY_*` env vars by
/// [`crate::env::apply_env_overrides`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservabilityFlags {
    /// Emit structured (JSON-lines) logs to stderr. Default: `true`.
    /// Valid values: `true`, `false`. **Security:** redaction for
    /// `SecretString` / `SecretBytes` is applied regardless of this flag;
    /// disabling structured logs falls back to an unstructured stderr
    /// formatter, not to zero output. Example:
    /// `structured_logs_enabled = true`.
    pub structured_logs_enabled: bool,
    /// Enable span-based tracing (OTEL-compatible span emission).
    /// Default: `false`. Valid values: `true`, `false`. **Security:**
    /// span attributes never carry secrets, but enabling tracing
    /// increases the amount of operational metadata exposed via the
    /// (owner-only) metrics endpoint. Example: `tracing_enabled = false`.
    pub tracing_enabled: bool,
    /// Expose the operator metrics endpoint on the owner-only IPC socket.
    /// Default: `false`. Valid values: `true`, `false`. **Security:**
    /// the endpoint is gated by IPC peer checks (same as any control
    /// surface); turning it on does not broaden exposure beyond the
    /// owner. Example: `metrics_enabled = false`.
    pub metrics_enabled: bool,
    /// Export audit events to the persistent audit store under
    /// `state_dir`. Default: `true`. Valid values: `true`, `false`.
    /// **Security:** the audit stream records auth, crypto, and
    /// admin-plane activity. Disabling this removes the forensic trail
    /// even though the underlying actions still happen. Example:
    /// `audit_export_enabled = true`.
    pub audit_export_enabled: bool,
}

impl Default for ObservabilityFlags {
    fn default() -> Self {
        Self::secure_defaults()
    }
}

impl ObservabilityFlags {
    /// Return the conservative default set: structured logs and audit
    /// export enabled; span tracing and operator metrics disabled.
    ///
    /// Applied automatically by serde when an on-disk envelope predates
    /// the observability block (v1 documents), and used directly by
    /// [`crate::ConfigProfile::secure_defaults`].
    #[must_use]
    pub const fn secure_defaults() -> Self {
        Self {
            structured_logs_enabled: true,
            tracing_enabled: false,
            metrics_enabled: false,
            audit_export_enabled: true,
        }
    }
}
