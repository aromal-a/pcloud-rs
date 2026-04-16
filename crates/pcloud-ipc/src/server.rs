//! # IPC server helpers
//!
//! **PLATFORM: all.**
//! **GATING: none — this module is transport-agnostic. The owner-uid
//! authorization check it performs is meaningful only once the transport
//! has recovered a real peer uid via the platform backend
//! ([`crate::platform`]). On Linux that is `SO_PEERCRED`, on BSD/macOS
//! `getpeereid(3)`, on Windows the SID comparison is still stubbed.**

use thiserror::Error;

use crate::{
    auth::PeerIdentity,
    methods::{Request, RequestEnvelope, Response, ResponseStatus},
    protocol::{ProtocolError, decode_request, encode_response},
};

/// Hard cap on the declared length of an inbound IPC frame payload:
/// one mebibyte (`1 * 1024 * 1024 = 1_048_576` bytes).
///
/// # Rationale (P0.8 OOM cap)
///
/// The 8-byte frame header carries an attacker-controlled
/// `u32 payload_len` prefix. Without a bound, a malicious peer could
/// declare a 4 GiB payload and force the daemon to pre-allocate a 4 GiB
/// buffer before reading anything. `MAX_REQUEST_BYTES` enforces the cap
/// *before* any allocation proportional to the declared length — see
/// `transport::read_framed_request` where the check precedes
/// `Vec::with_capacity(8 + payload_len)` and `vec![0u8; payload_len]`.
///
/// 1 MiB is two orders of magnitude above the largest real request in
/// the schema (pcloudc commands are well under 64 KiB in practice),
/// leaving comfortable headroom for future fields while keeping
/// per-connection peak memory bounded.
///
/// Matches [`crate::protocol::MAX_IPC_PAYLOAD_LEN`] on the framing side.
///
/// ```
/// assert_eq!(pcloud_ipc::server::MAX_REQUEST_BYTES, 1024 * 1024);
/// ```
#[allow(clippy::identity_op)]
pub const MAX_REQUEST_BYTES: usize = 1 * 1024 * 1024;

/// Server-side IPC errors that are distinct from protocol decode errors.
///
/// These are raised *before* any allocation proportional to client-declared
/// sizes, so an attacker cannot use a forged length prefix to cause OOM.
#[derive(Debug, Error)]
pub enum IpcError {
    /// The peer's declared frame length exceeds [`MAX_REQUEST_BYTES`].
    /// Emitted by `transport::read_framed_request` *before* any
    /// allocation proportional to the declared length — this is the
    /// pre-allocation guard that satisfies the P0.8 OOM-cap invariant.
    ///
    /// # Recovery
    /// Fatal for the connection. The caller MUST close the socket
    /// without writing a response; a reply would itself be an
    /// amplification vector. Not retryable without reducing the
    /// request body. The listener continues to accept new clients.
    #[error("ipc request declared {declared} bytes, exceeds per-request maximum of {max} bytes")]
    RequestTooLarge {
        /// Number of bytes the peer declared in the frame length prefix.
        declared: usize,
        /// Maximum bytes allowed per request, mirroring [`MAX_REQUEST_BYTES`].
        max: usize,
    },
}

/// Stateless IPC server helper. Owns the `owner_uid` used to reject
/// connections from any other local user.
///
/// # Thread-safety
///
/// `IpcServer` carries only a `u32` owner id and is `Send + Sync`,
/// trivially shareable across threads. The bound socket wrapper
/// ([`crate::transport::BoundIpcServer`]) is also `Send + Sync`:
/// concurrent `accept()` from multiple threads is permitted.
///
/// # Socket permissions
///
/// On Unix, [`crate::transport::BoundIpcServer`] creates the socket
/// file with mode `0600` (owner read/write only) under a `0700` parent
/// directory, and authorizes each accepted connection by recovering the
/// peer uid via `SO_PEERCRED` (Linux) or `getpeereid(3)` (BSD/macOS).
/// Mismatched uids are rejected with
/// [`ResponseStatus::Unauthorized`][`crate::methods::ResponseStatus::Unauthorized`].
/// On Windows the equivalent is a named pipe with a DACL granting
/// `GENERIC_READ|GENERIC_WRITE` only to the current-user SID plus a
/// `GetNamedPipeClientProcessId`-driven TokenUser SID comparison —
/// see the `platform::windows` module.
///
/// ```
/// use pcloud_ipc::{IpcServer, auth::PeerIdentity};
/// let server = IpcServer::new(1000);
/// assert!(server.authorize_peer(&PeerIdentity { uid: 1000, pid: 1 }));
/// assert!(!server.authorize_peer(&PeerIdentity { uid: 0, pid: 1 }));
/// ```
#[derive(Debug)]
pub struct IpcServer {
    owner_uid: u32,
}

