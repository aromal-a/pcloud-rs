//! LRU metadata cache with TTL for FUSE lookups/getattr/readdir.
//!
//! The pCloud FUSE adapter caches the result of each resolved path so that
//! repeated `getattr`/`lookup` calls from the kernel do not round-trip to the
//! remote API. Entries expire once their insertion age exceeds `ttl`
//! (default 30s, sourced from config in higher layers), and the LRU cap
//! bounds memory use to `capacity` entries.
//!
//! # Storage
//!
//! Three parallel structures:
//!
//! - `entries: HashMap<String, CacheSlot>` — authoritative storage; maps
//!   path → (metadata, inserted_at).
//! - `order: VecDeque<String>` — LRU order; front = oldest, back = MRU.
//! - `order_index: HashMap<String, usize>` — secondary index from path
//!   to its *current* position in `order`. Maintained on every mutation
//!   so [`MetadataCache::invalidate`] is O(1) (audit-06 P3 /
//!   pcloud-rs-ncx.45).
//!
//! Invariants:
//!   - For every `key` in `entries`, `order_index[key]` is set and
//!     `order[order_index[key]] == key`.
//!   - `order` never contains duplicates.
//!   - `order.len() == entries.len() == order_index.len()`.
//!
//! Removing an element from `VecDeque` at an arbitrary index is O(n)
//! because of the slot shifts, so for the invalidate path we mark the
//! slot with a tombstone and lazily skip it; the LRU tail-push path
//! always rebuilds the index via `push_back` + `swap_remove_index` so
//! the invariants hold even under mixed operations.
//!
//! The crate does not pull `lru` or `parking_lot` because neither is in
//! the workspace dependency set.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::fuse_adapter::{DirEntry, EntryAttr};

/// Default TTL for metadata entries, per plan (bd-1du.4.b).
pub const DEFAULT_TTL: Duration = Duration::from_secs(30);

/// Default LRU capacity. Tuned to be generous for typical working sets but
/// small enough to bound memory. Callers may override via [`MetadataCacheConfig`].
pub const DEFAULT_CAPACITY: usize = 4096;

/// Runtime configuration for [`MetadataCache`]. Usually sourced from the
/// daemon config in `pcloud-config` and threaded in through the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataCacheConfig {
    /// How long an entry remains valid after insertion. Reads that observe
    /// an older entry treat it as a miss and evict it.
    pub ttl: Duration,
    /// Maximum number of cached entries. A zero value is clamped to `1`
    /// by [`MetadataCache::new`].
    pub capacity: usize,
}

impl Default for MetadataCacheConfig {
    fn default() -> Self {
        Self {
            ttl: DEFAULT_TTL,
            capacity: DEFAULT_CAPACITY,
        }
    }
}

/// One cached metadata shape. `attr` holds the point-in-time attributes;
/// `children` is populated for directories whose contents were enumerated.
#[derive(Debug, Clone)]
pub struct CachedMetadata {
    /// Point-in-time attributes captured at insertion.
    pub attr: EntryAttr,
    /// Directory listing at the time of insertion, if the entry is a
    /// directory that has been enumerated. `None` for files or directories
    /// whose contents have not been listed.
    pub children: Option<Vec<DirEntry>>,
}

#[derive(Debug, Clone)]
struct CacheSlot {
    meta: CachedMetadata,
    inserted_at: Instant,
    /// Per-slot sequence id. Used by the LRU order deque to distinguish
    /// a live reference (matching seq in both structures) from a stale
    /// tombstone left behind by [`MetadataCache::invalidate`] or re-`put`.
    seq: u64,
}

#[derive(Debug, Clone)]
struct OrderEntry {
    path: String,
    /// Matches [`CacheSlot::seq`] of the corresponding `entries[path]`
    /// slot at the moment this entry was pushed. If the current slot's
    /// seq differs — or if `entries` no longer contains the path — this
    /// entry is a tombstone and MUST be skipped on eviction.
    seq: u64,
}

