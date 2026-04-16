#![allow(clippy::pedantic)]
//! Constructs a `PageCache`, warms it with 16 pages, then replays a mixed
//! read workload that would hit and miss the cache. Reports the observed
//! hit/miss ratio so operators can tune page size and capacity.
//!
//! Run with: `cargo run -p pcloud-cache --example warm_cache`

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_cache::page_cache::PageCache;

fn main() {
    // 1 MiB capacity, 64 KiB page size — small enough to observe evictions
    // if the workload scales up, big enough to hold our 16 warm pages.
    let cache = PageCache::with_capacity(1024 * 1024, 64 * 1024);

    const WARM_PAGES: usize = 16;
    const PAGE_BYTES: usize = 4096;

    // Warm-up phase.
    for i in 0..WARM_PAGES {
        let key = format!("page:{i}");
        cache.put(key, vec![i as u8; PAGE_BYTES]);
    }
    println!(
        "warmed:  {} pages, used={} bytes, entry_count={}",
        WARM_PAGES,
        cache.used_bytes(),
        cache.entry_count()
    );

    // Replay phase: 32 reads, half of which target pages we warmed and half
    // of which target pages we never inserted, so we see both hits and misses.
    let mut hits = 0usize;
    let mut misses = 0usize;
    for i in 0..32 {
        let key = format!("page:{}", i % (WARM_PAGES * 2));
        if cache.get(&key).is_some() {
            hits += 1;
        } else {
            misses += 1;
        }
    }

    let total = hits + misses;
    let ratio = if total == 0 {
        0.0
    } else {
        (hits as f64) / (total as f64) * 100.0
    };
    println!("replay:  {total} reads, {hits} hits, {misses} misses ({ratio:.1}% hit rate)");
}
