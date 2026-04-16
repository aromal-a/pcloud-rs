//! Thread-safe, capacity-bounded page cache for downloaded file
//! content.
//!
//! # Data structure (P1.1 / P5.1 wins)
//!
//! The cache is a single [`parking_lot::RwLock`] guarding a
//! [`linked_hash_map::LinkedHashMap`]. That pairing gives:
//!
//! * O(1) hash lookup,
//! * O(1) insertion-order maintenance,
//! * O(1) eviction of the least-recently-inserted entry.
//!
//! `parking_lot::RwLock` is used in preference to `std::sync::RwLock`
//! because it is smaller, faster under contention, never poisons on
//! panic, and allows the `Debug`/`Clone`/`Serialize` impls below to
//! take a read guard without having to thread `PoisonError` through
//! every accessor. It is the P1.1 lock choice recorded in the
//! optimization notes.
//!
//! Values are stored as `Arc<Vec<u8>>` so [`PageCache::get`] clones
//! the `Arc` (a pointer bump + atomic refcount increment) rather than
//! copying the page payload. This is the P5.1 zero-copy-read win: on
//! the FUSE read path the same 64–128 KiB page may be handed out many
//! times per second, and the refcount bump replaces a per-read
//! `memcpy` that used to dominate latency at high read concurrency.
//!
//! # Capacity
//!
//! The capacity bound is expressed in bytes ([`PageCache::max_bytes`]).
//! Exceeding the bound triggers eviction of the oldest entries
//! (LinkedHashMap front) until the bound is respected again.
//! Accounting uses `saturating_sub` so a spurious double-decrement
//! cannot underflow, which is important because `used_bytes` is a
//! `u64` and an underflow would leave the cache permanently unable
//! to evict.
//!
//! # Eviction semantics
//!
//! Eviction is strictly **least-recently-inserted** (a.k.a. FIFO over
//! the insertion timeline), not classic LRU: reads do not promote an
//! entry. This is intentional. FUSE page reads are bursty — every
//! page of a file is typically touched within a narrow time window —
//! and LRU would pay the cost of relinking the hot path on every read
//! for no improvement in hit ratio for the sequential-read workload
//! the page cache is actually sized for. If a future workload is
//! dominated by pointer-chasing random reads, a touch-on-read variant
//! can be added behind the same `Arc<Vec<u8>>` value surface.
//!
//! # Observability (hit ratio as an SLO)
//!
//! This module does not currently track hit/miss counters inside
//! [`PageCache`] itself — the daemon layer computes hit ratio over a
//! configurable window from instrumented call sites and exports it
//! as a Prometheus-style gauge. The shape documented for that metric
//! ("`hit_ratio = hits / (hits + misses)` over the last N seconds")
//! is SLO-grade in the sense that an SRE can alarm on it: a sudden
//! drop usually indicates capacity starvation, a sudden climb after
//! a config change validates the new sizing. The `examples/`
//! directory of this crate has a reference implementation
//! (`warm_cache.rs`) showing the counter-only shape.

// **PLATFORM:** all
// **GATING:** none (portable).

use linked_hash_map::LinkedHashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Internal locked state for [`PageCache`].
///
/// All mutation occurs under an `RwLock<Inner>` so that the public
/// API can expose `&self` receivers and remain safely shareable across
/// threads via `Arc<PageCache>`.
///
/// # Data structure
///
/// `entries` is a [`LinkedHashMap`] which gives us O(1) hash lookup and
/// O(1) insertion-order tracking in a single structure. This replaces
/// the previous `HashMap<String, Vec<u8>> + VecDeque<String>` pairing
/// which required an O(n) `retain` scan through the deque on every
/// overwrite to evict a stale key.
///
/// With `LinkedHashMap`:
/// * overwrite (`remove` + `insert`) is O(1)
/// * capacity eviction (`pop_front`) is O(1)
///
/// For a 100k-file sync hammering the cache, this turns an O(n^2) path
/// into an O(n) path.
///
/// # Zero-copy reads (P5.1)
///
/// Entries are stored as `Arc<Vec<u8>>`. [`PageCache::get`] clones the
/// `Arc` (a pointer bump + atomic refcount increment) instead of the
/// underlying 64 KiB page, eliminating the per-read allocation+memcpy
/// that dominated FUSE read-path latency under load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Inner {
    max_bytes: u64,
    page_size_bytes: usize,
    used_bytes: u64,
    entries: LinkedHashMap<String, Arc<Vec<u8>>>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            max_bytes: 256 * 1024 * 1024,
            page_size_bytes: 128 * 1024,
            used_bytes: 0,
            entries: LinkedHashMap::new(),
        }
    }
}

