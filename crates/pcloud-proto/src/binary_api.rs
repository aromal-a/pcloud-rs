//! pCloud binary protocol framer: encodes typed parameters into wire
//! frames and parses server frames back into typed responses without
//! panicking on malformed input. Foundation layer consumed by every
//! API module in this crate.
//!
//! ## Wire layout
//!
//! A request frame is laid out as:
//!
//! ```text
//! +--------+----+-----------+-----------+---+---------+---------+
//! | len:u16| cmd| [body_u64]| cmd_bytes | n | param_0 | param_1 |...
//! +--------+----+-----------+-----------+---+---------+---------+
//! ```
//!
//! - `len` — `u16` little-endian payload length.
//! - `cmd` — one-byte header: low 7 bits are the command-name length,
//!   top bit set iff a raw data body is attached.
//! - `[body_u64]` — optional 8-byte `u64` data-body length, present
//!   only when the high bit of `cmd` is set.
//! - `cmd_bytes` — ASCII command name, no null terminator.
//! - `n` — one-byte parameter count.
//! - each parameter: one header byte `(type << 6) | name_len`, the
//!   name bytes, then the value (`u32 len + UTF-8` for strings,
//!   `u64 LE` for numbers, `u8` for booleans).
//!
//! Request frames are bounded by [`MAX_REQUEST_FRAME_LEN`]; responses
//! are bounded by [`MAX_RESPONSE_FRAME_LEN`] at the transport layer.
//!
//! ## Security considerations
//!
//! - Command and parameter names have tight length caps
//!   ([`MAX_PARAM_NAME_LEN`]) because the wire format reserves only
//!   six or seven bits for the length. Exceeding the cap is a hard
//!   error — we refuse to silently truncate.
//! - Parameter values of type `String` are written verbatim; callers
//!   must not pass secret material without wrapping it in
//!   `pcloud-secret` first. This module does not log parameter
//!   contents.
//! - The encoder pre-computes the total frame length and rejects
//!   anything over [`MAX_REQUEST_FRAME_LEN`] **before** allocating
//!   the output buffer, so oversized inputs cannot force a large
//!   allocation.
//!
//! Portable; no platform gating.

use std::{fmt, ops::Deref};

use thiserror::Error;
use zeroize::Zeroizing;

/// Hard upper bound on the encoded length of a single request frame.
///
/// The on-wire length prefix is a `u16`, so frames cannot exceed
/// `65_535` bytes even in principle. [`encode_request`] enforces
/// this before any allocation and returns
/// [`FrameParseError::RequestTooLarge`] on overflow.
pub const MAX_REQUEST_FRAME_LEN: usize = u16::MAX as usize;

/// Maximum byte length of a parameter name.
///
/// The parameter header byte reserves six bits for the name length
/// (the top two encode the value type), giving a hard cap of `63`.
/// Longer names cannot be represented on the wire; [`encode_request`]
/// rejects them with [`FrameParseError::ParamNameTooLong`] rather
/// than silently truncating.
pub const MAX_PARAM_NAME_LEN: usize = 63;

/// Soft upper bound on a single server response frame.
///
/// 256 MiB is large enough to cover any legitimate response (large
/// folder listings, block-checksum tables for multi-GiB uploads) but
/// small enough to bound the worst-case allocation if a misbehaving
/// or hostile server advertises a pathological length. The
/// transport layer rejects oversized frames with
/// [`FrameParseError::ResponseTooLarge`].
pub const MAX_RESPONSE_FRAME_LEN: usize = 256 * 1024 * 1024;

/// Metadata header extracted from or used to build a request frame.
///
/// ## Wire layout
///
/// Summarises the two fields that identify a request on the wire:
/// the command name and the number of encoded parameters. The raw
/// bytes are carried separately on [`EncodedRequest::bytes`]; this
/// struct is kept owned (rather than borrowed) so callers can retain
/// it for logging and replay after the encoded buffer has been
/// consumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestFrame {
    /// pCloud command name, e.g. `"login"` or `"listfolder"`.
    ///
    /// ASCII only, no null terminator, max 127 bytes (the top bit of
    /// the command-length byte is reserved for the raw-body flag).
    pub command: String,
    /// Number of parameters packed into the frame body.
    ///
    /// Matches `params.len()` in the corresponding
    /// [`EncodedRequest`]. Kept here as a sanity check for callers
    /// that log frames without decoding the whole payload.
    pub parameter_count: usize,
}

