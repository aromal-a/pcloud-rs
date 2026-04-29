//! In-memory staging buffer for in-flight local writes.
//!
//! The staging cache holds the bytes of files that have been written
//! locally but not yet uploaded. It is bounded by file count
//! ([`StagingCache::max_open_files`]) **and** by total byte budget
//! ([`StagingCache::max_bytes`]) to prevent large staged writes from
//! exhausting process memory. Eviction is LRU on insertion order.
//!
//! # Back-pressure contract
//!
//! [`StagingCache::stage`] now returns a [`StagingResult`] that tells the
//! caller whether the entry was accepted or rejected due to the byte budget.
//! Callers **must not** silently discard the rejection: a rejected payload
//! is the only copy of pending upload bytes, so the caller is responsible
//! for writing it to disk-backed staging storage before dropping it.
//!
//! Eviction is **still lossy** for entries that were already resident when
//! the limit is hit. Callers must ensure any pre-resident entries that could
//! be evicted have already been flushed to disk.

// **PLATFORM:** all
// **GATING:** none (portable).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Default maximum number of distinct staged files.
pub const DEFAULT_MAX_OPEN_FILES: usize = 64;
/// Default byte budget: 32 MiB — large enough for typical interactive edits
/// while preventing a single large staged file from consuming unbounded RAM.
pub const DEFAULT_MAX_BYTES: usize = 32 * 1024 * 1024;

/// Outcome of a [`StagingCache::stage`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagingResult {
    /// The entry was accepted and is now resident in the cache.
    Accepted,
    /// The entry was **rejected** because its byte size alone exceeds the
    /// cache's byte budget. The caller must persist the payload to disk-backed
    /// storage.
    RejectedByteBudget {
        /// Size of the rejected payload in bytes.
        payload_bytes: usize,
        /// Current byte budget.
        budget_bytes: usize,
    },
}

/// Bounded staging buffer keyed by remote-relative path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagingCache {
    /// Maximum number of distinct staged files. Extra writes evict the
    /// least-recently-staged entry.
    pub max_open_files: usize,
    /// Maximum total byte budget across all resident entries. New entries
    /// whose size alone exceeds this limit are rejected with
    /// [`StagingResult::RejectedByteBudget`] rather than admitted and
    /// immediately evicting everything else.
    pub max_bytes: usize,
    /// Staged file contents keyed by path.
    pub files: HashMap<String, Vec<u8>>,
    /// Insertion order used to select the eviction victim.
    pub open_order: VecDeque<String>,
    /// Running total of resident bytes.
    pub current_bytes: usize,
}

impl Default for StagingCache {
    fn default() -> Self {
        Self {
            max_open_files: DEFAULT_MAX_OPEN_FILES,
            max_bytes: DEFAULT_MAX_BYTES,
            files: HashMap::new(),
            open_order: VecDeque::new(),
            current_bytes: 0,
        }
    }
}

impl StagingCache {
    /// Stage `bytes` under `path`.
    ///
    /// Returns [`StagingResult::Accepted`] when the entry was admitted and
    /// [`StagingResult::RejectedByteBudget`] when `bytes.len() >
    /// max_bytes`. Rejections must not be silently ignored: the caller is
    /// responsible for persisting the payload to disk-backed storage.
    ///
    /// If `path` was previously staged, the old entry is replaced and the
    /// staged order is updated so the new write is treated as most recently
    /// used. Exceeding [`StagingCache::max_open_files`] or the byte budget
    /// evicts the oldest entry.
    ///
    /// # Example
    ///
    /// ```
    /// use pcloud_cache::staging::{StagingCache, StagingResult};
    /// let mut cache = StagingCache::default();
    /// let result = cache.stage("a.txt", b"contents".to_vec());
    /// assert_eq!(result, StagingResult::Accepted);
    /// assert_eq!(cache.get("a.txt"), Some(&b"contents"[..]));
    /// ```
    pub fn stage(&mut self, path: impl Into<String>, bytes: Vec<u8>) -> StagingResult {
        let payload_len = bytes.len();
        // Reject payloads whose byte size alone exceeds the whole budget.
        // This prevents a single large write from evicting every resident
        // entry while still not being admitted itself.
        if payload_len > self.max_bytes {
            return StagingResult::RejectedByteBudget {
                payload_bytes: payload_len,
                budget_bytes: self.max_bytes,
            };
        }

        let path = path.into();
        // If the path was already staged, remove the old byte count first.
        if let Some(old) = self.files.get(&path) {
            self.current_bytes = self.current_bytes.saturating_sub(old.len());
            self.open_order.retain(|entry| entry != &path);
        }
        self.open_order.push_back(path.clone());
        self.files.insert(path, bytes);
        self.current_bytes = self.current_bytes.saturating_add(payload_len);
        self.evict_if_needed();
        StagingResult::Accepted
    }

