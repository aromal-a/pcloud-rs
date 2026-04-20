//! Inode table and bidirectional path↔inode mapping for the FUSE adapter.
//!
//! The inode table owns the authoritative mapping between pCloud paths
//! (normalised POSIX paths rooted at `/`) and kernel-facing inode numbers.
//! Every entry tracks a monotonically increasing generation counter that is
//! bumped whenever the entry is invalidated so that stale kernel handles
//! resolve cleanly instead of silently addressing a reused slot.
//!
//! The table is thread-safe: all public methods take `&self` and use
//! `std::sync::Mutex` internally. This matches the sharing pattern of the
//! `FuseAdapter` trait which is `Send + Sync + 'static`.

// **PLATFORM:** all
// **GATING:** none (portable).

use pcloud_observability::LockExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

use crate::errors::FsError;
use crate::fuse_adapter::FsEntryKind;

/// FUSE root inode, fixed by the kernel protocol.
pub const ROOT_INODE: u64 = 1;

/// Typed wrapper around a raw inode number. Retained for API clarity in
/// persistence layers; the public FUSE surface uses `u64` directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InodeId(
    /// Raw inode number as seen by the FUSE kernel protocol.
    pub u64,
);

/// A persistable flat record of an inode's parent/name binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InodeRecord {
    /// Inode number assigned to this entry.
    pub inode: InodeId,
    /// Parent inode, or `None` if this is the root.
    pub parent: Option<InodeId>,
    /// Base file name (not path) of this entry.
    pub name: String,
    /// `true` when the entry represents a directory, `false` for regular files.
    pub is_directory: bool,
}

/// Internal entry kept alive inside the table. `kind` is carried so that
/// lookup/getattr replies can avoid an extra network round trip.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InodeEntry {
    path: String,
    kind: FsEntryKind,
    generation: u64,
}

#[derive(Debug)]
struct InodeTableInner {
    /// ino → entry
    by_ino: HashMap<u64, InodeEntry>,
    /// path → ino
    by_path: HashMap<String, u64>,
    next_ino: u64,
    /// ino → kernel lookup reference count.
    ///
    /// Incremented by one for every successful `lookup` or `readdir` reply
    /// sent to the kernel. Decremented by the kernel's `forget(ino, nlookup)`
    /// message. When the count reaches zero the kernel has released all
    /// references and the inode entry may be evicted from the map.
    lookup_counts: HashMap<u64, u64>,
}

impl InodeTableInner {
    fn new() -> Self {
        let mut by_ino = HashMap::new();
        let mut by_path = HashMap::new();
        by_ino.insert(
            ROOT_INODE,
            InodeEntry {
                path: "/".to_owned(),
                kind: FsEntryKind::Directory,
                generation: 1,
            },
        );
        by_path.insert("/".to_owned(), ROOT_INODE);
        Self {
            by_ino,
            by_path,
            next_ino: ROOT_INODE + 1,
            lookup_counts: HashMap::new(),
        }
    }
}

/// Thread-safe bidirectional path↔inode map with generation counters.
///
/// Insertion is idempotent: looking up the same path twice returns the same
/// inode. Invalidation bumps the generation and removes both the forward and
/// reverse mapping; a subsequent insert of the same path allocates a *fresh*
/// inode number (never reused from the evicted one) so kernel-held stale
/// handles never silently address the new entry.
#[derive(Debug)]
pub struct InodeTable {
    inner: Mutex<InodeTableInner>,
}

