#![allow(clippy::pedantic)]
//! Property tests for the pcloud-proto binary response parser and
//! request-frame encoder. Fuzzy inputs must never panic and must never
//! violate documented parser limits.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_proto::binary_api::{
    BinaryParam, BinaryParamValue, FrameParseError, MAX_PARAM_NAME_LEN, MAX_REQUEST_FRAME_LEN,
    encode_request,
};
use pcloud_proto::response::{ParseLimits, Value, parse_response_frame};
use proptest::prelude::*;

// Proptest is not currently a dev-dep on pcloud-proto; tests fail-fast if it
// is missing from Cargo.toml.

fn arb_param_value() -> impl Strategy<Value = BinaryParamValue> {
    prop_oneof![
        ".{0,128}".prop_map(BinaryParamValue::String),
        any::<u64>().prop_map(BinaryParamValue::Number),
        any::<bool>().prop_map(BinaryParamValue::Bool),
    ]
}

fn arb_param() -> impl Strategy<Value = BinaryParam> {
    // Param names are bounded by MAX_PARAM_NAME_LEN.
    (0usize..=MAX_PARAM_NAME_LEN, arb_param_value()).prop_map(|(n, value)| BinaryParam {
        name: "a".repeat(n),
        value,
    })
}

proptest! {
    /// Encoded request frames never exceed the documented on-wire limit.
    #[test]
    fn prop_encode_request_respects_frame_limit(
        cmd_len in 0usize..=64,
        params in prop::collection::vec(arb_param(), 0..8),
    ) {
        let cmd = "x".repeat(cmd_len);
        match encode_request(&cmd, &params, None) {
            Ok(encoded) => {
                prop_assert!(encoded.bytes.len() <= MAX_REQUEST_FRAME_LEN);
                prop_assert_eq!(encoded.frame.parameter_count, params.len());
                prop_assert_eq!(encoded.frame.command, cmd);
            }
            Err(FrameParseError::ParamNameTooLong(_)) => {
                // Only possible if we generated a too-long name, which we don't.
                prop_assert!(false, "unexpected ParamNameTooLong");
            }
            Err(FrameParseError::RequestTooLarge) | Err(FrameParseError::CommandTooLong) => {
                // Acceptable rejection; we stayed within bounds but safety check tripped.
            }
            Err(other) => prop_assert!(false, "unexpected encode error: {other:?}"),
        }
    }

    /// An over-long parameter name must be rejected with the proper variant.
    #[test]
    fn prop_encode_rejects_overlong_param_name(len in (MAX_PARAM_NAME_LEN + 1)..=(MAX_PARAM_NAME_LEN + 16)) {
        let params = vec![BinaryParam {
            name: "z".repeat(len),
            value: BinaryParamValue::Bool(true),
        }];
        let err = encode_request("cmd", &params, None).expect_err("should reject");
        prop_assert!(matches!(err, FrameParseError::ParamNameTooLong(_)));
    }

    /// Random byte input to the response parser must never panic and must
    /// produce an error or a `Value` that structurally respects `ParseLimits`.
    #[test]
    fn prop_response_parser_does_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let limits = ParseLimits::default();
        if let Ok(v) = parse_response_frame(&bytes, &limits) {
            // Sanity: returned value must respect the depth limit
            // (reached indirectly through array/hash nesting).
            prop_assert!(max_depth(&v, 0) <= limits.max_depth + 1);
        }
    }

    /// Tiny inputs (< 4 bytes) always return `TruncatedFrame`.
    #[test]
    fn prop_tiny_inputs_always_truncated(bytes in prop::collection::vec(any::<u8>(), 0..4)) {
        let err = parse_response_frame(&bytes, &ParseLimits::default()).expect_err("must err");
        let msg = format!("{err}");
        prop_assert!(msg.contains("truncated"));
    }

    /// A frame header that declares a length beyond `limits.max_frame_len`
    /// must be rejected as `FrameTooLarge`.
    #[test]
    fn prop_oversized_frame_header_rejected(extra in 0u32..=1024) {
        let limits = ParseLimits { max_frame_len: 64, ..ParseLimits::default() };
        let bogus_len = (limits.max_frame_len as u32).saturating_add(1).saturating_add(extra);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&bogus_len.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, bogus_len as usize));
        let err = parse_response_frame(&bytes, &limits).expect_err("must err");
        let msg = format!("{err}");
        prop_assert!(msg.contains("limit"));
    }
}

fn max_depth(value: &Value, depth: usize) -> usize {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|v| max_depth(v, depth + 1))
            .max()
            .unwrap_or(depth),
        Value::Hash(kv) => kv
            .iter()
            .map(|(_, v)| max_depth(v, depth + 1))
            .max()
            .unwrap_or(depth),
        _ => depth,
    }
}
