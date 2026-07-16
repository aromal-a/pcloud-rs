//! Fuzz target: code / path / URL string handling along the public-link
//! request and response code paths.
//!
//! `crates/pcloud-backends/src/public_link_backend.rs` does NOT host a
//! standalone URL parser; the user-facing API takes opaque
//! `code: impl Into<String>` and `path: impl Into<String>` arguments and
//! routes them straight into `pcloud_proto::binary_api::encode_request`,
//! which is what actually has to cope with arbitrary user / server
//! string content (control bytes, broken UTF-8, embedded NULs, very long
//! payloads, etc.). On the response side, the backend reads `code`,
//! `link`, and `name` strings out of the parsed binary-protocol response
//! via `HashView::get_string`. This target fuzzes both halves of that
//! flow:
//!
//! 1. **Request side.** Treat the fuzzer input as a (`code`, `path`,
//!    `linkid`) triple, build an `EncodedRequest` mirroring the shape
//!    `show_public_link` / `create_file_public_link` /
//!    `change_public_link_password` produce, and assert that
//!    `encode_request` either succeeds with frame-bounds intact or
//!    returns a structured `FrameParseError` — never panics.
//!
//! 2. **Response side.** Feed the same bytes into
//!    `parse_response_frame`. If the parse produces a hash, traverse it
//!    looking for `code`, `link`, `name`, and `url` string fields the
//!    backend would extract, exercising `HashView::get_string` on
//!    fuzzer-controlled key shapes.
//!
//! Run with:
//!
//! ```text
//! cd fuzz
//! cargo +nightly fuzz run public_link_uri
//! ```
#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_proto::binary_api::{encode_request, BinaryParam, BinaryParamValue};
use pcloud_proto::response::{parse_response_frame, ParseLimits, Value};

fn split_chunk<'a>(data: &'a [u8], cursor: &mut usize, max: usize) -> &'a [u8] {
    if *cursor >= data.len() {
        return &[];
    }
    let len = (data[*cursor] as usize) % (max + 1);
    *cursor += 1;
    let end = (*cursor).saturating_add(len).min(data.len());
    let slice = &data[*cursor..end];
    *cursor = end;
    slice
}

fn lossy_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn walk_value(value: &Value) {
    if let Some(hash) = value.as_hash() {
        // Probe the four string fields the public-link backend reads off
        // the wire. The keys themselves are static; the *values* are the
        // fuzzer-controlled strings whose handling we want to exercise.
        for key in ["code", "link", "name", "url"] {
            let _ = hash.get_string(key);
        }
        // Recurse into nested hashes (e.g. `metadata`) without unbounded
        // depth — `parse_response_frame` already enforces
        // `ParseLimits::default()` on the producer side.
        if let Some(metadata) = hash.get_hash("metadata") {
            for key in ["code", "link", "name", "url"] {
                let _ = metadata.get_string(key);
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // ---------- Request side ----------
    //
    // Carve out three short fuzzer-controlled byte slices, lossy-decode
    // them as UTF-8, and feed them as the `code`, `path`, and a numeric
    // link-id parameter to the binary-protocol encoder. The encoder must
    // either succeed (and respect MAX_REQUEST_FRAME_LEN, asserted by
    // construction inside `encode_request`) or return a structured
    // FrameParseError. No panic is acceptable.
    let mut cursor = 0usize;
    let cmd_bytes = split_chunk(data, &mut cursor, 31);
    let code_bytes = split_chunk(data, &mut cursor, 96);
    let path_bytes = split_chunk(data, &mut cursor, 96);
    let mut linkid_bytes = [0u8; 8];
    let take = (data.len().saturating_sub(cursor)).min(8);
    if take > 0 {
        linkid_bytes[..take].copy_from_slice(&data[cursor..cursor + take]);
    }
    cursor += take;

    let cmd = lossy_string(cmd_bytes);
    let code = lossy_string(code_bytes);
    let path = lossy_string(path_bytes);
    let linkid = u64::from_le_bytes(linkid_bytes);

    let params = vec![
        BinaryParam {
            name: "code".to_string(),
            value: BinaryParamValue::String(code),
        },
        BinaryParam {
            name: "path".to_string(),
            value: BinaryParamValue::String(path),
        },
        BinaryParam {
            name: "linkid".to_string(),
            value: BinaryParamValue::Number(linkid),
        },
    ];

    if !cmd.is_empty() {
        let _ = encode_request(&cmd, &params, None);
    }

    // ---------- Response side ----------
    //
    // Feed any leftover input through the response parser. Whatever
    // comes out — Hash, Array, scalar — the backend's string-extraction
    // helpers must cope with it. We probe the `code`, `link`, `name`,
    // `url` keys plus the nested `metadata` hash, exactly mirroring the
    // backend's response-walking pattern.
    let limits = ParseLimits::default();
    let body = if cursor < data.len() {
        &data[cursor..]
    } else {
        data
    };
    if let Ok(value) = parse_response_frame(body, &limits) {
        walk_value(&value);
    }
});
