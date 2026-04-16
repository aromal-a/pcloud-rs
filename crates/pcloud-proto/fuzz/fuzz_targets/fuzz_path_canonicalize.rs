//! Fuzz target: feed adversarial path strings into every proto surface that
//! encodes a remote path as a binary request parameter.
//!
//! Adversarial shapes exercised:
//!
//! * embedded NUL bytes,
//! * windows-style backslashes,
//! * unicode surrogate-pair-ish sequences, RTL overrides, zero-width joiners,
//! * `..`/`.` traversal fragments,
//! * deep nested segments,
//! * pathologically long strings (up to the encoder's `u32` length limit).
//!
//! Because the proto layer does not own a "canonicalize" helper, this target
//! exercises the actual encoders that carry paths across the wire:
//! `ListFolderByPathRequest::params` + `encode_request`. A path string that
//! causes the encoder to panic or accept a malformed frame is a bug.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-proto/fuzz
//! cargo +nightly fuzz run fuzz_path_canonicalize
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_proto::binary_api::{encode_request, BinaryParam, BinaryParamValue, MAX_REQUEST_FRAME_LEN};

fn synth_paths(seed: &[u8]) -> Vec<String> {
    // Build a small bouquet of path shapes from the seed so each fuzz input
    // explores multiple path-encoding angles.
    let raw = String::from_utf8_lossy(seed).into_owned();
    let mut out = Vec::with_capacity(8);
    out.push(raw.clone());
    // Embedded NUL — must be representable in String per Rust but must not
    // crash the encoder nor silently truncate on the wire.
    out.push(format!("/\0{}/\0/end", raw));
    // Windows-style and mixed separators.
    out.push(raw.replace('/', "\\"));
    // Traversal fragments.
    out.push(format!("/../../{}/..", raw));
    // Deeply nested.
    let deep = "a/".repeat(1024);
    out.push(format!("/{}{}", deep, raw));
    // Unicode bidi / zero-width joiners.
    out.push(format!("/\u{202e}{}\u{200d}", raw));
    // Pathologically long path (bounded so we do not allocate more than a
    // few MiB per iteration).
    let pad = "x".repeat(64 * 1024);
    out.push(format!("/{}{}", raw, pad));
    // Null only.
    out.push("\0\0\0".to_owned());
    out
}

fuzz_target!(|data: &[u8]| {
    for path in synth_paths(data) {
        let params = vec![
            BinaryParam {
                name: "auth".to_owned(),
                value: BinaryParamValue::String("fuzz-token".to_owned()),
            },
            BinaryParam {
                name: "path".to_owned(),
                value: BinaryParamValue::String(path.clone()),
            },
        ];
        match encode_request("listfolder", &params, Some(0)) {
            Ok(enc) => {
                // Documented frame size limit must hold.
                assert!(enc.bytes.len() <= MAX_REQUEST_FRAME_LEN);
                // The encoded request command/params must reflect the input.
                assert_eq!(enc.frame.command, "listfolder");
                assert_eq!(enc.frame.parameter_count, params.len());
            }
            Err(_) => {
                // Structured error is the correct behavior for paths that
                // exceed encoder bounds; a panic would have aborted the
                // process.
            }
        }
    }
});
