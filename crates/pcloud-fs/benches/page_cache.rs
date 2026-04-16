#![allow(clippy::pedantic)]
//! Criterion micro-benchmarks for [`pcloud_fs::page_cache::PageCache`].
//!
//! Reviewer 07 asked for performance coverage of the FUSE page cache:
//!
//! * `sequential_read_cold_fill_hit` — miss-then-fill on fresh cache, then
//!   re-read (hit path).
//! * `random_read_1gib`             — 10 000 uniformly distributed page
//!   lookups across a 1 GiB logical range; measures hit/miss throughput.
//! * `eviction_pressure_256mib`     — fills a 128 MiB-capped cache with
//!   256 MiB of pages to force LRU eviction on every other insert.
//! * `concurrent_read_4_threads`    — four threads hammering the same
//!   page keys through `Arc<PageCache>` to measure Mutex contention.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::sync::Arc;
use std::thread;

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};

use pcloud_fs::page_cache::{DEFAULT_PAGE_SIZE, PageCache, PageCacheConfig, PageKey};

const MIB: usize = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

fn make_page(byte: u8, size: usize) -> Vec<u8> {
    vec![byte; size]
}

fn cfg_default_128mib() -> PageCacheConfig {
    PageCacheConfig {
        page_size: DEFAULT_PAGE_SIZE,
        max_bytes: 128 * MIB,
    }
}

/// Sequential cold-fill + hit. The first pass inserts, the second reads,
/// measuring the combined cold+warm path throughput per iteration.
fn bench_sequential_cold_fill_hit(c: &mut Criterion) {
    // 1 MiB working set — fits entirely, so the warm pass is pure hits.
    let pages = 16usize; // 16 * 64 KiB = 1 MiB
    let mut group = c.benchmark_group("page_cache/sequential");
    group.throughput(Throughput::Bytes((pages * DEFAULT_PAGE_SIZE) as u64));
    group.bench_function("cold_fill_then_hit", |b| {
        b.iter(|| {
            let cache = PageCache::new(cfg_default_128mib());
            // Cold fill.
            for i in 0..pages {
                let key = PageKey {
                    file_id: 1,
                    page_index: i as u64,
                };
                cache.put(key, make_page(i as u8, DEFAULT_PAGE_SIZE));
            }
            // Warm hit.
            for i in 0..pages {
                let key = PageKey {
                    file_id: 1,
                    page_index: i as u64,
                };
                black_box(cache.get(key));
            }
        })
    });
    group.finish();
}

/// Random reads over a 1 GiB logical range. The cache is pre-primed with a
/// subset so we see a realistic mix of hits and misses.
fn bench_random_read_1gib(c: &mut Criterion) {
    let page_size = DEFAULT_PAGE_SIZE as u64;
    let total_pages = GIB / page_size; // = 16384
    let resident_pages = 2048u64; // ~128 MiB worth

    let cache = PageCache::new(cfg_default_128mib());
    for i in 0..resident_pages {
        let key = PageKey {
            file_id: 1,
            page_index: i,
        };
        cache.put(key, make_page(i as u8, DEFAULT_PAGE_SIZE));
    }

    // Deterministic pseudo-random sequence — avoids pulling `rand`.
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let requests: Vec<u64> = (0..10_000u64)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % total_pages
        })
        .collect();

    let mut group = c.benchmark_group("page_cache/random");
    group.throughput(Throughput::Elements(requests.len() as u64));
    group.bench_function("10k_uniform_over_1gib", |b| {
        b.iter(|| {
            for &idx in &requests {
                let key = PageKey {
                    file_id: 1,
                    page_index: idx,
                };
                if cache.get(key).is_none() {
                    // On miss: synthesise a fill so subsequent hits improve.
                    cache.put(key, make_page(idx as u8, DEFAULT_PAGE_SIZE));
                }
                black_box(());
            }
        })
    });
    group.finish();
}

/// Eviction pressure: write twice the cache cap to force LRU churn.
fn bench_eviction_pressure_256mib(c: &mut Criterion) {
    let page_size = DEFAULT_PAGE_SIZE;
    let cap = 128 * MIB;
    let total_bytes = 256 * MIB;
    let total_pages = total_bytes / page_size;

    let mut group = c.benchmark_group("page_cache/eviction");
    group.throughput(Throughput::Bytes(total_bytes as u64));
    // Only small sample count — each iteration allocates 256 MiB worth of
    // page buffers and churns the LRU.
    group.sample_size(10);
    group.bench_function("fill_256mib_into_128mib_cap", |b| {
        b.iter(|| {
            let cache = PageCache::new(PageCacheConfig {
                page_size,
                max_bytes: cap,
            });
            for i in 0..total_pages {
                let key = PageKey {
                    file_id: 1,
                    page_index: i as u64,
                };
                cache.put(key, make_page(i as u8, page_size));
            }
            black_box(cache.stats());
        })
    });
    group.finish();
}

/// Concurrent reads from four threads against a shared cache with a fixed
/// working set — surfaces Mutex contention cost on the hot path.
fn bench_concurrent_read_4_threads(c: &mut Criterion) {
    const THREADS: usize = 4;
    const OPS_PER_THREAD: usize = 1_000;

    let cache = Arc::new(PageCache::new(cfg_default_128mib()));
    // Pre-fill a small hot set so all threads hit.
    for i in 0..32u64 {
        let key = PageKey {
            file_id: 1,
            page_index: i,
        };
        cache.put(key, make_page(i as u8, DEFAULT_PAGE_SIZE));
    }

    let mut group = c.benchmark_group("page_cache/concurrent");
    group.throughput(Throughput::Elements((THREADS * OPS_PER_THREAD) as u64));
    group.bench_function("4_threads_hot_set", |b| {
        b.iter(|| {
            let mut handles = Vec::with_capacity(THREADS);
            for tid in 0..THREADS {
                let c = Arc::clone(&cache);
                handles.push(thread::spawn(move || {
                    let mut seed = 0x1234_5678u64.wrapping_add(tid as u64);
                    for _ in 0..OPS_PER_THREAD {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        let key = PageKey {
                            file_id: 1,
                            page_index: seed % 32,
                        };
                        black_box(c.get(key));
                    }
                }));
            }
            for h in handles {
                h.join().unwrap();
            }
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_sequential_cold_fill_hit,
    bench_random_read_1gib,
    bench_eviction_pressure_256mib,
    bench_concurrent_read_4_threads,
);
criterion_main!(benches);
