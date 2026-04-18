#![allow(clippy::pedantic)]
//! FUSE writeback-flush latency benchmark.
//!
//! Measures the end-to-end latency of staging a write, flushing the page
//! cache, and finalising via the upload backend shim. Uses an in-memory
//! no-op backend so only the pcloud-fs state-machine overhead is measured.
//!
//! Run with:
//!   cargo bench -p pcloud-fs --bench writeback_flush

// **PLATFORM:** all
// **GATING:** none (portable at build time; FUSE kernel paths are excluded).

// TODO(bd-1du.4 / audit-04 §10-L-10.1): This is a stub bench target.
// Full implementation requires:
//   1. A public `WritePath::flush_for_bench` entry point that accepts a
//      pre-staged `Vec<u8>` payload and a no-op `FileUploadBackend`.
//   2. Criterion parametrisation over payload sizes (64 KiB, 1 MiB, 16 MiB)
//      to expose the staging→flush overhead at each granularity.
//   3. The `slo_hook::observe_flush` call must be preserved so the bench
//      measures realistic code paths including the observability shim.
//
// Tracking bead: pcloud-rs-s1p.113

use criterion::{Criterion, criterion_group, criterion_main};

fn writeback_flush_stub(_c: &mut Criterion) {
    // Placeholder: criterion group must be non-empty for `cargo bench` to
    // compile and link the bench binary without error.  Replace the body
    // with real benches once `WritePath::flush_for_bench` is available.
    let _ = std::hint::black_box(42u64);
}

criterion_group!(benches, writeback_flush_stub);
criterion_main!(benches);
