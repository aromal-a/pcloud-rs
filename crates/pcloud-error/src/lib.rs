#![forbid(unsafe_code)]
//! # pcloud-error
//!
//! Unified, top-level error taxonomy for the pcloud-rs Rust workspace.
//!
//! Historically every crate defined its own `*Error` enum. Those are retained
//! (and re-exported) for public-API stability, but every helper error in the
//! workspace can now be `From<_>`-converted into the single [`enum@Error`] type for
//! clean enterprise API boundaries (CLI exit codes, SDK consumers, structured
//! logging, IPC status serialisation).
//!
//! ## Categories
//!
//! Each variant is a category, not a specific failure. Categories mirror the
//! pCloud API error surface plus the local/transport/crypto/config boundaries:
//!
//! | Category       | Numeric code range | Description                                  |
//! |----------------|--------------------|----------------------------------------------|
//! | `Auth`         | 1000-1099          | authentication, TFA, session                 |
//! | `Permission`   | 1100-1199          | authorization / access-denied                |
//! | `Api`          | 1200-1299          | pCloud API result errors, not-found, quota   |
//! | `Transport`    | 1300-1399          | TCP, TLS, framing, binary-protocol           |
//! | `Ipc`          | 1400-1499          | local daemon IPC                             |
//! | `Protocol`     | 1500-1599          | request/response schema mismatches           |
//! | `Crypto`       | 1600-1699          | E2E crypto: locked, unset, bad password, ... |
//! | `Storage`      | 1700-1799          | local `SQLite` / vault / migrations          |
//! | `Config`       | 1800-1899          | invalid config, secure-default violations    |
//! | `LocalIo`      | 1900-1999          | local filesystem I/O                         |
//! | `NotFound`     | 2000-2099          | logical entity lookup miss                   |
//! | `InvalidInput` | 2100-2199          | caller-supplied argument rejected            |
//! | `Busy`         | 2200-2299          | resource locked / already-in-progress        |
//! | `Plugin`       | 2300-2399          | plugin registration / dispatch               |
//! | `Internal`     | 9000-9099          | bug / invariant / unexpected                 |
//!
//! The numeric codes are **stable** and covered by a snapshot test in
//! `tests/code_stability.rs`. Scripts may rely on them.
//!
//! ## Retryability policy
//!
//! Retryability is a property of the **category**, not of the message. The
//! table below is the authoritative contract for retry logic in the daemon,
//! CLI, SDK, and any external script that keys off [`Error::code`]:
//!
//! | Category       | Retryable? | Rationale                                              |
//! |----------------|------------|--------------------------------------------------------|
//! | `Auth`         | No         | Needs fresh user credentials or TFA interaction        |
//! | `Permission`   | No         | Server-side ACL decision; retry will re-deny           |
//! | `Api`          | Conditional| See per-`api_code`; `2000`/`2001` (net/limit) are yes  |
//! | `Transport`    | Yes        | Transient TCP/TLS; apply exponential backoff           |
//! | `Ipc`          | Yes        | Socket flap; reconnect and resend the envelope         |
//! | `Protocol`     | No         | Schema bug; retrying sends the same malformed payload  |
//! | `Crypto`       | No         | Locked/bad-password is a user action, not time         |
//! | `Storage`      | No         | `SQLite` corruption / migration mismatch needs repair  |
//! | `Config`       | No         | Misconfiguration; requires operator intervention       |
//! | `LocalIo`      | Conditional| `WouldBlock`/`Interrupted`: yes; permission/NF: no     |
//! | `NotFound`     | No         | Entity absent; retry returns the same result           |
//! | `InvalidInput` | No         | Caller bug; retry is pointless                         |
//! | `Busy`         | Yes        | Another operation holds the lock; backoff + retry      |
//! | `Plugin`       | No         | Registry state needs administrative correction         |
//! | `Internal`     | No         | Bug. Emit to audit, do **not** loop                    |
//!
//! ## When each variant is emitted
//!
//! * `Auth` — credential rejection from `userinfo`, expired session tokens,
//!   missing or ownership-mismatched auth vault, TFA timeout/rejection.
//! * `Permission` — API returned a 2xxx result indicating access denial, or
//!   a daemon capability check blocked the caller.
//! * `Api` — any non-zero `result` from a pCloud API call that is not
//!   already classified into a narrower category; `api_code` preserves the
//!   original numeric value so scripts can distinguish e.g. quota-exhaustion
//!   from name-collision.
//! * `Transport` — TLS handshake failure, TCP reset, binary-protocol framing
//!   mismatch. Always triggers a reconnect path in the retained daemon.
//! * `Ipc` — the local CLI-to-daemon unix-domain-socket channel dropped or
//!   rejected the envelope; distinct from `Transport` because it never
//!   leaves the host.
//! * `Protocol` — response schema mismatch, unknown method, missing required
//!   field. Indicates either server drift or a client bug.
//! * `Crypto` — crypto not set up, locked, password mismatch, fingerprint
//!   verification mismatch. Secrets are never embedded in `message`.
//! * `Storage` — `SQLite` open/migrate/statement failure; auth-vault schema
//!   mismatch; vault file ownership rejection.
//! * `Config` — rejected configuration, e.g. a production build trying to
//!   opt out of TLS or set a world-writable socket path.
//! * `LocalIo` — any `std::io::Error` from the host filesystem, wrapped
//!   verbatim via the `From<io::Error>` impl below.
//! * `NotFound` — logical lookup miss (folder id, sync root id, share id,
//!   public-link code).
//! * `InvalidInput` — argument validation at a public API boundary (empty
//!   password, non-absolute path, invalid opcode).
//! * `Busy` — sync root add/remove is already in progress, crypto unlock
//!   mid-flight, backup device being torn down.
//! * `Plugin` — plugin registry duplicate registration, dispatch to an
//!   unknown plugin, plugin signature mismatch.
//! * `Internal` — broken invariant detected in the daemon. Emit instead of
//!   panicking so IPC peers receive a structured error.
//!
//! ## Construction patterns
//!
//! ```rust
//! use pcloud_error::Error;
//!
//! let e = Error::auth("tfa rejected");
//! assert_eq!(e.code(), 1000);
//! ```
//!
//! Helper crates implement `From<TheirError> for pcloud_error::Error` so the
//! unified type participates cleanly in `?` chains. The unified type is
//! `#[non_exhaustive]`: downstream `match` sites must include a catch-all
//! arm so future categories can be added without breaking `SemVer`.

