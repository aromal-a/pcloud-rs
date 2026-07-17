//! Response envelope and parsing primitives shared by every API module
//! in this crate. Centralizes the `result`/`error` convention and the
//! typed-error classification helpers used by the binary and HTTP
//! transports.
//!
//! ## Wire format
//!
//! Every pCloud binary response starts with a little-endian `u32`
//! frame length, followed by exactly that many bytes of tagged data.
//! Each tagged value is one of: short / long UTF-8 string, reused
//! string reference, variable-length unsigned integer, boolean,
//! hash (key/value map), array, or a raw-data marker. The tag byte
//! ranges are captured by the `RPARAM_*` constants below and mirror
//! the upstream pCloud C client's `rparam_*` taxonomy exactly.
//!
//! ## Security considerations
//!
//! The parser treats its input as **untrusted**:
//!
//! - Frame length is checked against [`ParseLimits::max_frame_len`]
//!   **before** allocating the body buffer, so a hostile server cannot
//!   make us allocate gigabytes.
//! - Recursive constructs (arrays and hashes) are bounded by
//!   [`ParseLimits::max_depth`]; nested payloads that try to exhaust
//!   the stack are rejected with
//!   [`ResponseParseError::NestingLimitExceeded`].
//! - String-reuse references are validated against a bounded table
//!   ([`ParseLimits::max_reused_strings`]) and invalid ids raise
//!   [`ResponseParseError::InvalidReuseReference`].
//! - No `unsafe`, no panic on malformed input. Every cursor advance
//!   is bounds-checked via `checked_add`.
//!
//! ## Role in the request pipeline
//!
//! [`parse_response_frame`] is called by [`crate::transport`] and
//! [`crate::http_download`] after the raw frame has been read from the
//! wire. The resulting [`Value`] tree is then projected into
//! domain-specific types by each `*_api` module using the
//! [`HashView`] accessors.
//!
//! Portable; no platform gating.

use thiserror::Error;

const RPARAM_STR1: u8 = 0;
const RPARAM_STR4: u8 = 3;
const RPARAM_RSTR1: u8 = 4;
const RPARAM_RSTR4: u8 = 7;
const RPARAM_NUM1: u8 = 8;
const RPARAM_NUM8: u8 = 15;
const RPARAM_HASH: u8 = 16;
const RPARAM_ARRAY: u8 = 17;
const RPARAM_BFALSE: u8 = 18;
const RPARAM_BTRUE: u8 = 19;
const RPARAM_DATA: u8 = 20;
const RPARAM_SHORT_STR_BASE: u8 = 100;
const RPARAM_SHORT_STR_MAX: u8 = 149;
const RPARAM_SHORT_RSTR_BASE: u8 = 150;
const RPARAM_SHORT_RSTR_MAX: u8 = 199;
const RPARAM_SMALL_NUM_BASE: u8 = 200;
const RPARAM_SMALL_NUM_MAX: u8 = 219;
const RPARAM_END: u8 = 255;

