//! Enterprise exit-code discipline for `pcloud-cli`.
//!
//! Each variant maps to a stable numeric exit code. Scripts and CI can rely on
//! these values. They are documented in the `--help` output via
//! [`EXIT_CODE_HELP`].
//!
//! Mapping (stable):
//!
//! - `0`   Ok                — command completed successfully
//! - `1`   GenericError      — unclassified runtime failure
//! - `2`   Usage             — argument parsing / usage error
//! - `3`   Auth              — authentication or authorization failure
//! - `4`   Network           — transport / IPC / network failure
//! - `5`   CryptoLocked      — crypto path locked or unavailable
//! - `6`   Unavailable       — daemon unavailable / disabled feature
//! - `7`   Conflict          — conflicting state (e.g. duplicate sync root)
//! - `8`   Internal          — daemon reported internal error

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_ipc::ResponseStatus;

/// Documented exit-code reference rendered in `--help`.
pub const EXIT_CODE_HELP: &str = "\
EXIT CODES:
  0  ok
  1  generic error
  2  usage / argument parsing error
  3  authentication / authorization failure
  4  network / IPC transport failure
  5  crypto locked / unavailable
  6  feature or daemon unavailable
  7  conflicting state
  8  daemon internal error";

/// Process exit code surfaced by `pcloudc`.
///
/// # Stable-ABI guarantee
///
/// The numeric discriminants below are a **public contract** for shell
/// scripts, CI pipelines, and orchestrators that branch on
/// `pcloudc`'s exit status. They are part of the crate's semver
/// surface:
///
/// - **patch/minor releases** MUST NOT change the integer value of any
///   existing variant, MUST NOT reuse a freed value for a different
///   meaning, and MUST NOT reorder variants in a way that changes
///   numbers,
/// - **patch/minor releases** MAY add new variants with fresh integer
///   values at the end of the enum (forward-compat: scripts that only
///   inspect the documented codes continue to work),
/// - a **major release** is required to remove or renumber a variant.
///
/// The mapping is also printed in `--help` via [`EXIT_CODE_HELP`] so
/// users never have to cross-reference source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    /// Command completed successfully (exit `0`).
    Ok = 0,
    /// Unclassified runtime failure (exit `1`) — fallback bucket when
    /// nothing more specific matched.
    GenericError = 1,
    /// Argument / usage error (exit `2`) — e.g. unknown flag, missing
    /// required positional argument, unknown subcommand.
    Usage = 2,
    /// Authentication or authorization failure (exit `3`) — bad
    /// credentials, expired token, TFA cancellation, daemon
    /// `Unauthorized`.
    Auth = 3,
    /// Transport-layer / IPC / network failure (exit `4`) — socket
    /// refused, connect timeout, broken pipe.
    Network = 4,
    /// Crypto subsystem is locked or unavailable (exit `5`) — needed
    /// when an operation requires an unlocked crypto folder but the
    /// user has not submitted the crypto password.
    CryptoLocked = 5,
    /// Daemon unavailable or the feature is disabled (exit `6`) — also
    /// the canonical `doctor` failure code (see [`crate::doctor`]).
    Unavailable = 6,
    /// Conflicting state (exit `7`) — e.g. duplicate sync-root,
    /// already-mounted path.
    Conflict = 7,
    /// Daemon reported an internal error (exit `8`) — treated as a
    /// bug; operator should collect logs and file a ticket.
    Internal = 8,
}

impl ExitCode {
    #[must_use]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Map an IPC [`ResponseStatus`] to the documented exit code.
    #[must_use]
    pub fn from_response_status(status: &ResponseStatus) -> Self {
        match status {
            ResponseStatus::Ok => Self::Ok,
            ResponseStatus::InvalidRequest => Self::Usage,
            ResponseStatus::Unauthorized => Self::Auth,
            ResponseStatus::Conflict => Self::Conflict,
            ResponseStatus::Unavailable => Self::Unavailable,
            ResponseStatus::InternalError => Self::Internal,
            ResponseStatus::PolicyViolation { .. } => Self::Conflict,
            _ => Self::Internal,
        }
    }

    /// Classify a free-form transport error message.
    ///
    /// The IPC client returns opaque error strings; we pattern-match on a
    /// conservative set of substrings to separate auth/crypto/network failures
    /// from generic ones. Unknown strings fall back to `Network` because that
    /// is the most common class of transport failure.
    #[must_use]
    pub fn classify_transport_error(msg: &str) -> Self {
        let l = msg.to_ascii_lowercase();
        if l.contains("unauthorized") || l.contains("auth") && l.contains("fail") {
            Self::Auth
        } else if l.contains("crypto") && (l.contains("lock") || l.contains("unavailable")) {
            Self::CryptoLocked
        } else if l.contains("connect")
            || l.contains("connection")
            || l.contains("timed out")
            || l.contains("timeout")
            || l.contains("refused")
            || l.contains("broken pipe")
            || l.contains("socket")
            || l.contains("network")
            || l.contains("no such file")
        {
            Self::Network
        } else {
            Self::GenericError
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_status_mapping_is_stable() {
        assert_eq!(
            ExitCode::from_response_status(&ResponseStatus::Ok).as_i32(),
            0
        );
        assert_eq!(
            ExitCode::from_response_status(&ResponseStatus::InvalidRequest).as_i32(),
            2
        );
        assert_eq!(
            ExitCode::from_response_status(&ResponseStatus::Unauthorized).as_i32(),
            3
        );
        assert_eq!(
            ExitCode::from_response_status(&ResponseStatus::Conflict).as_i32(),
            7
        );
        assert_eq!(
            ExitCode::from_response_status(&ResponseStatus::Unavailable).as_i32(),
            6
        );
        assert_eq!(
            ExitCode::from_response_status(&ResponseStatus::InternalError).as_i32(),
            8
        );
    }

    #[test]
    fn transport_error_classification() {
        assert_eq!(
            ExitCode::classify_transport_error("connection refused"),
            ExitCode::Network
        );
        assert_eq!(
            ExitCode::classify_transport_error("socket not found"),
            ExitCode::Network
        );
        assert_eq!(
            ExitCode::classify_transport_error("request timed out"),
            ExitCode::Network
        );
        assert_eq!(
            ExitCode::classify_transport_error("no such file or directory"),
            ExitCode::Network
        );
        assert_eq!(
            ExitCode::classify_transport_error("auth failure"),
            ExitCode::Auth
        );
        assert_eq!(
            ExitCode::classify_transport_error("Unauthorized"),
            ExitCode::Auth
        );
        assert_eq!(
            ExitCode::classify_transport_error("crypto is locked"),
            ExitCode::CryptoLocked
        );
        assert_eq!(
            ExitCode::classify_transport_error("totally unexpected thing"),
            ExitCode::GenericError
        );
    }

    #[test]
    fn ok_is_zero() {
        assert_eq!(ExitCode::Ok.as_i32(), 0);
    }
}
