// **PLATFORM:** all
// **GATING:** none (portable).

//! Key-typed generic LRU page cache. Canonical string-keyed page cache
//! for the workspace as of CLAUDEREV deferred-set D1.3 (fire 43).
//!
//! This module hosts `PageCacheGeneric<K>` plus the supporting
//! `PageCacheConfig` / `PageCacheStats` types. `pcloud_fs::page_cache`
//! re-exports them so existing
//! `use pcloud_fs::page_cache::PageCacheConfig` imports resolve
//! through the re-export chain. `pcloud_cache::CacheShell.pages` and
//! `pcloud_fs::read_path::ReadPathService.pages` both use
//! `PageCacheGeneric<String>` directly — the legacy
//! `pcloud_cache::page_cache::PageCache` was deleted in fire 43 once
//! both production callers had migrated.
//!
//! ## Why the body lives in `pcloud-cache`
//!
//! `pcloud-fs → pcloud-cache` is the existing dep direction; placing
//! the generic in the lower-level crate avoids the cyclic dep that
//! would otherwise arise.
//!
//! ## Remaining unification work
//!
//! `pcloud_fs::page_cache::PageCache` is the typed
//! `PageKey = (file_id, page_index)` variant carrying a secondary
//! `by_file` index for O(k) per-file invalidation. **D1.2** will lift
//! that index into the generic via a `CacheKey` trait with an
//! optional `group()` method, after which the typed variant becomes
//! `PageCacheGeneric<PageKey>` and the workspace has a single
//! canonical cache primitive.
//!
//! See `CLAUDEREV/DEFERRED-PLAN.md` D1 for the full unification plan.

use std::sync::{Arc, Mutex};

use lru::LruCache;
use serde::{Deserialize, Serialize};

/// Default FUSE-aligned page size. Matches the 64 KiB page size used by
/// the reference C client's block cache. Re-exported by
/// `pcloud_fs::page_cache::DEFAULT_PAGE_SIZE`.
pub const DEFAULT_PAGE_SIZE: usize = 64 * 1024;

/// Default cache cap: 128 MiB. Re-exported by
/// `pcloud_fs::page_cache::DEFAULT_MAX_BYTES`.
pub const DEFAULT_MAX_BYTES: usize = 128 * 1024 * 1024;

/// Runtime configuration shared by every `PageCacheGeneric<K>` and by
/// `pcloud_fs::page_cache::PageCache` (which re-exports this struct).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCacheConfig {
    /// Size of each cached page in bytes. Reads are aligned to multiples
    /// of this value; a trailing short page may exist at EOF.
    pub page_size: usize,
    /// Upper bound on total resident bytes. LRU eviction keeps resident
    /// bytes at or below this value.
    pub max_bytes: usize,
}

impl Default for PageCacheConfig {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// Observed cache statistics. Re-exported by
/// `pcloud_fs::page_cache::PageCacheStats`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCacheStats {
    /// Number of `get` calls that served a cached page.
    pub hits: u64,
    /// Number of `get` calls that found no cached page.
    pub misses: u64,
    /// Total bytes currently resident in the cache.
    pub bytes_resident: usize,
    /// Total number of pages currently resident in the cache.
    pub pages_resident: usize,
    /// Total bytes rejected by `put` because a single page exceeded
    /// `max_bytes` and would immediately self-evict.
    pub bytes_rejected_oversized: u64,
}

impl PageCacheStats {
    /// Lifetime hit ratio computed from `hits / (hits + misses)`. Returns
    /// `0.0` when no reads have been observed.
    #[must_use]
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits.saturating_add(self.misses);
        if total == 0 {
            return 0.0;
        }
        self.hits as f64 / total as f64
    }
}

/// Cached page entry. Holds the page bytes behind an [`Arc`] so a `get`
/// returns a cheap refcount bump instead of a full memcpy.
#[derive(Debug, Clone)]
struct Slot {
    bytes: Arc<Vec<u8>>,
}