/// Typed view of a decoded pCloud binary-protocol response value.
///
/// `Value` is the root AST node produced by [`parse_response_frame`].
/// Every `*_api` module in this crate consumes a `Value` tree (usually
/// via [`HashView`]) and projects it into domain types.
///
/// ## Design choices
///
/// - **Owned storage** (`String`, `Vec<Value>`, `Vec<(String, Value)>`)
///   rather than borrowing from the input buffer: responses are often
///   kept alive past the transport read, and owned data simplifies
///   caching and cross-thread handoff. Borrowed variants would require
///   a lifetime parameter that leaks into every downstream type.
/// - **Enum rather than a `dyn`-trait hierarchy**: the variant set is
///   closed — the pCloud wire format does not admit extension — and
///   exhaustive `match` lets callers discover missed variants at
///   compile time. `#[non_exhaustive]` is intentionally *not* applied
///   so consumers can pattern-match without catch-alls.
/// - **`Number` is `u64`**: all pCloud numeric fields fit, including
///   file sizes and timestamps. Signed arithmetic is deferred to
///   domain types that know the correct signedness.
///
/// The [`Value::Data`] variant carries the *length* of an attached
/// out-of-band data blob, not the blob itself; the blob is streamed
/// separately by the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// UTF-8 string payload (possibly empty).
    ///
    /// Emitted for both short (tag-encoded length) and long
    /// (length-prefixed) strings, as well as for reused-string
    /// references that have already been resolved. Callers should
    /// treat the contents as untrusted and validate before
    /// interpolating into paths, URLs, or log lines.
    String(String),
    /// Unsigned 64-bit integer.
    ///
    /// Represents any `NUM1`–`NUM8` wire value as well as the
    /// `SMALL_NUM` fast-path tags. File ids, folder ids, sizes, and
    /// unix timestamps all arrive here.
    Number(u64),
    /// Boolean true/false.
    ///
    /// Emitted from the dedicated `BTRUE` / `BFALSE` tag bytes; the
    /// pCloud protocol never encodes booleans as 0/1 numbers at the
    /// AST level.
    Bool(bool),
    /// Heterogeneous ordered sequence of values.
    ///
    /// Bounded by [`ParseLimits::max_array_len`] to prevent
    /// resource-exhaustion attacks. Nested arrays count against
    /// [`ParseLimits::max_depth`].
    Array(Vec<Value>),
    /// Ordered key/value map with UTF-8 string keys.
    ///
    /// Insertion order is preserved (pCloud field ordering is
    /// sometimes load-bearing in legacy callers). Use [`HashView`]
    /// for fast, typed lookup. Bounded by
    /// [`ParseLimits::max_hash_len`]; non-string keys are rejected
    /// with [`ResponseParseError::InvalidHashKeyType`].
    Hash(Vec<(String, Value)>),
    /// Out-of-band data marker carrying the attached blob's byte
    /// length.
    ///
    /// The blob bytes themselves are not in the parsed AST; the
    /// transport layer reads them from the socket after the frame
    /// proper. Callers observing `Data(n)` should expect `n` bytes
    /// of attached payload.
    Data(u64),
}

/// Resource caps applied by [`parse_response_frame`] to defend
/// against malformed or hostile server payloads.
///
/// ## Security role
///
/// Every field bounds one dimension of memory / stack consumption so
/// that a malicious server cannot trigger an OOM or stack overflow
/// simply by sending us a cleverly shaped frame. The [`Default`]
/// implementation encodes the values we use in production; override
/// fields only if a specific call site has a justified reason (e.g.
/// an admin-facing listing that legitimately exceeds the array cap).
///
/// ## Wire layout
///
/// The parser enforces the limits *before* performing the allocation
/// they guard, so an oversized frame is rejected without the cost of
/// the underlying `Vec`/`String` it would have produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseLimits {
    /// Maximum total response frame length in bytes (header + body).
    ///
    /// Enforced on the `u32` length prefix before any body read.
    /// Default: 1 MiB.
    pub max_frame_len: usize,
    /// Maximum nested container depth (arrays + hashes combined).
    ///
    /// Rejects pathological inputs that would otherwise blow the
    /// parser's recursion stack. Default: 32 — comfortably larger
    /// than any legitimate pCloud response.
    pub max_depth: usize,
    /// Maximum number of elements in a single array.
    ///
    /// Rejects arrays that would exhaust heap memory. Listings that
    /// legitimately exceed this cap must be paginated.
    /// Default: 4096.
    pub max_array_len: usize,
    /// Maximum number of key/value pairs in a single hash.
    ///
    /// Default: 4096.
    pub max_hash_len: usize,
    /// Maximum length, in bytes, of a single UTF-8 string value.
    ///
    /// Default: 64 KiB — large enough for error messages, path
    /// fragments, and JWT-style tokens, small enough to bound
    /// per-string allocation.
    pub max_string_len: usize,
    /// Maximum number of distinct strings retained in the reuse
    /// table during a single parse.
    ///
    /// The pCloud binary protocol supports back-references to
    /// previously emitted strings; this cap bounds the table used to
    /// resolve those references. A string costs at least one frame
    /// byte, so a frame can never legitimately contain more strings
    /// than its own length: the effective cap is clamped to
    /// `min(max_reused_strings, frame_len)` at parse time, which
    /// keeps the table honest without ever desynchronising a valid
    /// frame. Default: 1 MiB, matching the default frame cap.
    pub max_reused_strings: usize,
}

