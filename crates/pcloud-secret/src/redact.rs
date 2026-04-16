// **PLATFORM:** all
// **GATING:** none (portable).

/// Format a `key=<redacted>` token for inclusion in log lines.
///
/// Use this when a structured log event must record that a secret was
/// present at a given point in the code without revealing its content.
/// The output shape is deliberately uniform so audit pipelines can grep
/// `=<redacted>` to confirm secret-handling discipline.
///
/// # When to use
///
/// * Emitting audit breadcrumbs around a secret-bearing operation
///   (e.g. `auth_token=<redacted>`).
/// * Building an error message or `Display` impl that must mention the
///   field without the value.
///
/// # When NOT to use
///
/// * **Do not** pair this with a second log line that prints the real
///   value — the redaction would be meaningless.
/// * **Do not** use it as a `Debug` impl for a secret-bearing type; use
///   [`crate::secret_string::SecretString`] / [`crate::secret_bytes::SecretBytes`]
///   which redact themselves.
///
/// ```
/// use pcloud_secret::redact::redact_field;
/// assert_eq!(redact_field("auth_token"), "auth_token=<redacted>");
/// ```
#[must_use]
pub fn redact_field(name: &str) -> String {
    format!("{name}=<redacted>")
}
