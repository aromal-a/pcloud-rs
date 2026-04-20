//! Binary codec for the C `rpc_message_t` frame.
//!
//! Wire layout (host-endian, GCC/clang default packing on Linux/x86_64):
//!
//! ```text
//! offset  size  field
//!  0      4     uint32_t type        // command id (pcommands.h)
//!  4      4     <padding>
//!  8      8     uint64_t length      // total bytes including header
//! 16      N     char value[]         // NUL-terminated argument / reply string
//! ```
//!
//! `length` covers the **full** frame (header + value bytes), matching
//! `pclsync/prpc.c`:
//! `ssize_t total_size = offsetof(rpc_message_t, value) + response->length;`
//! The C reader caps at `POVERLAY_BUFSIZE = 512` bytes; we enforce the same
//! upper bound by default and expose the constant for callers.

// **PLATFORM:** all
// **GATING:** none (portable).

use thiserror::Error;

/// C `POVERLAY_BUFSIZE` — maximum frame size the C server tolerates.
pub const POVERLAY_BUFSIZE: usize = 512;

/// Offset of `value[]` inside the C `rpc_message_t`.
///
/// `uint32_t` (4) + 4 bytes of natural-alignment padding + `uint64_t` (8).
pub const HEADER_SIZE: usize = 16;

/// Command opcodes from `pclsync/pcommands.h` (values 20..=32).
///
/// The enum is `u32`-repr so it matches the wire `type` field exactly.
/// Unknown opcodes are represented separately via [`RpcOpcode::try_from`].
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RpcOpcode {
    /// Unlock crypto with password argument.
    StartCrypto = 20,
    /// Lock crypto. Empty argument.
    StopCrypto = 21,
    /// Request daemon shutdown. Empty argument.
    Finalize = 22,
    /// List sync folders. Result delivered via shm.
    ListSync = 23,
    /// Add sync folder. Argument: `"local|remote"`.
    AddSync = 24,
    /// Remove sync folder. Argument: decimal ASCII folder id.
    StopSync = 25,
    /// Pending-change counter query. Result via shm (`u32`).
    CheckPending = 26,
    /// Global pause of all syncs. Empty argument.
    SyncPause = 27,
    /// Global resume of all syncs. Empty argument.
    SyncResume = 28,
    /// Submit TFA code.
    SendTfa = 29,
    /// Submit password (ephemeral).
    SendAuth = 30,
    /// Submit password and request persistent auth.
    SendAuthSave = 31,
    /// Query daemon status. Result via shm (NUL-terminated string).
    GetStatus = 32,
}

impl RpcOpcode {
    /// Numeric opcode on the wire.
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    /// All known opcodes in declaration order.
    pub const ALL: [Self; 13] = [
        Self::StartCrypto,
        Self::StopCrypto,
        Self::Finalize,
        Self::ListSync,
        Self::AddSync,
        Self::StopSync,
        Self::CheckPending,
        Self::SyncPause,
        Self::SyncResume,
        Self::SendTfa,
        Self::SendAuth,
        Self::SendAuthSave,
        Self::GetStatus,
    ];
}

impl TryFrom<u32> for RpcOpcode {
    type Error = UnknownOpcode;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Ok(match value {
            20 => Self::StartCrypto,
            21 => Self::StopCrypto,
            22 => Self::Finalize,
            23 => Self::ListSync,
            24 => Self::AddSync,
            25 => Self::StopSync,
            26 => Self::CheckPending,
            27 => Self::SyncPause,
            28 => Self::SyncResume,
            29 => Self::SendTfa,
            30 => Self::SendAuth,
            31 => Self::SendAuthSave,
            32 => Self::GetStatus,
            other => return Err(UnknownOpcode(other)),
        })
    }
}

/// Unknown opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("unknown rpc opcode: {0}")]
pub struct UnknownOpcode(pub u32);

/// Decoded `rpc_message_t`.
///
/// `length` on the wire is total frame bytes (header + value); the struct
/// stores the `value` bytes directly for convenience. [`Self::encode`]
/// recomputes the wire `length` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcMessage {
    /// Raw wire type code. Use [`RpcOpcode::try_from`] to interpret.
    pub type_code: u32,
    /// Total frame length on the wire (header + value). Populated by
    /// [`Self::decode`]; ignored by [`Self::encode`] which recomputes it.
    pub length: u64,
    /// Argument / reply string bytes (no implicit trailing NUL added).
    pub value: Vec<u8>,
}

