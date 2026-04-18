#![allow(clippy::pedantic)]
//! Auth-vault open/close latency benchmark.
//!
//! Measures round-trip cost of opening the file-backed vault (decrypt +
//! parse) and writing a token (encrypt + fsync). Uses a temp directory so
//! no persistent state leaks between runs.
//!
//! Run with:
//!   cargo bench -p pcloud-daemon --bench vault_open_close

// **PLATFORM:** all
// **GATING:** none (portable).

// TODO(bd-1du.10 / audit-04 §10-L-10.1): This is a stub bench target.
// Full implementation requires:
//   1. A public `AuthVault::open_for_bench` constructor that accepts an
//      arbitrary root path (bypassing the production singleton check).
//   2. A `SecretString`-typed fixture token derived from a deterministic
//      test key so the benchmark is reproducible across machines.
//   3. Criterion warm-up over 100+ iterations to surface p50/p99 latency
//      on the critical path hit on every daemon startup.
//
// Tracking bead: pcloud-rs-s1p.113

use criterion::{Criterion, criterion_group, criterion_main};

fn vault_open_close_stub(_c: &mut Criterion) {
    // Placeholder: criterion group must be non-empty for `cargo bench` to
    // compile and link the bench binary without error.  Replace the body
    // with real benches once the AuthVault bench constructor is available.
    let _ = std::hint::black_box(42u64);
}

criterion_group!(benches, vault_open_close_stub);
criterion_main!(benches);
