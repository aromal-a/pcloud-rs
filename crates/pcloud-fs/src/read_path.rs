//! Read path: in-memory read service with prefetch window and cache,
//! serving bytes from the staging area and page cache. Consumed by
//! `FilesystemShell::read_staged_path` and the FUSE adapter's read
//! callbacks.
//!
//! Portable; no platform gating.

use std::sync::Arc;

// CLAUDEREV deferred-set D1.1b.2c (fire 41): migrated `pages` from the
// legacy `pcloud_cache::page_cache::PageCache` (string-keyed, RwLock +
// LinkedHashMap) to the unified `PageCacheGeneric<String>` (string-keyed,
// Mutex + LruCache). The wire format of `ReadPathService` is unchanged
// because both impls' serde Serialize/Deserialize emit equivalent JSON
// shapes (an `entries: Vec<(String, Vec<u8>)>` payload).
use pcloud_cache::{page_cache_generic::PageCacheGeneric, staging::StagingCache};
use serde::{Deserialize, Serialize};

/// Result of a successful staged read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadResult {
    /// Path that was read.
    pub path: String,
    /// Starting byte offset within the file.
    pub offset: usize,
    /// Bytes returned to the caller (length may be shorter than requested
    /// if the request reached EOF).
    pub bytes: Vec<u8>,
    /// Whether the bytes came from the staging area or the page cache.
    pub source: ReadSource,
    /// Upper byte bound populated in the page cache by this read (used
    /// by tests to assert prefetch extent).
    pub prefetched_until: usize,
}

/// Origin of a [`ReadResult`]'s bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadSource {
    /// Bytes were read from the staging area (at least one miss).
    Stage,
    /// Every byte in this read was served from the page cache.
    Cache,
}

/// Error returned by [`ReadPathService::read`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadPathError {
    /// The requested `offset` is beyond the end of the staged buffer.
    InvalidRange {
        /// Path the request targeted.
        path: String,
        /// Byte offset that was out of range.
        offset: usize,
    },
    /// No staged buffer exists for `path`.
    MissingPath {
        /// Path the request targeted.
        path: String,
    },
}

/// Read-path service combining a prefetch window and a page cache over a
/// staging area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadPathService {
    /// Number of bytes to load ahead of the requested offset on a miss.
    pub prefetch_window_bytes: usize,
    /// Page cache used to serve subsequent reads without re-fetching from
    /// the staging area. Backed by `PageCacheGeneric<String>` since
    /// CLAUDEREV deferred-set D1.1b.2c (fire 41).
    pub pages: PageCacheGeneric<String>,
}

impl Default for ReadPathService {
    fn default() -> Self {
        Self {
            prefetch_window_bytes: 256 * 1024,
            pages: PageCacheGeneric::default(),
        }
    }
}

impl ReadPathService {
    /// Read `requested_bytes` starting at `offset` from `path` in the
    /// staging area. On a page-cache miss the aligned window is populated
    /// from `staging`; subsequent reads in the window are served from the
    /// cache. Returns `InvalidRange` when `offset` is past EOF and
    /// `MissingPath` when the staging area has no entry for `path`.
    pub fn read(
        &mut self,
        staging: &StagingCache,
        path: &str,
        offset: usize,
        requested_bytes: usize,
    ) -> Result<ReadResult, ReadPathError> {
        let Some(staged) = staging.get(path) else {
            return Err(ReadPathError::MissingPath {
                path: path.to_owned(),
            });
        };
        if offset > staged.len() {
            return Err(ReadPathError::InvalidRange {
                path: path.to_owned(),
                offset,
            });
        }

        if requested_bytes == 0 {
            return Ok(ReadResult {
                path: path.to_owned(),
                offset,
                bytes: Vec::new(),
                source: ReadSource::Cache,
                prefetched_until: offset,
            });
        }

        let mut cursor = offset;
        let mut bytes = Vec::new();
        let mut prefetched_until = offset;
        let mut all_from_cache = true;
        let target_len = staged.len().min(offset.saturating_add(requested_bytes));

        while cursor < target_len {
            let window_start = self.window_start(cursor);
            let cache_key = self.window_cache_key(path, cursor);
            // P5.1: `PageCacheGeneric::get` returns `Arc<Vec<u8>>` so a
            // hit is a pointer bump instead of a 64 KiB clone. On a
            // miss we put the bytes into the cache (which wraps them
            // in an `Arc` internally), then `get` them back so the
            // post-put `Arc` we hold is the **same** allocation the
            // cache stores — no duplicated 64 KiB copy. The `cache_key`
            // clone is a small `String`, much cheaper than a Vec clone.
            let (window, source): (Arc<Vec<u8>>, ReadSource) =
                if let Some(window) = self.pages.get(&cache_key) {
                    (window, ReadSource::Cache)
                } else {
                    let (_, window_end) = self.window_bounds(staged.len(), cursor)?;
                    let window_bytes = staged[window_start..window_end].to_vec();
                    self.pages.put(cache_key.clone(), window_bytes);
                    // SAFETY: we just `put` the entry under `cache_key` on
                    // the line above. `PageCacheGeneric` is owned by `&mut
                    // self` here, so no concurrent mutator can have evicted
                    // it between the `put` and this `get`. The lookup is
                    // therefore infallible by construction.
                    let window = self
                        .pages
                        .get(&cache_key)
                        .expect("just-inserted cache entry must be present");
                    (window, ReadSource::Stage)
                };

            let relative_offset = cursor.saturating_sub(window_start);
            if relative_offset > window.len() {
                return Err(ReadPathError::InvalidRange {
                    path: path.to_owned(),
                    offset: cursor,
                });
            }

            let remaining = target_len.saturating_sub(cursor);
            let available = window.len().saturating_sub(relative_offset);
            let chunk_len = available.min(remaining);
            bytes.extend_from_slice(&window[relative_offset..relative_offset + chunk_len]);
            prefetched_until = prefetched_until.max(window_start + window.len());
            all_from_cache &= source == ReadSource::Cache;
            cursor = cursor.saturating_add(chunk_len);
        }

        Ok(ReadResult {
            path: path.to_owned(),
            offset,
            bytes,
            source: if all_from_cache {
                ReadSource::Cache
            } else {
                ReadSource::Stage
            },
            prefetched_until,
        })
    }