#![deny(missing_debug_implementations)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

use std::error::Error as StdError;
use std::fmt;
use std::io;

use thiserror::Error;

/// Boxed, type-erased source error used for cause chaining without dragging
/// every sub-crate's concrete error type into the unified enum.
pub type BoxedSource = Box<dyn StdError + Send + Sync + 'static>;

/// Unified, top-level error type for the pcloud-rs Rust workspace.
///
/// Each variant is a **category**. The free-form `message` captures the
/// original error's `Display`; optional `source` chains preserve the cause
/// (`std::error::Error::source` is implemented via `#[source]`).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// Authentication or session lifecycle failure (not authenticated, TFA
    /// failed, logout failed, vault missing, ...).
    #[error("auth: {message}")]
    Auth {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Authorization / permission-denied, distinct from authentication.
    #[error("permission: {message}")]
    Permission {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// pCloud API returned a non-zero `result`, or a business-logic error
    /// (quota, rate limit, not-found at API level, ...).
    #[error("api error {api_code:?}: {message}")]
    Api {
        /// Original numeric `result` code returned by the pCloud API, when known.
        api_code: Option<u64>,
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Transport-level failure (TCP, TLS, framing).
    #[error("transport: {message}")]
    Transport {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Local IPC (unix domain socket between CLI and daemon) failure.
    #[error("ipc: {message}")]
    Ipc {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Protocol / schema failure (unknown method, malformed response, ...).
    #[error("protocol: {message}")]
    Protocol {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// End-to-end crypto failure (locked, not set up, wrong password, ...).
    #[error("crypto: {message}")]
    Crypto {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Local persistent storage (`SQLite`, vault, migration) failure.
    #[error("storage: {message}")]
    Storage {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Config-layer rejection (insecure defaults, malformed paths, ...).
    #[error("config: {message}")]
    Config {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Local filesystem I/O failure.
    #[error("local io: {message}")]
    LocalIo {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Entity not found (folder, file, link, share, ...).
    #[error("not found: {message}")]
    NotFound {
        /// Human-readable description of the missing entity.
        message: String,
    },

    /// Caller-supplied input rejected (empty password, invalid path, ...).
    #[error("invalid input: {message}")]
    InvalidInput {
        /// Human-readable description of the rejection.
        message: String,
    },

    /// Resource busy / locked / already-in-progress.
    #[error("busy: {message}")]
    Busy {
        /// Human-readable description of the contended resource.
        message: String,
    },

    /// Plugin registration or dispatch failure.
    #[error("plugin: {message}")]
    Plugin {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },

    /// Internal invariant violation. Indicates a bug in the daemon, not a
    /// remote/user fault.
    #[error("internal: {message}")]
    Internal {
        /// Human-readable summary captured from the originating error.
        message: String,
        /// Optional boxed cause chain (preserved via `std::error::Error::source`).
        #[source]
        source: Option<BoxedSource>,
    },
}

impl Error {
    /// Stable numeric error code for the variant. Scripts may depend on
    /// these numbers; any change MUST update the snapshot test.
    #[must_use]
    pub fn code(&self) -> u32 {
        match self {
            Self::Auth { .. } => 1000,
            Self::Permission { .. } => 1100,
            Self::Api { .. } => 1200,
            Self::Transport { .. } => 1300,
            Self::Ipc { .. } => 1400,
            Self::Protocol { .. } => 1500,
            Self::Crypto { .. } => 1600,
            Self::Storage { .. } => 1700,
            Self::Config { .. } => 1800,
            Self::LocalIo { .. } => 1900,
            Self::NotFound { .. } => 2000,
            Self::InvalidInput { .. } => 2100,
            Self::Busy { .. } => 2200,
            Self::Plugin { .. } => 2300,
            Self::Internal { .. } => 9000,
        }
    }

    /// Whether a caller may meaningfully retry the failed operation.
    ///
    /// The mapping here is the canonical implementation of the retryability
    /// table documented at the crate root. Callers using exponential backoff
    /// should honor this directly rather than heuristically inspecting
    /// messages.
    ///
    /// For `Api`, retryability depends on the numeric `api_code`: a small
    /// allow-list of transient pCloud API codes (net-busy, rate-limit) is
    /// considered retryable; all other codes are not. For `LocalIo`, the
    /// message is inspected for the `WouldBlock`/`Interrupted` markers
    /// produced by the `From<io::Error>` impl below.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport { .. } | Self::Ipc { .. } | Self::Busy { .. } => true,
            Self::Api { api_code, .. } => matches!(api_code, Some(2000 | 2001 | 4000)),
            Self::LocalIo { message, .. } => {
                message.contains("would block") || message.contains("interrupted")
            }
            _ => false,
        }
    }

    /// Short, script-friendly category slug (stable).
    #[must_use]
    pub fn category(&self) -> &'static str {
        match self {
            Self::Auth { .. } => "auth",
            Self::Permission { .. } => "permission",
            Self::Api { .. } => "api",
            Self::Transport { .. } => "transport",
            Self::Ipc { .. } => "ipc",
            Self::Protocol { .. } => "protocol",
            Self::Crypto { .. } => "crypto",
            Self::Storage { .. } => "storage",
            Self::Config { .. } => "config",
            Self::LocalIo { .. } => "local_io",
            Self::NotFound { .. } => "not_found",
            Self::InvalidInput { .. } => "invalid_input",
            Self::Busy { .. } => "busy",
            Self::Plugin { .. } => "plugin",
            Self::Internal { .. } => "internal",
        }
    }

    // ---- constructors ----

    /// Build a new [`Error::Auth`] with no cause chain attached.
    pub fn auth(msg: impl Into<String>) -> Self {
        Self::Auth {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Permission`] with no cause chain attached.
    pub fn permission(msg: impl Into<String>) -> Self {
        Self::Permission {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Api`], optionally carrying the original numeric
    /// `result` code returned by the pCloud API.
    pub fn api(api_code: Option<u64>, msg: impl Into<String>) -> Self {
        Self::Api {
            api_code,
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Transport`] with no cause chain attached.
    pub fn transport(msg: impl Into<String>) -> Self {
        Self::Transport {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Ipc`] with no cause chain attached.
    pub fn ipc(msg: impl Into<String>) -> Self {
        Self::Ipc {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Protocol`] with no cause chain attached.
    pub fn protocol(msg: impl Into<String>) -> Self {
        Self::Protocol {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Crypto`] with no cause chain attached.
    pub fn crypto(msg: impl Into<String>) -> Self {
        Self::Crypto {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Storage`] with no cause chain attached.
    pub fn storage(msg: impl Into<String>) -> Self {
        Self::Storage {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Config`] with no cause chain attached.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::LocalIo`] with no cause chain attached.
    pub fn local_io(msg: impl Into<String>) -> Self {
        Self::LocalIo {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::NotFound`]. This variant intentionally does not
    /// carry a cause chain; see [`Self::with_source`].
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound {
            message: msg.into(),
        }
    }
    /// Build a new [`Error::InvalidInput`]. This variant intentionally does
    /// not carry a cause chain; see [`Self::with_source`].
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput {
            message: msg.into(),
        }
    }
    /// Build a new [`Error::Busy`]. This variant intentionally does not
    /// carry a cause chain; see [`Self::with_source`].
    pub fn busy(msg: impl Into<String>) -> Self {
        Self::Busy {
            message: msg.into(),
        }
    }
    /// Build a new [`Error::Plugin`] with no cause chain attached.
    pub fn plugin(msg: impl Into<String>) -> Self {
        Self::Plugin {
            message: msg.into(),
            source: None,
        }
    }
    /// Build a new [`Error::Internal`] with no cause chain attached.
    ///
    /// Prefer this over panicking for broken invariants so the daemon can
    /// surface a structured error across IPC instead of aborting.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal {
            message: msg.into(),
            source: None,
        }
    }

    /// Attach a boxed cause to a category that supports one. For
    /// `NotFound`/`InvalidInput`/`Busy` the source is silently dropped because
    /// those variants are intentionally leaf errors.
    #[must_use]
    pub fn with_source<E>(mut self, src: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        let boxed: BoxedSource = Box::new(src);
        match &mut self {
            Self::Auth { source, .. }
            | Self::Permission { source, .. }
            | Self::Api { source, .. }
            | Self::Transport { source, .. }
            | Self::Ipc { source, .. }
            | Self::Protocol { source, .. }
            | Self::Crypto { source, .. }
            | Self::Storage { source, .. }
            | Self::Config { source, .. }
            | Self::LocalIo { source, .. }
            | Self::Plugin { source, .. }
            | Self::Internal { source, .. } => {
                *source = Some(boxed);
            }
            Self::NotFound { .. } | Self::InvalidInput { .. } | Self::Busy { .. } => {}
        }
        self
    }
}

// -------- std conversions that are safe to put here --------

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::local_io(err.to_string()).with_source(err)
    }
}

/// Helper used by downstream crates that want to funnel an opaque helper
/// error into a category. Keeps the call site a one-liner.
pub trait IntoUnified {
    /// Convert `self` into a unified [`enum@Error`] of the given [`Category`],
    /// preserving `self.to_string()` as the message and attaching the
    /// original error as the `source` cause (where the variant supports one).
    fn into_unified(self, category: Category) -> Error;
}

/// Category selector used with [`IntoUnified::into_unified`].
///
/// Each variant maps 1:1 onto an [`enum@Error`] variant via [`Category::build`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Maps to [`Error::Auth`].
    Auth,
    /// Maps to [`Error::Permission`].
    Permission,
    /// Maps to [`Error::Api`] (with `api_code = None`).
    Api,
    /// Maps to [`Error::Transport`].
    Transport,
    /// Maps to [`Error::Ipc`].
    Ipc,
    /// Maps to [`Error::Protocol`].
    Protocol,
    /// Maps to [`Error::Crypto`].
    Crypto,
    /// Maps to [`Error::Storage`].
    Storage,
    /// Maps to [`Error::Config`].
    Config,
    /// Maps to [`Error::LocalIo`].
    LocalIo,
    /// Maps to [`Error::NotFound`].
    NotFound,
    /// Maps to [`Error::InvalidInput`].
    InvalidInput,
    /// Maps to [`Error::Busy`].
    Busy,
    /// Maps to [`Error::Plugin`].
    Plugin,
    /// Maps to [`Error::Internal`].
    Internal,
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auth => "auth",
            Self::Permission => "permission",
            Self::Api => "api",
            Self::Transport => "transport",
            Self::Ipc => "ipc",
            Self::Protocol => "protocol",
            Self::Crypto => "crypto",
            Self::Storage => "storage",
            Self::Config => "config",
            Self::LocalIo => "local_io",
            Self::NotFound => "not_found",
            Self::InvalidInput => "invalid_input",
            Self::Busy => "busy",
            Self::Plugin => "plugin",
            Self::Internal => "internal",
        })
    }
}