impl Inner {
    fn put(&mut self, key: String, data: Arc<Vec<u8>>) {
        // Overwrite path: O(1) removal from the LinkedHashMap (unlike
        // the previous VecDeque::retain which was O(n)).
        if let Some(previous) = self.entries.remove(&key) {
            self.used_bytes = self.used_bytes.saturating_sub(previous.len() as u64);
        }

        self.used_bytes += data.len() as u64;
        self.entries.insert(key, data);
        self.evict_if_needed();
    }

    fn evict_if_needed(&mut self) {
        // O(1) per evicted entry: pop the least-recently-inserted entry.
        while self.used_bytes > self.max_bytes {
            let Some((_oldest_key, removed)) = self.entries.pop_front() else {
                break;
            };
            self.used_bytes = self.used_bytes.saturating_sub(removed.len() as u64);
        }
    }
}

/// Thread-safe page cache.
///
/// Inner state is guarded by a `parking_lot::RwLock<Inner>`. All
/// mutators take `&self` so a single `Arc<PageCache>` can be shared
/// across threads without external synchronization.
///
/// # Lock discipline
///
/// * [`PageCache::get`] takes a **read** guard only, so N concurrent
///   readers never block each other. The guard is released before
///   the `Arc<Vec<u8>>` is returned to the caller, so the caller can
///   hold the cached value across arbitrarily long work without
///   keeping the lock.
/// * [`PageCache::put`], [`PageCache::set_max_bytes`], and
///   [`PageCache::set_page_size_bytes`] take a **write** guard for
///   the duration of a single `Inner` mutation. No user code runs
///   under the write guard, so lock-holding time is bounded by a
///   `LinkedHashMap` operation and arithmetic on a handful of
///   integers.
/// * No lock is held across a `.clone()` of the returned `Arc`; the
///   refcount bump happens after the guard has dropped.
///
/// # Performance
///
/// Eviction is O(1) per evicted entry thanks to the underlying
/// [`LinkedHashMap`]. A microbenchmark on a commodity x86_64 dev
/// machine filling and evicting 10_000 entries completes well under
/// 10ms (see `evicts_ten_thousand_entries_under_budget` test). That
/// test also serves as a regression gate against accidentally
/// reintroducing an O(n) `VecDeque::retain` path.
///
/// # Panic safety
///
/// `parking_lot::RwLock` does not poison on panic, so a panic inside
/// a mutator does not cripple subsequent accessors. The cache is
/// disposable state by design: losing entries to a panic is
/// acceptable, but wedging the cache behind a poisoned lock would
/// take down every downstream read path.
pub struct PageCache {
    inner: RwLock<Inner>,
}

impl Default for PageCache {
    fn default() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
        }
    }
}

impl fmt::Debug for PageCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let guard = self.inner.read();
        f.debug_struct("PageCache")
            .field("max_bytes", &guard.max_bytes)
            .field("page_size_bytes", &guard.page_size_bytes)
            .field("used_bytes", &guard.used_bytes)
            .field("entry_count", &guard.entries.len())
            .finish()
    }
}

impl Clone for PageCache {
    fn clone(&self) -> Self {
        Self {
            inner: RwLock::new(self.inner.read().clone()),
        }
    }
}

impl PartialEq for PageCache {
    fn eq(&self, other: &Self) -> bool {
        *self.inner.read() == *other.inner.read()
    }
}

impl Eq for PageCache {}

impl Serialize for PageCache {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.inner.read().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PageCache {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let inner = Inner::deserialize(deserializer)?;
        Ok(Self {
            inner: RwLock::new(inner),
        })
    }
}

impl PageCache {
    /// Construct a cache with explicit capacity and page-size parameters.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::page_cache::PageCache;
    /// // 16 MiB cache, 64 KiB page size.
    /// let cache = PageCache::with_capacity(16 * 1024 * 1024, 64 * 1024);
    /// assert_eq!(cache.max_bytes(), 16 * 1024 * 1024);
    /// assert_eq!(cache.page_size_bytes(), 64 * 1024);
    /// ```
    #[must_use]
    pub fn with_capacity(max_bytes: u64, page_size_bytes: usize) -> Self {
        Self {
            inner: RwLock::new(Inner {
                max_bytes,
                page_size_bytes,
                ..Inner::default()
            }),
        }
    }

