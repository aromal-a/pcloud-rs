#![allow(clippy::pedantic)]
//! Property tests for the sync-root canonicalization classifier and the
//! static `PublicLinkPathResolver` state transitions.
//!
//! These tests use only the public daemon/proto surface.

// **PLATFORM:** all
// **GATING:** none (portable).

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use pcloud_daemon::public_link_backend::{StaticPublicLinkPathResolver, UnregisteredPathResolver};
use pcloud_daemon::sync_backend::{
    FolderSyncabilityIssue, classify_folder_syncability_with_lists as classify_folder_syncability,
};
use pcloud_proto::public_links_api::PublicLinkPathResolver;
use proptest::prelude::*;

static TEST_ID: AtomicU64 = AtomicU64::new(0);

fn unique_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seq = TEST_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "pcloud-daemon-proptest-{label}-{}-{nonce}-{seq}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temp root should be created");
    path
}

#[test]
fn classify_missing_path_returns_path_does_not_exist() {
    let root = unique_root("missing");
    let candidate = root.join("does-not-exist");
    let err = classify_folder_syncability(&candidate, &[], &[], &[])
        .expect_err("missing path must be rejected");
    assert!(matches!(err, FolderSyncabilityIssue::PathDoesNotExist));
}

#[test]
fn classify_file_returns_not_a_directory() {
    let root = unique_root("file");
    let file = root.join("a-file");
    fs::write(&file, b"x").unwrap();
    let err = classify_folder_syncability(&file, &[], &[], &[]).expect_err("file must be rejected");
    // canonicalize on a file still succeeds, but is_dir should reject it.
    assert!(matches!(err, FolderSyncabilityIssue::PathIsNotADirectory));
}

#[test]
fn classify_duplicate_root_rejects() {
    let root = unique_root("dup");
    let canonical = fs::canonicalize(&root).unwrap();
    let canonical_str = canonical.display().to_string();
    let err = classify_folder_syncability(&root, &[canonical_str.as_str()], &[], &[])
        .expect_err("duplicate must reject");
    assert!(matches!(
        err,
        FolderSyncabilityIssue::AlreadyTrackedAsSyncRoot
    ));
}

#[test]
fn classify_child_rejects_with_overlap() {
    let root = unique_root("child");
    let child = root.join("inner");
    fs::create_dir_all(&child).unwrap();
    let parent_canonical = fs::canonicalize(&root).unwrap().display().to_string();
    let err = classify_folder_syncability(&child, &[parent_canonical.as_str()], &[], &[])
        .expect_err("child must reject");
    assert!(matches!(
        err,
        FolderSyncabilityIssue::OverlapsExistingSyncRoot { .. }
    ));
}

#[test]
fn classify_parent_rejects_with_overlap() {
    let root = unique_root("parent");
    let child = root.join("inner");
    fs::create_dir_all(&child).unwrap();
    let child_canonical = fs::canonicalize(&child).unwrap().display().to_string();
    let err = classify_folder_syncability(&root, &[child_canonical.as_str()], &[], &[])
        .expect_err("parent-of-existing must reject");
    assert!(matches!(
        err,
        FolderSyncabilityIssue::OverlapsExistingSyncRoot { .. }
    ));
}

#[test]
fn classify_inside_mount_rejects() {
    let root = unique_root("mount");
    let mount_canonical = fs::canonicalize(&root).unwrap().display().to_string();
    let err = classify_folder_syncability(&root, &[], &[mount_canonical.as_str()], &[])
        .expect_err("mount must reject");
    assert!(matches!(
        err,
        FolderSyncabilityIssue::InsideMountedPCloudDrive { .. }
    ));
}

#[test]
fn classify_inside_ignored_rejects() {
    let root = unique_root("ignored");
    let ignored_canonical = fs::canonicalize(&root).unwrap().display().to_string();
    let err = classify_folder_syncability(&root, &[], &[], &[ignored_canonical.as_str()])
        .expect_err("ignored must reject");
    assert!(matches!(
        err,
        FolderSyncabilityIssue::InsideIgnoredFolder { .. }
    ));
}

#[test]
fn classify_canonicalization_is_idempotent() {
    let root = unique_root("idem");
    let c1 = classify_folder_syncability(&root, &[], &[], &[]).expect("accepts");
    let c2 = classify_folder_syncability(&c1, &[], &[], &[]).expect("accepts again");
    assert_eq!(c1, c2);
}

proptest! {
    /// Unregistered resolver must ALWAYS error for every input. Never fabricate ids.
    #[test]
    fn prop_unregistered_resolver_never_resolves(path in ".{0,128}") {
        let resolver = UnregisteredPathResolver;
        prop_assert!(resolver.resolve_folder(&path).is_err());
        prop_assert!(resolver.resolve_file(&path).is_err());
    }

    /// Static resolver state transitions:
    /// 1. empty -> lookup fails
    /// 2. insert(path, id) -> lookup returns id
    /// 3. insert(path, id2) overwrites -> lookup returns id2
    /// 4. unrelated paths remain unresolved
    #[test]
    fn prop_static_resolver_state_transitions(
        path in "[a-z]{1,16}",
        other_path in "[A-Z]{1,16}",
        id in any::<u64>(),
        id2 in any::<u64>(),
    ) {
        prop_assume!(path != other_path);
        let mut resolver = StaticPublicLinkPathResolver::new();

        // State 1: empty
        prop_assert!(resolver.resolve_folder(&path).is_err());
        prop_assert!(resolver.resolve_file(&path).is_err());

        // State 2: inserted folder
        resolver.insert_folder(path.clone(), id);
        prop_assert_eq!(resolver.resolve_folder(&path).ok(), Some(id));
        prop_assert!(resolver.resolve_folder(&other_path).is_err());
        // Files remain unresolved when only a folder was registered.
        prop_assert!(resolver.resolve_file(&path).is_err());

        // State 3: overwrite
        resolver.insert_folder(path.clone(), id2);
        prop_assert_eq!(resolver.resolve_folder(&path).ok(), Some(id2));

        // Files are a separate keyspace.
        resolver.insert_file(path.clone(), id);
        prop_assert_eq!(resolver.resolve_file(&path).ok(), Some(id));
        prop_assert_eq!(resolver.resolve_folder(&path).ok(), Some(id2));
    }
}
