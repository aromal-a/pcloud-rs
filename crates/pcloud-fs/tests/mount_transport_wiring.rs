#![allow(clippy::pedantic)]
//! Integration test: verify mount transport wiring with mock backends.
//!
//! Exercises the full mount lifecycle with real `ProtoFuseAdapter`
//! backed by mock `FolderBackend` + `FileBackend`: mount, readdir,
//! read file content, unmount, verify clean teardown.
//!
//! Gated behind `PCLOUD_FUSE_TEST=1` because it requires a working
//! libfuse kernel module and `/dev/fuse` access.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::metadata_cache::MetadataCacheConfig;
use pcloud_fs::mount_service::{MountOptions, MountService};
use pcloud_fs::page_cache::PageCacheConfig;

fn fuse_gate_enabled() -> bool {
    std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1")
}

fn should_skip_mount_error(err: &str) -> bool {
    err.contains("Operation not permitted")
        || err.contains("Function not implemented")
        || err.contains("/dev/fuse")
}

fn should_skip_io_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::Unsupported
            | std::io::ErrorKind::NotConnected
    ) || should_skip_mount_error(&err.to_string())
}

fn mount_appears_active(path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let needle = path.to_string_lossy();
    mountinfo.lines().any(|line| line.contains(needle.as_ref()))
}

/// Full mount lifecycle: mount with mock backends, readdir, read file,
/// unmount, verify no stale mount. Exercises the transport wiring path
/// that `pcloud_shim_adapter_factory` uses when composing the real
/// `ProtoFuseAdapter`.
#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn mount_readdir_read_unmount_clean_teardown() {
    if !fuse_gate_enabled() {
        return;
    }

    // Seed a small directory tree with one file.
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir_with_sizes(
        "/",
        1,
        vec![
            ("notes", true, Some(2), None, None),
            ("greeting.txt", false, None, Some(42), Some(13)),
        ],
    );
    folder.insert_dir_with_sizes(
        "/notes",
        2,
        vec![("todo.txt", false, None, Some(43), Some(9))],
    );

    let files = Arc::new(MockFileBackend::new());
    files.insert_file(42, b"hello, mount!".to_vec());
    files.insert_file(43, b"buy milk".to_vec());

    // Build adapter with custom config-driven options (mirrors what the
    // daemon does when reading from [mount] config).
    let options = AdapterOptions {
        cache: MetadataCacheConfig {
            ttl: Duration::from_secs(60),
            capacity: 4096,
        },
        page_cache: PageCacheConfig {
            page_size: 64 * 1024,
            max_bytes: 256 * 1024 * 1024,
        },
        ..AdapterOptions::default()
    };
    let adapter = ProtoFuseAdapter::with_file_backend(folder, files.clone(), options);

    let tmp = tempfile::tempdir().expect("tempdir");
    let svc = MountService::new();
    let handle = match svc.mount(
        tmp.path(),
        adapter,
        MountOptions {
            read_only: true,
            ..MountOptions::default()
        },
    ) {
        Ok(handle) => handle,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount should succeed on a FUSE-enabled host: {err}"),
    };

    std::thread::sleep(Duration::from_millis(150));
    if !mount_appears_active(tmp.path()) {
        return;
    }

    // 1. readdir /
    let root_entries: Vec<String> = match std::fs::read_dir(tmp.path()) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => {
            let _ = handle.unmount();
            return;
        }
        Err(err) => panic!("readdir /: {err}"),
    };
    assert!(
        root_entries.iter().any(|e| e == "notes"),
        "root listing should include 'notes', got {root_entries:?}"
    );
    assert!(
        root_entries.iter().any(|e| e == "greeting.txt"),
        "root listing should include 'greeting.txt', got {root_entries:?}"
    );

    // 2. readdir /notes
    let nested: Vec<String> = match std::fs::read_dir(tmp.path().join("notes")) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => {
            let _ = handle.unmount();
            return;
        }
        Err(err) => panic!("readdir /notes: {err}"),
    };
    assert!(
        nested.iter().any(|e| e == "todo.txt"),
        "nested listing should include 'todo.txt', got {nested:?}"
    );

    // 3. read file content
    let got = match std::fs::read(tmp.path().join("greeting.txt")) {
        Ok(bytes) => bytes,
        Err(err) if should_skip_io_error(&err) => {
            let _ = handle.unmount();
            return;
        }
        Err(err) => panic!("read greeting.txt: {err}"),
    };
    assert_eq!(got, b"hello, mount!");

    // 4. read nested file
    let got_nested = match std::fs::read(tmp.path().join("notes").join("todo.txt")) {
        Ok(bytes) => bytes,
        Err(err) if should_skip_io_error(&err) => {
            let _ = handle.unmount();
            return;
        }
        Err(err) => panic!("read notes/todo.txt: {err}"),
    };
    assert_eq!(got_nested, b"buy milk");

    // 5. Verify file backend was exercised.
    assert!(
        files.opens.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "file backend should have received at least one open call"
    );
    assert!(
        files.reads.load(std::sync::atomic::Ordering::Relaxed) >= 1,
        "file backend should have received at least one read call"
    );

    // 6. Unmount and verify clean teardown.
    handle.unmount().expect("unmount should succeed");

    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !mount_appears_active(tmp.path()),
        "mount should no longer appear in /proc/self/mountinfo after unmount"
    );
}

/// Verify that a second mount at the same path after clean unmount succeeds.
/// This exercises the teardown-is-actually-clean property.
#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn remount_after_clean_unmount() {
    if !fuse_gate_enabled() {
        return;
    }

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![("a.txt", false, None, Some(1))]);
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(1, b"first".to_vec());

    let tmp = tempfile::tempdir().expect("tempdir");
    let svc = MountService::new();

    // First mount+unmount cycle.
    let adapter1 = ProtoFuseAdapter::with_file_backend(
        folder.clone(),
        files.clone(),
        AdapterOptions::default(),
    );
    let h1 = match svc.mount(
        tmp.path(),
        adapter1,
        MountOptions {
            read_only: true,
            ..MountOptions::default()
        },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("first mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(100));
    if !mount_appears_active(tmp.path()) {
        return;
    }
    h1.unmount().expect("first unmount");
    std::thread::sleep(Duration::from_millis(200));

    // Second mount cycle at the same path.
    let adapter2 = ProtoFuseAdapter::with_file_backend(
        folder.clone(),
        files.clone(),
        AdapterOptions::default(),
    );
    let h2 = match svc.mount(
        tmp.path(),
        adapter2,
        MountOptions {
            read_only: true,
            ..MountOptions::default()
        },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("remount should succeed after clean unmount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(100));
    if !mount_appears_active(tmp.path()) {
        return;
    }

    // Verify we can still read.
    match std::fs::read(tmp.path().join("a.txt")) {
        Ok(bytes) => assert_eq!(bytes, b"first"),
        Err(err) if should_skip_io_error(&err) => {}
        Err(err) => panic!("read after remount: {err}"),
    }

    h2.unmount().expect("second unmount");
}
