#![allow(clippy::pedantic)]
//! Integration tests for `InodeTable` focused on path↔ino round-trip,
//! kernel-forget refcount discipline, and root-inode safety.
//!
//! These run without a FUSE kernel mount and mirror the unit tests already
//! in the `inode.rs` source, but also exercise the behaviours that matter
//! at the integration boundary (saturation on over-forget, root immunity).

use std::sync::Arc;

use pcloud_fs::fuse_adapter::FsEntryKind;
use pcloud_fs::inode::{InodeTable, ROOT_INODE};

#[test]
fn insert_and_lookup_roundtrip() {
    let t = InodeTable::new();
    let (ino, gen1) = t
        .insert_or_get("/docs", FsEntryKind::Directory)
        .expect("insert");
    assert_ne!(ino, ROOT_INODE);
    assert_eq!(gen1, 1);
    assert_eq!(t.ino_for_path("/docs"), Some(ino));
    let (path, gen2, kind) = t.resolve(ino).expect("resolve");
    assert_eq!(path, "/docs");
    assert_eq!(gen2, 1);
    assert_eq!(kind, FsEntryKind::Directory);
    // Idempotent.
    let (ino2, gen3) = t.insert_or_get("/docs", FsEntryKind::Directory).unwrap();
    assert_eq!(ino2, ino);
    assert_eq!(gen3, 1);
}

#[test]
fn forget_at_zero_evicts_entry() {
    let t = InodeTable::new();
    let (ino, _) = t
        .insert_or_get("/victim.txt", FsEntryKind::RegularFile)
        .unwrap();
    t.increment_lookup(ino);
    assert_eq!(t.lookup_count(ino), 1);
    t.forget(ino, 1);
    assert_eq!(t.lookup_count(ino), 0);
    // Once the lookup count reaches zero the entry is evicted.
    assert!(t.resolve(ino).is_none());
    assert!(t.ino_for_path("/victim.txt").is_none());
}

#[test]
fn forget_below_zero_saturates_not_panics() {
    let t = InodeTable::new();
    let (ino, _) = t
        .insert_or_get("/over.txt", FsEntryKind::RegularFile)
        .unwrap();
    t.increment_lookup(ino);
    assert_eq!(t.lookup_count(ino), 1);
    // Forget more than was counted — must not panic, must saturate at 0
    // and evict.
    t.forget(ino, 1_000);
    assert_eq!(t.lookup_count(ino), 0);
    assert!(t.resolve(ino).is_none());
}

#[test]
fn root_inode_is_never_evicted_by_forget() {
    let t = InodeTable::new();
    // Root is pre-allocated.
    assert!(t.resolve(ROOT_INODE).is_some());
    t.increment_lookup(ROOT_INODE);
    t.forget(ROOT_INODE, u64::MAX);
    // Root must still resolve after a pathological forget.
    let (path, _, kind) = t.resolve(ROOT_INODE).expect("root is preserved");
    assert_eq!(path, "/");
    assert_eq!(kind, FsEntryKind::Directory);
}

#[test]
fn invalidate_path_then_reinsert_allocates_new_inode() {
    let t = InodeTable::new();
    let (old, _) = t
        .insert_or_get("/doomed", FsEntryKind::RegularFile)
        .unwrap();
    assert_eq!(t.invalidate_path("/doomed"), Some(old));
    assert!(t.resolve(old).is_none());
    let (new, _) = t
        .insert_or_get("/doomed", FsEntryKind::RegularFile)
        .unwrap();
    assert_ne!(new, old, "new ino must not reuse the evicted slot");
}

#[test]
fn invalidate_root_is_noop() {
    let t = InodeTable::new();
    assert_eq!(t.invalidate_path("/"), None);
    assert_eq!(t.invalidate_ino(ROOT_INODE), None);
    assert!(t.resolve(ROOT_INODE).is_some());
}

#[test]
fn multiple_inserts_allocate_monotonic_inos() {
    let t = InodeTable::new();
    let (a, _) = t.insert_or_get("/a", FsEntryKind::RegularFile).unwrap();
    let (b, _) = t.insert_or_get("/b", FsEntryKind::RegularFile).unwrap();
    let (c, _) = t.insert_or_get("/c", FsEntryKind::RegularFile).unwrap();
    assert!(a < b && b < c, "inos must be strictly monotonic");
    assert_eq!(t.len(), 4); // root + 3
}

#[test]
fn concurrent_inserts_do_not_duplicate_paths() {
    let t = Arc::new(InodeTable::new());
    let mut threads = Vec::new();
    for _ in 0..8 {
        let t = Arc::clone(&t);
        threads.push(std::thread::spawn(move || {
            for i in 0..32 {
                let path = format!("/race/{}", i % 4);
                let (_ino, _gen) = t
                    .insert_or_get(&path, FsEntryKind::RegularFile)
                    .expect("insert");
            }
        }));
    }
    for th in threads {
        th.join().unwrap();
    }
    // Each of the 4 overlapping paths must resolve to exactly one ino.
    for i in 0..4 {
        let path = format!("/race/{i}");
        let ino = t.ino_for_path(&path).expect("path registered");
        let (p2, _, _) = t.resolve(ino).expect("ino resolves");
        assert_eq!(p2, path);
    }
}

#[test]
fn bump_generation_monotonically_increments() {
    let t = InodeTable::new();
    let (ino, g0) = t.insert_or_get("/g", FsEntryKind::RegularFile).unwrap();
    let g1 = t.bump_generation(ino).unwrap();
    let g2 = t.bump_generation(ino).unwrap();
    assert_eq!(g0, 1);
    assert_eq!(g1, 2);
    assert_eq!(g2, 3);
}

#[test]
fn forget_unknown_inode_is_silent_noop() {
    let t = InodeTable::new();
    // Forgetting an ino that was never allocated must not panic or poison.
    t.forget(9_999, 5);
    // Table is still functional.
    let (ino, _) = t.insert_or_get("/after", FsEntryKind::RegularFile).unwrap();
    assert_ne!(ino, 9_999);
}
