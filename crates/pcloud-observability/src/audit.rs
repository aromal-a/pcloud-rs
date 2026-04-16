//! Audit event envelope.
//!
//! The daemon emits an [`AuditEvent`] whenever a security-relevant action is
//! taken (authentication, crypto unlock, sync root mutation, etc.). The
//! runtime is responsible for persistence: this module only defines the
//! serialised wire/storage shape so producers and consumers agree on the
//! schema.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Single audit event ready for persistence.
///
/// `category` is a dotted low-cardinality identifier such as
/// `"auth.login.success"` or `"crypto.unlock"`. `details` carries an optional
/// free-form human-readable explanation and MUST NOT contain secrets —
/// redaction is enforced at logging time, not here.
///
/// # Example
///
/// ```
/// use pcloud_observability::audit::AuditEvent;
/// let evt = AuditEvent {
///     category: "auth.login.success".to_owned(),
///     details: Some("user@example.com".to_owned()),
/// };
/// assert_eq!(evt.category, "auth.login.success");
/// assert_eq!(evt.clone(), evt);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Dotted event category string (for example `"daemon.startup"`).
    pub category: String,
    /// Optional human-readable details. Secrets must never be written here.
    pub details: Option<String>,
}
