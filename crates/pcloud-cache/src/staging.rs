//! In-memory staging buffer for in-flight local writes.
//!
//! The staging cache holds the bytes of files that have been written
//! locally but not yet uploaded. It is bounded by file count
//! ([`StagingCache::max_open_files`]) and evicts on a least-recently-
//! staged order. Eviction here is lossy: evicted buffers are dropped,
//! so callers must have already flushed them to disk-backed storage
//! before allowing the cache to displace them.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Bounded staging buffer keyed by remote-relative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagingCache {
    /// Maximum number of distinct staged files. Extra writes evict the
    /// least-recently-staged entry.
    pub max_open_files: usize,
    /// Staged file contents keyed by path.
    pub files: HashMap<String, Vec<u8>>,
    /// Insertion order used to select the eviction victim.
    pub open_order: VecDeque<String>,
}

impl Default for StagingCache {
    fn default() -> Self {
        Self {
            max_open_files: 64,
            files: HashMap::new(),
            open_order: VecDeque::new(),
        }
    }
}

impl StagingCache {
    /// Stage `bytes` under `path`. If `path` was previously staged, the
    /// old entry is replaced and the staged order is updated so the new
    /// write is treated as the most recently used. Exceeding
    /// [`StagingCache::max_open_files`] evicts the oldest entry.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::staging::StagingCache;
    /// let mut cache = StagingCache::default();
    /// cache.stage("a.txt", b"contents".to_vec());
    /// assert_eq!(cache.get("a.txt"), Some(&b"contents"[..]));
    /// ```
    pub fn stage(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        let path = path.into();
        if self.files.contains_key(&path) {
            self.open_order.retain(|entry| entry != &path);
        }
        self.open_order.push_back(path.clone());
        self.files.insert(path, bytes);
        self.evict_if_needed();
    }

    /// Return the staged buffer for `path`, or `None` if absent / evicted.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::staging::StagingCache;
    /// let mut cache = StagingCache::default();
    /// assert_eq!(cache.get("missing.txt"), None);
    /// cache.stage("hello.txt", b"world".to_vec());
    /// assert_eq!(cache.get("hello.txt"), Some(&b"world"[..]));
    /// ```
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.files.get(path).map(Vec::as_slice)
    }

    /// Number of staged files currently resident.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::staging::StagingCache;
    /// let mut cache = StagingCache::default();
    /// assert_eq!(cache.staged_count(), 0);
    /// cache.stage("a", vec![1]);
    /// cache.stage("b", vec![2]);
    /// assert_eq!(cache.staged_count(), 2);
    /// ```
    #[must_use]
    pub fn staged_count(&self) -> usize {
        self.files.len()
    }

    fn evict_if_needed(&mut self) {
        while self.files.len() > self.max_open_files {
            let Some(oldest) = self.open_order.pop_front() else {
                break;
            };
            self.files.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StagingCache;

    #[test]
    fn stages_and_reads_file_buffers() {
        let mut cache = StagingCache::default();
        cache.stage("docs/report.txt", b"buffer".to_vec());

        assert_eq!(cache.get("docs/report.txt"), Some(&b"buffer"[..]));
        assert_eq!(cache.staged_count(), 1);
    }

    #[test]
    fn evicts_oldest_staged_file_when_limit_is_exceeded() {
        let mut cache = StagingCache {
            max_open_files: 1,
            ..StagingCache::default()
        };
        cache.stage("a.txt", b"a".to_vec());
        cache.stage("b.txt", b"b".to_vec());

        assert_eq!(cache.get("a.txt"), None);
        assert_eq!(cache.get("b.txt"), Some(&b"b"[..]));
    }
}
