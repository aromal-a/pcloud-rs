//! T2.7 — W3C `traceparent` parser + child-span generator.
//!
//! # AI-scope deliverable
//!
//! `RequestEnvelope::traceparent` already carries a string; this
//! module owns the parse / validate / propagate logic so every
//! backend that inherits a parent trace context spawns a
//! correctly-formatted child span without re-implementing the wire
//! format. The OTLP exporter integration (which would pull
//! `opentelemetry-otlp`) is the follow-up — it needs an OTLP
//! collector to talk to.
//!
//! # W3C traceparent wire format (RFC TC-1)
//!
//! ```text
//! 00-<trace_id_hex_32>-<span_id_hex_16>-<flags_hex_2>
//! ```
//!
//! - `version`: `"00"` (only version we accept; future versions
//!   per spec must be backward-compatible at the parse level).
//! - `trace_id`: 32 lowercase hex chars (16 bytes).
//! - `span_id`: 16 lowercase hex chars (8 bytes). Each new span
//!   in the trace gets a fresh `span_id`; the trace_id is
//!   constant for the entire trace.
//! - `flags`: 2 lowercase hex chars; `01` means `sampled`, `00`
//!   means `not_sampled`.
//!
//! # Why a separate module
//!
//! The parser is small (~50 LOC) but exercising every reject
//! path matters — a malformed traceparent that leaks through
//! breaks distributed correlation silently. Pulling it into its
//! own module with explicit reject tests means the wire-format
//! contract is enforced once and the rest of the workspace can
//! consume it as a black box.

// **PLATFORM:** all
// **GATING:** none.

use serde::{Deserialize, Serialize};

/// Errors raised parsing a W3C `traceparent` header string.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum TraceparentError {
    /// Header was not split by `-` into exactly 4 fields.
    #[error("traceparent must have 4 dash-separated fields")]
    BadShape,
    /// Version field was not `"00"`.
    #[error("unsupported traceparent version: {0}")]
    UnsupportedVersion(String),
    /// `trace_id` was not 32 lowercase hex chars.
    #[error("trace_id must be 32 lowercase hex chars")]
    BadTraceId,
    /// `span_id` was not 16 lowercase hex chars.
    #[error("span_id must be 16 lowercase hex chars")]
    BadSpanId,
    /// `flags` was not 2 lowercase hex chars.
    #[error("flags must be 2 lowercase hex chars")]
    BadFlags,
    /// `trace_id` was all zeros — RFC requires the value to be a
    /// random non-zero id.
    #[error("trace_id must not be all-zero")]
    AllZeroTraceId,
    /// `span_id` was all zeros — RFC requires the value to be a
    /// random non-zero id.
    #[error("span_id must not be all-zero")]
    AllZeroSpanId,
}

/// Parsed `traceparent`. Carry the parent context across an
/// `Arc<Traceparent>` boundary to avoid re-parsing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traceparent {
    /// Constant for the entire trace. 16 bytes (32 hex chars).
    pub trace_id: [u8; 16],
    /// Per-span. 8 bytes (16 hex chars). The dispatcher assigns a
    /// fresh span_id when forwarding the parent context across an
    /// IPC boundary so the receiver gets a distinct child span.
    pub span_id: [u8; 8],
    /// Sampling / debug flags. `0x01` = sampled.
    pub flags: u8,
}

impl Traceparent {
    /// Parse a `traceparent` header value. Returns
    /// [`TraceparentError`] on any malformed input.
    pub fn parse(s: &str) -> Result<Self, TraceparentError> {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 4 {
            return Err(TraceparentError::BadShape);
        }
        if parts[0] != "00" {
            return Err(TraceparentError::UnsupportedVersion(parts[0].to_owned()));
        }
        let trace_id = decode_hex_array::<16>(parts[1]).ok_or(TraceparentError::BadTraceId)?;
        if trace_id.iter().all(|b| *b == 0) {
            return Err(TraceparentError::AllZeroTraceId);
        }
        let span_id = decode_hex_array::<8>(parts[2]).ok_or(TraceparentError::BadSpanId)?;
        if span_id.iter().all(|b| *b == 0) {
            return Err(TraceparentError::AllZeroSpanId);
        }
        let flags_arr: [u8; 1] =
            decode_hex_array::<1>(parts[3]).ok_or(TraceparentError::BadFlags)?;
        Ok(Self {
            trace_id,
            span_id,
            flags: flags_arr[0],
        })
    }