/// Codec errors.
#[derive(Debug, Error)]
pub enum CodecError {
    /// Buffer shorter than the 16-byte header.
    #[error("frame shorter than header ({0} bytes, need at least {HEADER_SIZE})")]
    ShortHeader(usize),
    /// Declared `length` is shorter than the header itself.
    #[error("declared length {declared} shorter than header ({HEADER_SIZE})")]
    LengthUnderHeader {
        /// The declared length from the header.
        declared: u64,
    },
    /// Declared `length` exceeds `POVERLAY_BUFSIZE`.
    #[error("declared length {declared} exceeds POVERLAY_BUFSIZE ({POVERLAY_BUFSIZE})")]
    LengthTooLarge {
        /// The declared length from the header.
        declared: u64,
    },
    /// Buffer smaller than declared length.
    #[error("buffer truncated: have {have}, declared {declared}")]
    Truncated {
        /// Bytes available in the input buffer.
        have: usize,
        /// Bytes declared by the header.
        declared: u64,
    },
    /// Payload bigger than will fit in `POVERLAY_BUFSIZE`.
    #[error("payload {payload} bytes exceeds POVERLAY_BUFSIZE - HEADER_SIZE ({max})")]
    PayloadTooLarge {
        /// Payload size.
        payload: usize,
        /// Maximum payload size.
        max: usize,
    },
}

impl RpcMessage {
    /// Construct a message from an opcode and value bytes.
    pub fn new(opcode: RpcOpcode, value: impl Into<Vec<u8>>) -> Self {
        let value = value.into();
        let length = (HEADER_SIZE + value.len()) as u64;
        Self {
            type_code: opcode.as_u32(),
            length,
            value,
        }
    }

