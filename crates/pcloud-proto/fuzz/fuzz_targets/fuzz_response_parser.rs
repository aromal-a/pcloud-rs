//! Fuzz target: malformed / random response frames fed to the pcloud-proto
//! binary response parser. Must NEVER panic or recurse past the configured
//! depth limit.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-proto/fuzz
//! cargo +nightly fuzz run fuzz_response_parser
//! ```
#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_proto::response::{parse_response_frame, ParseLimits};

fuzz_target!(|data: &[u8]| {
    let limits = ParseLimits::default();
    let _ = parse_response_frame(data, &limits);
});
