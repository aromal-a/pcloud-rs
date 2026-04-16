#![forbid(unsafe_code)]
//! # pcloud-cache
//!
//! Local caching primitives: page cache, checksum cache, staging area,
//! and eviction policy. Cache directories live under the user cache root
//! with `0700` permissions; cached content is disposable — losing it
//! is a performance event, not a correctness event.
//!
//! The page cache ([`page_cache::PageCache`]) is the hot path: it is
//! guarded by a single [`parking_lot::RwLock`] over a
//! [`linked_hash_map::LinkedHashMap`], so lookups are O(1) and eviction
//! (least-recently-inserted) is O(1) per evicted entry. Values are
//! stored as `Arc<Vec<u8>>` so reads return a cheap refcount bump
//! instead of copying the underlying page bytes. See the
//! [`page_cache`] module docs for the P1.1 / P5.1 rationale.
//!
//! ## Observability
//!
//! Cache throughput is tracked by the daemon one layer up: every
//! call site that queries [`page_cache::PageCache::get`] increments
//! a hit or miss counter, and the daemon exports
//! `hit_ratio = hits / (hits + misses)` as an SLO-grade gauge. A
//! sustained dip below the configured SLO threshold typically means
//! the page cache has been sized too small for the current working
//! set, not that the cache itself is misbehaving — the cache is a
//! pure in-memory data structure with no error paths visible to the
//! hit/miss counters.
//!
//! All sub-caches are `Send + Sync` and can be shared via `Arc`.
//! Nothing in this crate persists to disk — callers that need durable
//! caching should plug this module into a larger staging/writeback
//! pipeline.

#![deny(missing_docs)]
#![allow(clippy::pedantic)]

// **PLATFORM:** all
// **GATING:** none (portable).

pub mod checksum_cache;
pub mod eviction;
pub mod page_cache;
pub mod staging;

/// Human-readable crate name. Used by telemetry / logging so the
/// originating crate can be identified without pulling in `env!`.
///
/// # Example
///
/// ```
/// assert_eq!(pcloud_cache::CRATE_NAME, "pcloud-cache");
/// ```
pub const CRATE_NAME: &str = "pcloud-cache";

/// Aggregate holder for every cache primitive used by the daemon.
///
/// `CacheShell` composes the individual caches (pages, staging,
/// checksums) so that the daemon can pass a single value around rather
/// than juggle four independent fields. Each inner cache is
/// independently owned and independently configurable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheShell {
    /// Metadata cache for local file checksums (bounded by entry count).
    pub checksums: checksum_cache::ChecksumCache,
    /// Page cache for downloaded/decrypted file pages.
    pub pages: page_cache::PageCache,
    /// Staging buffer for in-flight local writes awaiting upload.
    pub staging: staging::StagingCache,
    /// Configured eviction policy. Advisory only; each sub-cache
    /// enforces its own capacity bound internally.
    pub eviction_policy: eviction::EvictionPolicy,
}

impl Default for CacheShell {
    fn default() -> Self {
        Self {
            checksums: checksum_cache::ChecksumCache::default(),
            pages: page_cache::PageCache::default(),
            staging: staging::StagingCache::default(),
            eviction_policy: eviction::EvictionPolicy::SizeBound,
        }
    }
}

impl CacheShell {
    /// Insert a page into the shared [`page_cache::PageCache`].
    ///
    /// Accepts an owned `Vec<u8>` for ergonomics; if the caller already
    /// holds an `Arc<Vec<u8>>` it should use [`page_cache::PageCache::put`]
    /// directly to avoid the extra allocation.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::CacheShell;
    /// let mut shell = CacheShell::default();
    /// shell.cache_page("file:42:page:0", vec![0xDE, 0xAD, 0xBE, 0xEF]);
    /// assert_eq!(shell.pages.entry_count(), 1);
    /// ```
    pub fn cache_page(&mut self, key: impl Into<String>, data: Vec<u8>) {
        self.pages.put(key, data);
    }

    /// Stage an in-flight write buffer for `path`.
    ///
    /// See [`staging::StagingCache::stage`] for the eviction contract.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::CacheShell;
    /// let mut shell = CacheShell::default();
    /// shell.stage_file("docs/report.txt", b"draft".to_vec());
    /// assert_eq!(shell.staging.staged_count(), 1);
    /// ```
    pub fn stage_file(&mut self, path: impl Into<String>, data: Vec<u8>) {
        self.staging.stage(path, data);
    }

    /// One-line human-readable summary of current cache state. Intended
    /// for logs and diagnostics only — the exact format is not stable.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::CacheShell;
    /// let shell = CacheShell::default();
    /// let s = shell.summary();
    /// assert!(s.starts_with("cache("));
    /// ```
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "cache(page_limit={}MiB, page_size={}KiB, cached_pages={}, used_bytes={}KiB, staging_files={})",
            self.pages.max_bytes() / (1024 * 1024),
            self.pages.page_size_bytes() / 1024,
            self.pages.entry_count(),
            self.pages.used_bytes() / 1024,
            self.staging.staged_count()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::CacheShell;

    #[test]
    fn summary_reflects_cached_and_staged_state() {
        let mut cache = CacheShell::default();
        cache.cache_page("page:1", b"hello".to_vec());
        cache.stage_file("docs/report.txt", b"draft".to_vec());

        let summary = cache.summary();
        assert!(summary.contains("cached_pages=1"));
        assert!(summary.contains("staging_files=1"));
    }
}