    /// Render back to wire format.
    #[must_use]
    pub fn to_wire(&self) -> String {
        format!(
            "00-{}-{}-{}",
            encode_hex(&self.trace_id),
            encode_hex(&self.span_id),
            encode_hex(&[self.flags]),
        )
    }

    /// Build a new traceparent for an outbound request that
    /// inherits this one's `trace_id` and `flags` but assigns a
    /// fresh `span_id`. Used when forwarding a parent context
    /// across an IPC boundary so the receiver shows up as a
    /// distinct child span in trace UIs.
    #[must_use]
    pub fn child(&self, child_span_id: [u8; 8]) -> Self {
        Self {
            trace_id: self.trace_id,
            span_id: child_span_id,
            flags: self.flags,
        }
    }

    /// `true` when the `sampled` flag bit is set.
    #[must_use]
    pub fn sampled(&self) -> bool {
        self.flags & 0x01 == 0x01
    }
}

fn decode_hex_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    let bytes = s.as_bytes();
    for i in 0..N {
        let hi = lowercase_hex_digit(bytes[i * 2])?;
        let lo = lowercase_hex_digit(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn lowercase_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        // RFC mandates lowercase; uppercase is a parse error.
        _ => None,
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(hex_nibble(b >> 4));
        out.push(hex_nibble(b & 0x0f));
    }
    out
}

fn hex_nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + n - 10) as char,
        _ => '?',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_sampled() {
        let s = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let tp = Traceparent::parse(s).unwrap();
        assert_eq!(tp.trace_id[0], 0x0a);
        assert_eq!(tp.trace_id[15], 0x9c);
        assert_eq!(tp.span_id[0], 0xb7);
        assert_eq!(tp.span_id[7], 0x31);
        assert_eq!(tp.flags, 0x01);
        assert!(tp.sampled());
    }

    #[test]
    fn parse_valid_not_sampled() {
        let s = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-00";
        let tp = Traceparent::parse(s).unwrap();
        assert!(!tp.sampled());
    }

    #[test]
    fn round_trip_via_to_wire() {
        let s = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        let tp = Traceparent::parse(s).unwrap();
        assert_eq!(tp.to_wire(), s);
    }

    #[test]
    fn rejects_bad_shape() {
        for bad in ["", "00", "00-trace-span", "00-trace-span-flags-extra"] {
            assert_eq!(
                Traceparent::parse(bad).unwrap_err(),
                TraceparentError::BadShape
            );
        }
    }

    #[test]
    fn rejects_unsupported_version() {
        let s = "ff-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
        match Traceparent::parse(s).unwrap_err() {
            TraceparentError::UnsupportedVersion(v) => assert_eq!(v, "ff"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_bad_trace_id_length() {
        let s = "00-0af7-b7ad6b7169203331-01";
        assert_eq!(
            Traceparent::parse(s).unwrap_err(),
            TraceparentError::BadTraceId
        );
    }

    #[test]
    fn rejects_uppercase_hex_per_rfc() {
        let s = "00-0AF7651916CD43DD8448EB211C80319C-b7ad6b7169203331-01";
        assert_eq!(
            Traceparent::parse(s).unwrap_err(),
            TraceparentError::BadTraceId
        );
    }

    #[test]
    fn rejects_all_zero_trace_id() {
        let s = "00-00000000000000000000000000000000-b7ad6b7169203331-01";
        assert_eq!(
            Traceparent::parse(s).unwrap_err(),
            TraceparentError::AllZeroTraceId
        );
    }

    #[test]
    fn rejects_all_zero_span_id() {
        let s = "00-0af7651916cd43dd8448eb211c80319c-0000000000000000-01";
        assert_eq!(
            Traceparent::parse(s).unwrap_err(),
            TraceparentError::AllZeroSpanId
        );
    }

    #[test]
    fn child_inherits_trace_id_and_flags_changes_span_id() {
        let parent =
            Traceparent::parse("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").unwrap();
        let new_span: [u8; 8] = [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x11, 0x22];
        let child = parent.child(new_span);
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.flags, parent.flags);
        assert_eq!(child.span_id, new_span);
        assert_ne!(child.span_id, parent.span_id);
    }

    #[test]
    fn serde_roundtrip() {
        let tp =
            Traceparent::parse("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").unwrap();
        let json = serde_json::to_string(&tp).unwrap();
        let back: Traceparent = serde_json::from_str(&json).unwrap();
        assert_eq!(tp, back);
    }
}
