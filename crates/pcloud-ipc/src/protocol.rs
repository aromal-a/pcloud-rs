//! Wire protocol: [`Request`] / [`Response`] envelopes, framing,
//! protocol-version negotiation, and JSON encode/decode helpers.
//! Shared by `client` and `server`; consumed by downstream callers
//! (`pcloud-daemon`, `pcloud-cli`, `pcloud-sdk`).
//!
//! Portable; the on-wire payload codec is platform-neutral JSON via
//! `serde_json::{to_vec, from_slice}`. Earlier revisions of this module
//! described the body as CBOR — that was never accurate; the code has
//! always emitted JSON. The 8-byte little-endian frame header is:
//!
//! ```text
//! offset 0..4 : u32 payload_len   // JSON byte length, ≤ MAX_IPC_PAYLOAD_LEN
//! offset 4..6 : u16 version       // IPC_PROTOCOL_VERSION = 1
//! offset 6..8 : u16 message_type  // 1=Request, 2=Response, 3=Event
//! offset 8..  : JSON body         // serde_json::to_vec(...) output
//! ```
//!
//! Error recovery: [`ProtocolError::TruncatedHeader`] and
//! [`ProtocolError::PayloadTooLarge`] leave the transport in a
//! non-framed-recoverable state — the caller must close the connection.
//! [`ProtocolError::VersionMismatch`] and [`ProtocolError::Codec`] can be
//! surfaced to the peer as an `InvalidRequest` response and the
//! connection can continue to serve subsequent clients.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::methods::{Request, RequestEnvelope, Response};

/// Current IPC framing version. Incompatible wire changes must bump this
/// so old clients are rejected with `ProtocolError::VersionMismatch`.
///
/// ```
/// assert_eq!(pcloud_ipc::protocol::IPC_PROTOCOL_VERSION, 1);
/// ```
pub const IPC_PROTOCOL_VERSION: u16 = 1;

/// Minimum IPC protocol version accepted by the server.
///
/// Clients advertising a version in the range
/// `[MIN_ACCEPTED_IPC_PROTOCOL_VERSION, IPC_PROTOCOL_VERSION]` are
/// admitted with a deprecation log at `warn!` level so operators are
/// notified without breaking running clients during rolling upgrades.
/// Clients advertising a version below this floor or above the current
/// version are rejected with [`ProtocolError::VersionMismatch`].
///
/// The window is intentionally narrow (one minor step). When a
/// breaking wire change is made, bump `IPC_PROTOCOL_VERSION` and update
/// this constant to `IPC_PROTOCOL_VERSION - 1` to allow one-version
/// rolling upgrades, or to `IPC_PROTOCOL_VERSION` to require hard
/// cutover.
pub const MIN_ACCEPTED_IPC_PROTOCOL_VERSION: u16 = 1;

/// Hard cap on an IPC payload. Protects the decoder from a malicious
/// `payload_len` prefix demanding unbounded allocation.
///
/// ```
/// assert_eq!(pcloud_ipc::protocol::MAX_IPC_PAYLOAD_LEN, 1024 * 1024);
/// ```
pub const MAX_IPC_PAYLOAD_LEN: usize = 1024 * 1024;

/// Wire tag for the top-level message classifier carried in the 8-byte
/// frame header. Numeric values are stable across the protocol version;
/// decoders reject any tag that is not valid for the specific decode path
/// before attempting JSON deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageKind {
    /// Client-to-daemon request envelope. Wire tag: `1`.
    Request = 1,
    /// Daemon-to-client response envelope. Wire tag: `2`.
    Response = 2,
    /// Reserved: daemon-to-client push event. Wire tag: `3`.
    Event = 3,
}

/// Parsed form of the 8-byte little-endian IPC frame header.
///
/// Layout on the wire (little-endian):
///
/// ```text
/// offset 0..4  : u32 payload_len  (JSON byte length, MAX = 1 MiB)
/// offset 4..6  : u16 version      (currently IPC_PROTOCOL_VERSION = 1)
/// offset 6..8  : u16 message_type (1 = Request, 2 = Response, 3 = Event)
/// offset 8..   : JSON payload bytes
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameHeader {
    /// Negotiated IPC protocol version. Decoders reject mismatches with
    /// [`ProtocolError::VersionMismatch`].
    pub version: u16,
    /// Message classifier (request/response/event).
    pub message_type: MessageKind,
    /// Length of the JSON payload bytes that follow the header. Capped
    /// at [`MAX_IPC_PAYLOAD_LEN`].
    pub payload_len: u32,
}

/// Decoded frame: header + deserialized payload of type `T`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<T> {
    /// Parsed frame header.
    pub header: FrameHeader,
    /// Deserialized payload. Typically [`Request`] or [`Response`].
    pub payload: T,
}