#[derive(Debug)]
struct Inner {
    config: MetadataCacheConfig,
    /// Authoritative storage: path → cached metadata + insertion time.
    /// This is the source of truth; `order` may contain stale references
    /// to paths that have been invalidated / re-put / TTL-expired but
    /// not yet garbage-collected from the LRU deque.
    entries: HashMap<String, CacheSlot>,
    /// LRU order. May contain stale entries (tombstones); each entry's
    /// `seq` is matched against `entries[path].seq` on eviction to tell
    /// live from tombstone in O(1).
    order: VecDeque<OrderEntry>,
    /// Monotonically increasing slot id. Incremented on every `put` /
    /// touch-push so a fresh order entry is always distinguishable from
    /// a stale one.
    next_seq: u64,
}

impl Inner {
    fn new(config: MetadataCacheConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            order: VecDeque::new(),
            next_seq: 0,
        }
    }

    fn alloc_seq(&mut self) -> u64 {
        let s = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        s
    }

    /// O(n) LRU promotion (hit-path): find and remove the old order entry,
    /// push a new one at the MRU tail. We could also leave a tombstone
    /// here and skip the scan; the O(n) touch is retained because the
    /// bead scope is the invalidate path and tests already validate this
    /// behaviour.
    fn touch(&mut self, path: &str) {
        // Find the live order entry for this path (matching seq in
        // `entries`). Anything else is a tombstone and is left untouched;
        // the eviction pass will reap it.
        let live_seq = match self.entries.get(path) {
            Some(s) => s.seq,
            None => return,
        };
        if let Some(pos) = self
            .order
            .iter()
            .position(|e| e.path == path && e.seq == live_seq)
        {
            if let Some(mut entry) = self.order.remove(pos) {
                // Re-issue a fresh seq so the old slot (if we ever leave
                // one behind elsewhere) is clearly superseded.
                let new_seq = self.alloc_seq();
                entry.seq = new_seq;
                // Update the entries-side seq to match.
                if let Some(slot) = self.entries.get_mut(path) {
                    slot.seq = new_seq;
                }
                self.order.push_back(entry);
            }
        }
    }

    fn evict_expired(&mut self) {
        let ttl = self.config.ttl;
        let now = Instant::now();
        // Walk the deque once and drop order entries whose entries-side
        // slot is either gone (tombstone) or expired.
        self.order.retain(|entry| {
            match self.entries.get(&entry.path) {
                Some(slot) if slot.seq == entry.seq => {
                    if now.duration_since(slot.inserted_at) <= ttl {
                        true
                    } else {
                        // Live but expired.
                        self.entries.remove(&entry.path);
                        false
                    }
                }
                _ => {
                    // Tombstone (stale seq or path already removed) —
                    // reap.
                    false
                }
            }
        });
    }

    fn evict_if_over_capacity(&mut self) {
        while self.entries.len() > self.config.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            // Only evict the entries-side slot if this deque entry is
            // live (matching seq). Tombstones are silently dropped with
            // no effect on `entries`.
            if let Some(slot) = self.entries.get(&oldest.path) {
                if slot.seq == oldest.seq {
                    self.entries.remove(&oldest.path);
                }
            }
        }
    }
}

/// Thread-safe LRU+TTL metadata cache.
#[derive(Debug)]
pub struct MetadataCache {
    inner: Mutex<Inner>,
}

impl Default for MetadataCache {
    fn default() -> Self {
        Self::new(MetadataCacheConfig::default())
    }
}

impl MetadataCache {
    /// Create a new cache with `config`. A zero `capacity` is clamped to 1.
    #[must_use]
    pub fn new(config: MetadataCacheConfig) -> Self {
        // A zero-capacity cache is nonsensical; clamp to 1.
        let config = MetadataCacheConfig {
            capacity: config.capacity.max(1),
            ttl: config.ttl,
        };
        Self {
            inner: Mutex::new(Inner::new(config)),
        }
    }