impl Default for ParseLimits {
    fn default() -> Self {
        Self {
            max_frame_len: 1024 * 1024,
            max_depth: 32,
            max_array_len: 4096,
            max_hash_len: 4096,
            max_string_len: 64 * 1024,
            max_reused_strings: 1024 * 1024,
        }
    }
}

/// Error produced by [`parse_response_frame`] on a malformed or
/// out-of-policy server frame.
///
/// ## When each variant is emitted
///
/// These errors are surfaced to callers as typed failures; none is
/// recoverable by the parser itself. A well-behaved client should
/// treat any variant as a protocol-level fault and either retry the
/// request on a fresh connection or surface the failure to the user.
///
/// The enum is *not* `#[non_exhaustive]`: the wire format is closed,
/// so adding new variants would be a meaningful API event and callers
/// should get a compile error when that happens.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ResponseParseError {
    /// The four-byte length prefix or body was cut short.
    ///
    /// Emitted when the buffer passed to the parser is smaller than
    /// the length header advertises. Callers should close the
    /// connection; retrying on the same socket is unsafe.
    #[error("response frame is truncated")]
    TruncatedFrame,
    /// Frame length prefix exceeds [`ParseLimits::max_frame_len`].
    ///
    /// Emitted before any body allocation. Usually indicates a bug
    /// in the server or a deliberate oversized-frame attack.
    #[error("response frame exceeds configured limit")]
    FrameTooLarge,
    /// Internal cursor walked past the end of the frame body.
    ///
    /// Emitted when a length field claims more bytes than the frame
    /// provides. The frame is unsafe to retry.
    #[error("response payload is truncated")]
    UnexpectedEof,
    /// Encountered a tag byte the parser does not recognise.
    ///
    /// The offending byte is included for diagnostics. Newer server
    /// versions must not introduce new tags without a coordinated
    /// client update.
    #[error("response contained invalid tag {0}")]
    InvalidTag(u8),
    /// Nested container depth exceeded
    /// [`ParseLimits::max_depth`].
    ///
    /// Defends against stack exhaustion from maliciously deep
    /// arrays/hashes.
    #[error("response nesting limit exceeded")]
    NestingLimitExceeded,
    /// Array length exceeded [`ParseLimits::max_array_len`].
    ///
    /// Defends against heap exhaustion from huge arrays.
    #[error("response array exceeded configured limit")]
    ArrayLimitExceeded,
    /// Hash length exceeded [`ParseLimits::max_hash_len`].
    ///
    /// Defends against heap exhaustion from huge key/value maps.
    #[error("response hash exceeded configured limit")]
    HashLimitExceeded,
    /// Declared string length exceeded
    /// [`ParseLimits::max_string_len`].
    ///
    /// The string bytes are *not* read when this fires — the length
    /// prefix alone is enough to reject the frame.
    #[error("response string exceeded configured limit")]
    StringTooLarge,
    /// Reused-string reference pointed at an id the parser never
    /// emitted.
    ///
    /// The offending id is included. Always indicates a server bug
    /// or an attempt to exploit the reuse table.
    #[error("response referenced invalid reused string id {0}")]
    InvalidReuseReference(usize),
    /// A hash entry's key was not a string.
    ///
    /// The pCloud protocol mandates string keys; arrays or numbers
    /// as keys indicate a corrupt frame.
    #[error("response hash key was not a string")]
    InvalidHashKeyType,
    /// The root value was parsed successfully but additional bytes
    /// remained inside the frame body.
    ///
    /// This is rejected rather than silently ignored so that frame
    /// desynchronisation is detected early.
    #[error("response had trailing bytes after root value")]
    TrailingBytes,
}

