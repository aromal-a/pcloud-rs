#![allow(clippy::pedantic)]
//! IPC codec benchmarks.
//!
//! Covers Method encode/decode and full Request/Response frame round-trip
//! throughput against the length-prefixed JSON framing in
//! `pcloud-ipc/src/protocol.rs`.

// **PLATFORM:** all
// **GATING:** none (portable).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

use pcloud_ipc::methods::{Method, Request, Response, ResponseStatus};
use pcloud_ipc::protocol::{
    decode_request, decode_response, encode_request_bare as encode_request, encode_response,
};

fn method_encode_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_method");
    let method = Method::GetStatus;

    group.bench_function("serde_json_encode_method", |b| {
        b.iter(|| {
            let v = serde_json::to_vec(black_box(&method)).expect("encode");
            black_box(v);
        });
    });

    let encoded = serde_json::to_vec(&method).expect("encode");
    group.bench_function("serde_json_decode_method", |b| {
        b.iter(|| {
            let m: Method = serde_json::from_slice(black_box(&encoded)).expect("decode");
            black_box(m);
        });
    });

    group.finish();
}

fn request_roundtrip_plain(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_request_roundtrip");
    let req = Request::Plain {
        method: Method::GetStatus,
    };
    let bytes = encode_request(&req).expect("encode");
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    group.bench_function("encode_plain_request", |b| {
        b.iter(|| {
            let v = encode_request(black_box(&req)).expect("encode");
            black_box(v);
        });
    });

    group.bench_function("decode_plain_request", |b| {
        b.iter(|| {
            let frame = decode_request(black_box(&bytes)).expect("decode");
            black_box(frame);
        });
    });

    group.bench_function("roundtrip_plain_request", |b| {
        b.iter(|| {
            let v = encode_request(black_box(&req)).expect("encode");
            let f = decode_request(&v).expect("decode");
            black_box(f);
        });
    });

    group.finish();
}

fn request_roundtrip_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_request_roundtrip_large");
    // Realistic larger variant: sync-root add with moderately long paths.
    let req = Request::SyncRootAdd {
        local_path: "/home/user/Documents/projects/deep/nested/path/for/benchmarking/purposes"
            .repeat(4),
        remote_path: "/remote/deep/nested/path/for/benchmarking/purposes".repeat(4),
        sync_type: None,
    };
    let bytes = encode_request(&req).expect("encode");
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    group.bench_function("encode_large_request", |b| {
        b.iter(|| {
            let v = encode_request(black_box(&req)).expect("encode");
            black_box(v);
        });
    });

    group.bench_function("decode_large_request", |b| {
        b.iter(|| {
            let frame = decode_request(black_box(&bytes)).expect("decode");
            black_box(frame);
        });
    });

    group.finish();
}

fn response_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("ipc_response_roundtrip");
    let resp = Response {
        status: ResponseStatus::Ok,
        message: "ready".to_string(),
    };
    let bytes = encode_response(&resp).expect("encode");
    group.throughput(Throughput::Bytes(bytes.len() as u64));

    group.bench_function("encode_response", |b| {
        b.iter(|| {
            let v = encode_response(black_box(&resp)).expect("encode");
            black_box(v);
        });
    });

    group.bench_function("decode_response", |b| {
        b.iter(|| {
            let frame = decode_response(black_box(&bytes)).expect("decode");
            black_box(frame);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    method_encode_decode,
    request_roundtrip_plain,
    request_roundtrip_large,
    response_roundtrip
);
criterion_main!(benches);
