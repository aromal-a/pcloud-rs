#![allow(clippy::pedantic)]
//! Proto transport in-process dispatch benchmarks.
//!
//! The full `BinaryApiTransport` pipeline requires TCP/TLS. We isolate the
//! CPU-bound halves here:
//!   - `encode_request`: build a wire frame for a representative command.
//!   - `parse_response_frame`: parse a small hand-built response frame
//!     (mirrors the test helper in `transport.rs`).
//!
//! Together these approximate a dev-mode in-process dispatch.

// **PLATFORM:** all
// **GATING:** none (portable).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

use pcloud_proto::{
    BinaryParam, BinaryParamValue, encode_request, parse_response_frame, response::ParseLimits,
};

fn bench_encode_request(c: &mut Criterion) {
    let mut group = c.benchmark_group("proto_encode_request");

    let params = vec![
        BinaryParam {
            name: "auth".to_owned(),
            value: BinaryParamValue::String("deadbeefcafebabe".repeat(4)),
        },
        BinaryParam {
            name: "folderid".to_owned(),
            value: BinaryParamValue::Number(1_234_567_u64),
        },
        BinaryParam {
            name: "nofiles".to_owned(),
            value: BinaryParamValue::Bool(true),
        },
    ];

    group.bench_function("encode_listfolder_small", |b| {
        b.iter(|| {
            let r = encode_request(black_box("listfolder"), black_box(&params), black_box(None))
                .expect("encode");
            black_box(r);
        });
    });

    group.finish();
}

fn bench_parse_response(c: &mut Criterion) {
    let mut group = c.benchmark_group("proto_parse_response");

    // Matches the test frame in pcloud-proto/src/transport.rs:
    // 10-byte payload, one hash entry { "result": 200 }.
    let frame: [u8; 14] = [
        10u8, 0, 0, 0, 16, 106, b'r', b'e', b's', b'u', b'l', b't', 200, 255,
    ];
    group.throughput(Throughput::Bytes(frame.len() as u64));
    let limits = ParseLimits::default();

    group.bench_function("parse_small_hash_response", |b| {
        b.iter(|| {
            let v = parse_response_frame(black_box(&frame), black_box(&limits)).expect("parse");
            black_box(v);
        });
    });

    group.finish();
}

fn bench_encode_parse_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("proto_encode_parse_roundtrip");

    let params = vec![BinaryParam {
        name: "ping".to_owned(),
        value: BinaryParamValue::Bool(true),
    }];
    let frame: [u8; 14] = [
        10u8, 0, 0, 0, 16, 106, b'r', b'e', b's', b'u', b'l', b't', 200, 255,
    ];
    let limits = ParseLimits::default();

    group.bench_function("encode_then_parse_small", |b| {
        b.iter(|| {
            let req = encode_request(black_box("noop"), black_box(&params), None).expect("encode");
            let parsed = parse_response_frame(black_box(&frame), &limits).expect("parse");
            black_box((req, parsed));
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_encode_request,
    bench_parse_response,
    bench_encode_parse_roundtrip
);
criterion_main!(benches);
