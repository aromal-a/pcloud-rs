#![allow(clippy::pedantic)]
//! Property tests for the pcloud-proto binary framer.
//!
//! These tests cover:
//! - `framer_roundtrip`: encode/parse identity for valid arbitrary frames.
//! - `framer_does_not_panic_on_any_input`: decoding arbitrary bytes via
//!   `parse_response_frame_len` never panics and yields either `Ok` or `Err`.
//! - `size_invariant`: the declared payload length in a freshly encoded
//!   request matches the actual trailing byte count (total encoded length
//!   equals declared length + 2).
//!
//! Case counts are capped via `ProptestConfig` to keep CI runtime bounded.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_proto::binary_api::{
    BinaryParam, BinaryParamValue, MAX_PARAM_NAME_LEN, MAX_RESPONSE_FRAME_LEN, encode_request,
    parse_response_frame_len,
};
use proptest::collection::vec;
use proptest::prelude::*;

/// Strategy for a plausible pCloud command name (ASCII, <=255 chars).
fn command_strategy() -> impl Strategy<Value = String> {
    // command length is u8 (0..=255) in the wire header.
    "[a-z_]{1,32}".prop_map(|s| s.to_string())
}

/// Strategy for a parameter name (ASCII, length <= MAX_PARAM_NAME_LEN).
fn param_name_strategy() -> impl Strategy<Value = String> {
    (1usize..=MAX_PARAM_NAME_LEN).prop_flat_map(|len| {
        prop::collection::vec(prop::char::range('a', 'z'), len..=len)
            .prop_map(|chars| chars.into_iter().collect::<String>())
    })
}

fn param_value_strategy() -> impl Strategy<Value = BinaryParamValue> {
    prop_oneof![
        // Bound string values so the total frame stays under 64 KiB.
        vec(any::<u8>(), 0..256).prop_map(|bytes| BinaryParamValue::String(
            String::from_utf8_lossy(&bytes).to_string()
        )),
        any::<u64>().prop_map(BinaryParamValue::Number),
        any::<bool>().prop_map(BinaryParamValue::Bool),
    ]
}

fn param_strategy() -> impl Strategy<Value = BinaryParam> {
    (param_name_strategy(), param_value_strategy())
        .prop_map(|(name, value)| BinaryParam { name, value })
}

fn request_strategy() -> impl Strategy<Value = (String, Vec<BinaryParam>)> {
    // Cap parameter count at 32; u8 limits it anyway and total frame <=65535.
    (command_strategy(), vec(param_strategy(), 0..16))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Encoding then re-inspecting the frame yields the original command
    /// and parameter count. (The framer is encode-only; we assert the
    /// round-trip invariants it does expose.)
    #[test]
    fn framer_roundtrip((cmd, params) in request_strategy()) {
        let encoded = encode_request(&cmd, &params, None)
            .expect("valid inputs must encode");
        prop_assert_eq!(encoded.frame.command, cmd);
        prop_assert_eq!(encoded.frame.parameter_count, params.len());
        prop_assert_eq!(encoded.params, params);
        // The first two bytes are the little-endian declared payload length.
        prop_assert!(encoded.bytes.len() >= 2);
    }

    /// `parse_response_frame_len` must never panic on arbitrary inputs; it
    /// either returns Ok(len) when len <= MAX_RESPONSE_FRAME_LEN or Err.
    #[test]
    fn framer_does_not_panic_on_any_input(bytes in vec(any::<u8>(), 0..512)) {
        // Any error variant is acceptable; on Ok, the length must be within bounds.
        if let Ok(len) = parse_response_frame_len(&bytes) {
            prop_assert!((len as usize) <= MAX_RESPONSE_FRAME_LEN);
        }
    }

    /// The declared on-wire payload length (first 2 LE bytes) plus the 2-byte
    /// length prefix itself must equal the total encoded frame byte count.
    #[test]
    fn size_invariant((cmd, params) in request_strategy()) {
        let encoded = encode_request(&cmd, &params, None)
            .expect("valid inputs must encode");
        prop_assert!(encoded.bytes.len() >= 2);
        let declared = u16::from_le_bytes([encoded.bytes[0], encoded.bytes[1]]) as usize;
        prop_assert_eq!(declared + 2, encoded.bytes.len());
    }

    /// The raw-body variant also satisfies the size invariant.
    #[test]
    fn size_invariant_with_raw_body(
        (cmd, params) in request_strategy(),
        body_len in any::<u64>(),
    ) {
        let encoded = match encode_request(&cmd, &params, Some(body_len)) {
            Ok(enc) => enc,
            Err(_) => return Ok(()), // request too large is a valid rejection
        };
        prop_assert!(encoded.bytes.len() >= 2);
        let declared = u16::from_le_bytes([encoded.bytes[0], encoded.bytes[1]]) as usize;
        prop_assert_eq!(declared + 2, encoded.bytes.len());
    }
}