    /// Seed `bytes` under `path`, bypassing the byte-budget guard.
    ///
    /// Intended for tests and deterministic fixtures where the budget
    /// must not reject oversized payloads. Production code must use
    /// [`Self::stage`] so that budget enforcement applies.
    pub fn seed_unchecked(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        let path = path.into();
        let payload_len = bytes.len();
        if let Some(old) = self.files.get(&path) {
            self.current_bytes = self.current_bytes.saturating_sub(old.len());
            self.open_order.retain(|entry| entry != &path);
        }
        self.open_order.push_back(path.clone());
        self.files.insert(path, bytes);
        self.current_bytes = self.current_bytes.saturating_add(payload_len);
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

    /// Total bytes of resident staged data.
    #[must_use]
    pub fn resident_bytes(&self) -> usize {
        self.current_bytes
    }

    fn evict_if_needed(&mut self) {
        while self.files.len() > self.max_open_files || self.current_bytes > self.max_bytes {
            let Some(oldest) = self.open_order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.files.remove(&oldest) {
                self.current_bytes = self.current_bytes.saturating_sub(evicted.len());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{StagingCache, StagingResult};

    #[test]
    fn stages_and_reads_file_buffers() {
        let mut cache = StagingCache::default();
        let r = cache.stage("docs/report.txt", b"buffer".to_vec());
        assert_eq!(r, StagingResult::Accepted);

        assert_eq!(cache.get("docs/report.txt"), Some(&b"buffer"[..]));
        assert_eq!(cache.staged_count(), 1);
        assert_eq!(cache.resident_bytes(), 6);
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

    #[test]
    fn rejects_payload_exceeding_byte_budget() {
        let mut cache = StagingCache {
            max_bytes: 10,
            ..StagingCache::default()
        };
        let result = cache.stage("big.bin", vec![0u8; 11]);
        assert!(
            matches!(
                result,
                StagingResult::RejectedByteBudget {
                    payload_bytes: 11,
                    budget_bytes: 10
                }
            ),
            "expected RejectedByteBudget, got {result:?}",
        );
        // The large payload must not have been admitted.
        assert_eq!(cache.get("big.bin"), None);
        assert_eq!(cache.resident_bytes(), 0);
    }

    #[test]
    fn byte_budget_evicts_oldest_when_exceeded_by_accumulation() {
        let mut cache = StagingCache {
            max_open_files: 100,
            max_bytes: 5,
            ..StagingCache::default()
        };
        // Stage first 3-byte entry: fits.
        cache.stage("a.bin", vec![0u8; 3]);
        assert_eq!(cache.resident_bytes(), 3);
        // Stage second 3-byte entry: total = 6 > 5, so oldest ("a.bin") is evicted.
        cache.stage("b.bin", vec![0u8; 3]);
        assert!(cache.resident_bytes() <= 5);
        assert_eq!(cache.get("a.bin"), None, "a.bin should have been evicted");
        assert_eq!(cache.get("b.bin"), Some(&[0u8; 3][..]));
    }

    #[test]
    fn replace_updates_byte_tracking() {
        let mut cache = StagingCache::default();
        cache.stage("f.txt", b"hello".to_vec());
        assert_eq!(cache.resident_bytes(), 5);
        // Replace with a larger payload.
        cache.stage("f.txt", b"hello world".to_vec());
        assert_eq!(cache.resident_bytes(), 11);
        assert_eq!(cache.staged_count(), 1);
    }
}