    fn window_bounds(
        &self,
        total_len: usize,
        offset: usize,
    ) -> Result<(usize, usize), ReadPathError> {
        if offset > total_len {
            return Err(ReadPathError::InvalidRange {
                path: String::new(),
                offset,
            });
        }
        let window_start = self.window_start(offset);
        let prefetched_until =
            total_len.min(window_start.saturating_add(self.prefetch_window_bytes.max(1)));
        Ok((window_start, prefetched_until))
    }

    fn window_start(&self, offset: usize) -> usize {
        let window = self.prefetch_window_bytes.max(1);
        offset / window * window
    }

    fn window_cache_key(&self, path: &str, offset: usize) -> String {
        format!("read:{}:{}", path, self.window_start(offset))
    }
}

#[cfg(test)]
mod tests {
    use pcloud_cache::staging::StagingCache;

    use super::{ReadPathError, ReadPathService, ReadSource};

    #[test]
    fn reads_staged_bytes_and_populates_cache() {
        let mut service = ReadPathService {
            prefetch_window_bytes: 4,
            ..ReadPathService::default()
        };
        let mut staging = StagingCache::default();
        staging.stage("docs/report.txt", b"abcdefgh".to_vec());

        let first = service.read(&staging, "docs/report.txt", 1, 2).unwrap();
        let second = service.read(&staging, "docs/report.txt", 1, 2).unwrap();

        assert_eq!(first.bytes, b"bc");
        assert_eq!(first.source, ReadSource::Stage);
        assert_eq!(first.prefetched_until, 4);
        assert_eq!(second.bytes, b"bc");
        assert_eq!(second.source, ReadSource::Cache);
    }

    #[test]
    fn returns_missing_path_when_no_staged_or_cached_data_exists() {
        let mut service = ReadPathService::default();
        let staging = StagingCache::default();

        let error = service.read(&staging, "missing.txt", 0, 10).unwrap_err();

        assert_eq!(
            error,
            ReadPathError::MissingPath {
                path: "missing.txt".to_owned(),
            }
        );
    }

    #[test]
    fn reads_across_multiple_prefetch_windows() {
        let mut service = ReadPathService {
            prefetch_window_bytes: 4,
            ..ReadPathService::default()
        };
        let mut staging = StagingCache::default();
        staging.stage("docs/report.txt", b"abcdefghij".to_vec());

        let read = service
            .read(&staging, "docs/report.txt", 1, usize::MAX)
            .expect("multi-window read should succeed");

        assert_eq!(read.bytes, b"bcdefghij");
        assert_eq!(read.source, ReadSource::Stage);
        assert_eq!(read.prefetched_until, 10);

        let cached = service
            .read(&staging, "docs/report.txt", 4, 4)
            .expect("cached read should succeed");
        assert_eq!(cached.bytes, b"efgh");
        assert_eq!(cached.source, ReadSource::Cache);
    }
}
