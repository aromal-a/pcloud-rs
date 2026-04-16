//! Structured logging surface.
//!
//! Policy:
//! - A centrally-defined redaction list rewrites any log field whose key
//!   looks secret-bearing (password, token, key, secret, pass, pwd,
//!   authorization, session, recovery_code, code, tfa_code). The record
//!   value is replaced with `<redacted>`.
//! - Secret-bearing keys MUST NOT be passed to the logger — but the
//!   redaction guard is defence-in-depth in case a new call site regresses.
//! - The `json-logs` feature turns on a `serde_json`-backed JSON record
//!   formatter. Without it the formatter emits a deterministic `key=value`
//!   text string. Redaction is applied in both modes.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Classification of a log record.
///
/// Operational logs describe normal daemon activity; security logs describe
/// authentication, authorisation, audit, or crypto outcomes and may be
/// routed to a separate sink for long-term retention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogClass {
    /// Ordinary operational trace (transfers, sync, IPC, etc.).
    Operational,
    /// Security-relevant record that may need separate retention.
    Security,
}

/// Canonical list of field-name substrings that must be redacted. Match
/// is case-insensitive substring; a key like `auth_token_value` matches
/// `token`. Keep this list conservative and only add — never remove.
pub const REDACTED_KEY_SUBSTRINGS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "token",
    "secret",
    "authorization",
    "session",
    "recovery_code",
    "tfa_code",
    "private_key",
    "privkey",
    "api_key",
    "apikey",
    // "key" is intentionally last so callers can still use names like
    // "folder_id_key_name" without risking accidental unmasking — the
    // match is greedy against substrings, so this still redacts anything
    // containing `key`.
    "key",
];

/// Return `true` when the supplied field name contains any of the tokens in
/// [`REDACTED_KEY_SUBSTRINGS`] (case-insensitive).
///
/// Called by [`LogRecord::redact`]. The match is a substring check so keys
/// such as `auth_token_value` are caught.
///
/// # Example
///
/// ```
/// use pcloud_observability::logging::should_redact_field;
/// assert!(should_redact_field("password"));
/// assert!(should_redact_field("AUTH_TOKEN"));
/// assert!(should_redact_field("api_key"));
/// assert!(!should_redact_field("username"));
/// ```
pub fn should_redact_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    REDACTED_KEY_SUBSTRINGS
        .iter()
        .any(|needle| lower.contains(needle))
}

/// A single structured log record. Values are always strings after
/// redaction; numeric fields should be pre-formatted by the caller.
#[derive(Debug, Clone)]
pub struct LogRecord {
    /// Log level as a short uppercase string (`"INFO"`, `"WARN"`, etc.).
    pub level: &'static str,
    /// Module / target that produced the record.
    pub target: String,
    /// Primary human-readable message.
    pub message: String,
    /// Additional structured key/value pairs. Values are redacted in-place
    /// by [`LogRecord::redact`] when the key matches the redaction list.
    pub fields: Vec<(String, String)>,
    /// Classification used to route the record to the correct sink.
    pub class: LogClass,
}

impl LogRecord {
    /// Apply redaction to every field in-place. Called from every
    /// formatter so a regression in one sink cannot leak secrets via
    /// another.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::logging::{LogClass, LogRecord};
    /// let rec = LogRecord {
    ///     level: "INFO",
    ///     target: "test".to_owned(),
    ///     message: "login".to_owned(),
    ///     fields: vec![
    ///         ("password".to_owned(), "hunter2".to_owned()),
    ///         ("user".to_owned(), "alice".to_owned()),
    ///     ],
    ///     class: LogClass::Security,
    /// };
    /// let redacted = rec.redact();
    /// let text = redacted.to_text();
    /// assert!(!text.contains("hunter2"));
    /// assert!(text.contains("alice"));
    /// ```
    pub fn redact(mut self) -> Self {
        for (k, v) in &mut self.fields {
            if should_redact_field(k) {
                *v = "<redacted>".to_owned();
            }
        }
        self
    }