/// Parse a complete pCloud binary-protocol response frame into a
/// typed [`Value`] tree.
///
/// The input slice must contain the full frame: a four-byte
/// little-endian length prefix followed by exactly that many body
/// bytes. Trailing bytes after the root value are rejected to catch
/// stream desynchronisation early.
///
/// # Errors
///
/// Returns a [`ResponseParseError`] variant on any of:
///
/// - truncated header or body,
/// - body length beyond [`ParseLimits::max_frame_len`],
/// - unknown tag byte,
/// - nested depth beyond [`ParseLimits::max_depth`],
/// - container or string size beyond the configured limits,
/// - invalid reused-string reference,
/// - non-string hash key,
/// - trailing bytes after the root value parses.
///
/// # Examples
///
/// ```no_run
/// use pcloud_proto::response::{ParseLimits, parse_response_frame};
///
/// // `raw_frame` would come from the transport after reading the
/// // four-byte length prefix + body from the socket.
/// # let raw_frame: &[u8] = &[];
/// let value = parse_response_frame(raw_frame, &ParseLimits::default())?;
/// if let Some(hash) = value.as_hash() {
///     let result = hash.get_number("result").unwrap_or(u64::MAX);
///     println!("server returned result={result}");
/// }
/// # Ok::<_, pcloud_proto::response::ResponseParseError>(())
/// ```
pub fn parse_response_frame(
    bytes: &[u8],
    limits: &ParseLimits,
) -> Result<Value, ResponseParseError> {
    if bytes.len() < 4 {
        return Err(ResponseParseError::TruncatedFrame);
    }
    let frame_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if frame_len > limits.max_frame_len {
        return Err(ResponseParseError::FrameTooLarge);
    }
    if bytes.len() < 4 + frame_len {
        return Err(ResponseParseError::TruncatedFrame);
    }

    let mut cursor = 4usize;
    let mut reused_strings = Vec::new();
    let limits = ParseLimits {
        max_reused_strings: limits.max_reused_strings.min(frame_len),
        ..limits.clone()
    };
    let value = parse_value(bytes, &mut cursor, 0, &limits, &mut reused_strings)?;
    if cursor != 4 + frame_len {
        return Err(ResponseParseError::TrailingBytes);
    }
    Ok(value)
}

impl Value {
    /// Borrow this value as a [`HashView`] if it is a hash,
    /// otherwise return `None`.
    ///
    /// Prefer this over pattern-matching on [`Value::Hash`] directly
    /// when you intend to look up keys: [`HashView`] exposes typed
    /// accessors that compose cleanly.
    #[must_use]
    pub fn as_hash(&self) -> Option<HashView<'_>> {
        match self {
            Self::Hash(values) => Some(HashView(values)),
            _ => None,
        }
    }

    /// Borrow this value as a string slice, or return `None` if it is
    /// any other variant.
    ///
    /// The returned slice is valid for the lifetime of `self`. The
    /// bytes are untrusted server input; treat them accordingly.
    #[must_use]
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value.as_str()),
            _ => None,
        }
    }

    /// Return the inner unsigned integer if this value is a
    /// [`Value::Number`] **or** a [`Value::Data`] size marker.
    ///
    /// Both variants are coalesced here because most callers treat
    /// them identically (a `u64` magnitude). Use direct pattern
    /// matching if you need to distinguish inline numbers from
    /// out-of-band data markers.
    #[must_use]
    pub fn as_number(&self) -> Option<u64> {
        match self {
            Self::Number(value) | Self::Data(value) => Some(*value),
            _ => None,
        }
    }

    /// Return the inner boolean, or `None` for any other variant.
    ///
    /// The pCloud protocol encodes booleans via dedicated tag bytes,
    /// so this never silently converts `0` / `1` numbers.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// Borrow this value as a slice of array elements, or return
    /// `None` if it is any other variant.
    ///
    /// The slice length is bounded by
    /// [`ParseLimits::max_array_len`] at parse time, so callers do
    /// not need to defend against unbounded iteration.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Self::Array(values) => Some(values.as_slice()),
            _ => None,
        }
    }
}

