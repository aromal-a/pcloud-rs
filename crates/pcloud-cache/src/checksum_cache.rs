//! Entry-count bounded cache of local file checksums.
//!
//! This is currently a placeholder shell: only the capacity bound is
//! persisted. The full checksum map lives elsewhere (inside the upload
//! state machine) and is written here only when cross-session
//! persistence is required. The struct is kept distinct so the
//! capacity policy can be tuned independently of the actual cache
//! storage.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};

/// Entry-count bound for the checksum cache.
///
/// `entry_limit` is the maximum number of `(path, sha1)` pairs the
/// enclosing daemon keeps resident. The default is sized to comfortably
/// cover a laptop-class sync root without evicting hot entries.
///
/// # Example
///
/// ```
/// use pcloud_cache::checksum_cache::ChecksumCache;
/// let c = ChecksumCache::default();
/// assert!(c.entry_limit > 0);
/// let tuned = ChecksumCache { entry_limit: 32 };
/// assert_eq!(tuned.entry_limit, 32);
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecksumCache {
    /// Maximum number of cached entries. Enforced by the daemon, not by
    /// this struct directly.
    pub entry_limit: usize,
}

impl Default for ChecksumCache {
    fn default() -> Self {
        Self { entry_limit: 8192 }
    }
}