// ── CacheKey trait (CLAUDEREV deferred-set D1.2, fire 44) ───────────────────
//
// `CacheKey` lifts the `pcloud_fs::page_cache::PageCache`'s typed-`PageKey`
// `by_file` secondary index into the generic. A key implements
// `CacheKey` by declaring a `Group` type and returning either
// `Some(group)` (the entry belongs to a group that supports O(k)
// invalidation) or `None` (no grouping; `invalidate_group` is a no-op
// for entries with this key).
//
// `String` (the `pcloud_cache::CacheShell.pages` / `read_path.rs` key)
// has no useful grouping and impls `Group = ()`, `group() -> None`.
//
// `pcloud_fs::page_cache::PageKey` (the FUSE adapter's key) impls
// `Group = u64`, `group() -> Some(self.file_id)` so per-file
// invalidation runs in O(k) instead of O(n).

/// Optional grouping discriminant for cache keys.
///
/// Implementing types declare a `Group` associated type and a
/// `group()` method that returns either `Some(group)` (the entry
/// participates in O(k) per-group invalidation) or `None` (the entry
/// is ungrouped). The cache maintains a secondary index from `Group`
/// to the set of resident keys whenever `group()` returns `Some`.
pub trait CacheKey: std::hash::Hash + Eq + Clone + std::fmt::Debug {
    /// Discriminant used to bucket entries for O(k) per-group
    /// invalidation. `()` is the conventional "no grouping" choice.
    type Group: std::hash::Hash + Eq + Clone + std::fmt::Debug;
    /// Return the group this key participates in, or `None` if the
    /// key is ungrouped. The returned value is stored in the cache's
    /// secondary index and can be passed to
    /// [`PageCacheGeneric::invalidate_group`].
    fn group(&self) -> Option<Self::Group>;
}

/// String keys do not participate in per-group invalidation. The
/// secondary index stays empty for `PageCacheGeneric<String>`, so the
/// per-key cost is one `Option::None` allocation-free check on `put`.
impl CacheKey for String {
    type Group = ();
    fn group(&self) -> Option<()> {
        None
    }
}

#[derive(Debug)]
struct InnerGeneric<K>
where
    K: CacheKey,
{
    config: PageCacheConfig,
    entries: LruCache<K, Slot>,
    /// Secondary index: `K::Group → { K }` maintained in lockstep with
    /// `entries` whenever `K::group(&k)` returns `Some(g)`. Lets
    /// `invalidate_group(g)` run in O(k) where `k` is the resident key
    /// count for group `g`, instead of O(n) over the whole LRU.
    /// Stays empty for ungrouped keys (e.g. `K = String` whose
    /// `group()` always returns `None`).
    by_group: std::collections::HashMap<K::Group, std::collections::HashSet<K>>,
    bytes_resident: usize,
    hits: u64,
    misses: u64,
    bytes_rejected_oversized: u64,
}

impl<K> InnerGeneric<K>
where
    K: CacheKey,
{
    fn new(config: PageCacheConfig) -> Self {
        Self {
            config,
            entries: LruCache::unbounded(),
            by_group: std::collections::HashMap::new(),
            bytes_resident: 0,
            hits: 0,
            misses: 0,
            bytes_rejected_oversized: 0,
        }
    }

    fn evict_until_fits(&mut self, incoming_bytes: usize) {
        while self
            .bytes_resident
            .saturating_add(incoming_bytes)
            .saturating_sub(self.config.max_bytes)
            > 0
        {
            let Some((evicted_key, slot)) = self.entries.pop_lru() else {
                break;
            };
            self.bytes_resident = self.bytes_resident.saturating_sub(slot.bytes.len());
            // Keep `by_group` in sync with the LRU.
            if let Some(group) = evicted_key.group()
                && let Some(set) = self.by_group.get_mut(&group)
            {
                set.remove(&evicted_key);
                if set.is_empty() {
                    self.by_group.remove(&group);
                }
            }
        }
    }
}

/// LRU page cache parameterised on the key type.
#[derive(Debug)]
pub struct PageCacheGeneric<K>
where
    K: CacheKey,
{
    inner: Mutex<InnerGeneric<K>>,
}

impl<K> Default for PageCacheGeneric<K>
where
    K: CacheKey,
{
    fn default() -> Self {
        Self::new(PageCacheConfig::default())
    }
}