/// Typed value carried by a single request parameter.
///
/// ## Design choices
///
/// The pCloud binary protocol admits three scalar parameter types:
/// UTF-8 string, 64-bit unsigned integer, and boolean. Modelling
/// them as a closed enum (rather than a `dyn`-trait object) keeps
/// the encoder branch-free in the hot path and lets callers pattern
/// match exhaustively. `#[non_exhaustive]` is intentionally not
/// applied: the protocol does not admit new scalar types without a
/// wire-format bump, and consumers benefit from compile-time
/// coverage when that hypothetical bump happens.
#[derive(Clone, PartialEq, Eq)]
pub enum BinaryParamValue {
    /// UTF-8 string value, encoded as a `u32` little-endian length
    /// followed by the bytes.
    ///
    /// No terminator, no escaping — the length prefix is
    /// authoritative. Bytes are written verbatim; secret material
    /// must be wrapped before reaching this variant.
    String(String),
    /// 64-bit unsigned integer, encoded as eight little-endian
    /// bytes.
    ///
    /// Used for ids, sizes, timestamps, and flag bitmaps.
    Number(u64),
    /// Boolean, encoded as a single `0x00` or `0x01` byte.
    ///
    /// Distinct wire type from `Number(0)` / `Number(1)` — servers
    /// that expect a boolean will reject a numeric substitute.
    Bool(bool),
}

impl fmt::Debug for BinaryParamValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => f
                .debug_tuple("String")
                .field(&RedactedStringDebug(value.len()))
                .finish(),
            Self::Number(value) => f.debug_tuple("Number").field(value).finish(),
            Self::Bool(value) => f.debug_tuple("Bool").field(value).finish(),
        }
    }
}

/// Named parameter, ready to be encoded into a request frame.
///
/// Pair of a name (ASCII, ≤ [`MAX_PARAM_NAME_LEN`]) and a typed
/// value. Construct via the [`BinaryParam::string`],
/// [`BinaryParam::number`], or [`BinaryParam::bool`] helpers to
/// avoid repeating the wrapping enum boilerplate.
///
/// ## Design choices
///
/// Both fields are owned rather than `&str` borrows because
/// parameter vectors are typically built by method-builder code
/// (`methods::*`) whose borrow graph is easier to manage with
/// owned data, and because the resulting `Vec<BinaryParam>` is
/// often logged / cloned for retry by the resilient transport.
#[derive(Clone, PartialEq, Eq)]
pub struct BinaryParam {
    /// Parameter name. ASCII, max [`MAX_PARAM_NAME_LEN`] bytes.
    pub name: String,
    /// Typed parameter value.
    pub value: BinaryParamValue,
}

impl fmt::Debug for BinaryParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BinaryParam")
            .field("name", &self.name)
            .field("value", &self.value)
            .finish()
    }
}

impl BinaryParam {
    /// Build a string-valued parameter without a caller-visible intermediate
    /// allocation surface. `name` accepts any `Into<String>` (static str,
    /// `String`, `&String`, ...) and `value` accepts the same.
    #[inline]
    pub fn string(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: BinaryParamValue::String(value.into()),
        }
    }

    /// Build a numeric-valued parameter. `#[inline]` so repeated callers in
    /// hot `params()` builders do not pay a function-call cost.
    #[inline]
    pub fn number(name: impl Into<String>, value: u64) -> Self {
        Self {
            name: name.into(),
            value: BinaryParamValue::Number(value),
        }
    }

    /// Build a bool-valued parameter.
    #[inline]
    pub fn bool(name: impl Into<String>, value: bool) -> Self {
        Self {
            name: name.into(),
            value: BinaryParamValue::Bool(value),
        }
    }
}

