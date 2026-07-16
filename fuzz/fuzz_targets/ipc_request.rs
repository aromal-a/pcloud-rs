//! Fuzz target: serde codec roundtrip of `pcloud_ipc::Request`.
//!
//! Bytes in -> deserialise as JSON `Request` -> re-serialise via
//! `encode_request_bare` -> re-decode via `decode_request` -> assert that
//! the recovered `Request` equals the first decode.
//!
//! # Codec note
//!
//! CLAUDEREV T3.4 originally specified a "serde-bincode" roundtrip. The
//! actual on-the-wire codec used by `pcloud-ipc::protocol` is
//! `serde_json` (see the module-level docs on `crates/pcloud-ipc/src/lib.rs`
//! correcting the prior CBOR/bincode mistake), so this target fuzzes the
//! real production codec. The roundtrip equality property the original
//! plan called for is preserved verbatim — only the serialization format
//! changes from bincode to JSON to match production reality.
//!
//! The first decode treats `data` as a serialised `RequestEnvelope` JSON
//! payload (the on-the-wire shape `try_from_wire` accepts, including the
//! bare-`Request` back-compat fallback). If decode succeeds, the inner
//! `Request` is re-encoded through `encode_request_bare` — which prepends
//! the framed 8-byte header — and the framed bytes are pushed back
//! through `decode_request`. The recovered `Request` MUST equal the
//! first one. Any failure of either decode step on the *re-encoded*
//! bytes is a parser/encoder bug, not a fuzzer-input bug.
//!
//! Run with:
//!
//! ```text
//! cd fuzz
//! cargo +nightly fuzz run ipc_request
//! ```
#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_ipc::methods::RequestEnvelope;
use pcloud_ipc::{decode_request, encode_request_bare};

fuzz_target!(|data: &[u8]| {
    // Step 1: try to interpret the input as a JSON-serialised
    // RequestEnvelope (or bare Request, via the back-compat fallback).
    // This is the same codec path `decode_request` uses internally on
    // the framed payload tail.
    let first = match RequestEnvelope::try_from_wire(data) {
        Ok(envelope) => envelope.request,
        Err(_) => return,
    };

    // Step 2: re-encode through the production framed-encoder. Any
    // failure here on a value we just decoded is a bug.
    let framed = match encode_request_bare(&first) {
        Ok(bytes) => bytes,
        Err(err) => panic!("re-encode of decoded Request failed: {err:?}"),
    };

    // Step 3: re-decode the framed bytes. Same panic-on-failure
    // discipline — a Request that just round-tripped through the
    // production encoder MUST decode again.
    let frame = match decode_request(&framed) {
        Ok(frame) => frame,
        Err(err) => panic!("re-decode after re-encode failed: {err:?}"),
    };

    // Step 4: equality check. Roundtrip stability is the property under
    // test.
    assert_eq!(
        frame.payload.request, first,
        "Request value drifted across encode/decode roundtrip",
    );
});
