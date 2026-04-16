#![allow(clippy::pedantic)]
//! Constant-time equality micro-benchmarks for `SecretString`.
//!
//! Measures timing for three classes of comparisons against a reference
//! secret of a fixed length:
//!   1. fully equal
//!   2. differs only at the final byte ("late mismatch")
//!   3. differs at the first byte ("early mismatch")
//!
//! The expectation is that all three distributions have indistinguishable
//! means (within noise) and a small coefficient of variation. A
//! byte-at-a-time `==` would show a strong mean gap between "early" and
//! "late" mismatches; `SecretString::eq` goes through `subtle::ConstantTimeEq`
//! so it should not.
//!
//! The raw per-iteration samples are stored under `target/criterion/...`;
//! the baseline markdown summarises mean + CV across the three classes.

// **PLATFORM:** all
// **GATING:** none (portable).

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use pcloud_secret::secret_string::SecretString;

fn secret_ct_eq(c: &mut Criterion) {
    let mut group = c.benchmark_group("secret_string_ct_eq");
    // 256-byte secret: large enough that a naive byte-at-a-time comparator
    // would show a measurable early-vs-late gap.
    let base: String = "A".repeat(256);
    let a = SecretString::new(base.clone());

    // Equal.
    let b_eq = SecretString::new(base.clone());
    // Late mismatch: flip the last byte.
    let mut late = base.clone().into_bytes();
    let last = late.len() - 1;
    late[last] ^= 0x01;
    let b_late = SecretString::new(String::from_utf8(late).unwrap());
    // Early mismatch: flip the first byte.
    let mut early = base.clone().into_bytes();
    early[0] ^= 0x01;
    let b_early = SecretString::new(String::from_utf8(early).unwrap());

    group.bench_function("eq_equal_256b", |bencher| {
        bencher.iter(|| {
            let r = black_box(&a) == black_box(&b_eq);
            black_box(r);
        });
    });

    group.bench_function("eq_late_mismatch_256b", |bencher| {
        bencher.iter(|| {
            let r = black_box(&a) == black_box(&b_late);
            black_box(r);
        });
    });

    group.bench_function("eq_early_mismatch_256b", |bencher| {
        bencher.iter(|| {
            let r = black_box(&a) == black_box(&b_early);
            black_box(r);
        });
    });

    group.finish();
}

criterion_group!(benches, secret_ct_eq);
criterion_main!(benches);
