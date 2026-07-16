//! Typed-key page cache facade for the FUSE read path.
//!
//! As of CLAUDEREV deferred-set D1.4 (fire 45) this module is a thin
//! facade: it owns the [`PageKey`] struct (the typed `(file_id,
//! page_index)` cache key) and re-exports the canonical
//! [`PageCacheGeneric`] from `pcloud_cache::page_cache_generic`. The
//! `fuse_adapter` reaches for `PageCacheGeneric<PageKey>` via these
//! re-exports.
//!
//! # Cache contract
//!
//! Pages are sized at [`DEFAULT_PAGE_SIZE`] (64 KiB) except for a
//! trailing short page at EOF. Total memory is bounded by
//! [`PageCacheConfig::max_bytes`] (default 128 MiB); once exceeded,
//! the least-recently-used page(s) are evicted. Per-file invalidation
//! is provided by `PageCacheGeneric::invalidate_group(&file_id)`
//! because [`PageKey`] implements `CacheKey` with `Group = u64` —
//! `group()` returns `Some(file_id)`. Operation is O(k) where k is
//! the number of resident pages for the target file.
//!
//! # `Arc<Vec<u8>>` value sharing rationale (P5.1)
//!
//! Cached page values are stored as `Arc<Vec<u8>>` rather than bare
//! `Vec<u8>`. On a hit the cache returns a cheap `Arc::clone` — an
//! atomic refcount bump — instead of a byte-by-byte memcpy of up to
//! [`DEFAULT_PAGE_SIZE`] (64 KiB). For realistic workloads with warm
//! caches this is roughly a **3-orders-of-magnitude** win on the hit
//! path:
//!
//! * memcpy of a 64 KiB page on a modern x86_64 ≈ 5–10 µs,
//! * `Arc::clone` of the same page ≈ 5–10 ns (one `lock xadd`).
//!
//! The refcount is shared between the cache entry and every concurrent
//! FUSE reader, so a page evicted from the cache while still referenced
//! by an in-flight read stays live until the last reader drops its
//! `Arc`. This eliminates the "use-after-evict" class of bugs without
//! any explicit pinning logic.
//!
//! # O(1) eviction via [`lru::LruCache`]
//!
//! The canonical implementation in `pcloud_cache::page_cache_generic`
//! threads an intrusive doubly-linked list through a hash map (the
//! `lru` crate's `LruCache`). All three mutating operations are O(1):
//! `get` (splice + relink at MRU), `put` (insert at MRU, optionally
//! pop the LRU until under-quota), and per-group eviction via the
//! secondary `by_group` index introduced in D1.2 (fire 44).

// **PLATFORM:** all
// **GATING:** none (portable).

// CLAUDEREV deferred-set D1.4 (fire 45): the legacy non-generic `PageCache`
// (Mutex + LruCache + by_file index) was deleted from this module. Its
// machinery lives in `pcloud_cache::page_cache_generic::PageCacheGeneric<K>`
// (re-exported below) and the typed-key shape is now
// `PageCacheGeneric<PageKey>` — see the `CacheKey` impl on `PageKey` below.

// `DEFAULT_PAGE_SIZE`, `DEFAULT_MAX_BYTES`, `PageCacheConfig`, and
// `PageCacheStats` are re-exported from `pcloud_cache::page_cache_generic`
// (CLAUDEREV deferred-set D1.1b, fire 38) so the legacy non-generic
// `PageCache` and the new `PageCacheGeneric<K>` share a single
// definition of these types. Existing call sites that import
// `pcloud_fs::page_cache::{PageCacheConfig, PageCacheStats,
// DEFAULT_PAGE_SIZE, DEFAULT_MAX_BYTES}` continue to work via the
// re-export below.
pub use pcloud_cache::page_cache_generic::{
    DEFAULT_MAX_BYTES, DEFAULT_PAGE_SIZE, PageCacheConfig, PageCacheStats,
};

/// Compound cache key: `(file_id, page_index)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageKey {
    /// pCloud numeric file id the page belongs to.
    pub file_id: u64,
    /// Zero-based page index within the file (`page_index * page_size`
    /// is the byte offset of the first byte in the page).
    pub page_index: u64,
}

/// `PageKey` participates in per-file group invalidation: its `Group`
/// is the `file_id`, so `PageCacheGeneric<PageKey>::invalidate_group(&file_id)`
/// runs in O(k) where k is the number of resident pages for that file.
/// CLAUDEREV deferred-set D1.2 (fire 44).
impl pcloud_cache::page_cache_generic::CacheKey for PageKey {
    type Group = u64;
    fn group(&self) -> Option<u64> {
        Some(self.file_id)
    }
}

// `PageCacheStats` is re-exported above from
// `pcloud_cache::page_cache_generic` so the legacy `PageCache` and the
// new `PageCacheGeneric<K>` share a single stats struct.

// ── Canonical generic page cache (CLAUDEREV deferred-set D1, closed fire 45) ─
//
// `PageCacheGeneric<K>` lives in `pcloud_cache::page_cache_generic` (the
// lower-level crate) and is re-exported here so existing
// `use pcloud_fs::page_cache::PageCacheGeneric` imports continue to
// work. Specialised on `PageKey` it replaces the legacy non-generic
// `PageCache` deleted in this fire.

