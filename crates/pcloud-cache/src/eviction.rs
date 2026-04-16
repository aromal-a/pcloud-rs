//! Declarative eviction-policy tag attached to a [`crate::CacheShell`].
//!
//! The individual caches (page / staging / checksum) each enforce
//! their own bound; this enum exists so the caller can declare *intent*
//! (advisory) without forcing a specific per-cache knob.

// **PLATFORM:** all
// **GATING:** none (portable).

/// Advisory eviction policy selector.
///
/// # Example
///
/// ```
/// use pcloud_cache::eviction::EvictionPolicy;
/// // SizeBound is the currently-enforced policy across sub-caches.
/// let p = EvictionPolicy::SizeBound;
/// assert_eq!(p, EvictionPolicy::SizeBound);
/// assert_ne!(p, EvictionPolicy::AgeBound);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvictionPolicy {
    /// Evict when a byte-count / entry-count bound is exceeded. This is
    /// what the live page and staging caches actually enforce today.
    SizeBound,
    /// Evict entries older than a configured age. Reserved for a future
    /// time-based eviction path; not currently implemented by any
    /// sub-cache.
    AgeBound,
}
