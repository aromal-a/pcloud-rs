#![forbid(unsafe_code)]
//! # pcloud-proto
//!
//! Typed pCloud protocol clients: auth, transfer (sync and async),
//! folder, sync, public links, shares, account, backup, crypto, and
//! notifications. Responses are fully typed — malformed payloads yield
//! typed errors, never panics. TLS is mandatory in production profiles.
//!
//! ## Role in the request pipeline
//!
//! This crate sits between `pcloud-transport` (raw TCP / HTTP / TLS
//! bytes) and the higher-level daemon / SDK layers:
//!
//! 1. The daemon constructs a typed request builder (see [`methods`]).
//! 2. The builder is encoded into an [`EncodedRequest`] via the
//!    binary-protocol framer in [`binary_api`] — strings are
//!    length-prefixed, numbers are little-endian, and the total request
//!    frame is bounded by [`binary_api::MAX_REQUEST_FRAME_LEN`].
//! 3. The encoded request is handed to a [`BinaryApiTransport`] (for
//!    the binary protocol) or an HTTP helper (see [`http_download`])
//!    which writes bytes to the wire and reads the response frame.
//! 4. The response frame is parsed by [`parse_response_frame`] into a
//!    typed [`response::Value`] tree, which the `*_api` modules project
//!    into domain types like [`UserInfo`] or [`RemoteFolderListing`].
//!
//! Every step surfaces typed errors; there are no panics on malformed
//! input and no `unwrap` on network-derived data.
//!
//! ## Security posture
//!
//! - TLS is mandatory in production — [`TransportConfig::use_tls`] is
//!   set to `true` and enforced by the daemon bootstrap. Plaintext
//!   transport is only reachable in tests and via explicit opt-in, and
//!   is rejected by the production profile.
//! - Parser limits ([`ParseLimits`]) cap frame length, nesting depth,
//!   array / hash sizes, and string lengths to defend against resource
//!   exhaustion from a malicious or buggy server.
//! - Untrusted input (server-sent bytes) is never fed into `unsafe`,
//!   never used as an allocator size without bound checks, and never
//!   used to drive control flow via panics.
//! - Crypto-sensitive inputs (passwords, TFA codes) are passed through
//!   `pcloud-secret` wrappers at higher layers; this crate avoids
//!   logging request parameters and does not place secret material on
//!   long-lived structs.
//!
//! **Architecture:** see `docs/book/src/architecture/crate-map.md` and
//! `docs/book/src/architecture/request-lifecycle.md`. Consumed by
//! `pcloud-backends` and `pcloud-daemon`; wraps
//! `pcloud-transport` / HTTP transports.
//!
//! **Stability:** T1 internal — API evolves with the upstream pCloud
//! binary/HTTP protocol and is not semver-stable.
//!
//! **MSRV:** Rust 1.82 (workspace-pinned; edition 2024).
//!
//! **Features:** none. TLS/transport selection is controlled by the
//! transport crate, not this one.
//!
//! **Platform:** portable.

// The pcloud-proto method builders pre-size `params` with
// `Vec::with_capacity(N)` and push exactly N entries so the capacity hint
// is preserved as documentation of the final wire-param count. Clippy's
// `vec_init_then_push` lint would rewrite that to `vec![..]` which loses
// the intent; silence it crate-wide.
#![allow(clippy::vec_init_then_push)]
// P3.5 docs pass complete: every `pub` item carries a rustdoc comment
// (see doc insertion pass 2026-04-15). The crate now enforces full doc
// coverage; reintroducing an undocumented `pub` item will fail the build.
#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

pub mod account_api;
pub mod async_transfer;
pub mod auth_api;
pub mod backup_api;
pub mod binary_api;
pub mod crypto_api;
pub mod diff_api;
pub mod folder_api;
pub mod http_download;
pub mod methods;
pub mod notifications_api;
pub mod public_links_api;
pub mod resilient_transport;
pub mod response;
pub mod revision_provider;
pub mod shares_api;
pub mod sync_api;
pub mod transfer_api;
pub mod transport;