pub use pcloud_cache::page_cache_generic::PageCacheGeneric;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn cfg(max_bytes: usize, page_size: usize) -> PageCacheConfig {
        PageCacheConfig {
            page_size,
            max_bytes,
        }
    }

    #[test]
    fn miss_then_hit() {
        let c = PageCacheGeneric::<PageKey>::new(cfg(64 * 1024, 64));
        let key = PageKey {
            file_id: 1,
            page_index: 0,
        };
        assert!(c.get(&key).is_none());
        c.put(key, vec![7u8; 64]);
        let got = c.get(&key).expect("hit");
        assert_eq!(*got, vec![7u8; 64]);
        let stats = c.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn lru_eviction_when_over_cap() {
        // Cap = 3 pages of 64 bytes each.
        let c = PageCacheGeneric::<PageKey>::new(cfg(64 * 3, 64));
        for i in 0..3u64 {
            c.put(
                PageKey {
                    file_id: 1,
                    page_index: i,
                },
                vec![i as u8; 64],
            );
        }
        // Insert fourth page — page 0 should be evicted as LRU.
        c.put(
            PageKey {
                file_id: 1,
                page_index: 3,
            },
            vec![3u8; 64],
        );
        assert!(
            c.get(&PageKey {
                file_id: 1,
                page_index: 0
            })
            .is_none()
        );
        assert!(
            c.get(&PageKey {
                file_id: 1,
                page_index: 3
            })
            .is_some()
        );
    }

    #[test]
    fn access_promotes_to_mru() {
        let c = PageCacheGeneric::<PageKey>::new(cfg(64 * 2, 64));
        let k0 = PageKey {
            file_id: 1,
            page_index: 0,
        };
        let k1 = PageKey {
            file_id: 1,
            page_index: 1,
        };
        let k2 = PageKey {
            file_id: 1,
            page_index: 2,
        };
        c.put(k0, vec![0u8; 64]);
        c.put(k1, vec![1u8; 64]);
        // Touch k0 so k1 becomes LRU.
        let _ = c.get(&k0);
        c.put(k2, vec![2u8; 64]);
        assert!(c.get(&k0).is_some(), "k0 survived via promotion");
        assert!(c.get(&k1).is_none(), "k1 evicted");
        assert!(c.get(&k2).is_some());
    }

    #[test]
    fn invalidate_file_drops_pages_for_that_file_only() {
        let c = PageCacheGeneric::<PageKey>::new(cfg(64 * 1024, 64));
        c.put(
            PageKey {
                file_id: 1,
                page_index: 0,
            },
            vec![1u8; 64],
        );
        c.put(
            PageKey {
                file_id: 1,
                page_index: 1,
            },
            vec![1u8; 64],
        );
        c.put(
            PageKey {
                file_id: 2,
                page_index: 0,
            },
            vec![2u8; 64],
        );
        c.invalidate_group(&1u64);
        assert!(
            c.get(&PageKey {
                file_id: 1,
                page_index: 0
            })
            .is_none()
        );
        assert!(
            c.get(&PageKey {
                file_id: 2,
                page_index: 0
            })
            .is_some()
        );
    }

    #[test]
    fn hit_ratio_reported_correctly() {
        let c = PageCacheGeneric::<PageKey>::new(cfg(64 * 1024, 64));
        let key = PageKey {
            file_id: 1,
            page_index: 0,
        };
        c.put(key, vec![0u8; 64]);
        for _ in 0..3 {
            let _ = c.get(&key);
        }
        let _ = c.get(&PageKey {
            file_id: 99,
            page_index: 0,
        });
        let stats = c.stats();
        assert_eq!(stats.hits, 3);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_ratio() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn oversized_page_is_silently_dropped() {
        let c = PageCacheGeneric::<PageKey>::new(cfg(64, 64));
        c.put(
            PageKey {
                file_id: 1,
                page_index: 0,
            },
            vec![0u8; 128],
        );
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn oversized_page_increments_rejection_counter() {
        let c = PageCacheGeneric::<PageKey>::new(cfg(64, 64));
        assert_eq!(c.stats().bytes_rejected_oversized, 0);
        c.put(
            PageKey {
                file_id: 1,
                page_index: 0,
            },
            vec![0u8; 128],
        );
        c.put(
            PageKey {
                file_id: 1,
                page_index: 1,
            },
            vec![0u8; 256],
        );
        assert_eq!(c.stats().bytes_rejected_oversized, 128 + 256);
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn concurrent_put_and_get_do_not_deadlock() {
        let c = Arc::new(PageCacheGeneric::<PageKey>::new(cfg(64 * 1024, 64)));
        let mut handles = Vec::new();
        for tid in 0..8 {
            let c = Arc::clone(&c);
            handles.push(thread::spawn(move || {
                for i in 0..64 {
                    let key = PageKey {
                        file_id: tid,
                        page_index: i,
                    };
                    c.put(key, vec![tid as u8; 64]);
                    let _ = c.get(&key);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(c.len() <= c.config().max_bytes / 64);
    }

    // ── PageCacheGeneric re-export smoke test (D1.1b, fire 38) ──────────
    //
    // The full generic test suite lives in
    // `crates/pcloud-cache/src/page_cache_generic.rs::tests` (round-trip,
    // byte-quota eviction, oversized rejection, typed-key smoke).
    // This single test stays here to prove the **re-export chain
    // `pcloud_fs::page_cache::PageCacheGeneric` →
    // `pcloud_cache::page_cache_generic::PageCacheGeneric`** is wired
    // correctly so existing call sites that import via `pcloud_fs`
    // see a working type. Specialised on `PageKey` to anticipate the
    // D1.3 migration.

    #[test]
    fn page_cache_generic_reexport_resolves_for_pagekey() {
        let c: PageCacheGeneric<PageKey> = PageCacheGeneric::new(cfg(1024, 64));
        let key = PageKey {
            file_id: 42,
            page_index: 7,
        };
        c.put(key, vec![9, 9, 9]);
        let v = c.get(&key).expect("hit");
        assert_eq!(v.as_slice(), &[9, 9, 9]);
    }
}