impl IpcServer {
    /// Construct a server that only accepts connections from `owner_uid`.
    ///
    /// ```
    /// let server = pcloud_ipc::IpcServer::new(1000);
    /// assert_eq!(server.owner_uid(), 1000);
    /// ```
    #[must_use]
    pub fn new(owner_uid: u32) -> Self {
        Self { owner_uid }
    }

    /// The uid bound at construction time.
    #[must_use]
    pub fn owner_uid(&self) -> u32 {
        self.owner_uid
    }

    /// Owner-only authorization check applied before any request is
    /// decoded. Reject on `false`.
    ///
    /// ```
    /// use pcloud_ipc::{IpcServer, auth::PeerIdentity};
    /// let s = IpcServer::new(42);
    /// assert!(s.authorize_peer(&PeerIdentity { uid: 42, pid: 9 }));
    /// ```
    #[must_use]
    pub fn authorize_peer(&self, peer: &PeerIdentity) -> bool {
        peer.matches_owner(self.owner_uid)
    }

    /// Produce a framed `(status, message)` response for the wire.
    ///
    /// ```
    /// use pcloud_ipc::{IpcServer, ResponseStatus};
    /// let bytes = IpcServer::new(0).encode_status(ResponseStatus::Ok, "ready").unwrap();
    /// assert!(bytes.len() > 8);
    /// ```
    pub fn encode_status(
        &self,
        status: ResponseStatus,
        message: impl Into<String>,
    ) -> Result<Vec<u8>, ProtocolError> {
        encode_response(&Response {
            status,
            message: message.into(),
        })
    }

    /// Decode an inbound framed request from its wire bytes. Returns the
    /// inner [`Request`] for back-compat with dispatchers that have not
    /// been updated to consume the envelope; observability-aware callers
    /// should use [`Self::decode_envelope`] to also recover the optional
    /// `traceparent` attached at the transport boundary.
    ///
    /// ```
    /// use pcloud_ipc::{IpcServer, Method, Request};
    /// use pcloud_ipc::protocol::encode_request_bare;
    /// let bytes = encode_request_bare(&Request::Plain { method: Method::GetHealth }).unwrap();
    /// let req = IpcServer::new(0).decode_request(&bytes).unwrap();
    /// assert!(matches!(req, Request::Plain { method: Method::GetHealth }));
    /// ```
    pub fn decode_request(&self, bytes: &[u8]) -> Result<Request, ProtocolError> {
        self.decode_envelope(bytes).map(|env| env.request)
    }

    /// Decode an inbound framed request and return the full
    /// [`RequestEnvelope`] (request + optional `traceparent`). Use this
    /// from observability-aware dispatchers that want to re-attach the
    /// trace context before invoking backend logic.
    ///
    /// ```
    /// use pcloud_ipc::{IpcServer, Method, Request};
    /// use pcloud_ipc::protocol::encode_request_bare;
    /// let bytes = encode_request_bare(&Request::Plain { method: Method::GetHealth }).unwrap();
    /// let env = IpcServer::new(0).decode_envelope(&bytes).unwrap();
    /// assert!(matches!(env.request, Request::Plain { method: Method::GetHealth }));
    /// assert!(env.traceparent().is_none());
    /// ```
    pub fn decode_envelope(&self, bytes: &[u8]) -> Result<RequestEnvelope, ProtocolError> {
        decode_request(bytes).map(|frame| frame.payload)
    }
}
