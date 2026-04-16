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

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

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

    /// Insert-or-return: idempotent. Returns `(ino, generation)`.
    ///
    /// If `path` is already registered, the existing inode is returned and
    /// the kind is refreshed to `kind` (cheap correction after a type flip
    /// detected in a remote listing).
    pub fn insert_or_get(&self, path: &str, kind: FsEntryKind) -> (u64, u64) {
        let mut inner = self
            .inner
            .lock()
            .expect("inode table mutex must not be poisoned");
        if let Some(&ino) = inner.by_path.get(path) {
            if let Some(entry) = inner.by_ino.get_mut(&ino) {
                entry.kind = kind;
                return (ino, entry.generation);
            }
        }
        let ino = inner.next_ino;
        inner.next_ino = inner
            .next_ino
            .checked_add(1)
            .expect("inode number space exhausted");
        inner.by_ino.insert(
            ino,
            InodeEntry {
                path: path.to_owned(),
                kind,
                generation: 1,
            },
        );
        inner.by_path.insert(path.to_owned(), ino);
        (ino, 1)
    }

    /// Invalidate the entry at `path`. Bumps the generation, removes the
    /// mapping, and returns the inode that was evicted (if any).
    ///
    /// The root inode (`/`) is never evicted; attempts return `None`.
    pub fn invalidate_path(&self, path: &str) -> Option<u64> {
        if path == "/" {
            return None;
        }
        let mut inner = self
            .inner
            .lock()
            .expect("inode table mutex must not be poisoned");
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
        let mut inner = self
            .inner
            .lock()
            .expect("inode table mutex must not be poisoned");
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
        let (a, gen_a) = t.insert_or_get("/docs", FsEntryKind::Directory);
        let (b, gen_b) = t.insert_or_get("/docs", FsEntryKind::Directory);
        assert_eq!(a, b);
        assert_eq!(gen_a, gen_b);
        assert_ne!(a, ROOT_INODE);
    }

    #[test]
    fn insert_allocates_monotonic_inos() {
        let t = InodeTable::new();
        let (a, _) = t.insert_or_get("/a", FsEntryKind::RegularFile);
        let (b, _) = t.insert_or_get("/b", FsEntryKind::RegularFile);
        let (c, _) = t.insert_or_get("/c", FsEntryKind::RegularFile);
        assert!(a < b && b < c);
    }

    #[test]
    fn invalidate_path_bumps_generation_on_reinsert() {
        let t = InodeTable::new();
        let (ino_old, gen_old) = t.insert_or_get("/x", FsEntryKind::RegularFile);
        assert_eq!(gen_old, 1);

        let evicted = t.invalidate_path("/x");
        assert_eq!(evicted, Some(ino_old));
        assert_eq!(t.resolve(ino_old), None);
        assert_eq!(t.ino_for_path("/x"), None);

        let (ino_new, _) = t.insert_or_get("/x", FsEntryKind::RegularFile);
        assert_ne!(ino_new, ino_old, "new inode must not reuse evicted slot");
    }

    #[test]
    fn invalidate_ino_evicts_path_mapping() {
        let t = InodeTable::new();
        let (ino, _) = t.insert_or_get("/y", FsEntryKind::Directory);
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
        let (ino, g0) = t.insert_or_get("/z", FsEntryKind::RegularFile);
        let g1 = t.bump_generation(ino).unwrap();
        let g2 = t.bump_generation(ino).unwrap();
        assert_eq!(g0, 1);
        assert_eq!(g1, 2);
        assert_eq!(g2, 3);
    }

    #[test]
    fn kind_refresh_on_reinsert_preserves_ino() {
        let t = InodeTable::new();
        let (ino, _) = t.insert_or_get("/q", FsEntryKind::RegularFile);
        let (ino2, _) = t.insert_or_get("/q", FsEntryKind::Directory);
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
                    let (ino, _) = t.insert_or_get(&path, FsEntryKind::RegularFile);
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