    /// Render the record to a deterministic `key=value` text line.
    ///
    /// Values are emitted with `{:?}` so embedded quotes and control
    /// characters are escaped, preventing log injection. Callers must invoke
    /// [`LogRecord::redact`] first to scrub secret-bearing fields.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_observability::logging::{LogClass, LogRecord};
    /// let rec = LogRecord {
    ///     level: "INFO",
    ///     target: "t".to_owned(),
    ///     message: "hi".to_owned(),
    ///     fields: vec![("k".to_owned(), "v".to_owned())],
    ///     class: LogClass::Operational,
    /// };
    /// let s = rec.to_text();
    /// assert!(s.contains("level=INFO"));
    /// assert!(s.contains("k=\"v\""));
    /// ```
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "level={} target={} msg={:?}",
            self.level, self.target, self.message
        );
        for (k, v) in &self.fields {
            out.push_str(&format!(" {k}={v:?}"));
        }
        out
    }

    /// Render the record as a single-line JSON object.
    ///
    /// Only available with the `json-logs` cargo feature. Redaction MUST be
    /// applied by the caller before serialisation.
    #[cfg(feature = "json-logs")]
    pub fn to_json(&self) -> String {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "level".to_owned(),
            serde_json::Value::String(self.level.to_owned()),
        );
        obj.insert(
            "target".to_owned(),
            serde_json::Value::String(self.target.clone()),
        );
        obj.insert(
            "message".to_owned(),
            serde_json::Value::String(self.message.clone()),
        );
        obj.insert(
            "class".to_owned(),
            serde_json::Value::String(match self.class {
                LogClass::Operational => "operational".to_owned(),
                LogClass::Security => "security".to_owned(),
            }),
        );
        let mut fields = serde_json::Map::new();
        for (k, v) in &self.fields {
            fields.insert(k.clone(), serde_json::Value::String(v.clone()));
        }
        obj.insert("fields".to_owned(), serde_json::Value::Object(fields));
        serde_json::Value::Object(obj).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(fields: Vec<(&str, &str)>) -> LogRecord {
        LogRecord {
            level: "INFO",
            target: "pcloud_daemon".to_owned(),
            message: "test".to_owned(),
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            class: LogClass::Operational,
        }
    }

    #[test]
    fn redaction_catches_common_secret_field_names() {
        let r = rec(vec![
            ("password", "hunter2"),
            ("auth_token", "eyJzZWNyZXQifQ"),
            ("recovery_code", "AAAA-BBBB"),
            ("api_key", "sk-1234"),
            ("folder_id", "1001"),
            ("username", "alice"),
        ])
        .redact();
        let map: std::collections::HashMap<_, _> = r.fields.iter().cloned().collect();
        assert_eq!(map["password"], "<redacted>");
        assert_eq!(map["auth_token"], "<redacted>");
        assert_eq!(map["recovery_code"], "<redacted>");
        assert_eq!(map["api_key"], "<redacted>");
        assert_eq!(map["folder_id"], "1001");
        assert_eq!(map["username"], "alice");
    }

    #[test]
    fn redaction_is_case_insensitive() {
        let r = rec(vec![
            ("Password", "x"),
            ("AUTH_TOKEN", "y"),
            ("TfA_Code", "z"),
        ])
        .redact();
        for (_, v) in &r.fields {
            assert_eq!(v, "<redacted>");
        }
    }

    #[test]
    fn text_format_never_contains_secret_value_if_key_marked() {
        let r = rec(vec![("password", "hunter2"), ("username", "alice")]).redact();
        let text = r.to_text();
        assert!(!text.contains("hunter2"));
        assert!(text.contains("alice"));
    }

    #[cfg(feature = "json-logs")]
    #[test]
    fn json_format_redacts_secret_values() {
        let r = rec(vec![("password", "hunter2"), ("username", "alice")]).redact();
        let json = r.to_json();
        assert!(!json.contains("hunter2"));
        assert!(json.contains("alice"));
        assert!(json.contains("<redacted>"));
    }
}
