//! LRU metadata cache with TTL for FUSE lookups/getattr/readdir.
//!
//! The pCloud FUSE adapter caches the result of each resolved path so that
//! repeated `getattr`/`lookup` calls from the kernel do not round-trip to the
//! remote API. Entries expire once their insertion age exceeds `ttl`
//! (default 30s, sourced from config in higher layers), and the LRU cap
//! bounds memory use to `capacity` entries.
//!
//! Storage is intentionally simple: a `HashMap` keyed by path plus a
//! `VecDeque` recording access order. This trades a tiny amount of work per
//! hit for dependency-free determinism. The crate does not pull `lru` or
//! `parking_lot` because neither is in the workspace dependency set.

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
}

#[derive(Debug)]
struct Inner {
    config: MetadataCacheConfig,
    entries: HashMap<String, CacheSlot>,
    order: VecDeque<String>,
}

impl Inner {
    fn new(config: MetadataCacheConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn touch(&mut self, path: &str) {
        if let Some(pos) = self.order.iter().position(|p| p == path) {
            // O(n) promotion; acceptable for 4K-entry cap.
            if let Some(key) = self.order.remove(pos) {
                self.order.push_back(key);
            }
        }
    }

    fn evict_expired(&mut self) {
        let ttl = self.config.ttl;
        let now = Instant::now();
        // Expired entries are scattered; walk and remove.
        self.order.retain(|path| {
            if let Some(slot) = self.entries.get(path) {
                if now.duration_since(slot.inserted_at) <= ttl {
                    return true;
                }
            }
            self.entries.remove(path);
            false
        });
    }

    fn evict_if_over_capacity(&mut self) {
        while self.entries.len() > self.config.capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
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
            inner.entries.remove(path);
            inner.order.retain(|p| p != path);
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
        let slot = CacheSlot {
            meta,
            inserted_at: now,
        };
        // Periodic expiry pass so stale slots cannot camp forever on a
        // never-touched key.
        inner.evict_expired();
        if inner.entries.insert(path.to_owned(), slot).is_some() {
            inner.order.retain(|p| p != path);
        }
        inner.order.push_back(path.to_owned());
        inner.evict_if_over_capacity();
    }

    /// Forget a single entry.
    ///
    /// The `order.retain()` call is O(n) in the number of cached entries.
    /// This is acceptable because the cache is bounded to at most
    /// [`DEFAULT_CAPACITY`] (4096) entries by design; the constant factor
    /// is small (pointer comparison) and invalidation is infrequent
    /// (write-path events only). A secondary skip-list or index would add
    /// dependency weight not justified at this scale.
    pub fn invalidate(&self, path: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        inner.entries.remove(path);
        inner.order.retain(|p| p != path);
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
