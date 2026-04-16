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

use thiserror::Error;

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
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryParam {
    /// Parameter name. ASCII, max [`MAX_PARAM_NAME_LEN`] bytes.
    pub name: String,
    /// Typed parameter value.
    pub value: BinaryParamValue,
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
/// The owned [`Self::frame`] and [`Self::params`] copies are retained
/// alongside the serialized [`Self::bytes`] so logs, tests, and retry
/// paths can introspect the request without re-parsing the wire
/// buffer. The modest duplication is worth the debuggability win.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedRequest {
    /// Decoded view of the command name and parameter count.
    pub frame: RequestFrame,
    /// Parameters that were encoded into [`Self::bytes`], retained
    /// verbatim so callers can log or replay without reparsing.
    pub params: Vec<BinaryParam>,
    /// Fully serialized wire frame (length prefix + header + body).
    ///
    /// Hand this directly to the transport; do not append or
    /// prepend anything — the length prefix is already populated.
    pub bytes: Vec<u8>,
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
        params: params.to_vec(),
        bytes,
    })
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