// ── Clone / PartialEq / Eq impls (D1.1b.2a, fire 39) ────────────────────────
//
// Required by downstream callers that already wrap the cache in
// `#[derive(Clone, PartialEq, Eq)]` types — `pcloud_fs::read_path::ReadPathService`,
// `pcloud_cache::CacheShell`, and any future migrator. The legacy
// `pcloud_cache::page_cache::PageCache` carries these impls; the generic
// must too before D1.1b.2c (caller migration) can land.
//
// Semantics:
//
// * `Clone` — deep-copies entry data, hits/misses counters, and
//   `bytes_rejected_oversized`. The cloned cache is independent: a `put`
//   on the clone does not propagate to the original.
// * `PartialEq` / `Eq` — content-equal iff (a) configs match, (b) both
//   have the same set of `(key, value)` pairs (LRU order is **not**
//   compared because it is operational state, not logical state), and
//   (c) the byte counters match. Stats counters are intentionally
//   excluded from equality so a cache that has served reads compares
//   equal to a cache that has not, given the same stored entries.

impl<K> Clone for PageCacheGeneric<K>
where
    K: CacheKey,
{
    fn clone(&self) -> Self {
        let Ok(inner) = self.inner.lock() else {
            // Mutex poisoned: return a fresh empty cache rather than
            // propagating the panic. The page cache is disposable
            // state by design.
            return Self::new(PageCacheConfig::default());
        };
        let mut new_lru: LruCache<K, Slot> = LruCache::unbounded();
        let mut new_by_group: std::collections::HashMap<K::Group, std::collections::HashSet<K>> =
            std::collections::HashMap::new();
        // `LruCache::iter()` walks MRU → LRU. Inserting in iteration
        // order with `put` rebuilds the same MRU ordering at the
        // destination.
        let pairs: Vec<(K, Slot)> = inner
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Re-insert in reverse so the eldest entry ends up at the LRU
        // position and the most-recent at MRU — matching the source.
        for (k, v) in pairs.into_iter().rev() {
            if let Some(group) = k.group() {
                new_by_group.entry(group).or_default().insert(k.clone());
            }
            new_lru.put(k, v);
        }
        let new_inner = InnerGeneric {
            config: inner.config,
            entries: new_lru,
            by_group: new_by_group,
            bytes_resident: inner.bytes_resident,
            hits: inner.hits,
            misses: inner.misses,
            bytes_rejected_oversized: inner.bytes_rejected_oversized,
        };
        Self {
            inner: Mutex::new(new_inner),
        }
    }
}

impl<K> PartialEq for PageCacheGeneric<K>
where
    K: CacheKey,
{
    fn eq(&self, other: &Self) -> bool {
        // Lock both — lower address first to avoid hold-and-wait
        // ordering quirks if the same cache is compared with itself.
        let (lhs, rhs) = if std::ptr::eq(self, other) {
            return true;
        } else if (self as *const _ as usize) < (other as *const _ as usize) {
            (self.inner.lock(), other.inner.lock())
        } else {
            (other.inner.lock(), self.inner.lock())
        };
        let (Ok(a), Ok(b)) = (lhs, rhs) else {
            return false;
        };
        if a.config != b.config {
            return false;
        }
        if a.bytes_resident != b.bytes_resident {
            return false;
        }
        if a.entries.len() != b.entries.len() {
            return false;
        }
        // Compare entry sets independent of LRU order. `iter()` walks
        // both in MRU→LRU; we collect into a HashMap-like compare.
        let a_set: std::collections::HashMap<&K, &Arc<Vec<u8>>> =
            a.entries.iter().map(|(k, v)| (k, &v.bytes)).collect();
        let b_set: std::collections::HashMap<&K, &Arc<Vec<u8>>> =
            b.entries.iter().map(|(k, v)| (k, &v.bytes)).collect();
        if a_set.len() != b_set.len() {
            return false;
        }
        for (k, v) in &a_set {
            match b_set.get(k) {
                Some(other_v) if other_v.as_slice() == v.as_slice() => {}
                _ => return false,
            }
        }
        true
    }
}

impl<K> Eq for PageCacheGeneric<K> where K: CacheKey {}