/// Errors raised by the framing + JSON codec. All variants are safe to
/// surface to untrusted peers (no secrets, no internal paths).
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Fewer than 8 bytes were available for the header. Emitted when
    /// `decode_header` is called on a buffer too small to hold the
    /// `u32 payload_len | u16 version | u16 message_type` prefix.
    ///
    /// # Recovery
    /// Fatal for this connection — the byte stream is not framed-
    /// recoverable. Callers must close the socket; no response is
    /// written back.
    #[error("frame header is truncated")]
    TruncatedHeader,
    /// The declared payload length exceeds [`MAX_IPC_PAYLOAD_LEN`] or
    /// the caller-declared length does not match the slice length.
    /// Emitted before any allocation proportional to the attacker-
    /// controlled length prefix (P0.8 OOM cap).
    ///
    /// # Recovery
    /// Fatal for this connection — a mis-framed stream cannot be
    /// realigned. Callers must close without replying so a misbehaving
    /// peer cannot amplify the error into a second round-trip.
    #[error("payload exceeds maximum IPC payload length")]
    PayloadTooLarge,
    /// Declared protocol version does not match [`IPC_PROTOCOL_VERSION`].
    /// Emitted when a wrong-version client connects to a daemon after a
    /// schema bump. Semantically "the wire schemas do not match".
    ///
    /// # Recovery
    /// Non-fatal for the listener but fatal for this request: surface
    /// to the peer as [`crate::methods::ResponseStatus::InvalidRequest`]
    /// and continue serving subsequent peers. Not retryable against the
    /// same daemon without upgrading the client.
    #[error("invalid protocol version: expected {expected}, got {actual}")]
    VersionMismatch {
        /// The version the daemon speaks.
        expected: u16,
        /// The version the peer advertised.
        actual: u16,
    },
    /// The frame's message-kind tag is not valid for the decode path. For
    /// example, [`decode_request`] requires kind `1` and rejects response or
    /// event frames before parsing the payload as a request.
    #[error("unexpected IPC message kind: expected {expected:?}, got {actual}")]
    UnexpectedMessageKind {
        /// Message kind required by the decoder.
        expected: MessageKind,
        /// Raw wire tag supplied by the peer.
        actual: u16,
    },
    /// Underlying `serde_json` encode/decode failure. Emitted when the
    /// body is valid-length but not a well-formed JSON representation
    /// of the expected `Request` / `Response` shape (unknown variant,
    /// missing field, invalid UTF-8, …).
    ///
    /// # Recovery
    /// Non-fatal for the listener; fatal for this request. Surface as
    /// `InvalidRequest` and keep serving. Not retryable — the request
    /// must be corrected by the caller.
    #[error("json codec failure: {0}")]
    Codec(#[from] serde_json::Error),
}

fn decode_header(bytes: &[u8]) -> Result<(u32, u16, u16), ProtocolError> {
    if bytes.len() < 8 {
        return Err(ProtocolError::TruncatedHeader);
    }

    let payload_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    let message_type = u16::from_le_bytes([bytes[6], bytes[7]]);
    Ok((payload_len, version, message_type))
}

/// Encode a [`RequestEnvelope`] into a length-prefixed framed payload.
/// The header is 8 bytes:
/// `u32 payload_len | u16 version | u16 kind` (little-endian).
///
/// The envelope JSON shape is `{ "request": ..., "traceparent": "..." }`
/// with the `traceparent` field omitted when `None`. Construction sites
/// that don't carry a trace context can pass any `Request` via
/// `RequestEnvelope::from(req)` or [`encode_request_bare`].
///
/// ```
/// use pcloud_ipc::{Method, Request};
/// use pcloud_ipc::methods::RequestEnvelope;
/// use pcloud_ipc::protocol::encode_request;
/// let env = RequestEnvelope::new(Request::Plain { method: Method::GetStatus });
/// let bytes = encode_request(&env).unwrap();
/// assert!(bytes.len() > 8);
/// ```
pub fn encode_request(envelope: &RequestEnvelope) -> Result<Vec<u8>, ProtocolError> {
    let payload = Zeroizing::new(serde_json::to_vec(envelope)?);
    encode_request_payload(payload)
}

/// Backward-compatible encoder that takes a bare [`Request`] and emits
/// the same envelope wire shape with no `traceparent` attached. Provided
/// for callers that have not yet been updated to construct a
/// [`RequestEnvelope`] explicitly.
///
/// ```
/// use pcloud_ipc::{Method, Request};
/// use pcloud_ipc::protocol::encode_request_bare;
/// let bytes = encode_request_bare(&Request::Plain { method: Method::GetStatus }).unwrap();
/// assert!(bytes.len() > 8);
/// ```
pub fn encode_request_bare(request: &Request) -> Result<Vec<u8>, ProtocolError> {
    encode_request(&RequestEnvelope::new(request.clone()))
}

