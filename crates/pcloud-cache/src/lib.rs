#![forbid(unsafe_code)]
//! # pcloud-cache
//!
//! Local caching primitives: page cache, checksum cache, staging area,
//! and eviction policy. Cache directories live under the user cache root
//! with `0700` permissions; cached content is disposable — losing it
//! is a performance event, not a correctness event.
//!
//! The page cache ([`page_cache_generic::PageCacheGeneric`]) is the
//! hot path: it is guarded by a single [`std::sync::Mutex`] over a
//! [`lru::LruCache`], so lookups are O(1) and eviction is O(1) per
//! evicted entry. Values are stored as `Arc<Vec<u8>>` so reads return
//! a cheap refcount bump instead of copying the underlying page
//! bytes. See the [`page_cache_generic`] module docs for the P1.1 /
//! P5.1 rationale.
//!
//! ## Observability
//!
//! Cache throughput is tracked both inside the cache (lifetime
//! `hits` / `misses` counters via
//! [`page_cache_generic::PageCacheGeneric::stats`] →
//! [`page_cache_generic::PageCacheStats::hit_ratio`]) and at the
//! daemon layer above. The daemon exports
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
pub mod cipher;
pub mod eviction;
/// Key-typed generic LRU page cache.
///
/// Canonical string-keyed page-cache primitive for this crate.
/// `pcloud-fs::page_cache` re-exports the types defined here, plus
/// hosts a `PageKey`-specialised variant with a secondary `by_file`
/// index for O(k) per-file invalidation (CLAUDEREV deferred-set D1.2
/// will lift that into the generic via a `CacheKey` trait).
pub mod page_cache_generic;
pub mod sealed_blob;
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
    ///
    /// Backed by `PageCacheGeneric<String>` since CLAUDEREV
    /// deferred-set D1.1b.2d (fire 42); the legacy `page_cache::PageCache`
    /// is no longer reachable from production code via this struct.
    pub pages: page_cache_generic::PageCacheGeneric<String>,
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
            pages: page_cache_generic::PageCacheGeneric::default(),
            staging: staging::StagingCache::default(),
            eviction_policy: eviction::EvictionPolicy::SizeBound,
        }
    }
}

impl CacheShell {
    /// Insert a page into the shared `PageCacheGeneric<String>`.
    ///
    /// Accepts anything convertible into `String` for the key plus an
    /// owned `Vec<u8>` for the page bytes. Callers that already hold a
    /// `String` key + an `Arc<Vec<u8>>` should call
    /// [`page_cache_generic::PageCacheGeneric::put`] directly with
    /// `(*arc).clone()` if they want to avoid the convenience wrapper.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::CacheShell;
    /// let mut shell = CacheShell::default();
    /// shell.cache_page("file:42:page:0", vec![0xDE, 0xAD, 0xBE, 0xEF]);
    /// assert_eq!(shell.pages.len(), 1);
    /// ```
    pub fn cache_page(&mut self, key: impl Into<String>, data: Vec<u8>) {
        self.pages.put(key.into(), data);
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
        let cfg = self.pages.config();
        let stats = self.pages.stats();
        format!(
            "cache(page_limit={}MiB, page_size={}KiB, cached_pages={}, used_bytes={}KiB, staging_files={})",
            cfg.max_bytes / (1024 * 1024),
            cfg.page_size / 1024,
            stats.pages_resident,
            stats.bytes_resident / 1024,
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
