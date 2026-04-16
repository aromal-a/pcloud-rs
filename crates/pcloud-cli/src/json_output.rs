//! Machine-readable JSON output for `pcloud-cli`.
//!
//! Every command result — success OR error — can be serialized through this
//! module. Consumers (CI scripts, orchestrators) get a stable JSON envelope:
//!
//! ```json
//! {
//!   "kind": "success" | "error",
//!   "command": "status",
//!   "status": "ok",
//!   "message": "...",
//!   "exit_code": 0,
//!   "error": { "category": "...", "detail": "..." }
//! }
//! ```
//!
//! SECURITY: This module NEVER accepts secret-bearing types. The only inputs
//! are the already-sanitized IPC [`Response`] (whose fields are `status` +
//! `message`) and parse/transport error strings. Secret-bearing request
//! fields (password, crypto password, auth token, public-link password) do
//! not flow into this serializer. Schema evolution must keep this invariant.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::{Response, ResponseStatus};
use serde::{Deserialize, Serialize};

use crate::exit_code::ExitCode;

/// Serialize form of [`ResponseStatus`], matching the text path's lower-case
/// rendering conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonStatus {
    Ok,
    InvalidRequest,
    Unauthorized,
    Conflict,
    Unavailable,
    InternalError,
}

impl From<ResponseStatus> for JsonStatus {
    fn from(s: ResponseStatus) -> Self {
        match s {
            ResponseStatus::Ok => Self::Ok,
            ResponseStatus::InvalidRequest => Self::InvalidRequest,
            ResponseStatus::Unauthorized => Self::Unauthorized,
            ResponseStatus::Conflict => Self::Conflict,
            ResponseStatus::Unavailable => Self::Unavailable,
            ResponseStatus::InternalError => Self::InternalError,
            ResponseStatus::PolicyViolation { .. } => Self::Conflict,
            _ => Self::InternalError,
        }
    }
}

/// Error category emitted in JSON error envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonErrorCategory {
    Usage,
    Auth,
    Network,
    CryptoLocked,
    Unavailable,
    Conflict,
    Internal,
    Generic,
}

impl JsonErrorCategory {
    #[must_use]
    pub const fn from_exit_code(code: ExitCode) -> Self {
        match code {
            ExitCode::Ok | ExitCode::GenericError => Self::Generic,
            ExitCode::Usage => Self::Usage,
            ExitCode::Auth => Self::Auth,
            ExitCode::Network => Self::Network,
            ExitCode::CryptoLocked => Self::CryptoLocked,
            ExitCode::Unavailable => Self::Unavailable,
            ExitCode::Conflict => Self::Conflict,
            ExitCode::Internal => Self::Internal,
        }
    }
}

/// JSON error envelope body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JsonError {
    pub category: JsonErrorCategory,
    pub detail: String,
}

/// Stable JSON envelope returned by the CLI in `--json` mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JsonEnvelope {
    /// Ordinary success envelope — message is the raw daemon payload.
    Success {
        /// Human-readable command label.
        command: String,
        /// Mapped response status.
        status: JsonStatus,
        /// Verbatim response message (already sanitised by the daemon).
        message: String,
        /// Resolved exit code.
        exit_code: i32,
    },
    /// Error envelope.
    Error {
        /// Command label if the error was raised after dispatch.
        command: Option<String>,
        /// Resolved exit code.
        exit_code: i32,
        /// Categorised error payload.
        error: JsonError,
    },
    /// Field-selector projection envelope, emitted only when the
    /// operator supplied `--field`/`-f`/`--select` (or bare trailing
    /// field names on a whitelisted command).
    ///
    /// The `fields` map carries one entry per requested selector,
    /// keyed by the original (dotted) path string so scripts can
    /// look each value up by the same token they asked for. Values
    /// are the native JSON shape produced by the selector — strings,
    /// numbers, bools, or nested arrays/objects — so scripts can
    /// consume them without a second parse.
    ///
    /// Map ordering follows [`serde_json::Map`]'s default behaviour
    /// (alphabetical). Scripts that need selector-order output
    /// should use the plain text rendering, which emits one value
    /// per line in the exact order the user supplied.
    Filtered {
        /// Human-readable command label.
        command: String,
        /// Mapped response status.
        status: JsonStatus,
        /// `selector path` → selected value.
        fields: serde_json::Map<String, serde_json::Value>,
        /// Resolved exit code.
        exit_code: i32,
    },
}