fn encode_request_payload(payload: Zeroizing<Vec<u8>>) -> Result<Vec<u8>, ProtocolError> {
    if payload.len() > MAX_IPC_PAYLOAD_LEN {
        return Err(ProtocolError::PayloadTooLarge);
    }

    let mut bytes = Vec::with_capacity(8 + payload.len());
    let len = payload.len() as u32;
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&IPC_PROTOCOL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(MessageKind::Request as u16).to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// Encode any serializable response body into the framed wire format.
///
/// ```
/// use pcloud_ipc::{Response, ResponseStatus};
/// use pcloud_ipc::protocol::encode_response;
/// let bytes = encode_response(
///     &Response { status: ResponseStatus::Ok, message: "ok".into() }
/// ).unwrap();
/// assert!(bytes.len() > 8);
/// ```
pub fn encode_response<T: Serialize>(response: &T) -> Result<Vec<u8>, ProtocolError> {
    let payload = serde_json::to_vec(response)?;
    if payload.len() > MAX_IPC_PAYLOAD_LEN {
        return Err(ProtocolError::PayloadTooLarge);
    }

    let mut bytes = Vec::with_capacity(8 + payload.len());
    let len = payload.len() as u32;
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&IPC_PROTOCOL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&(MessageKind::Response as u16).to_le_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

/// Decode a framed [`RequestEnvelope`] from wire bytes. Rejects
/// mismatched versions and oversized payloads before doing any
/// deserialization. Accepts both the modern envelope shape
/// (`{"request": ..., "traceparent": ...}`) and a bare-[`Request`]
/// payload from pre-envelope clients via
/// [`RequestEnvelope::try_from_wire`].
///
/// ```
/// use pcloud_ipc::{Method, Request};
/// use pcloud_ipc::methods::RequestEnvelope;
/// use pcloud_ipc::protocol::{encode_request, decode_request};
/// let env = RequestEnvelope::new(Request::Plain { method: Method::GetHealth });
/// let bytes = encode_request(&env).unwrap();
/// let frame = decode_request(&bytes).unwrap();
/// assert_eq!(frame.header.payload_len as usize, bytes.len() - 8);
/// assert!(frame.payload.traceparent().is_none());
/// ```
pub fn decode_request(bytes: &[u8]) -> Result<Frame<RequestEnvelope>, ProtocolError> {
    let (payload_len, version, message_type) = decode_header(bytes)?;

    // Accept versions in the compat window [MIN_ACCEPTED, CURRENT].
    // Versions below the floor or above the ceiling are rejected.
    // Versions in the window but below CURRENT trigger a deprecation warning.
    if version < MIN_ACCEPTED_IPC_PROTOCOL_VERSION || version > IPC_PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch {
            expected: IPC_PROTOCOL_VERSION,
            actual: version,
        });
    }
    if version < IPC_PROTOCOL_VERSION {
        log::warn!(
            "IPC client using deprecated protocol version {version}; \
             current version is {IPC_PROTOCOL_VERSION}. \
             Please upgrade the client to avoid future compatibility breaks."
        );
    }
    if message_type != MessageKind::Request as u16 {
        return Err(ProtocolError::UnexpectedMessageKind {
            expected: MessageKind::Request,
            actual: message_type,
        });
    }

    let payload = &bytes[8..];
    if payload.len() > MAX_IPC_PAYLOAD_LEN || payload.len() != payload_len as usize {
        return Err(ProtocolError::PayloadTooLarge);
    }

    let envelope = RequestEnvelope::try_from_wire(payload)?;

    Ok(Frame {
        header: FrameHeader {
            version,
            message_type: MessageKind::Request,
            payload_len,
        },
        payload: envelope,
    })
}

/// Decode a framed [`Response`] from wire bytes.
///
/// ```
/// use pcloud_ipc::{Response, ResponseStatus};
/// use pcloud_ipc::protocol::{encode_response, decode_response};
/// let bytes = encode_response(
///     &Response { status: ResponseStatus::Ok, message: "ok".into() }
/// ).unwrap();
/// let frame = decode_response(&bytes).unwrap();
/// assert_eq!(frame.payload.status, ResponseStatus::Ok);
/// ```
pub fn decode_response(bytes: &[u8]) -> Result<Frame<Response>, ProtocolError> {
    let (payload_len, version, message_type) = decode_header(bytes)?;

    // Accept versions in the compat window [MIN_ACCEPTED, CURRENT].
    if version < MIN_ACCEPTED_IPC_PROTOCOL_VERSION || version > IPC_PROTOCOL_VERSION {
        return Err(ProtocolError::VersionMismatch {
            expected: IPC_PROTOCOL_VERSION,
            actual: version,
        });
    }
    if version < IPC_PROTOCOL_VERSION {
        log::warn!(
            "IPC response using deprecated protocol version {version}; \
             current version is {IPC_PROTOCOL_VERSION}."
        );
    }
    if message_type != MessageKind::Response as u16 {
        return Err(ProtocolError::UnexpectedMessageKind {
            expected: MessageKind::Response,
            actual: message_type,
        });
    }

    let payload = &bytes[8..];
    if payload.len() > MAX_IPC_PAYLOAD_LEN || payload.len() != payload_len as usize {
        return Err(ProtocolError::PayloadTooLarge);
    }

    let response = serde_json::from_slice(payload)?;

    Ok(Frame {
        header: FrameHeader {
            version,
            message_type: MessageKind::Response,
            payload_len,
        },
        payload: response,
    })
}

