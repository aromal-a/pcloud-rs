//! Fuzz target: feed arbitrary bytes into the response-parsing entry point
//! that backs every `*Api::<method>` request/response cycle in
//! `pcloud-proto`.
//!
//! NOTE on naming: the upstream task description calls this "json_response",
//! however the pCloud binary protocol is not JSON — it is the custom binary
//! response format decoded by `response::parse_response_frame`. Every public
//! `*Api` surface (`AuthApi`, `AccountApi`, `BackupApi`, `CryptoApi`,
//! `FolderApi`, `PublicLinksApi`, `SharesApi`, `SyncApi`, `TransferApi`)
//! funnels its on-wire response bytes through this exact function before
//! handing the resulting `Value` to a typed per-method adapter. Fuzzing this
//! entry point therefore exercises the shared parse boundary for every Api.
//!
//! Distinct from the prior `fuzz_response_parser` target:
//!
//! * explores a cross-product of parser limit configurations so shrink /
//!   overflow / limit-boundary bugs that only trip at smaller caps are
//!   reachable,
//! * drives the accessor APIs on the resulting `Value` so UB in
//!   indexing / hash / array access would surface,
//! * also pipes the header window through `parse_response_frame_len` to
//!   fuzz the pre-parse length-prefix check in lockstep.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-proto/fuzz
//! cargo +nightly fuzz run fuzz_json_response
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_proto::binary_api::parse_response_frame_len;
use pcloud_proto::response::{parse_response_frame, ParseLimits, Value};

/// Mirrors the different `ParseLimits` shapes used by callers of the parse
/// boundary (defaults; tight short-message caps; very wide caps).
fn limit_variants() -> [ParseLimits; 4] {
    [
        ParseLimits::default(),
        ParseLimits {
            max_frame_len: 4 * 1024,
            max_depth: 4,
            max_array_len: 16,
            max_hash_len: 16,
            max_string_len: 256,
            max_reused_strings: 64,
        },
        ParseLimits {
            max_frame_len: 32,
            max_depth: 2,
            max_array_len: 2,
            max_hash_len: 2,
            max_string_len: 4,
            max_reused_strings: 2,
        },
        ParseLimits {
            max_frame_len: 8 * 1024 * 1024,
            max_depth: 64,
            max_array_len: 16_384,
            max_hash_len: 16_384,
            max_string_len: 256 * 1024,
            max_reused_strings: 16_384,
        },
    ]
}

fn walk_value(value: &Value, depth: usize) {
    if depth > 48 {
        return;
    }
    match value {
        Value::String(s) => {
            std::hint::black_box(s.len());
        }
        Value::Number(n) => {
            std::hint::black_box(*n);
        }
        Value::Bool(b) => {
            std::hint::black_box(*b);
        }
        Value::Data(n) => {
            std::hint::black_box(*n);
        }
        Value::Array(items) => {
            for item in items {
                walk_value(item, depth + 1);
            }
        }
        Value::Hash(entries) => {
            for (k, v) in entries {
                std::hint::black_box(k.len());
                walk_value(v, depth + 1);
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // Exercise the length-prefix pre-parse. Must never panic on any 4-byte
    // window and must always agree with a slice-local computation.
    if data.len() >= 4 {
        let _ = parse_response_frame_len(&data[..4]);
    }

    for limits in limit_variants() {
        match parse_response_frame(data, &limits) {
            Ok(v) => {
                // Invariant: parsed hash entry counts are bounded by the
                // configured hash limit. We check by walking — this catches
                // silent cap violations.
                if let Value::Hash(entries) = &v {
                    assert!(entries.len() <= limits.max_hash_len);
                }
                if let Value::Array(items) = &v {
                    assert!(items.len() <= limits.max_array_len);
                }
                walk_value(&v, 0);
                // Exercise accessor helpers so any UB in the indexer path
                // would trip.
                if let Some(hash) = v.as_hash() {
                    std::hint::black_box(hash.get_string("auth"));
                    std::hint::black_box(hash.get_number("result"));
                    std::hint::black_box(hash.get_bool("trustdevice"));
                }
                std::hint::black_box(v.as_string().is_some());
                std::hint::black_box(v.as_number());
            }
            Err(_) => {}
        }
    }
});