fn parse_value(
    bytes: &[u8],
    cursor: &mut usize,
    depth: usize,
    limits: &ParseLimits,
    reused_strings: &mut Vec<String>,
) -> Result<Value, ResponseParseError> {
    if depth > limits.max_depth {
        return Err(ResponseParseError::NestingLimitExceeded);
    }

    let tag = read_u8(bytes, cursor)?;
    match tag {
        RPARAM_STR1..=RPARAM_STR4 => {
            let len_len = (tag - RPARAM_STR1 + 1) as usize;
            let string_len = read_var_u32(bytes, cursor, len_len)? as usize;
            read_string(bytes, cursor, string_len, limits, reused_strings)
        }
        RPARAM_RSTR1..=RPARAM_RSTR4 => {
            let len_len = (tag - RPARAM_RSTR1 + 1) as usize;
            let id = read_var_u32(bytes, cursor, len_len)? as usize;
            reused_strings
                .get(id)
                .cloned()
                .map(Value::String)
                .ok_or(ResponseParseError::InvalidReuseReference(id))
        }
        RPARAM_NUM1..=RPARAM_NUM8 => {
            let len_len = (tag - RPARAM_NUM1 + 1) as usize;
            Ok(Value::Number(read_var_u64(bytes, cursor, len_len)?))
        }
        RPARAM_HASH => parse_hash(bytes, cursor, depth + 1, limits, reused_strings),
        RPARAM_ARRAY => parse_array(bytes, cursor, depth + 1, limits, reused_strings),
        RPARAM_BFALSE => Ok(Value::Bool(false)),
        RPARAM_BTRUE => Ok(Value::Bool(true)),
        RPARAM_DATA => Ok(Value::Data(read_var_u64(bytes, cursor, 8)?)),
        RPARAM_SHORT_STR_BASE..=RPARAM_SHORT_STR_MAX => {
            let string_len = (tag - RPARAM_SHORT_STR_BASE) as usize;
            read_string(bytes, cursor, string_len, limits, reused_strings)
        }
        RPARAM_SHORT_RSTR_BASE..=RPARAM_SHORT_RSTR_MAX => {
            let id = (tag - RPARAM_SHORT_RSTR_BASE) as usize;
            reused_strings
                .get(id)
                .cloned()
                .map(Value::String)
                .ok_or(ResponseParseError::InvalidReuseReference(id))
        }
        RPARAM_SMALL_NUM_BASE..=RPARAM_SMALL_NUM_MAX => {
            Ok(Value::Number((tag - RPARAM_SMALL_NUM_BASE) as u64))
        }
        other => Err(ResponseParseError::InvalidTag(other)),
    }
}

fn parse_array(
    bytes: &[u8],
    cursor: &mut usize,
    depth: usize,
    limits: &ParseLimits,
    reused_strings: &mut Vec<String>,
) -> Result<Value, ResponseParseError> {
    let mut values = Vec::new();
    loop {
        if peek_u8(bytes, *cursor)? == RPARAM_END {
            *cursor += 1;
            break;
        }
        if values.len() >= limits.max_array_len {
            return Err(ResponseParseError::ArrayLimitExceeded);
        }
        values.push(parse_value(bytes, cursor, depth, limits, reused_strings)?);
    }
    Ok(Value::Array(values))
}

fn parse_hash(
    bytes: &[u8],
    cursor: &mut usize,
    depth: usize,
    limits: &ParseLimits,
    reused_strings: &mut Vec<String>,
) -> Result<Value, ResponseParseError> {
    let mut values = Vec::new();
    loop {
        if peek_u8(bytes, *cursor)? == RPARAM_END {
            *cursor += 1;
            break;
        }
        if values.len() >= limits.max_hash_len {
            return Err(ResponseParseError::HashLimitExceeded);
        }
        let key = match parse_value(bytes, cursor, depth, limits, reused_strings)? {
            Value::String(key) => key,
            _ => return Err(ResponseParseError::InvalidHashKeyType),
        };
        let value = parse_value(bytes, cursor, depth, limits, reused_strings)?;
        values.push((key, value));
    }
    Ok(Value::Hash(values))
}