// ── Serde Serialize / Deserialize (D1.1b.2b, fire 40) ───────────────────────
//
// `lru::LruCache` does not ship serde impls, so we hand-roll a wire
// shape that captures everything observable through the public API:
//   - the `PageCacheConfig`,
//   - every `(key, bytes)` pair in **MRU → LRU** order so a
//     deserialize round-trip preserves the same eviction priority,
//   - the lifetime stats counters (hits, misses, bytes_rejected_oversized).
//
// `bytes_resident` and `pages_resident` are derivable from the entry
// list and so are not transmitted on the wire — `Deserialize` rebuilds
// them from the entries it sees, which guarantees the post-deserialize
// view is internally consistent even if the wire bytes were tampered
// with.
//
// Required for D1.1b.2c: `pcloud_fs::read_path::ReadPathService`
// derives `Serialize + Deserialize` and holds a `PageCache` field.
// Once D1.1b.2c migrates that field to `PageCacheGeneric<String>`,
// the derive must continue to work — these impls satisfy that bound.

#[derive(Serialize, Deserialize)]
struct PageCacheGenericWire<K> {
    config: PageCacheConfig,
    /// MRU → LRU ordered. Bytes are wrapped in `Arc` only inside the
    /// in-memory cache; the wire shape stores them as raw `Vec<u8>` so
    /// no `serde rc` feature interaction is required at this layer.
    entries: Vec<(K, Vec<u8>)>,
    hits: u64,
    misses: u64,
    bytes_rejected_oversized: u64,
}

impl<K> Serialize for PageCacheGeneric<K>
where
    K: CacheKey + Serialize,
{
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let inner_guard = self
            .inner
            .lock()
            .map_err(|_| serde::ser::Error::custom("page cache mutex poisoned"))?;
        // `LruCache::iter()` walks MRU → LRU; we serialize in that
        // order so the deserialize side can rebuild MRU positions by
        // re-inserting in reverse (matching the `Clone` impl).
        let entries: Vec<(K, Vec<u8>)> = inner_guard
            .entries
            .iter()
            .map(|(k, slot)| (k.clone(), (*slot.bytes).clone()))
            .collect();
        let wire = PageCacheGenericWire::<K> {
            config: inner_guard.config,
            entries,
            hits: inner_guard.hits,
            misses: inner_guard.misses,
            bytes_rejected_oversized: inner_guard.bytes_rejected_oversized,
        };
        wire.serialize(serializer)
    }
}

impl<'de, K> Deserialize<'de> for PageCacheGeneric<K>
where
    K: CacheKey + Deserialize<'de>,
{
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PageCacheGenericWire::<K>::deserialize(deserializer)?;
        let cache = Self::new(wire.config);
        // Re-insert in reverse so the eldest entry ends at the LRU
        // position and the most-recent at MRU — same logic as `Clone`.
        // Use the public `put` so byte accounting + eviction-on-quota
        // run; pre-deserialize tampering with bytes_resident is
        // therefore self-corrected.
        for (k, v) in wire.entries.into_iter().rev() {
            cache.put(k, v);
        }
        // After re-population the stats counters were `Default`. Lift
        // the persisted hits/misses/rejected back into Inner so a
        // round-trip preserves observability.
        if let Ok(mut inner) = cache.inner.lock() {
            inner.hits = wire.hits;
            inner.misses = wire.misses;
            inner.bytes_rejected_oversized = wire.bytes_rejected_oversized;
        }
        Ok(cache)
    }
}

