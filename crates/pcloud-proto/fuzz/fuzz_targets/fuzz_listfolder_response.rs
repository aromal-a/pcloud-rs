//! Fuzz target: structured-parser fuzzing of `listfolder` responses.
//!
//! Starts from a well-formed `listfolder` response frame (same shape the
//! unit test `folder_api::tests::list_folder_by_path_parses_metadata`
//! exercises) and applies random byte-level mutations driven by the
//! fuzzer. Both the byte-level parse (`parse_response_frame`) and the
//! structural accessors (`as_hash`, `get_array`, ...) that
//! `FolderApi::list_folder_by_path` relies on must cope without panicking
//! or looping unboundedly.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-proto/fuzz
//! cargo +nightly fuzz run fuzz_listfolder_response
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_proto::response::{parse_response_frame, ParseLimits, Value};

fn seed_frame() -> Vec<u8> {
    const RPARAM_HASH: u8 = 16;
    const RPARAM_ARRAY: u8 = 17;
    const RPARAM_BFALSE: u8 = 18;
    const RPARAM_SHORT_STR_BASE: u8 = 100;
    const RPARAM_SMALL_NUM_BASE: u8 = 200;
    const RPARAM_END: u8 = 255;

    fn push_short_str(payload: &mut Vec<u8>, s: &str) {
        assert!(s.len() <= 49);
        payload.push(RPARAM_SHORT_STR_BASE + s.len() as u8);
        payload.extend_from_slice(s.as_bytes());
    }

    fn push_small_num(payload: &mut Vec<u8>, n: u8) {
        assert!(n <= 19);
        payload.push(RPARAM_SMALL_NUM_BASE + n);
    }

    let mut payload = vec![RPARAM_HASH];
    push_short_str(&mut payload, "result");
    push_small_num(&mut payload, 0);
    push_short_str(&mut payload, "metadata");
    payload.push(RPARAM_HASH);
    push_short_str(&mut payload, "folderid");
    push_small_num(&mut payload, 2);
    push_short_str(&mut payload, "contents");
    payload.push(RPARAM_ARRAY);
    payload.push(RPARAM_HASH);
    push_short_str(&mut payload, "name");
    push_short_str(&mut payload, "file.txt");
    push_short_str(&mut payload, "isfolder");
    payload.push(RPARAM_BFALSE);
    push_short_str(&mut payload, "fileid");
    push_small_num(&mut payload, 7);
    push_short_str(&mut payload, "size");
    push_small_num(&mut payload, 12);
    payload.push(RPARAM_END);
    payload.push(RPARAM_END);
    payload.push(RPARAM_END);
    payload.push(RPARAM_END);

    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame
}

fn structurally_walk(value: &Value) {
    let Some(hash) = value.as_hash() else { return };
    std::hint::black_box(hash.get_number("result"));
    let Some(metadata) = hash.get_hash("metadata") else { return };
    std::hint::black_box(metadata.get_number("folderid"));
    let Some(contents) = metadata.get_array("contents") else { return };
    for entry in contents {
        if let Some(h) = entry.as_hash() {
            std::hint::black_box(h.get_string("name"));
            std::hint::black_box(h.get_bool("isfolder"));
            std::hint::black_box(h.get_number("fileid"));
            std::hint::black_box(h.get_number("size"));
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut frame = seed_frame();
    let payload_len = frame.len().saturating_sub(4);

    // Fuzzer-directed byte XORs on the payload portion only.
    for (i, byte) in data.iter().enumerate().take(payload_len) {
        if payload_len == 0 {
            break;
        }
        let idx = 4 + (i % payload_len);
        if let Some(slot) = frame.get_mut(idx) {
            *slot ^= *byte;
        }
    }

    let limits = ParseLimits::default();
    if let Ok(v) = parse_response_frame(&frame, &limits) {
        structurally_walk(&v);
    }
    if let Ok(v) = parse_response_frame(data, &limits) {
        structurally_walk(&v);
    }
});
