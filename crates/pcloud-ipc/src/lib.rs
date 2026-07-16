#![warn(unsafe_op_in_unsafe_fn)]
// IPC crate requires targeted unsafe for libc peer-credential and socket
// option syscalls.  Individual call-sites carry SAFETY comments.
//! # pcloud-ipc
//!
//! Local IPC transport between CLI/SDK clients and the daemon: Unix
//! domain socket with mode `0600` under a `0700` parent directory;
//! peer-UID is checked on accept via `SO_PEERCRED` (Linux),
//! `getpeereid(3)` (FreeBSD/OpenBSD/NetBSD/macOS), or a named-pipe plus
//! TokenUser SID DACL comparison (Windows). Framed JSON payloads behind
//! an 8-byte little-endian header (u32 `payload_len`, u16 `version`,
//! u16 `MessageKind`); typed [`Method`], [`Request`], and [`Response`]
//! schema.
//!
//! The prior revision of this doc block described payloads as CBOR. The
//! real codec has always been `serde_json::{to_vec, from_slice}` — see
//! [`protocol::encode_request`] / [`protocol::encode_response`]. The
//! mismatch is corrected here.
//!
//! **Architecture:** see `docs/book/src/architecture/crate-map.md` and
//! `ARCHITECTURE.md` ("IPC protocol reference") and
//! `SECURITY-MODEL.md` ("IPC permissions"). Consumed by
//! `pcloud-daemon`, `pcloud-cli`, and `pcloud-sdk`.
//!
//! **Stability:** T1 internal — schema versioned via `ProtocolVersion`;
//! not semver-stable across workspace revisions.
//!
//! **MSRV:** Rust 1.89 for the portable crate; full workspace and release
//! validation use the repository-pinned Rust 1.96.1 toolchain.
//!
//! **Features:** none.
//!
//! **Platform:** portable façade over `platform/{unix,linux,windows}`.
//! Production deployments use owner-only Unix sockets on Unix-family
//! systems and an owner-SID named pipe on Windows.
//!
//! # Examples
//!
//! Encode a request and decode it back on the other side of a framed
//! Unix-socket connection:
//!
//! ```
//! use pcloud_ipc::{Method, Request, decode_request, encode_request_bare};
//! let bytes = encode_request_bare(&Request::Plain { method: Method::GetHealth }).unwrap();
//! let frame = decode_request(&bytes).unwrap();
//! assert!(matches!(frame.payload.request, Request::Plain { method: Method::GetHealth }));
//! ```

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

pub mod auth;
pub mod client;
pub mod methods;
pub mod platform;
pub mod protocol;
pub mod redacted;
pub mod server;
pub mod transport;

/// Crate identifier for audit/telemetry.
///
/// ```
/// assert_eq!(pcloud_ipc::CRATE_NAME, "pcloud-ipc");
/// ```
pub const CRATE_NAME: &str = "pcloud-ipc";

/// Zero-sized marker type retained for historical embedders. New code
/// should construct [`IpcServer`] or [`IpcClient`] directly.
///
/// ```
/// let _shell = pcloud_ipc::IpcShell::default();
/// ```
#[derive(Debug, Default)]
pub struct IpcShell;

pub use auth::{PeerIdentity, current_effective_uid};
pub use client::IpcClient;
pub use methods::{
    AuditVerifierStatusPayload, AuditVerifyRange, ConflictEntry, DrainStatusPayload,
    IntegrityStatusPayload, ListFolderEntry, Method, ReadRangePayload, RemoteCopyPayload,
    RemoteDownloadPayload, RemoteUploadPayload, Request, RequestEnvelope, Response, ResponseStatus,
    SessionStatusPayload, SloReportEntry, SloReportPayload, SnapshotAction, StatPathPayload,
    UploadConflictMode, ValueKvKind, ValueKvPayload,
};
pub use protocol::{
    Frame, FrameHeader, MessageKind, decode_request, decode_response, encode_request,
    encode_request_bare, encode_response,
};
pub use redacted::RedactedString;
pub use server::{IpcError, IpcServer, MAX_REQUEST_BYTES};
pub use transport::{
    BoundIpcServer, IpcTransportError, MAX_IPC_CONNECTIONS, MAX_IPC_CONNECTIONS_PER_PEER,
    ipc_connection_cap, ipc_connection_cap_per_peer, set_ipc_connection_caps,
};