fn read_string(
    bytes: &[u8],
    cursor: &mut usize,
    string_len: usize,
    limits: &ParseLimits,
    reused_strings: &mut Vec<String>,
) -> Result<Value, ResponseParseError> {
    if string_len > limits.max_string_len {
        return Err(ResponseParseError::StringTooLarge);
    }
    let start = *cursor;
    let end = start
        .checked_add(string_len)
        .ok_or(ResponseParseError::UnexpectedEof)?;
    if end > bytes.len() {
        return Err(ResponseParseError::UnexpectedEof);
    }
    let value = String::from_utf8_lossy(&bytes[start..end]).into_owned();
    *cursor = end;
    if reused_strings.len() < limits.max_reused_strings {
        reused_strings.push(value.clone());
    }
    Ok(Value::String(value))
}

fn read_var_u32(bytes: &[u8], cursor: &mut usize, len: usize) -> Result<u32, ResponseParseError> {
    Ok(read_var_u64(bytes, cursor, len)? as u32)
}

fn read_var_u64(bytes: &[u8], cursor: &mut usize, len: usize) -> Result<u64, ResponseParseError> {
    if len == 0 || len > 8 {
        return Err(ResponseParseError::UnexpectedEof);
    }
    let start = *cursor;
    let end = start
        .checked_add(len)
        .ok_or(ResponseParseError::UnexpectedEof)?;
    if end > bytes.len() {
        return Err(ResponseParseError::UnexpectedEof);
    }
    let mut value = 0u64;
    for (idx, byte) in bytes[start..end].iter().enumerate() {
        value |= (*byte as u64) << (idx * 8);
    }
    *cursor = end;
    Ok(value)
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, ResponseParseError> {
    let byte = *bytes
        .get(*cursor)
        .ok_or(ResponseParseError::UnexpectedEof)?;
    *cursor += 1;
    Ok(byte)
}

fn peek_u8(bytes: &[u8], cursor: usize) -> Result<u8, ResponseParseError> {
    bytes
        .get(cursor)
        .copied()
        .ok_or(ResponseParseError::UnexpectedEof)
}

/// Borrowed, read-only projection over a [`Value::Hash`] that offers
/// typed key lookup.
///
/// ## Design choices
///
/// `HashView` is `Copy` because it wraps a single slice reference,
/// so passing it to helpers is free. Lookup is O(n) linear scan —
/// pCloud hashes are small (a few dozen keys at most), a hash map
/// would carry more allocation overhead than it saves, and
/// insertion-order preservation is sometimes required by the
/// domain layer.
///
/// The lifetime `'a` ties the view to the original [`Value`] tree;
/// clone the tree (cheap for small responses) if you need to escape
/// that lifetime.
#[derive(Debug, Clone, Copy)]
pub struct HashView<'a>(&'a [(String, Value)]);