impl JsonEnvelope {
    /// Build a success envelope from a command label and an IPC response.
    #[must_use]
    pub fn from_response(command: impl Into<String>, response: &Response) -> Self {
        let exit = ExitCode::from_response_status(&response.status);
        Self::Success {
            command: command.into(),
            status: response.status.clone().into(),
            message: response.message.clone(),
            exit_code: exit.as_i32(),
        }
    }

    /// Build an error envelope.
    #[must_use]
    pub fn from_error(command: Option<String>, exit: ExitCode, detail: impl Into<String>) -> Self {
        Self::Error {
            command,
            exit_code: exit.as_i32(),
            error: JsonError {
                category: JsonErrorCategory::from_exit_code(exit),
                detail: detail.into(),
            },
        }
    }

    /// Build a field-projection envelope.
    ///
    /// `fields` is expected to already contain the selected values in
    /// selector order. The caller (the CLI projection stage) is
    /// responsible for running [`crate::field_selector::FieldSelector`]
    /// against the parsed response and for reporting NotFound/TypeMismatch
    /// errors as usage failures rather than partially-populated
    /// envelopes.
    #[must_use]
    pub fn from_fields(
        command: impl Into<String>,
        response: &Response,
        fields: serde_json::Map<String, serde_json::Value>,
    ) -> Self {
        let exit = ExitCode::from_response_status(&response.status);
        Self::Filtered {
            command: command.into(),
            status: response.status.clone().into(),
            fields,
            exit_code: exit.as_i32(),
        }
    }

