//! Fuzz target: feed arbitrary bytes into the IPC frame decoders.
//!
//! Both `decode_request` and `decode_response` must return a structured
//! `ProtocolError` or a valid `Frame`. They must NEVER panic, over-read,
//! or allocate unbounded memory.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-ipc/fuzz
//! cargo +nightly fuzz run fuzz_ipc_frame
//! ```
#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_ipc::{decode_request, decode_response};

fuzz_target!(|data: &[u8]| {
    let _ = decode_request(data);
    let _ = decode_response(data);
});