/// Fully-encoded request ready to be written to a transport.
///
/// ## Lifecycle
///
/// 1. Build a `Vec<BinaryParam>` via the method-builder layer
///    (`methods::*`).
/// 2. Call [`encode_request`] — this validates name lengths, pre-sizes
///    the output buffer, and packs the frame.
/// 3. Hand the resulting `EncodedRequest` to a transport
///    (`BinaryApiTransport::execute` / `execute_with_body`).
/// 4. The transport writes [`Self::bytes`] verbatim and then reads
///    the response frame.
///
/// The owned [`Self::frame`] is retained in all builds. [`Self::params`] is
/// retained only in debug builds so development transports and normal test
/// runs can introspect the request without re-parsing the wire buffer; release builds
/// keep it empty to avoid extending plaintext credential lifetimes. Debug
/// output redacts every string value and reports the serialized frame only by
/// length; the serialized byte buffer is zeroized on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedRequest {
    /// Decoded view of the command name and parameter count.
    pub frame: RequestFrame,
    /// Parameters that were encoded into [`Self::bytes`], retained
    /// verbatim only in debug builds. Release builds leave this empty;
    /// transports must use [`Self::bytes`] for production execution.
    pub params: Vec<BinaryParam>,
    /// Fully serialized wire frame (length prefix + header + body).
    ///
    /// Hand this directly to the transport; do not append or
    /// prepend anything — the length prefix is already populated.
    pub bytes: EncodedRequestBytes,
}

/// Serialized binary request bytes that zeroize on drop and redact `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct EncodedRequestBytes(Zeroizing<Vec<u8>>);

impl EncodedRequestBytes {
    /// Wrap serialized request bytes in zeroizing storage.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// Borrow the serialized request frame.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }

    /// Length of the serialized request frame.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` when the serialized request frame is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for EncodedRequestBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedRequestBytes")
            .field("len", &self.len())
            .field("bytes", &"<redacted>")
            .finish()
    }
}

impl AsRef<[u8]> for EncodedRequestBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Deref for EncodedRequestBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl fmt::Debug for EncodedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EncodedRequest")
            .field("frame", &self.frame)
            .field("params", &self.params)
            .field("bytes_len", &self.bytes.len())
            .finish()
    }
}

struct RedactedStringDebug(usize);

impl fmt::Debug for RedactedStringDebug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<redacted {} bytes>", self.0)
    }
}

/// Error produced by [`encode_request`] or
/// [`parse_response_frame_len`].
///
/// Each variant maps to a specific protocol-level invariant that the
/// frame layer enforces before handing data to / from the network.
/// The enum is not `#[non_exhaustive]` because the variant set
/// mirrors the wire format's closed taxonomy; adding a new variant
/// is a meaningful API change callers should be forced to handle.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FrameParseError {
    /// Command name exceeded the 127-byte header cap.
    ///
    /// Emitted by [`encode_request`] before any bytes are written.
    /// A programming error — pCloud command names are short ASCII
    /// tokens.
    #[error("command name is too long for on-wire encoding")]
    CommandTooLong,
    /// Parameter name exceeded [`MAX_PARAM_NAME_LEN`].
    ///
    /// The offending name is returned verbatim for diagnostics.
    #[error("parameter name '{0}' is too long for on-wire encoding")]
    ParamNameTooLong(String),
    /// Fully encoded request would exceed [`MAX_REQUEST_FRAME_LEN`].
    ///
    /// Raised *before* allocating the output buffer, so hostile
    /// inputs cannot force a large allocation.
    #[error("request frame exceeds protocol limit")]
    RequestTooLarge,
    /// Response header was shorter than the four-byte length
    /// prefix.
    ///
    /// Emitted by [`parse_response_frame_len`]. The transport
    /// should treat this as a fatal connection error.
    #[error("response frame header is truncated")]
    TruncatedResponseHeader,
    /// Response length prefix claimed a body larger than
    /// [`MAX_RESPONSE_FRAME_LEN`].
    ///
    /// The claimed length is included for diagnostics. The
    /// transport must not read the body — it may never arrive, or
    /// may be adversarial.
    #[error("response frame length {0} exceeds parser limit")]
    ResponseTooLarge(u32),
}