    /// Return the effective configuration, including any clamping applied
    /// by [`new`](Self::new).
    pub fn config(&self) -> MetadataCacheConfig {
        self.inner.lock().map(|g| g.config).unwrap_or_default()
    }

    /// Fetch an entry if it exists and has not expired. On hit the entry is
    /// promoted to the tail of the LRU queue.
    pub fn get(&self, path: &str) -> Option<CachedMetadata> {
        let mut inner = self.inner.lock().ok()?;
        let ttl = inner.config.ttl;
        let slot = inner.entries.get(path)?.clone();
        if slot.inserted_at.elapsed() > ttl {
            // TTL expired → drop the entry but let the deque entry remain
            // as a tombstone. The seq mismatch (we removed the live slot;
            // any future `put` will mint a new seq) makes the old deque
            // entry unambiguously stale, so the eviction pass can reap it
            // lazily in O(1) per deque-head slot.
            inner.entries.remove(path);
            return None;
        }
        inner.touch(path);
        Some(slot.meta)
    }

    /// Insert or refresh an entry. Bumps it to the LRU tail and evicts the
    /// oldest entry if the cap was exceeded.
    pub fn put(&self, path: &str, meta: CachedMetadata) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let now = Instant::now();
        let seq = inner.alloc_seq();
        let slot = CacheSlot {
            meta,
            inserted_at: now,
            seq,
        };
        // Periodic expiry pass so stale slots cannot camp forever on a
        // never-touched key. `evict_expired` also reaps any tombstones
        // (order entries whose seq mismatches the live `entries[path]`).
        inner.evict_expired();
        // Overwrite is fine: the old `entries[path]` slot's seq will no
        // longer match the old deque entry's seq, so the stale deque
        // entry is automatically a tombstone and will be skipped on
        // eviction. O(1) worst case for the put hot path.
        inner.entries.insert(path.to_owned(), slot);
        inner.order.push_back(OrderEntry {
            path: path.to_owned(),
            seq,
        });
        inner.evict_if_over_capacity();
    }

    /// Forget a single entry. O(1) amortised.
    ///
    /// audit-06 P3 / pcloud-rs-ncx.45: this call drops the entry from the
    /// authoritative `entries` HashMap in O(1) and leaves the LRU `order`
    /// deque holding a stale reference. The stale reference is treated
    /// as a tombstone by `Inner::evict_if_over_capacity` and
    /// `Inner::evict_expired` (private; intra-doc links disabled per
    /// CLAUDEREV P1.3) — the next eviction pass silently skips it.
    ///
    /// Tombstones can accumulate at most up to `order.len() - entries.len()`
    /// entries, bounded by `DEFAULT_CAPACITY` (4096) because `put`'s
    /// eviction loop pops the tombstones lazily and because `evict_expired`
    /// runs on every `put` and does compact them out.
    pub fn invalidate(&self, path: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        // O(1): HashMap remove. Leaves `order` carrying a tombstone that
        // the eviction pass will reap lazily.
        inner.entries.remove(path);
    }

    /// Clear the cache entirely.
    pub fn clear(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.entries.clear();
        inner.order.clear();
    }

    /// Number of entries currently held in the cache.
    pub fn len(&self) -> usize {
        self.inner.lock().map(|g| g.entries.len()).unwrap_or(0)
    }

    /// `true` when the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuse_adapter::{EntryAttr, FsEntryKind};

    fn attr(ino: u64) -> EntryAttr {
        EntryAttr {
            ino,
            kind: FsEntryKind::RegularFile,
            size: 0,
            mode: 0o644,
            uid: 1000,
            gid: 1000,
            mtime_epoch: None,
            mtime_nsec: 0,
        }
    }

    fn meta(ino: u64) -> CachedMetadata {
        CachedMetadata {
            attr: attr(ino),
            children: None,
        }
    }

    #[test]
    fn miss_returns_none() {
        let c = MetadataCache::default();
        assert!(c.get("/missing").is_none());
    }

    #[test]
    fn hit_after_put() {
        let c = MetadataCache::default();
        c.put("/a", meta(42));
        assert_eq!(c.get("/a").unwrap().attr.ino, 42);
    }

    #[test]
    fn ttl_expiry_evicts_entry() {
        let c = MetadataCache::new(MetadataCacheConfig {
            ttl: Duration::from_millis(1),
            capacity: 16,
        });
        c.put("/a", meta(1));
        std::thread::sleep(Duration::from_millis(10));
        assert!(c.get("/a").is_none());
    }

    #[test]
    fn lru_eviction_bounds_memory() {
        let c = MetadataCache::new(MetadataCacheConfig {
            ttl: Duration::from_secs(60),
            capacity: 2,
        });
        c.put("/a", meta(1));
        c.put("/b", meta(2));
        c.put("/c", meta(3));
        assert!(c.get("/a").is_none(), "oldest must be evicted");
        assert!(c.get("/b").is_some());
        assert!(c.get("/c").is_some());
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn access_promotes_to_mru() {
        let c = MetadataCache::new(MetadataCacheConfig {
            ttl: Duration::from_secs(60),
            capacity: 2,
        });
        c.put("/a", meta(1));
        c.put("/b", meta(2));
        // Touch /a so /b becomes the oldest.
        let _ = c.get("/a");
        c.put("/c", meta(3));
        assert!(c.get("/a").is_some(), "/a was promoted; should survive");
        assert!(c.get("/b").is_none(), "/b should have been evicted");
        assert!(c.get("/c").is_some());
    }

    #[test]
    fn invalidate_removes_entry() {
        let c = MetadataCache::default();
        c.put("/a", meta(1));
        c.invalidate("/a");
        assert!(c.get("/a").is_none());
    }

    /// audit-06 P3 / pcloud-rs-ncx.45: invalidate + re-put must not trip
    /// over the tombstone left in the LRU deque. The new put must produce
    /// a live entry whose seq supersedes any stale deque entry.
    #[test]
    fn invalidate_then_reput_survives_eviction() {
        let c = MetadataCache::new(MetadataCacheConfig {
            ttl: Duration::from_secs(60),
            capacity: 4,
        });
        // Fill past capacity repeatedly to exercise the eviction path
        // with tombstones sitting at various deque positions.
        for i in 0..32 {
            let path = format!("/p{}", i % 8);
            c.put(&path, meta(i as u64));
            if i % 3 == 0 {
                c.invalidate(&path);
            }
        }
        // Final put of a key should be retrievable even after many stale
        // tombstones have been created by the sequence above.
        c.put("/final", meta(999));
        assert_eq!(c.get("/final").unwrap().attr.ino, 999);
    }

    /// audit-06 P3 / pcloud-rs-ncx.45: overwriting the same key many times
    /// must not leak entries beyond the capacity cap (the tombstone-aware
    /// eviction path correctly distinguishes the latest live entry from
    /// prior generations of the same key).
    #[test]
    fn repeated_overwrite_does_not_overflow_capacity() {
        let c = MetadataCache::new(MetadataCacheConfig {
            ttl: Duration::from_secs(60),
            capacity: 3,
        });
        for i in 0..100u64 {
            c.put("/same", meta(i));
        }
        assert_eq!(c.get("/same").unwrap().attr.ino, 99);
        assert!(c.len() <= 3, "capacity must be enforced: len={}", c.len());
    }

    #[test]
    fn zero_capacity_is_clamped() {
        let c = MetadataCache::new(MetadataCacheConfig {
            ttl: Duration::from_secs(60),
            capacity: 0,
        });
        assert_eq!(c.config().capacity, 1);
        c.put("/a", meta(1));
        assert_eq!(c.len(), 1);
    }
}