    /// Render as a single JSON line (newline appended) for stdout.
    #[must_use]
    pub fn render(&self) -> String {
        // serde_json::to_string on a fully-typed envelope is infallible in
        // practice; fall back to a minimal valid JSON error envelope if it
        // somehow errors (e.g. OOM).
        match serde_json::to_string(self) {
            Ok(mut s) => {
                s.push('\n');
                s
            }
            Err(_) => "{\"kind\":\"error\",\"exit_code\":1,\"error\":{\"category\":\"internal\",\"detail\":\"json_render_failed\"},\"command\":null}\n".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_envelope_roundtrip() {
        let resp = Response {
            status: ResponseStatus::Ok,
            message: "all good".into(),
        };
        let env = JsonEnvelope::from_response("status", &resp);
        let json = env.render();
        let parsed: JsonEnvelope = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(env, parsed);
        match parsed {
            JsonEnvelope::Success {
                command,
                status,
                message,
                exit_code,
            } => {
                assert_eq!(command, "status");
                assert_eq!(status, JsonStatus::Ok);
                assert_eq!(message, "all good");
                assert_eq!(exit_code, 0);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn error_envelope_roundtrip() {
        let env = JsonEnvelope::from_error(
            Some("status".into()),
            ExitCode::Network,
            "connection refused",
        );
        let json = env.render();
        let parsed: JsonEnvelope = serde_json::from_str(json.trim()).unwrap();
        assert_eq!(env, parsed);
        match parsed {
            JsonEnvelope::Error {
                command,
                exit_code,
                error,
            } => {
                assert_eq!(command.as_deref(), Some("status"));
                assert_eq!(exit_code, 4);
                assert_eq!(error.category, JsonErrorCategory::Network);
                assert_eq!(error.detail, "connection refused");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn every_response_status_maps_to_envelope() {
        for (status, expected_exit) in [
            (ResponseStatus::Ok, 0),
            (ResponseStatus::InvalidRequest, 2),
            (ResponseStatus::Unauthorized, 3),
            (ResponseStatus::Conflict, 7),
            (ResponseStatus::Unavailable, 6),
            (ResponseStatus::InternalError, 8),
        ] {
            let env = JsonEnvelope::from_response(
                "any",
                &Response {
                    status: status.clone(),
                    message: "m".into(),
                },
            );
            let rendered = env.render();
            let parsed: JsonEnvelope = serde_json::from_str(rendered.trim()).unwrap();
            if let JsonEnvelope::Success { exit_code, .. } = parsed {
                assert_eq!(exit_code, expected_exit, "status={status:?}");
            } else {
                panic!("expected success variant");
            }
        }
    }

    #[test]
    fn envelope_never_serializes_anything_unexpected() {
        // Guard: the Success envelope has exactly four fields and nothing
        // secret-bearing can slip in via a #[serde(flatten)] surprise.
        let env = JsonEnvelope::from_response(
            "status",
            &Response {
                status: ResponseStatus::Ok,
                message: "m".into(),
            },
        );
        let v: serde_json::Value = serde_json::to_value(&env).unwrap();
        let obj = v.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["command", "exit_code", "kind", "message", "status"]
        );
    }

    #[test]
    fn file_history_unavailable_envelope_round_trips() {
        // R9 #9: `pcloudc log <PATH>` currently returns Unavailable
        // from the daemon (bd-1du.10). Make sure the standard success
        // envelope preserves the exit code + message verbatim so the
        // operator sees the honest-scope detail.
        let response = Response {
            status: ResponseStatus::Unavailable,
            message: "file-history: not yet supported by pCloud public API".into(),
        };
        let env = JsonEnvelope::from_response("log", &response);
        let rendered = env.render();
        let parsed: JsonEnvelope = serde_json::from_str(rendered.trim()).unwrap();
        match parsed {
            JsonEnvelope::Success {
                command,
                status,
                message,
                exit_code,
            } => {
                assert_eq!(command, "log");
                assert_eq!(status, JsonStatus::Unavailable);
                assert_eq!(exit_code, ExitCode::Unavailable.as_i32());
                assert!(message.contains("not yet supported"));
            }
            _ => panic!("expected success variant with unavailable status"),
        }
    }

    #[test]
    fn file_history_ok_envelope_preserves_revision_array_message() {
        // R9 #9: when the daemon eventually supports listrevisions it
        // will return a JSON array in `message`. Ensure the envelope
        // does not mangle the payload — callers parse it as-is.
        let payload = "[{\"rev_id\":\"deadbeef\",\"mtime\":123,\"size\":10,\"user\":\"a@b\",\"comment\":\"\"}]";
        let response = Response {
            status: ResponseStatus::Ok,
            message: payload.to_owned(),
        };
        let env = JsonEnvelope::from_response("log", &response);
        let rendered = env.render();
        // The envelope must embed the array verbatim inside the
        // `message` JSON string. Deserialising + re-reading `message`
        // round-trips to the same JSON text.
        let v: serde_json::Value = serde_json::from_str(rendered.trim()).unwrap();
        assert_eq!(v["command"], "log");
        assert_eq!(v["status"], "ok");
        assert_eq!(v["exit_code"], 0);
        let msg = v["message"].as_str().unwrap();
        let inner: serde_json::Value = serde_json::from_str(msg).unwrap();
        assert_eq!(inner[0]["rev_id"], "deadbeef");
    }

    #[test]
    fn filtered_envelope_round_trips() {
        use serde_json::json;
        let resp = Response {
            status: ResponseStatus::Ok,
            message: "ignored-by-filtered".into(),
        };
        let mut fields = serde_json::Map::new();
        fields.insert("quota".into(), json!(10737418240u64));
        fields.insert("premium".into(), json!(false));
        let env = JsonEnvelope::from_fields("userinfo", &resp, fields);
        let rendered = env.render();
        let parsed: JsonEnvelope = serde_json::from_str(rendered.trim()).unwrap();
        match parsed {
            JsonEnvelope::Filtered {
                command,
                status,
                fields,
                exit_code,
            } => {
                assert_eq!(command, "userinfo");
                assert_eq!(status, JsonStatus::Ok);
                assert_eq!(exit_code, 0);
                assert_eq!(fields["quota"], json!(10737418240u64));
                assert_eq!(fields["premium"], json!(false));
            }
            other => panic!("expected Filtered variant, got {other:?}"),
        }
    }

    #[test]
    fn every_exit_code_maps_to_error_category() {
        for code in [
            ExitCode::Ok,
            ExitCode::GenericError,
            ExitCode::Usage,
            ExitCode::Auth,
            ExitCode::Network,
            ExitCode::CryptoLocked,
            ExitCode::Unavailable,
            ExitCode::Conflict,
            ExitCode::Internal,
        ] {
            let cat = JsonErrorCategory::from_exit_code(code);
            // round-trip via JSON to make sure every category is serializable
            let s = serde_json::to_string(&cat).unwrap();
            let parsed: JsonErrorCategory = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed, cat);
        }
    }
}