impl<K> PageCacheGeneric<K>
where
    K: CacheKey,
{
    /// Construct a cache. Zero-valued `page_size` is replaced with
    /// [`DEFAULT_PAGE_SIZE`]; `max_bytes` is floored so at least one
    /// page always fits.
    #[must_use]
    pub fn new(mut config: PageCacheConfig) -> Self {
        if config.page_size == 0 {
            config.page_size = DEFAULT_PAGE_SIZE;
        }
        if config.max_bytes < config.page_size {
            config.max_bytes = config.page_size;
        }
        Self {
            inner: Mutex::new(InnerGeneric::new(config)),
        }
    }

    /// Active configuration (after normalisation by [`Self::new`]).
    #[must_use]
    pub fn config(&self) -> PageCacheConfig {
        self.inner.lock().map(|g| g.config).unwrap_or_default()
    }

    /// Lookup. On hit promotes to MRU; returns the page bytes via a
    /// cheap [`Arc`] clone.
    ///
    /// Accepts any `&Q` where `K: Borrow<Q>`, mirroring the
    /// `HashMap::get` / `LruCache::get` ergonomic. For
    /// `PageCacheGeneric<String>` this means callers can pass `&str`
    /// directly (no `String` allocation needed for the lookup); for
    /// typed-key shapes the call site continues to write `&K`.
    pub fn get<Q>(&self, key: &Q) -> Option<Arc<Vec<u8>>>
    where
        K: std::borrow::Borrow<Q>,
        Q: std::hash::Hash + Eq + ?Sized,
    {
        let mut inner = self.inner.lock().ok()?;
        if let Some(slot) = inner.entries.get(key) {
            let bytes = Arc::clone(&slot.bytes);
            inner.hits = inner.hits.saturating_add(1);
            return Some(bytes);
        }
        inner.misses = inner.misses.saturating_add(1);
        None
    }

    /// Insert or replace a page. Pages larger than `max_bytes` are
    /// silently dropped and counted in `bytes_rejected_oversized`.
    pub fn put(&self, key: K, bytes: Vec<u8>) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let new_len = bytes.len();
        if new_len > inner.config.max_bytes {
            inner.bytes_rejected_oversized = inner
                .bytes_rejected_oversized
                .saturating_add(new_len as u64);
            return;
        }
        if let Some(old) = inner.entries.pop(&key) {
            inner.bytes_resident = inner.bytes_resident.saturating_sub(old.bytes.len());
            // The replacement below re-inserts the same key; we leave
            // the secondary-index entry in place rather than removing
            // and re-inserting it (no-op).
        }
        inner.evict_until_fits(new_len);
        // Maintain the by_group secondary index in lockstep with the
        // LRU. Cloning the key is required because LruCache::put takes
        // ownership; the clone bound is part of CacheKey.
        if let Some(group) = key.group() {
            inner.by_group.entry(group).or_default().insert(key.clone());
        }
        inner.entries.put(
            key,
            Slot {
                bytes: Arc::new(bytes),
            },
        );
        inner.bytes_resident = inner.bytes_resident.saturating_add(new_len);
    }

    /// Drop every page belonging to `group`. Returns the number of
    /// pages evicted. O(k) where k is the resident page count for
    /// `group`, not O(n) over the whole cache.
    ///
    /// For ungrouped keys (e.g. `PageCacheGeneric<String>` whose
    /// `String::group()` always returns `None`) this method has no
    /// effect — the secondary index stays empty so `invalidate_group`
    /// of any value is a no-op.
    pub fn invalidate_group(&self, group: &K::Group) -> usize {
        let Ok(mut inner) = self.inner.lock() else {
            return 0;
        };
        let Some(victims) = inner.by_group.remove(group) else {
            return 0;
        };
        let mut evicted = 0;
        for key in victims {
            if let Some(slot) = inner.entries.pop(&key) {
                inner.bytes_resident = inner.bytes_resident.saturating_sub(slot.bytes.len());
                evicted += 1;
            }
        }
        evicted
    }

    /// Clear the entire cache. Does not reset hit/miss counters.
    pub fn clear(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.entries.clear();
        inner.by_group.clear();
        inner.bytes_resident = 0;
    }

    /// Snapshot the current cache statistics.
    #[must_use]
    pub fn stats(&self) -> PageCacheStats {
        let Ok(inner) = self.inner.lock() else {
            return PageCacheStats::default();
        };
        PageCacheStats {
            hits: inner.hits,
            misses: inner.misses,
            bytes_resident: inner.bytes_resident,
            pages_resident: inner.entries.len(),
            bytes_rejected_oversized: inner.bytes_rejected_oversized,
        }
    }

    /// Lifetime hit ratio. `0.0` when no reads have been observed.
    #[must_use]
    pub fn hit_ratio(&self) -> f64 {
        self.stats().hit_ratio()
    }

    /// Number of pages currently resident.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.entries.len()).unwrap_or(0)
    }

    /// Whether no pages are resident.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(max_bytes: usize, page_size: usize) -> PageCacheConfig {
        PageCacheConfig {
            page_size,
            max_bytes,
        }
    }

    #[test]
    fn round_trips_value() {
        let c: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(1024, 64));
        c.put("hello".to_owned(), vec![1, 2, 3]);
        let v = c.get(&"hello".to_owned()).expect("hit");
        assert_eq!(v.as_slice(), &[1, 2, 3]);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn evicts_under_byte_quota() {
        let c: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(128, 64));
        c.put("a".to_owned(), vec![0u8; 64]);
        c.put("b".to_owned(), vec![0u8; 64]);
        c.put("c".to_owned(), vec![0u8; 64]);
        assert!(c.len() <= 2, "len={} should be <= 2", c.len());
        assert!(c.stats().bytes_resident <= 128);
    }

    #[test]
    fn records_oversized_rejection() {
        let c: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(64, 64));
        c.put("oversized".to_owned(), vec![0u8; 200]);
        let stats = c.stats();
        assert_eq!(stats.pages_resident, 0);
        assert!(stats.bytes_rejected_oversized >= 200);
    }

    #[test]
    fn clone_produces_independent_content_equal_cache() {
        let a: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(1024, 64));
        a.put("k1".to_owned(), vec![1, 2, 3]);
        a.put("k2".to_owned(), vec![4, 5, 6]);
        let b = a.clone();
        assert_eq!(a, b, "clone must be content-equal");
        // Clone must be independent: mutating `a` does not show in `b`.
        a.put("k3".to_owned(), vec![7, 8, 9]);
        assert_ne!(a, b, "post-mutation a and b must differ");
        assert_eq!(b.len(), 2, "clone retains its original entries");
    }

    #[test]
    fn equality_excludes_stats_counters() {
        // Two caches with identical entries but different read history
        // should still compare equal — stats are operational, not
        // logical, state.
        let a: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(1024, 64));
        let b: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(1024, 64));
        a.put("x".to_owned(), vec![0, 0, 0]);
        b.put("x".to_owned(), vec![0, 0, 0]);
        // Drive `a`'s hit counter; do not touch `b`.
        let _ = a.get(&"x".to_owned());
        let _ = a.get(&"x".to_owned());
        assert!(a.stats().hits >= 2);
        assert_eq!(b.stats().hits, 0);
        assert_eq!(a, b, "equality must ignore hits/misses");
    }

    #[test]
    fn serde_round_trip_preserves_entries_and_stats() {
        let a: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(1024, 64));
        a.put("k1".to_owned(), vec![1, 2, 3]);
        a.put("k2".to_owned(), vec![4, 5, 6]);
        // Drive the hit counter so stats persistence is exercised.
        let _ = a.get(&"k1".to_owned());
        let _ = a.get(&"k1".to_owned());
        let _ = a.get(&"missing".to_owned());

        // Round-trip via JSON.
        let json = serde_json::to_string(&a).expect("serialize");
        let b: PageCacheGeneric<String> = serde_json::from_str(&json).expect("deserialize");

        // Content equality (Eq impl excludes stats; we check stats below).
        assert_eq!(a, b, "round-trip should be content-equal");

        // Stats counters were carried by the wire shape and restored
        // post-deserialize.
        let sa = a.stats();
        let sb = b.stats();
        assert_eq!(sa.hits, sb.hits, "hits preserved across round-trip");
        assert_eq!(sa.misses, sb.misses, "misses preserved across round-trip");
        assert_eq!(
            sa.bytes_rejected_oversized, sb.bytes_rejected_oversized,
            "rejection counter preserved"
        );
    }

    #[test]
    fn serde_round_trip_preserves_mru_ordering() {
        // After a round-trip, the next eviction must drop the same
        // entry the original would have dropped — i.e. MRU/LRU
        // positions survive the wire format.
        let a: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(192, 64));
        a.put("oldest".to_owned(), vec![0u8; 64]);
        a.put("middle".to_owned(), vec![0u8; 64]);
        a.put("newest".to_owned(), vec![0u8; 64]);
        // 192-byte cap exactly fits 3 entries; force eviction by
        // putting a 4th. The "oldest" is the LRU and should be dropped.
        let json = serde_json::to_string(&a).expect("serialize");
        let b: PageCacheGeneric<String> = serde_json::from_str(&json).expect("deserialize");
        b.put("trigger_eviction".to_owned(), vec![0u8; 64]);
        // After eviction the oldest entry must be gone, the newer two
        // must still be present.
        assert!(
            b.get(&"oldest".to_owned()).is_none(),
            "round-trip + eviction must drop the LRU entry"
        );
        assert!(b.get(&"middle".to_owned()).is_some());
        assert!(b.get(&"newest".to_owned()).is_some());
    }

    #[test]
    fn typed_key_struct_works_too() {
        // Smoke test that the generic accepts a CacheKey-implementing
        // typed-key struct, anticipating the D1.4 migration of
        // `pcloud_fs::page_cache::PageCache` to `PageCacheGeneric<PageKey>`.
        #[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
        struct TestKey {
            a: u64,
            b: u64,
        }
        impl CacheKey for TestKey {
            type Group = u64;
            fn group(&self) -> Option<u64> {
                Some(self.a)
            }
        }
        let c: PageCacheGeneric<TestKey> = PageCacheGeneric::new(cfg(1024, 64));
        let key = TestKey { a: 42, b: 7 };
        c.put(key, vec![9, 9, 9]);
        assert_eq!(c.get(&key).expect("hit").as_slice(), &[9, 9, 9]);
    }

    // ── CacheKey + invalidate_group regression tests (D1.2, fire 44) ─────

    #[test]
    fn invalidate_group_drops_only_matching_entries() {
        #[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
        struct GKey {
            file_id: u64,
            page_index: u64,
        }
        impl CacheKey for GKey {
            type Group = u64;
            fn group(&self) -> Option<u64> {
                Some(self.file_id)
            }
        }
        let c: PageCacheGeneric<GKey> = PageCacheGeneric::new(cfg(4096, 64));
        // Two pages each for files 1, 2, 3.
        for fid in [1u64, 2, 3] {
            for pi in [0u64, 1] {
                c.put(
                    GKey {
                        file_id: fid,
                        page_index: pi,
                    },
                    vec![fid as u8; 64],
                );
            }
        }
        assert_eq!(c.len(), 6);

        // Invalidating file 2 must drop exactly 2 entries.
        let evicted = c.invalidate_group(&2u64);
        assert_eq!(evicted, 2);
        assert_eq!(c.len(), 4);

        // File 1 and file 3 entries must still be reachable.
        assert!(
            c.get(&GKey {
                file_id: 1,
                page_index: 0,
            })
            .is_some()
        );
        assert!(
            c.get(&GKey {
                file_id: 3,
                page_index: 1,
            })
            .is_some()
        );
        // File 2 entries must be gone.
        assert!(
            c.get(&GKey {
                file_id: 2,
                page_index: 0,
            })
            .is_none()
        );
    }

    #[test]
    fn invalidate_group_is_noop_for_ungrouped_keys() {
        // String keys have group() = None; invalidate_group of any
        // value must be a no-op and return 0.
        let c: PageCacheGeneric<String> = PageCacheGeneric::new(cfg(1024, 64));
        c.put("a".to_owned(), vec![0u8; 64]);
        c.put("b".to_owned(), vec![0u8; 64]);
        let evicted = c.invalidate_group(&());
        assert_eq!(evicted, 0);
        assert_eq!(c.len(), 2, "ungrouped entries must not be touched");
    }

    #[test]
    fn invalidate_group_after_eviction_is_consistent() {
        // Confirm the by_group index is kept in sync when the LRU
        // evicts entries to stay under the byte quota.
        #[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
        struct GKey(u64, u64);
        impl CacheKey for GKey {
            type Group = u64;
            fn group(&self) -> Option<u64> {
                Some(self.0)
            }
        }
        // 128-byte quota / 64-byte pages → at most 2 resident.
        let c: PageCacheGeneric<GKey> = PageCacheGeneric::new(cfg(128, 64));
        c.put(GKey(1, 0), vec![1u8; 64]);
        c.put(GKey(1, 1), vec![1u8; 64]);
        // Inserting a third page evicts the LRU (GKey(1,0)) — but the
        // by_group index for file 1 still has GKey(1,1) and gains
        // GKey(2,0).
        c.put(GKey(2, 0), vec![2u8; 64]);
        assert_eq!(c.len(), 2);

        // invalidate_group(1) should drop only GKey(1,1), leaving GKey(2,0).
        let evicted = c.invalidate_group(&1u64);
        assert_eq!(evicted, 1);
        assert_eq!(c.len(), 1);
        assert!(c.get(&GKey(2, 0)).is_some());
    }
}