impl<'a> HashView<'a> {
    /// Look up `key` in the hash and return the raw [`Value`] if
    /// present.
    ///
    /// Case-sensitive, first-match wins (the pCloud protocol does
    /// not permit duplicate keys in practice). Prefer the typed
    /// variants below when you know the expected shape.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.0
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    /// Look up `key` and return the inner string if the value is a
    /// [`Value::String`], else `None`.
    ///
    /// Returns `None` both when the key is absent and when it is
    /// present but of the wrong type — callers that need to
    /// distinguish the two cases should use [`Self::get`].
    #[must_use]
    pub fn get_string(&self, key: &str) -> Option<&'a str> {
        self.get(key).and_then(Value::as_string)
    }

    /// Look up `key` and return the inner number if the value is a
    /// [`Value::Number`] or [`Value::Data`], else `None`.
    #[must_use]
    pub fn get_number(&self, key: &str) -> Option<u64> {
        self.get(key).and_then(Value::as_number)
    }

    /// Look up `key` and return the inner boolean if the value is a
    /// [`Value::Bool`], else `None`.
    #[must_use]
    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::as_bool)
    }

    /// Look up `key` and return a nested [`HashView`] if the value
    /// is itself a hash.
    ///
    /// The returned view inherits the same lifetime and can be
    /// chained (`outer.get_hash("a")?.get_hash("b")?`).
    #[must_use]
    pub fn get_hash(&self, key: &str) -> Option<HashView<'a>> {
        self.get(key).and_then(Value::as_hash)
    }

    /// Look up `key` and return the inner array slice if the value
    /// is a [`Value::Array`], else `None`.
    #[must_use]
    pub fn get_array(&self, key: &str) -> Option<&'a [Value]> {
        self.get(key).and_then(Value::as_array)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ParseLimits, RPARAM_ARRAY, RPARAM_BTRUE, RPARAM_END, RPARAM_HASH, RPARAM_SHORT_STR_BASE,
        Value, parse_response_frame,
    };

    #[test]
    fn hash_accessors_extract_typed_fields() {
        let value = Value::Hash(vec![
            ("result".to_owned(), Value::Number(0)),
            ("auth".to_owned(), Value::String("token".to_owned())),
            ("trustdevice".to_owned(), Value::Bool(true)),
            (
                "apiserver".to_owned(),
                Value::Hash(vec![(
                    "binapi".to_owned(),
                    Value::Array(vec![Value::String("bineapi-eu.pcloud.com".to_owned())]),
                )]),
            ),
        ]);
        let hash = value.as_hash().expect("hash should be available");
        assert_eq!(hash.get_number("result"), Some(0));
        assert_eq!(hash.get_string("auth"), Some("token"));
        assert_eq!(hash.get_bool("trustdevice"), Some(true));
        let apiserver = hash
            .get_hash("apiserver")
            .expect("nested hash should be available");
        let binapi = apiserver
            .get_array("binapi")
            .expect("nested array should be available");
        assert_eq!(
            binapi.first().and_then(Value::as_string),
            Some("bineapi-eu.pcloud.com")
        );
    }

    #[test]
    fn parse_hash_response_frame() {
        let mut payload = vec![RPARAM_HASH];
        payload.push(RPARAM_SHORT_STR_BASE + 6);
        payload.extend_from_slice(b"result");
        payload.push(200);
        payload.push(RPARAM_SHORT_STR_BASE + 11);
        payload.extend_from_slice(b"trustdevice");
        payload.push(RPARAM_BTRUE);
        payload.push(RPARAM_END);

        let mut frame = Vec::new();
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);

        let value =
            parse_response_frame(&frame, &ParseLimits::default()).expect("frame should parse");
        let hash = value.as_hash().expect("hash expected");
        assert_eq!(hash.get_number("result"), Some(0));
        assert_eq!(hash.get_bool("trustdevice"), Some(true));
    }

    #[test]
    fn parse_frame_with_more_than_4096_reused_strings() {
        let mut payload = vec![RPARAM_ARRAY];
        for idx in 0..5000u32 {
            let s = format!("s{idx:04}");
            payload.push(RPARAM_SHORT_STR_BASE + s.len() as u8);
            payload.extend_from_slice(s.as_bytes());
        }
        payload.push(4 + 1);
        payload.extend_from_slice(&4999u32.to_le_bytes()[..2]);
        payload.push(RPARAM_END);

        let mut frame = Vec::new();
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(&payload);

        let limits = ParseLimits {
            max_array_len: 8192,
            ..ParseLimits::default()
        };
        let value = parse_response_frame(&frame, &limits).expect("frame should parse");
        let array = value.as_array().expect("array expected");
        assert_eq!(array.len(), 5001);
        assert_eq!(array[4999].as_string(), Some("s4999"));
        assert_eq!(array[5000].as_string(), Some("s4999"));
    }
}
