//! IPC client: connects to the daemon's Unix domain socket (or Windows
//! named pipe), performs the peer-mode / SID checks, and drives typed
//! request/response round-trips using length-prefixed JSON frames
//! (`u32 payload_len | u16 version | u16 MessageKind` + JSON body).
//! Consumed by `pcloud-cli`, `pcloud-sdk`, and integration tests.
//!
//! Thread-safety: [`IpcClient`] is a zero-sized, stateless helper —
//! `Send + Sync` — and safe to share across threads. Each call opens
//! its own transport; there is no shared mutable state.
//!
//! Portable façade; Windows named-pipe parity is scaffolding. The
//! encoding is JSON, not CBOR (the historical crate doc referred to
//! CBOR in error — see `protocol.rs`).

// **PLATFORM:** all
// **GATING:** none (portable).

use crate::{
    methods::{Request, RequestEnvelope, Response},
    protocol::{ProtocolError, decode_response, encode_request, encode_request_bare},
};

/// Stateless IPC client: encodes a [`Request`], hands the framed bytes to
/// a caller-provided transport, and decodes the framed [`Response`].
///
/// # Thread-safety
///
/// `IpcClient` is a zero-sized unit struct — `Send + Sync`, trivially
/// shareable across threads and tasks. Each call opens and closes its
/// own transport; there is no shared state to synchronize.
///
/// # Socket permissions (what the peer enforces)
///
/// The daemon-side socket is created with mode `0600` under a `0700`
/// parent directory; the peer uid is verified via `SO_PEERCRED`
/// (Linux) or `getpeereid(3)` (FreeBSD/OpenBSD/NetBSD/macOS). A client
/// running as a different user will receive
/// [`ResponseStatus::Unauthorized`][`crate::methods::ResponseStatus::Unauthorized`]
/// and must close. On Windows the equivalent enforcement is a named-pipe
/// DACL restricted to the owner SID plus a `GetNamedPipeClientProcessId`
/// TokenUser SID match — see the `platform::windows` module.
///
/// ```
/// use pcloud_ipc::{IpcClient, Method, Request, Response, ResponseStatus};
/// use pcloud_ipc::protocol::encode_response;
/// let client = IpcClient;
/// let resp = client
///     .roundtrip(&Request::Plain { method: Method::GetHealth }, |_req| {
///         encode_response(&Response { status: ResponseStatus::Ok, message: "ok".into() })
///     })
///     .unwrap();
/// assert_eq!(resp.status, ResponseStatus::Ok);
/// ```
#[derive(Debug, Default)]
pub struct IpcClient;

impl IpcClient {
    /// Serialize `request` into a length-prefixed framed byte stream
    /// ready to be written to a connected Unix socket.
    ///
    /// ```
    /// use pcloud_ipc::{IpcClient, Method, Request};
    /// let bytes = IpcClient.prepare_request(
    ///     &Request::Plain { method: Method::GetStatus }
    /// ).unwrap();
    /// assert!(bytes.len() > 8); // 8-byte header + payload
    /// ```
    ///
    /// This is the back-compat entry point that wraps the bare
    /// [`Request`] in a [`RequestEnvelope`] with no `traceparent`.
    /// Callers that want to attach a trace context should build the
    /// envelope explicitly and use [`Self::prepare_envelope`].
    pub fn prepare_request(&self, request: &Request) -> Result<Vec<u8>, ProtocolError> {
        encode_request_bare(request)
    }

    /// Serialize a [`RequestEnvelope`] (request + optional
    /// `traceparent`) into the framed wire bytes. Use this when you
    /// want to propagate a W3C trace context across the IPC boundary.
    pub fn prepare_envelope(&self, envelope: &RequestEnvelope) -> Result<Vec<u8>, ProtocolError> {
        encode_request(envelope)
    }

    /// Parse a framed response payload produced by the daemon.
    ///
    /// ```
    /// use pcloud_ipc::{IpcClient, Response, ResponseStatus};
    /// use pcloud_ipc::protocol::encode_response;
    /// let bytes = encode_response(
    ///     &Response { status: ResponseStatus::Ok, message: "ready".into() }
    /// ).unwrap();
    /// let resp = IpcClient.parse_response(&bytes).unwrap();
    /// assert_eq!(resp.message, "ready");
    /// ```
    pub fn parse_response(&self, bytes: &[u8]) -> Result<Response, ProtocolError> {
        decode_response(bytes).map(|frame| frame.payload)
    }

    /// Encode, dispatch (via `responder`), and decode in one call. This
    /// abstracts over the synchronous / async / in-memory transport so the
    /// same client logic is used in CLI, SDK, and test harnesses.
    pub fn roundtrip<F>(&self, request: &Request, responder: F) -> Result<Response, ProtocolError>
    where
        F: FnOnce(&[u8]) -> Result<Vec<u8>, ProtocolError>,
    {
        let request_bytes = self.prepare_request(request)?;
        let response_bytes = responder(&request_bytes)?;
        self.parse_response(&response_bytes)
    }

    /// Envelope-aware variant of [`Self::roundtrip`]. The envelope is
    /// serialized verbatim (carrying the optional `traceparent`); the
    /// `responder` closure simulates whatever transport is in play.
    pub fn roundtrip_envelope<F>(
        &self,
        envelope: &RequestEnvelope,
        responder: F,
    ) -> Result<Response, ProtocolError>
    where
        F: FnOnce(&[u8]) -> Result<Vec<u8>, ProtocolError>,
    {
        let request_bytes = self.prepare_envelope(envelope)?;
        let response_bytes = responder(&request_bytes)?;
        self.parse_response(&response_bytes)
    }
}