#[cfg(test)]
mod tests {
    use crate::methods::{Method, Request, Response, ResponseStatus};

    use super::{
        IPC_PROTOCOL_VERSION, decode_request, decode_response,
        encode_request_bare as encode_request, encode_response,
    };

    #[test]
    fn request_roundtrip_works() {
        let bytes = encode_request(&Request::Plain {
            method: Method::GetStatus,
        })
        .expect("request should encode");
        let frame = decode_request(&bytes).expect("request should decode");
        assert_eq!(frame.header.version, IPC_PROTOCOL_VERSION);
        assert!(matches!(
            frame.payload.request,
            Request::Plain {
                method: Method::GetStatus
            }
        ));
        assert!(frame.payload.traceparent().is_none());
    }

    #[test]
    fn mount_unmount_request_roundtrip() {
        use std::path::PathBuf;
        let bytes = encode_request(&Request::Mount {
            path: PathBuf::from("/mnt/pcloud"),
        })
        .expect("mount request should encode");
        let frame = decode_request(&bytes).expect("mount request should decode");
        match frame.payload.request {
            Request::Mount { path } => assert_eq!(path, PathBuf::from("/mnt/pcloud")),
            other => panic!("unexpected variant: {other:?}"),
        }

        let bytes = encode_request(&Request::Unmount).expect("unmount encodes");
        let frame = decode_request(&bytes).expect("unmount decodes");
        assert!(matches!(frame.payload.request, Request::Unmount));
    }

    #[test]
    fn session_status_request_roundtrip() {
        let bytes = encode_request(&Request::Plain {
            method: Method::SessionStatus,
        })
        .expect("session-status request should encode");
        let frame = decode_request(&bytes).expect("session-status should decode");
        assert!(matches!(
            frame.payload.request,
            Request::Plain {
                method: Method::SessionStatus
            }
        ));
    }

    #[test]
    fn session_status_payload_roundtrip_via_response_message() {
        use crate::methods::SessionStatusPayload;

        let payload = SessionStatusPayload {
            expires_at: Some(1_700_000_000),
            last_used_at: Some(1_699_999_000),
            refresh_in_flight: true,
        };
        // The daemon carries SessionStatusPayload inside Response.message
        // as JSON; here we exercise that full round-trip.
        let serialized = serde_json::to_string(&payload).expect("payload encodes");
        let resp = Response {
            status: ResponseStatus::Ok,
            message: serialized,
        };
        let bytes = encode_response(&resp).expect("response encodes");
        let frame = decode_response(&bytes).expect("response decodes");
        assert_eq!(frame.payload.status, ResponseStatus::Ok);
        let decoded: SessionStatusPayload =
            serde_json::from_str(&frame.payload.message).expect("payload decodes");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn run_localscan_request_roundtrip() {
        let bytes = encode_request(&Request::RunLocalScan).expect("encode");
        let frame = decode_request(&bytes).expect("decode");
        assert!(matches!(frame.payload.request, Request::RunLocalScan));
    }

    #[test]
    fn send_publink_request_roundtrip() {
        let req = Request::SendPublink {
            code: "alpha".to_owned(),
            mails: "a@x.com,b@x.com".to_owned(),
            message: "hi".to_owned(),
        };
        let bytes = encode_request(&req).expect("encode");
        let frame = decode_request(&bytes).expect("decode");
        match frame.payload.request {
            Request::SendPublink {
                code,
                mails,
                message,
            } => {
                assert_eq!(code, "alpha");
                assert_eq!(mails, "a@x.com,b@x.com");
                assert_eq!(message, "hi");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn response_roundtrip_works() {
        let bytes = encode_response(&Response {
            status: ResponseStatus::Ok,
            message: "ready".to_string(),
        })
        .expect("response should encode");
        let frame = decode_response(&bytes).expect("response should decode");
        assert_eq!(frame.payload.status, ResponseStatus::Ok);
        assert_eq!(frame.payload.message, "ready");
    }
}
