#![allow(clippy::pedantic)]
//! CI-friendly smoke fuzzing driven by the `arbitrary` crate.
//!
//! Mirrors the logic of the cargo-fuzz targets under `fuzz/fuzz_targets/`
//! but runs as a normal `cargo test` invocation on stable Rust. No nightly,
//! no libFuzzer, no separate harness. Failures here indicate a real panic
//! or assertion violation in the parse / decode surface.

// **PLATFORM:** all
// **GATING:** none (portable).

use arbitrary::{Arbitrary, Unstructured};
use pcloud_ipc::protocol::{decode_request, decode_response};
use pcloud_proto::binary_api::{
    BinaryParam, BinaryParamValue, MAX_REQUEST_FRAME_LEN, encode_request,
};
use pcloud_proto::response::{ParseLimits, Value, parse_response_frame};

fn prng_bytes(seed: u64, len: usize) -> Vec<u8> {
    // xorshift64* — deterministic across runs, no external crate.
    let mut state = seed | 1;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let v = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn walk(value: &Value, depth: usize) {
    if depth > 32 {
        return;
    }
    match value {
        Value::Array(items) => items.iter().for_each(|v| walk(v, depth + 1)),
        Value::Hash(entries) => entries.iter().for_each(|(_, v)| walk(v, depth + 1)),
        _ => {}
    }
}

#[test]
fn smoke_parse_response_frame_does_not_panic() {
    let limits = ParseLimits::default();
    for seed in 0u64..4096 {
        let bytes = prng_bytes(seed, 1 + (seed as usize % 4096));
        if let Ok(v) = parse_response_frame(&bytes, &limits) {
            walk(&v, 0);
        }
    }
}

#[test]
fn smoke_ipc_decode_does_not_panic() {
    for seed in 0u64..2048 {
        let bytes = prng_bytes(seed, 1 + (seed as usize % 2048));
        let _ = decode_request(&bytes);
        let _ = decode_response(&bytes);
    }
}

#[test]
fn smoke_encode_request_respects_limits() {
    #[derive(Arbitrary)]
    struct ReqShape {
        cmd_tail: Vec<u8>,
        params: Vec<(Vec<u8>, u8, Vec<u8>, u64, bool)>,
    }

    for seed in 0u64..1024 {
        let bytes = prng_bytes(seed, 512);
        let mut unstructured = Unstructured::new(&bytes);
        let Ok(shape) = ReqShape::arbitrary(&mut unstructured) else {
            continue;
        };
        let cmd_bytes: Vec<u8> = shape
            .cmd_tail
            .into_iter()
            .take(32)
            .filter(|b| *b != 0)
            .collect();
        let cmd = String::from_utf8_lossy(&cmd_bytes).into_owned();
        let params: Vec<BinaryParam> = shape
            .params
            .into_iter()
            .take(8)
            .map(|(name_bytes, kind, str_bytes, num, boolean)| {
                let name =
                    String::from_utf8_lossy(&name_bytes.into_iter().take(60).collect::<Vec<_>>())
                        .into_owned();
                let value = match kind % 3 {
                    0 => BinaryParamValue::Bool(boolean),
                    1 => BinaryParamValue::Number(num),
                    _ => BinaryParamValue::String(
                        String::from_utf8_lossy(
                            &str_bytes.into_iter().take(256).collect::<Vec<_>>(),
                        )
                        .into_owned(),
                    ),
                };
                BinaryParam { name, value }
            })
            .collect();

        if let Ok(enc) = encode_request(&cmd, &params, None) {
            assert!(enc.bytes.len() <= MAX_REQUEST_FRAME_LEN);
            assert_eq!(enc.frame.command, cmd);
            assert_eq!(enc.frame.parameter_count, params.len());
        }
    }
}

#[test]
fn smoke_path_shapes_do_not_panic_encoder() {
    let adversarial: &[&str] = &[
        "",
        "/",
        "/\0",
        "/../etc/passwd",
        "//double//slash",
        "/\u{202e}rtl-override",
        "\\\\windows\\style",
    ];
    for path in adversarial {
        let params = vec![
            BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String("t".to_owned()),
            },
            BinaryParam {
                name: "path".to_owned(),
                value: BinaryParamValue::String((*path).to_owned()),
            },
        ];
        let _ = encode_request("listfolder", &params, Some(0));
    }

    for size in [1_024usize, 64 * 1024, 256 * 1024] {
        let long = "a".repeat(size);
        let params = vec![BinaryParam {
            name: "path".to_owned(),
            value: BinaryParamValue::String(long),
        }];
        let _ = encode_request("listfolder", &params, Some(0));
    }
}