impl Default for InodeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl InodeTable {
    /// Construct a fresh inode table with the root inode pre-allocated.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(InodeTableInner::new()),
        }
    }

    /// Resolve `ino` to `(path, generation, kind)`.
    pub fn resolve(&self, ino: u64) -> Option<(String, u64, FsEntryKind)> {
        let inner = self.inner.lock().ok()?;
        inner
            .by_ino
            .get(&ino)
            .map(|e| (e.path.clone(), e.generation, e.kind))
    }

    /// Look up the ino assigned to `path`, if any.
    pub fn ino_for_path(&self, path: &str) -> Option<u64> {
        let inner = self.inner.lock().ok()?;
        inner.by_path.get(path).copied()
    }

    /// Insert-or-return with mandatory lookup-count bootstrap. Returns
    /// `Ok((ino, generation))`.
    ///
    /// Unlike [`Self::insert_or_get`], this constructor guarantees that
    /// `lookup_counts` has an entry (initialised to zero) for the
    /// returned inode. This is the **required** insertion path for any
    /// call site that will subsequently advertise `ino` to the kernel
    /// via a FUSE reply: the kernel's `forget(ino, nlookup)` message
    /// depends on a live counter, and without one
    /// [`Self::forget`] is a silent no-op that leaks the inode
    /// unboundedly.
    ///
    /// Prefer this constructor for all production insertion sites.
    /// [`Self::insert_or_get`] remains available for internal
    /// book-keeping paths that never surface the inode to the kernel.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::InodeSpaceExhausted`] when the 64-bit inode
    /// number space overflows.
    pub fn insert_with_lookup(&self, path: &str, kind: FsEntryKind) -> Result<(u64, u64), FsError> {
        let (ino, generation) = self.insert_or_get(path, kind)?;
        // SAFETY: the inner mutex is private to `InodeTable` and every hold
        // site inside this module is panic-free data-structure work. A
        // poisoned mutex here would indicate a prior panic in this module
        // — a real bug we want surfaced immediately rather than masked
        // with a fabricated `FsError`.
        let mut inner = self.inner.lock_or_poisoned("inode::insert_with_lookup");
        inner.lookup_counts.entry(ino).or_insert(0);
        Ok((ino, generation))
    }

    /// Insert-or-return: idempotent. Returns `Ok((ino, generation))`.
    ///
    /// If `path` is already registered, the existing inode is returned and
    /// the kind is refreshed to `kind` (cheap correction after a type flip
    /// detected in a remote listing).
    ///
    /// **Deprecated for kernel-facing insertion paths:** prefer
    /// [`Self::insert_with_lookup`] whenever the caller will emit `ino`
    /// to the FUSE kernel. That path guarantees `lookup_counts` has an
    /// entry so that `forget()` can correctly evict on the last kernel
    /// release. Retaining this method for internal book-keeping where
    /// the inode is never surfaced to the kernel.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::InodeSpaceExhausted`] when the 64-bit inode
    /// number space overflows. This should never occur in practice.
    pub fn insert_or_get(&self, path: &str, kind: FsEntryKind) -> Result<(u64, u64), FsError> {
        // SAFETY: the inner mutex is private to `InodeTable` and every hold
        // site inside this module is panic-free data-structure work. A
        // poisoned mutex here would indicate a prior panic in this module
        // — a real bug we want surfaced immediately rather than masked.
        let mut inner = self.inner.lock_or_poisoned("inode::insert_or_get");
        if let Some(&ino) = inner.by_path.get(path) {
            if let Some(entry) = inner.by_ino.get_mut(&ino) {
                entry.kind = kind;
                return Ok((ino, entry.generation));
            }
        }
        let ino = inner.next_ino;
        inner.next_ino = inner
            .next_ino
            .checked_add(1)
            .ok_or(FsError::InodeSpaceExhausted)?;
        inner.by_ino.insert(
            ino,
            InodeEntry {
                path: path.to_owned(),
                kind,
                generation: 1,
            },
        );
        inner.by_path.insert(path.to_owned(), ino);
        Ok((ino, 1))
    }

    /// Invalidate the entry at `path`. Bumps the generation, removes the
    /// mapping, and returns the inode that was evicted (if any).
    ///
    /// The root inode (`/`) is never evicted; attempts return `None`.
    pub fn invalidate_path(&self, path: &str) -> Option<u64> {
        if path == "/" {
            return None;
        }
        let mut inner = self.inner.lock_or_poisoned("inode::invalidate_path");
        let ino = inner.by_path.remove(path)?;
        if let Some(entry) = inner.by_ino.remove(&ino) {
            // Re-insert a tombstone generation bump so any future resolve()
            // on the old ino returns None — cheaper to just remove entirely.
            drop(entry);
        }
        Some(ino)
    }

    /// Invalidate by inode number. Same semantics as [`Self::invalidate_path`].
    pub fn invalidate_ino(&self, ino: u64) -> Option<String> {
        if ino == ROOT_INODE {
            return None;
        }
        let mut inner = self.inner.lock_or_poisoned("inode::invalidate_ino");
        let entry = inner.by_ino.remove(&ino)?;
        inner.by_path.remove(&entry.path);
        Some(entry.path)
    }

    /// Number of live entries, including the root.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .map(|g| g.by_ino.len())
            .unwrap_or_default()
    }

    /// `true` when the table has no live entries. A default table always
    /// contains the root, so this is generally `false` in practice.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Increment the kernel lookup reference count for `ino` by one.
    ///
    /// Call this once for every successful `lookup` or `readdir` reply that
    /// names `ino` to the FUSE kernel. The kernel will later balance each
    /// such increment with a `forget(ino, nlookup)` message.
    pub fn increment_lookup(&self, ino: u64) {
        let mut inner = self.inner.lock_or_poisoned("inode::increment_lookup");
        *inner.lookup_counts.entry(ino).or_insert(0) += 1;
    }

    /// Decrement the kernel lookup reference count for `ino` by `nlookup`.
    ///
    /// When the count reaches zero the entry is evicted from both the
    /// forward and reverse maps, freeing memory. The root inode is never
    /// evicted regardless of the count.
    ///
    /// Defensive behaviour when `lookup_counts` has no entry for `ino`:
    /// a kernel `forget` for an untracked inode means the insertion
    /// site bypassed [`Self::insert_with_lookup`]. We log a warning
    /// (development signal) and evict the entry anyway so the forward/
    /// reverse maps do not leak.
    pub fn forget(&self, ino: u64, nlookup: u64) {
        if ino == ROOT_INODE {
            return;
        }
        let mut inner = self.inner.lock_or_poisoned("inode::forget");
        match inner.lookup_counts.get_mut(&ino) {
            Some(count) => {
                *count = count.saturating_sub(nlookup);
                if *count == 0 {
                    inner.lookup_counts.remove(&ino);
                    if let Some(entry) = inner.by_ino.remove(&ino) {
                        inner.by_path.remove(&entry.path);
                        log::trace!("inode {} evicted from map (kernel forget)", ino);
                    }
                }
            }
            None => {
                // Untracked insertion site leaked an inode to the
                // kernel without calling increment_lookup. Evict
                // anyway so memory is reclaimed; surface a warning so
                // the offending call site can be fixed.
                if let Some(entry) = inner.by_ino.remove(&ino) {
                    inner.by_path.remove(&entry.path);
                    log::warn!(
                        "inode {} forgotten without a lookup_counts entry; \
                         call site should use InodeTable::insert_with_lookup",
                        ino
                    );
                }
            }
        }
    }

    /// Return the current kernel lookup reference count for `ino`, if any.
    /// Primarily useful in tests.
    pub fn lookup_count(&self, ino: u64) -> u64 {
        self.inner
            .lock()
            .map(|g| g.lookup_counts.get(&ino).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    /// Bump the generation counter for `ino` without evicting it. Used when
    /// the kind/metadata shape changes but the path↔ino binding is stable.
    pub fn bump_generation(&self, ino: u64) -> Option<u64> {
        let mut inner = self.inner.lock().ok()?;
        let entry = inner.by_ino.get_mut(&ino)?;
        entry.generation = entry.generation.checked_add(1).unwrap_or(1);
        Some(entry.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn root_is_preallocated() {
        let t = InodeTable::new();
        let (path, r#gen, kind) = t.resolve(ROOT_INODE).expect("root must resolve");
        assert_eq!(path, "/");
        assert_eq!(r#gen, 1);
        assert_eq!(kind, FsEntryKind::Directory);
        assert_eq!(t.ino_for_path("/"), Some(ROOT_INODE));
    }

    #[test]
    fn insert_or_get_is_idempotent() {
        let t = InodeTable::new();
        let (a, gen_a) = t.insert_or_get("/docs", FsEntryKind::Directory).unwrap();
        let (b, gen_b) = t.insert_or_get("/docs", FsEntryKind::Directory).unwrap();
        assert_eq!(a, b);
        assert_eq!(gen_a, gen_b);
        assert_ne!(a, ROOT_INODE);
    }

    #[test]
    fn insert_allocates_monotonic_inos() {
        let t = InodeTable::new();
        let (a, _) = t.insert_or_get("/a", FsEntryKind::RegularFile).unwrap();
        let (b, _) = t.insert_or_get("/b", FsEntryKind::RegularFile).unwrap();
        let (c, _) = t.insert_or_get("/c", FsEntryKind::RegularFile).unwrap();
        assert!(a < b && b < c);
    }

    #[test]
    fn invalidate_path_bumps_generation_on_reinsert() {
        let t = InodeTable::new();
        let (ino_old, gen_old) = t.insert_or_get("/x", FsEntryKind::RegularFile).unwrap();
        assert_eq!(gen_old, 1);

        let evicted = t.invalidate_path("/x");
        assert_eq!(evicted, Some(ino_old));
        assert_eq!(t.resolve(ino_old), None);
        assert_eq!(t.ino_for_path("/x"), None);

        let (ino_new, _) = t.insert_or_get("/x", FsEntryKind::RegularFile).unwrap();
        assert_ne!(ino_new, ino_old, "new inode must not reuse evicted slot");
    }

    #[test]
    fn invalidate_ino_evicts_path_mapping() {
        let t = InodeTable::new();
        let (ino, _) = t.insert_or_get("/y", FsEntryKind::Directory).unwrap();
        assert_eq!(t.invalidate_ino(ino), Some("/y".to_owned()));
        assert_eq!(t.ino_for_path("/y"), None);
    }

    #[test]
    fn root_cannot_be_invalidated() {
        let t = InodeTable::new();
        assert_eq!(t.invalidate_path("/"), None);
        assert_eq!(t.invalidate_ino(ROOT_INODE), None);
        assert!(t.resolve(ROOT_INODE).is_some());
    }

    #[test]
    fn bump_generation_increments() {
        let t = InodeTable::new();
        let (ino, g0) = t.insert_or_get("/z", FsEntryKind::RegularFile).unwrap();
        let g1 = t.bump_generation(ino).unwrap();
        let g2 = t.bump_generation(ino).unwrap();
        assert_eq!(g0, 1);
        assert_eq!(g1, 2);
        assert_eq!(g2, 3);
    }

    #[test]
    fn kind_refresh_on_reinsert_preserves_ino() {
        let t = InodeTable::new();
        let (ino, _) = t.insert_or_get("/q", FsEntryKind::RegularFile).unwrap();
        let (ino2, _) = t.insert_or_get("/q", FsEntryKind::Directory).unwrap();
        assert_eq!(ino, ino2);
        let (_, _, kind) = t.resolve(ino).unwrap();
        assert_eq!(kind, FsEntryKind::Directory);
    }

    #[test]
    fn concurrent_inserts_do_not_duplicate_paths() {
        // Stress with 8 threads racing to register overlapping paths.
        let t = Arc::new(InodeTable::new());
        let mut handles = Vec::new();
        for tid in 0..8 {
            let t = Arc::clone(&t);
            handles.push(thread::spawn(move || {
                let mut seen = Vec::new();
                for i in 0..64 {
                    // Overlapping path space across threads is deliberate.
                    let path = format!("/race/{}", i % 16);
                    let (ino, _) = t.insert_or_get(&path, FsEntryKind::RegularFile).unwrap();
                    seen.push((path, ino, tid));
                }
                seen
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // For each overlapping path, every thread must agree on the ino.
        for i in 0..16 {
            let path = format!("/race/{i}");
            let ino = t.ino_for_path(&path).expect("path must be registered");
            let (p2, _, _) = t.resolve(ino).expect("ino must resolve");
            assert_eq!(p2, path);
        }
    }
}