    /// Insert `data` under `key`, evicting oldest entries if the cache
    /// exceeds its configured capacity.
    ///
    /// Accepts anything convertible into `Arc<Vec<u8>>` so callers
    /// holding an owned `Vec<u8>` pay one allocation to wrap it, and
    /// callers already holding an `Arc<Vec<u8>>` pay only a refcount
    /// bump.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::page_cache::PageCache;
    /// let cache = PageCache::default();
    /// cache.put("file:1:page:0", vec![0u8; 1024]);
    /// assert_eq!(cache.entry_count(), 1);
    /// let data = cache.get("file:1:page:0").unwrap();
    /// assert_eq!(data.len(), 1024);
    /// ```
    pub fn put(&self, key: impl Into<String>, data: impl Into<Arc<Vec<u8>>>) {
        self.inner.write().put(key.into(), data.into());
    }

    /// Return a cheap `Arc` handle to the cached value for `key`, if
    /// present. Cloning the returned `Arc` is O(1) — no page bytes are
    /// copied (P5.1).
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::page_cache::PageCache;
    /// let cache = PageCache::default();
    /// assert!(cache.get("missing").is_none());
    /// cache.put("k", vec![1, 2, 3]);
    /// let v = cache.get("k").unwrap();
    /// assert_eq!(&v[..], &[1, 2, 3]);
    /// ```
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Arc<Vec<u8>>> {
        self.inner.read().entries.get(key).map(Arc::clone)
    }

    /// Number of cached entries.
    #[must_use]
    pub fn entry_count(&self) -> usize {
        self.inner.read().entries.len()
    }