impl Category {
    /// Construct an [`enum@Error`] of this category with the supplied message
    /// and no cause chain attached. For `Api`, `api_code` is set to `None`.
    #[must_use]
    pub fn build(self, message: impl Into<String>) -> Error {
        let message = message.into();
        match self {
            Self::Auth => Error::auth(message),
            Self::Permission => Error::permission(message),
            Self::Api => Error::api(None, message),
            Self::Transport => Error::transport(message),
            Self::Ipc => Error::ipc(message),
            Self::Protocol => Error::protocol(message),
            Self::Crypto => Error::crypto(message),
            Self::Storage => Error::storage(message),
            Self::Config => Error::config(message),
            Self::LocalIo => Error::local_io(message),
            Self::NotFound => Error::not_found(message),
            Self::InvalidInput => Error::invalid_input(message),
            Self::Busy => Error::busy(message),
            Self::Plugin => Error::plugin(message),
            Self::Internal => Error::internal(message),
        }
    }
}

impl<E> IntoUnified for E
where
    E: StdError + Send + Sync + 'static,
{
    fn into_unified(self, category: Category) -> Error {
        let msg = self.to_string();
        category.build(msg).with_source(self)
    }
}

/// Convenience `Result` alias scoped to the unified error.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_categories() {
        assert_eq!(Error::auth("x").code(), 1000);
        assert_eq!(Error::permission("x").code(), 1100);
        assert_eq!(Error::api(None, "x").code(), 1200);
        assert_eq!(Error::transport("x").code(), 1300);
        assert_eq!(Error::ipc("x").code(), 1400);
        assert_eq!(Error::protocol("x").code(), 1500);
        assert_eq!(Error::crypto("x").code(), 1600);
        assert_eq!(Error::storage("x").code(), 1700);
        assert_eq!(Error::config("x").code(), 1800);
        assert_eq!(Error::local_io("x").code(), 1900);
        assert_eq!(Error::not_found("x").code(), 2000);
        assert_eq!(Error::invalid_input("x").code(), 2100);
        assert_eq!(Error::busy("x").code(), 2200);
        assert_eq!(Error::plugin("x").code(), 2300);
        assert_eq!(Error::internal("x").code(), 9000);
    }

    #[test]
    fn from_io_preserves_chain() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "nope");
        let err: Error = io_err.into();
        assert_eq!(err.code(), 1900);
        assert!(err.source().is_some());
    }

    #[test]
    fn into_unified_preserves_cause() {
        #[derive(Debug, Error)]
        #[error("inner bad thing")]
        struct Inner;
        let unified = Inner.into_unified(Category::Crypto);
        assert_eq!(unified.code(), 1600);
        assert!(unified.to_string().contains("inner bad thing"));
        assert!(unified.source().is_some());
    }

    #[test]
    fn with_source_is_noop_for_leaf_variants() {
        let e = Error::not_found("x").with_source(io::Error::other("y"));
        assert!(e.source().is_none());
    }

    #[test]
    fn retryability_matches_documented_policy() {
        assert!(Error::transport("x").is_retryable());
        assert!(Error::ipc("x").is_retryable());
        assert!(Error::busy("x").is_retryable());
        assert!(Error::api(Some(2000), "net-busy").is_retryable());
        assert!(Error::api(Some(4000), "rate-limit").is_retryable());
        assert!(!Error::api(Some(1000), "bad-login").is_retryable());
        assert!(!Error::api(None, "unknown").is_retryable());
        assert!(!Error::auth("x").is_retryable());
        assert!(!Error::permission("x").is_retryable());
        assert!(!Error::crypto("x").is_retryable());
        assert!(!Error::storage("x").is_retryable());
        assert!(!Error::config("x").is_retryable());
        assert!(!Error::not_found("x").is_retryable());
        assert!(!Error::invalid_input("x").is_retryable());
        assert!(!Error::plugin("x").is_retryable());
        assert!(!Error::protocol("x").is_retryable());
        assert!(!Error::internal("x").is_retryable());
    }

    #[test]
    fn category_display_is_stable() {
        assert_eq!(Category::Auth.to_string(), "auth");
        assert_eq!(Category::LocalIo.to_string(), "local_io");
    }
}