    /// Encode to an owned byte buffer in the C wire layout.
    ///
    /// The `length` field on the output frame is always computed from the
    /// current `value.len()`; the struct field is ignored here to prevent
    /// encode/decode drift.
    pub fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let max_payload = POVERLAY_BUFSIZE - HEADER_SIZE;
        if self.value.len() > max_payload {
            return Err(CodecError::PayloadTooLarge {
                payload: self.value.len(),
                max: max_payload,
            });
        }
        let total = HEADER_SIZE + self.value.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&self.type_code.to_ne_bytes());
        out.extend_from_slice(&[0u8; 4]); // padding between type and length
        out.extend_from_slice(&(total as u64).to_ne_bytes());
        out.extend_from_slice(&self.value);
        Ok(out)
    }

    /// Decode a frame from a buffer.
    ///
    /// Returns the parsed message and the number of bytes consumed (equal
    /// to the declared `length`).
    pub fn decode(buf: &[u8]) -> Result<(Self, usize), CodecError> {
        if buf.len() < HEADER_SIZE {
            return Err(CodecError::ShortHeader(buf.len()));
        }
        // SAFETY: the preceding length check guarantees `buf.len() >= HEADER_SIZE`
        // (= 16 bytes), so `buf[0..4]` and `buf[8..16]` are in-bounds slices of
        // exactly 4 and 8 bytes respectively. `try_into::<[u8;N]>` on a
        // same-length slice is infallible — a panic here would mean the
        // bounds check above was elided, a compiler/bug case.
        let type_code = u32::from_ne_bytes(buf[0..4].try_into().expect("4 bytes"));
        let length = u64::from_ne_bytes(buf[8..16].try_into().expect("8 bytes"));
        if length < HEADER_SIZE as u64 {
            return Err(CodecError::LengthUnderHeader { declared: length });
        }
        if length > POVERLAY_BUFSIZE as u64 {
            return Err(CodecError::LengthTooLarge { declared: length });
        }
        if (buf.len() as u64) < length {
            return Err(CodecError::Truncated {
                have: buf.len(),
                declared: length,
            });
        }
        let value = buf[HEADER_SIZE..length as usize].to_vec();
        Ok((
            Self {
                type_code,
                length,
                value,
            },
            length as usize,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-built host-endian wire fixture: opcode 24 (ADDSYNC), value "a|b".
    ///
    /// Constructed without using the encoder so we verify layout independently.
    fn fixture_addsync() -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&24u32.to_ne_bytes());
        buf.extend_from_slice(&[0, 0, 0, 0]);
        buf.extend_from_slice(&((HEADER_SIZE + 3) as u64).to_ne_bytes());
        buf.extend_from_slice(b"a|b");
        buf
    }

    #[test]
    fn opcode_numeric_values_match_c_header() {
        // pcommands.h: STARTCRYPTO = 20, then sequential.
        assert_eq!(RpcOpcode::StartCrypto as u32, 20);
        assert_eq!(RpcOpcode::StopCrypto as u32, 21);
        assert_eq!(RpcOpcode::Finalize as u32, 22);
        assert_eq!(RpcOpcode::ListSync as u32, 23);
        assert_eq!(RpcOpcode::AddSync as u32, 24);
        assert_eq!(RpcOpcode::StopSync as u32, 25);
        assert_eq!(RpcOpcode::CheckPending as u32, 26);
        assert_eq!(RpcOpcode::SyncPause as u32, 27);
        assert_eq!(RpcOpcode::SyncResume as u32, 28);
        assert_eq!(RpcOpcode::SendTfa as u32, 29);
        assert_eq!(RpcOpcode::SendAuth as u32, 30);
        assert_eq!(RpcOpcode::SendAuthSave as u32, 31);
        assert_eq!(RpcOpcode::GetStatus as u32, 32);
    }

    #[test]
    fn opcode_try_from_roundtrip() {
        for op in RpcOpcode::ALL {
            assert_eq!(RpcOpcode::try_from(op.as_u32()).unwrap(), op);
        }
        assert!(RpcOpcode::try_from(0).is_err());
        assert!(RpcOpcode::try_from(19).is_err());
        assert!(RpcOpcode::try_from(33).is_err());
        assert!(RpcOpcode::try_from(u32::MAX).is_err());
    }

    #[test]
    fn header_size_is_sixteen_bytes() {
        // Matches `offsetof(rpc_message_t, value)` on x86_64 / aarch64 Linux.
        assert_eq!(HEADER_SIZE, 16);
    }

    #[test]
    fn decode_fixture_matches_expected() {
        let buf = fixture_addsync();
        let (msg, consumed) = RpcMessage::decode(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(msg.type_code, RpcOpcode::AddSync.as_u32());
        assert_eq!(msg.length, buf.len() as u64);
        assert_eq!(msg.value, b"a|b");
    }

    #[test]
    fn encode_then_decode_is_identity() {
        for op in RpcOpcode::ALL {
            let original = RpcMessage::new(op, b"payload-bytes".to_vec());
            let wire = original.encode().unwrap();
            let (parsed, consumed) = RpcMessage::decode(&wire).unwrap();
            assert_eq!(consumed, wire.len());
            assert_eq!(parsed, original);
        }
    }

    #[test]
    fn encode_matches_hand_built_fixture() {
        let msg = RpcMessage::new(RpcOpcode::AddSync, b"a|b".to_vec());
        assert_eq!(msg.encode().unwrap(), fixture_addsync());
    }

    #[test]
    fn encode_rejects_oversized_payload() {
        let big = vec![0u8; POVERLAY_BUFSIZE];
        let msg = RpcMessage::new(RpcOpcode::GetStatus, big);
        assert!(matches!(
            msg.encode(),
            Err(CodecError::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn decode_short_header_errors() {
        assert!(matches!(
            RpcMessage::decode(&[0u8; 8]),
            Err(CodecError::ShortHeader(8))
        ));
    }

    #[test]
    fn decode_rejects_length_under_header() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[0..4].copy_from_slice(&20u32.to_ne_bytes());
        buf[8..16].copy_from_slice(&1u64.to_ne_bytes());
        assert!(matches!(
            RpcMessage::decode(&buf),
            Err(CodecError::LengthUnderHeader { declared: 1 })
        ));
    }

    #[test]
    fn decode_rejects_length_over_limit() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[8..16].copy_from_slice(&(POVERLAY_BUFSIZE as u64 + 1).to_ne_bytes());
        assert!(matches!(
            RpcMessage::decode(&buf),
            Err(CodecError::LengthTooLarge { .. })
        ));
    }

    #[test]
    fn decode_rejects_truncation() {
        let mut buf = vec![0u8; HEADER_SIZE];
        buf[8..16].copy_from_slice(&((HEADER_SIZE + 8) as u64).to_ne_bytes());
        // actual buffer only has the header — missing 8 payload bytes
        assert!(matches!(
            RpcMessage::decode(&buf),
            Err(CodecError::Truncated { .. })
        ));
    }

    #[test]
    fn empty_value_roundtrip() {
        let msg = RpcMessage::new(RpcOpcode::Finalize, Vec::new());
        let wire = msg.encode().unwrap();
        assert_eq!(wire.len(), HEADER_SIZE);
        let (parsed, _) = RpcMessage::decode(&wire).unwrap();
        assert_eq!(parsed, msg);
    }
}
