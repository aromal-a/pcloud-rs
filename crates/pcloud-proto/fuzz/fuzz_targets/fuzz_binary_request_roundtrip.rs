//! Fuzz target: construct random binary-protocol requests and ensure the
//! encoder never panics. The encoder validates command/param lengths and
//! should always either produce a frame within `MAX_REQUEST_FRAME_LEN` or
//! return a structured `FrameParseError`.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-proto/fuzz
//! cargo +nightly fuzz run fuzz_binary_request_roundtrip
//! ```
#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_proto::binary_api::{encode_request, BinaryParam, BinaryParamValue, MAX_REQUEST_FRAME_LEN};

fuzz_target!(|data: &[u8]| {
    // Slice the input into a command name + a handful of params. The goal is
    // breadth of encoder inputs, not semantic validity.
    if data.is_empty() {
        return;
    }
    let cmd_len = (data[0] as usize) % 32;
    if data.len() < 1 + cmd_len {
        return;
    }
    let cmd = String::from_utf8_lossy(&data[1..1 + cmd_len]).into_owned();
    let mut cursor = 1 + cmd_len;
    let mut params = Vec::new();
    while cursor + 3 < data.len() && params.len() < 8 {
        let name_len = (data[cursor] as usize) % 80; // may exceed MAX_PARAM_NAME_LEN
        cursor += 1;
        if cursor + name_len > data.len() {
            break;
        }
        let name = String::from_utf8_lossy(&data[cursor..cursor + name_len]).into_owned();
        cursor += name_len;
        if cursor >= data.len() {
            break;
        }
        let kind = data[cursor] % 3;
        cursor += 1;
        let value = match kind {
            0 => BinaryParamValue::Bool((cursor < data.len()) && (data[cursor] & 1 == 1)),
            1 => {
                let mut n = [0u8; 8];
                let take = (data.len() - cursor).min(8);
                n[..take].copy_from_slice(&data[cursor..cursor + take]);
                cursor += take;
                BinaryParamValue::Number(u64::from_le_bytes(n))
            }
            _ => {
                let vlen = (cursor < data.len()).then(|| data[cursor] as usize).unwrap_or(0);
                cursor += 1;
                if cursor + vlen > data.len() {
                    BinaryParamValue::String(String::new())
                } else {
                    let s = String::from_utf8_lossy(&data[cursor..cursor + vlen]).into_owned();
                    cursor += vlen;
                    BinaryParamValue::String(s)
                }
            }
        };
        params.push(BinaryParam { name, value });
    }

    if let Ok(encoded) = encode_request(&cmd, &params, None) {
        assert!(encoded.bytes.len() <= MAX_REQUEST_FRAME_LEN);
        assert_eq!(encoded.frame.command, cmd);
        assert_eq!(encoded.frame.parameter_count, params.len());
    }
});
