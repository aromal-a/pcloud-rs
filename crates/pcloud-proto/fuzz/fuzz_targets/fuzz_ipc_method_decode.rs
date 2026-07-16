//! Fuzz target: feed arbitrary bytes into the `pcloud-ipc` frame decoder.
//!
//! NOTE on naming: the upstream task description says "bincode-ish". The
//! actual IPC wire format uses a little-endian 8-byte header (payload_len
//! u32 + version u16 + message_type u16) followed by a JSON payload that
//! encodes the `Method` / `Request` / `Response` enums. Fuzzing the decoder
//! covers the full attack surface of a malicious local peer.
//!
//! This target asserts:
//!
//! * no panic for any byte sequence,
//! * no oversized-payload silent acceptance,
//! * version / message-type parsing never reads out of bounds.
//!
//! Run with:
//!
//! ```text
//! cd crates/pcloud-proto/fuzz
//! cargo +nightly fuzz run fuzz_ipc_method_decode
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use pcloud_ipc::methods::{Method, Request};
use pcloud_ipc::protocol::{
    MAX_IPC_PAYLOAD_LEN, decode_request, decode_response, encode_request_bare,
};

fn build_frame(data: &[u8]) -> Vec<u8> {
    // Interpret the first byte of input as a "mutation mode" so each fuzz
    // iteration explores a slightly different framing shape.
    if data.is_empty() {
        return Vec::new();
    }
    let mode = data[0] % 4;
    let body = &data[1..];
    match mode {
        0 => body.to_vec(),
        1 => {
            // Well-formed length prefix, random version and kind, arbitrary
            // payload bytes.
            let payload = if body.len() > 8 { &body[8..] } else { body };
            let mut out = Vec::with_capacity(8 + payload.len());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&[
                body.first().copied().unwrap_or(0),
                body.get(1).copied().unwrap_or(0),
                body.get(2).copied().unwrap_or(0),
                body.get(3).copied().unwrap_or(0),
            ]);
            out.extend_from_slice(payload);
            out
        }
        2 => {
            // Claimed length deliberately larger than MAX.
            let mut out = Vec::with_capacity(8 + body.len());
            out.extend_from_slice(&((MAX_IPC_PAYLOAD_LEN as u32).saturating_add(1)).to_le_bytes());
            out.extend_from_slice(&[1, 0, 1, 0]);
            out.extend_from_slice(body);
            out
        }
        _ => {
            // A valid request encoded and then truncated / flipped.
            let mut enc = encode_request_bare(&Request::Plain {
                method: Method::GetStatus,
            })
                .unwrap_or_default();
            let cut = body.first().copied().unwrap_or(0) as usize % (enc.len() + 1);
            enc.truncate(cut);
            for (i, b) in body.iter().take(8).enumerate() {
                if let Some(slot) = enc.get_mut(i) {
                    *slot ^= *b;
                }
            }
            enc
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let frame = build_frame(data);

    // Neither decoder is allowed to panic on untrusted input.
    match decode_request(&frame) {
        Ok(f) => {
            // Documented invariant: accepted payloads never exceed MAX.
            assert!((f.header.payload_len as usize) <= MAX_IPC_PAYLOAD_LEN);
        }
        Err(_) => {}
    }
    match decode_response(&frame) {
        Ok(f) => {
            assert!((f.header.payload_len as usize) <= MAX_IPC_PAYLOAD_LEN);
        }
        Err(_) => {}
    }

    // Also feed the raw bytes unmodified.
    let _ = decode_request(data);
    let _ = decode_response(data);
});
