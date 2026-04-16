#![allow(clippy::pedantic)]
//! SQLite-backed KV throughput benchmarks.
//!
//! Bootstraps a warm WAL-mode SQLite profile once, then benchmarks the
//! `value_kv::{get,set}_*` and `settings_kv::{get,set}_*` helpers that
//! mirror the C `psync_{get,set}_*_value` / `_setting` families.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::path::PathBuf;

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use pcloud_store::{StoreHandle, bootstrap_profile, settings_kv, value_kv};

fn temp_db(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pcloud-store-bench-{}-{}.sqlite3",
        std::process::id(),
        name
    ))
}

fn bench_value_kv(c: &mut Criterion) {
    let path = temp_db("value-kv");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    let mut group = c.benchmark_group("store_value_kv");
    value_kv::set_uint(&path, "bench_uint", 42).expect("seed");
    value_kv::set_string(&path, "bench_string", "hello world").expect("seed");

    group.bench_function("set_uint", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i = i.wrapping_add(1);
            value_kv::set_uint(&path, "bench_uint", black_box(i)).expect("set");
        });
    });

    group.bench_function("get_uint_warm", |b| {
        b.iter(|| {
            let v = value_kv::get_uint(&path, black_box("bench_uint")).expect("get");
            black_box(v);
        });
    });

    group.bench_function("set_string", |b| {
        b.iter(|| {
            value_kv::set_string(&path, "bench_string", black_box("hello world")).expect("set");
        });
    });

    group.bench_function("get_string_warm", |b| {
        b.iter(|| {
            let v = value_kv::get_string(&path, black_box("bench_string")).expect("get");
            black_box(v);
        });
    });

    group.finish();
    let _ = std::fs::remove_file(&path);
}

fn bench_settings_kv(c: &mut Criterion) {
    let path = temp_db("settings-kv");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");

    let mut group = c.benchmark_group("store_settings_kv");
    settings_kv::set_int(&path, "bench_int", 7).expect("seed");
    settings_kv::set_bool(&path, "bench_bool", true).expect("seed");

    group.bench_function("set_int", |b| {
        let mut i = 0i64;
        b.iter(|| {
            i = i.wrapping_add(1);
            settings_kv::set_int(&path, "bench_int", black_box(i)).expect("set");
        });
    });

    group.bench_function("get_int_warm", |b| {
        b.iter(|| {
            let v = settings_kv::get_int(&path, black_box("bench_int")).expect("get");
            black_box(v);
        });
    });

    group.bench_function("get_bool_warm", |b| {
        b.iter(|| {
            let v = settings_kv::get_bool(&path, black_box("bench_bool")).expect("get");
            black_box(v);
        });
    });

    group.finish();
    let _ = std::fs::remove_file(&path);
}

fn bench_pooled_handle(c: &mut Criterion) {
    let path = temp_db("pooled-handle");
    let _ = std::fs::remove_file(&path);
    let _ = bootstrap_profile(&path).expect("bootstrap");
    let handle = StoreHandle::open(&path).expect("pooled handle open");

    let mut group = c.benchmark_group("store_pooled_handle");

    handle
        .value_kv()
        .set_uint("bench_uint", 42)
        .expect("seed uint");
    handle
        .value_kv()
        .set_string("bench_string", "hello world")
        .expect("seed string");
    handle
        .settings_kv()
        .set_int("bench_int", 1)
        .expect("seed int");

    group.bench_function("value_set_uint", |b| {
        let mut i = 0u64;
        b.iter(|| {
            i = i.wrapping_add(1);
            handle
                .value_kv()
                .set_uint("bench_uint", black_box(i))
                .expect("set");
        });
    });

    group.bench_function("value_get_uint", |b| {
        b.iter(|| {
            let v = handle
                .value_kv()
                .get_uint(black_box("bench_uint"))
                .expect("get");
            black_box(v);
        });
    });

    group.bench_function("value_set_string", |b| {
        b.iter(|| {
            handle
                .value_kv()
                .set_string("bench_string", black_box("hello world"))
                .expect("set");
        });
    });

    group.bench_function("settings_set_int", |b| {
        let mut i = 0i64;
        b.iter(|| {
            i = i.wrapping_add(1);
            handle
                .settings_kv()
                .set_int("bench_int", black_box(i))
                .expect("set");
        });
    });

    group.bench_function("settings_get_int", |b| {
        b.iter(|| {
            let v = handle
                .settings_kv()
                .get_int(black_box("bench_int"))
                .expect("get");
            black_box(v);
        });
    });

    group.finish();
    let _ = std::fs::remove_file(&path);
}

criterion_group!(
    benches,
    bench_value_kv,
    bench_settings_kv,
    bench_pooled_handle
);
criterion_main!(benches);