/// Serialize a command + parameters into a wire-format
/// [`EncodedRequest`].
///
/// If `raw_body_len` is `Some(n)`, the top bit of the command header
/// is set and an additional `u64` little-endian body-length field is
/// packed; the raw body bytes themselves are **not** included and
/// must be written by the caller *after* the returned frame.
///
/// # Errors
///
/// - [`FrameParseError::CommandTooLong`] if `command.len() > 127`.
/// - [`FrameParseError::ParamNameTooLong`] if any parameter name
///   exceeds [`MAX_PARAM_NAME_LEN`].
/// - [`FrameParseError::RequestTooLarge`] if the total encoded size
///   would exceed [`MAX_REQUEST_FRAME_LEN`].
///
/// # Examples
///
/// ```
/// use pcloud_proto::binary_api::{BinaryParam, BinaryParamValue, encode_request};
///
/// let req = encode_request(
///     "login",
///     &[BinaryParam {
///         name: "username".to_owned(),
///         value: BinaryParamValue::String("alice".to_owned()),
///     }],
///     None,
/// ).expect("request encodes");
/// assert_eq!(req.frame.command, "login");
/// assert_eq!(req.frame.parameter_count, 1);
/// ```
pub fn encode_request(
    command: &str,
    params: &[BinaryParam],
    raw_body_len: Option<u64>,
) -> Result<EncodedRequest, FrameParseError> {
    let cmd_len = u8::try_from(command.len()).map_err(|_| FrameParseError::CommandTooLong)?;
    let mut payload_len = command.len() + 2;

    if raw_body_len.is_some() {
        payload_len += 8;
    }

    for param in params {
        if param.name.len() > MAX_PARAM_NAME_LEN {
            return Err(FrameParseError::ParamNameTooLong(param.name.clone()));
        }

        payload_len += 1 + param.name.len();
        match &param.value {
            BinaryParamValue::String(value) => {
                payload_len += 4 + value.len();
            }
            BinaryParamValue::Number(_) => {
                payload_len += 8;
            }
            BinaryParamValue::Bool(_) => {
                payload_len += 1;
            }
        }
    }

    let total_len = payload_len + 2;
    if total_len > MAX_REQUEST_FRAME_LEN {
        return Err(FrameParseError::RequestTooLarge);
    }

    let mut bytes = Vec::with_capacity(total_len);
    let wire_len = payload_len as u16;
    bytes.extend_from_slice(&wire_len.to_le_bytes());
    bytes.push(if raw_body_len.is_some() {
        cmd_len | 0x80
    } else {
        cmd_len
    });
    if let Some(body_len) = raw_body_len {
        bytes.extend_from_slice(&body_len.to_le_bytes());
    }
    bytes.extend_from_slice(command.as_bytes());
    bytes.push(u8::try_from(params.len()).unwrap_or(u8::MAX));

    for param in params {
        let (param_type, value_len) = match &param.value {
            BinaryParamValue::String(value) => (0u8, value.len()),
            BinaryParamValue::Number(_) => (1u8, 8usize),
            BinaryParamValue::Bool(_) => (2u8, 1usize),
        };
        let header = (param_type << 6) | (param.name.len() as u8);
        bytes.push(header);
        bytes.extend_from_slice(param.name.as_bytes());
        match &param.value {
            BinaryParamValue::String(value) => {
                bytes.extend_from_slice(&(value_len as u32).to_le_bytes());
                bytes.extend_from_slice(value.as_bytes());
            }
            BinaryParamValue::Number(value) => {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            BinaryParamValue::Bool(value) => {
                bytes.push(u8::from(*value));
            }
        }
    }

    Ok(EncodedRequest {
        frame: RequestFrame {
            command: command.to_owned(),
            parameter_count: params.len(),
        },
        params: retained_params(params),
        bytes: EncodedRequestBytes::new(bytes),
    })
}

fn retained_params(params: &[BinaryParam]) -> Vec<BinaryParam> {
    #[cfg(debug_assertions)]
    {
        params.to_vec()
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = params;
        Vec::new()
    }
}

/// Decode the four-byte little-endian length prefix of a response
/// frame and validate it against [`MAX_RESPONSE_FRAME_LEN`].
///
/// Intended to be called by the transport layer after reading
/// exactly four bytes from the socket; the caller then reads the
/// returned number of bytes to form the frame body.
///
/// # Errors
///
/// - [`FrameParseError::TruncatedResponseHeader`] if `header` has
///   fewer than four bytes.
/// - [`FrameParseError::ResponseTooLarge`] if the advertised length
///   exceeds [`MAX_RESPONSE_FRAME_LEN`]; the claimed length is
///   included in the variant for diagnostics.
///
/// # Examples
///
/// ```
/// use pcloud_proto::binary_api::parse_response_frame_len;
///
/// let len = parse_response_frame_len(&[0x10, 0, 0, 0]).unwrap();
/// assert_eq!(len, 16);
/// ```
pub fn parse_response_frame_len(header: &[u8]) -> Result<u32, FrameParseError> {
    if header.len() < 4 {
        return Err(FrameParseError::TruncatedResponseHeader);
    }

    let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    if len as usize > MAX_RESPONSE_FRAME_LEN {
        return Err(FrameParseError::ResponseTooLarge(len));
    }

    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::{
        BinaryParam, BinaryParamValue, FrameParseError, encode_request, parse_response_frame_len,
    };

    #[test]
    fn encodes_basic_request() {
        let encoded = encode_request(
            "login",
            &[
                BinaryParam {
                    name: "username".to_string(),
                    value: BinaryParamValue::String("alice".to_string()),
                },
                BinaryParam {
                    name: "os".to_string(),
                    value: BinaryParamValue::Number(3),
                },
            ],
            None,
        )
        .expect("request should encode");

        assert_eq!(encoded.frame.command, "login");
        assert_eq!(encoded.frame.parameter_count, 2);
        assert!(encoded.bytes.len() >= 2);
    }

    #[test]
    fn debug_redacts_string_params_and_frame_bytes() {
        let encoded = encode_request(
            "login",
            &[
                BinaryParam::string("auth", "secret-auth-token"),
                BinaryParam::string("password", "correct-horse-battery-staple"),
                BinaryParam::number("userid", 42),
            ],
            None,
        )
        .expect("request should encode");

        let rendered = format!("{encoded:?}");
        assert!(!rendered.contains("secret-auth-token"));
        assert!(!rendered.contains("correct-horse-battery-staple"));
        assert!(rendered.contains("bytes_len"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn encoded_bytes_debug_redacts_serialized_frame() {
        let encoded = encode_request(
            "login",
            &[BinaryParam::string("auth", "secret-auth-token")],
            None,
        )
        .expect("request should encode");

        let rendered = format!("{:?}", encoded.bytes);
        assert!(!rendered.contains("secret-auth-token"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn plaintext_param_retention_is_not_enabled_for_release_builds() {
        let encoded = encode_request(
            "login",
            &[BinaryParam::string("auth", "secret-auth-token")],
            None,
        )
        .expect("request should encode");

        if cfg!(debug_assertions) {
            assert_eq!(encoded.params.len(), 1);
        } else {
            assert!(
                encoded.params.is_empty(),
                "release builds must not retain plaintext params"
            );
        }
    }

    #[test]
    fn param_debug_redacts_string_values() {
        let rendered = format!("{:?}", BinaryParam::string("path", "/not/secret"));
        assert!(!rendered.contains("/not/secret"));
        assert!(rendered.contains("redacted"));
    }

    #[test]
    fn rejects_long_parameter_name() {
        let err = encode_request(
            "login",
            &[BinaryParam {
                name: "x".repeat(64),
                value: BinaryParamValue::Bool(true),
            }],
            None,
        )
        .expect_err("parameter name should be rejected");

        assert!(matches!(err, FrameParseError::ParamNameTooLong(_)));
    }

    #[test]
    fn parses_response_header_length() {
        let len = parse_response_frame_len(&[0x10, 0x00, 0x00, 0x00]).expect("valid length");
        assert_eq!(len, 16);
    }
}
