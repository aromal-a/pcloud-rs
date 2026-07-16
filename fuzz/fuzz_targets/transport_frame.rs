//! Fuzz target: binary protocol transport-frame parser.
//!
//! Models the two-step read sequence the [`BinaryApiTransport`] uses on
//! the wire: first parse a 4-byte little-endian length prefix via
//! [`parse_response_frame_len`], then hand the indicated number of body
//! bytes to [`parse_response_frame`]. Neither step is allowed to panic,
//! over-read, or recurse past `ParseLimits::default()`.
//!
//! Distinct from `crates/pcloud-proto/fuzz/fuzz_targets/fuzz_response_parser.rs`
//! (which fuzzes the body parser in isolation) and from
//! `fuzz_binary_request_roundtrip.rs` (which fuzzes the encoder side):
//! this target exercises the *composition* of length-prefix parsing and
//! body parsing the way the real transport reads frames off a socket.
//!
//! Run with:
//!
//! ```text
//! cd fuzz
//! cargo +nightly fuzz run transport_frame
//! ```
#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_proto::binary_api::parse_response_frame_len;
use pcloud_proto::response::{parse_response_frame, ParseLimits};

fuzz_target!(|data: &[u8]| {
    // Step 1: parse the 4-byte length header. Any input <4 bytes or with
    // a length above MAX_RESPONSE_FRAME_LEN must produce a structured
    // error, never a panic.
    if data.len() < 4 {
        let _ = parse_response_frame_len(data);
        return;
    }

    let header = &data[..4];
    let advertised = match parse_response_frame_len(header) {
        Ok(n) => n as usize,
        Err(_) => return,
    };

    // Step 2: feed the full frame (length prefix + body) to the body
    // parser. `parse_response_frame` re-reads the 4-byte prefix
    // internally and bounds the body against `ParseLimits::max_frame_len`,
    // so the input we hand it must include those four bytes.
    //
    // We slice the input two ways and run the parser on each:
    //
    //   * `clamped`   — input truncated to (advertised + 4) bytes, the
    //                   exact frame length the transport read off the
    //                   socket.
    //   * `full`      — the entire fuzzer input, possibly with trailing
    //                   bytes the parser must ignore or reject without
    //                   panicking.
    //
    // The real transport only ever hands `clamped` to the parser, but
    // catching divergent behaviour between the two slices is worth a
    // second call.
    let clamped_end = (4usize).saturating_add(advertised).min(data.len());
    let clamped = &data[..clamped_end];
    let limits = ParseLimits::default();
    let _ = parse_response_frame(clamped, &limits);
    let _ = parse_response_frame(data, &limits);
});
