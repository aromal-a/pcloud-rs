#![allow(clippy::pedantic)]
//! AES-256-GCM sector seal/open throughput.
//!
//! Covers the `pcloud-crypto::content` sector layer that mirrors the C
//! `PSYNC_CRYPTO_SECTOR_SIZE` content path.

// **PLATFORM:** all
// **GATING:** none (portable).

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

use pcloud_crypto::content::{SECTOR_SIZE_BYTES, derive_file_key, open_sector, seal_sector};
use pcloud_secret::secret_bytes::SecretBytes;

fn bench_seal(c: &mut Criterion) {
    let master = SecretBytes::new(vec![7u8; 32]);
    let file_key = derive_file_key(&master, b"bench-seed");
    let pt = vec![0xA5u8; SECTOR_SIZE_BYTES];

    let mut group = c.benchmark_group("crypto_seal_sector");
    group.throughput(Throughput::Bytes(SECTOR_SIZE_BYTES as u64));
    group.bench_function("seal_4k", |b| {
        b.iter(|| {
            let frame = seal_sector(
                black_box(&file_key),
                black_box(0),
                black_box(&pt),
                SECTOR_SIZE_BYTES,
            )
            .expect("seal");
            black_box(frame);
        });
    });
    group.finish();
}

fn bench_open(c: &mut Criterion) {
    let master = SecretBytes::new(vec![7u8; 32]);
    let file_key = derive_file_key(&master, b"bench-seed");
    let pt = vec![0xA5u8; SECTOR_SIZE_BYTES];
    let frame = seal_sector(&file_key, 0, &pt, SECTOR_SIZE_BYTES).expect("seal");

    let mut group = c.benchmark_group("crypto_open_sector");
    group.throughput(Throughput::Bytes(SECTOR_SIZE_BYTES as u64));
    group.bench_function("open_4k", |b| {
        b.iter(|| {
            let out = open_sector(black_box(&file_key), 0, black_box(&frame)).expect("open");
            black_box(out);
        });
    });
    group.finish();
}

fn bench_key_derivation(c: &mut Criterion) {
    let master = SecretBytes::new(vec![7u8; 32]);
    let mut group = c.benchmark_group("crypto_derive_file_key");
    group.bench_function("hmac_sha256", |b| {
        b.iter(|| {
            let k = derive_file_key(black_box(&master), black_box(b"seed-value"));
            black_box(k);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_seal, bench_open, bench_key_derivation);
criterion_main!(benches);