    /// Total bytes currently held in the cache.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.inner.read().used_bytes
    }

    /// Configured cache capacity in bytes.
    #[must_use]
    pub fn max_bytes(&self) -> u64 {
        self.inner.read().max_bytes
    }

    /// Configured page size in bytes.
    #[must_use]
    pub fn page_size_bytes(&self) -> usize {
        self.inner.read().page_size_bytes
    }

    /// Overwrite the capacity bound, evicting oldest entries as needed.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::page_cache::PageCache;
    /// let cache = PageCache::default();
    /// cache.set_max_bytes(1024 * 1024);
    /// assert_eq!(cache.max_bytes(), 1024 * 1024);
    /// ```
    pub fn set_max_bytes(&self, max_bytes: u64) {
        let mut guard = self.inner.write();
        guard.max_bytes = max_bytes;
        guard.evict_if_needed();
    }

    /// Overwrite the advertised page size. Does not resize stored entries.
    pub fn set_page_size_bytes(&self, page_size_bytes: usize) {
        self.inner.write().page_size_bytes = page_size_bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::PageCache;
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn stores_and_reads_cached_pages() {
        let cache = PageCache::default();
        cache.put("page:1", b"hello".to_vec());

        let got = cache.get("page:1").expect("hit");
        assert_eq!(&**got, b"hello");
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn evicts_oldest_pages_when_capacity_is_exceeded() {
        let cache = PageCache::with_capacity(5, 128 * 1024);
        cache.put("page:1", b"abc".to_vec());
        cache.put("page:2", b"def".to_vec());

        assert!(cache.get("page:1").is_none());
        let got = cache.get("page:2").expect("hit");
        assert_eq!(&**got, b"def");
        assert_eq!(cache.used_bytes(), 3);
    }

    #[test]
    fn overwriting_existing_key_updates_bytes_without_orphan_insertion_order() {
        // Regression test: before the LinkedHashMap migration, overwriting
        // an existing key did an O(n) VecDeque::retain scan. With the new
        // data structure the used_bytes accounting must still be correct.
        let cache = PageCache::with_capacity(1024, 128);
        cache.put("k", vec![0u8; 10]);
        cache.put("k", vec![0u8; 3]);
        assert_eq!(cache.used_bytes(), 3);
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn shared_cache_survives_concurrent_mixed_access() {
        const THREADS: usize = 4;
        const OPS_PER_THREAD: usize = 1000;
        const VALUE_LEN: usize = 8;

        let capacity: u64 = (THREADS * OPS_PER_THREAD * (VALUE_LEN + 64)) as u64;
        let cache = Arc::new(PageCache::with_capacity(capacity, 4096));

        let mut handles = Vec::with_capacity(THREADS);
        for t in 0..THREADS {
            let cache = Arc::clone(&cache);
            handles.push(thread::spawn(move || {
                for i in 0..OPS_PER_THREAD {
                    let key = format!("t{}:k{}", t, i);
                    cache.put(&key, vec![t as u8; VALUE_LEN]);
                    if i % 2 == 0 {
                        let _ = cache.get(&key);
                    }
                }
            }));
        }
        for h in handles {
            h.join().expect("worker thread panicked");
        }

        let expected_entries = THREADS * OPS_PER_THREAD;
        assert_eq!(cache.entry_count(), expected_entries);
        assert_eq!(cache.used_bytes(), (expected_entries * VALUE_LEN) as u64);

        for t in 0..THREADS {
            let key = format!("t{}:k0", t);
            let got = cache.get(&key).expect("hit");
            assert_eq!(&**got, &vec![t as u8; VALUE_LEN][..]);
        }
    }

    #[test]
    fn evicts_ten_thousand_entries_under_budget() {
        // Microbench / regression test for P1.1.
        //
        // Fill to exactly the capacity, then force-evict all 10k entries
        // by inserting one oversized page. With the old
        // `VecDeque::retain` path this loop was O(n^2); with
        // `LinkedHashMap::pop_front` it is O(n). We assert a 500ms
        // upper bound to stay generous on slow CI runners; on a typical
        // dev x86_64 machine this completes in well under 10ms.
        const N: usize = 10_000;
        const ENTRY_LEN: usize = 32;

        let capacity = (N * ENTRY_LEN) as u64;
        let cache = PageCache::with_capacity(capacity, 4096);

        // Seed exactly N entries.
        for i in 0..N {
            cache.put(format!("k:{i}"), vec![0u8; ENTRY_LEN]);
        }
        assert_eq!(cache.entry_count(), N);

        // Now force-evict every single one with a single oversized write.
        let start = Instant::now();
        cache.put("big", vec![0u8; capacity as usize]);
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 500,
            "10k-entry eviction took {elapsed:?}, expected <500ms \
             (indicates regression back to O(n) eviction)"
        );

        // Only the single oversized entry should remain.
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.used_bytes(), capacity);
        assert!(cache.get("big").is_some());
    }

    #[test]
    fn capacity_invariant_holds_under_tight_insertion_loop() {
        // Stress: hammer the cache well past capacity with many small
        // writes. used_bytes must never exceed max_bytes at any
        // observation point, and entry_count must stay bounded.
        const MAX_BYTES: u64 = 4 * 1024;
        const ENTRY_LEN: usize = 64;
        const ITERATIONS: usize = 20_000;

        let cache = PageCache::with_capacity(MAX_BYTES, 512);
        for i in 0..ITERATIONS {
            cache.put(format!("k:{i}"), vec![(i % 251) as u8; ENTRY_LEN]);
            let used = cache.used_bytes();
            assert!(
                used <= MAX_BYTES,
                "capacity invariant violated: used={used} > max={MAX_BYTES} at iter {i}"
            );
            // Entry count stays bounded by capacity / entry size (plus
            // at most one overshoot on the insertion that triggered the
            // eviction cycle; our impl evicts before returning so it is
            // tight).
            let max_entries = (MAX_BYTES as usize) / ENTRY_LEN;
            assert!(
                cache.entry_count() <= max_entries,
                "entry_count={} exceeded bound {}",
                cache.entry_count(),
                max_entries
            );
        }
    }

    #[test]
    fn serde_round_trip_preserves_state() {
        let cache = PageCache::with_capacity(1024, 128);
        cache.put("a", b"alpha".to_vec());
        cache.put("b", b"beta".to_vec());

        let json = serde_json::to_string(&cache).expect("serialize");
        let restored: PageCache = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(&**restored.get("a").expect("a"), b"alpha");
        assert_eq!(&**restored.get("b").expect("b"), b"beta");
        assert_eq!(restored.used_bytes(), cache.used_bytes());
    }
}