/// Human-readable crate identifier.
///
/// Exposed as a constant so daemons, loggers, and telemetry emitters
/// can tag events with a stable name without hard-coding the string.
/// Useful in panic hooks, `User-Agent` construction, and diagnostic
/// dumps. The value matches the `name` field in this crate's
/// `Cargo.toml`.
pub const CRATE_NAME: &str = "pcloud-proto";

/// Two-component semantic identifier of the pCloud wire protocol a
/// given request or response conforms to.
///
/// ## Wire layout
///
/// pCloud's binary and HTTP protocols do not carry a version byte on
/// the wire; version negotiation is implicit via the `login` /
/// `userinfo` command set the server accepts. This type is therefore
/// a *logical* descriptor used by higher layers to pick encoders and
/// parsers, not a value ever read from or written to the socket.
///
/// ## Design choices
///
/// Two `u16` fields (rather than a single packed integer or a string)
/// let callers compare with `==`, derive `Ord`, and format as
/// `"{major}.{minor}"` without allocating. The struct is *not* marked
/// `#[non_exhaustive]` because the protocol only admits two
/// well-defined components; adding a third would be a breaking change
/// by design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolVersion {
    /// Major version component.
    ///
    /// Incremented for wire-incompatible protocol revisions; a change
    /// here implies that encoders from an older crate version will no
    /// longer interoperate.
    pub major: u16,
    /// Minor version component.
    ///
    /// Incremented for backwards-compatible additions (new optional
    /// parameters, new response fields). Callers should tolerate minor
    /// bumps without code changes.
    pub minor: u16,
}

pub use account_api::{AccountApi, AccountApiError, ApiServerInfo, PromoInfo};
pub use auth_api::{
    ApiServerHint, ApiServerHintConsumer, AuthApi, AuthApiError, AuthRefreshError, LoggedDevice,
    PasswordLoginOutcome, ProtocolTransport, TwoFactorNotificationDelivery, TwoFactorSmsDelivery,
    UserInfo,
};
pub use backup_api::{BackupApi, BackupApiError, CreatedBackup};
pub use binary_api::{
    BinaryParam, BinaryParamValue, EncodedRequest, FrameParseError, encode_request,
};
pub use crypto_api::{CryptoApi, CryptoApiError};
pub use diff_api::{
    DiffApi, DiffApiError, DiffEntry as DiffApiEntry, DiffFileMetadata, DiffResponse,
    DiffResponseBatch, DiffResponseEntry, diff_response_to_batch,
};
pub use folder_api::{
    CreateFolderResponse, FolderApi, FolderApiError, FsMutationErrorClass, RemoteFolderEntry,
    RemoteFolderInfo, RemoteFolderListing, RenamedFolderResponse,
};
pub use http_download::{
    HttpDownloadConfig, HttpDownloadError, ResumableOutcome, SignedDownload, fetch_download,
    fetch_download_resumable, fetch_download_verified, fetch_download_verified_streaming,
};
pub use methods::ProtocolMethod;
pub use notifications_api::{Notification, NotificationsApi, NotificationsApiError};
pub use public_links_api::{PublicLinksApi, PublicLinksApiError};
pub use resilient_transport::{
    Classifier, ErrorClass, RateLimitMode, ResilientError, ResilientTransport, ThreadSleepWaiter,
    Waiter, default_classifier,
};
pub use response::{ParseLimits, ResponseParseError, parse_response_frame};
pub use shares_api::{SharesApi, SharesApiError};
pub use sync_api::{DiffBatch, DiffEntry, DiffEntryMetadata, SyncApi, SyncApiError};
pub use transfer_api::{
    BlockChecksum, BlockChecksumHeader, ChecksumLink, DownloadLink, PSYNC_COPY_BUFFER_SIZE,
    PSYNC_MAX_COPY_FROM_REQ, PSYNC_MAX_PENDING_UPLOAD_REQS, PSYNC_MIN_SIZE_FOR_CHECKSUMS,
    PSYNC_SLEEP_ON_FAILED_UPLOAD_MS, RenamedFileResponse, TransferApi, TransferApiError,
    UploadErrorClass, UploadFileResult, UploadInfo, UploadSession, decode_block_checksums,
    upload_sha1_hex,
};
pub use transport::{BinaryApiTransport, TransportConfig, TransportError};
