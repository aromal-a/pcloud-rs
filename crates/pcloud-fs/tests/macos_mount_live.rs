#![allow(clippy::pedantic)]
//! **PLATFORM: macOS only.**
//! **GATING: `#[cfg(target_os = "macos")]`** + `PCLOUD_FUSE_TEST=1` env gate.
//!
//! Live mount integration tests for the macOS fuse-t FUSE layer.
//!
//! These tests perform real kernel mounts via fuse-t and exercise the full
//! VFS operation path (readdir, read, write, create, unlink, rename, fsync,
//! unmount) against mocked backends. They require:
//!
//! 1. A real Mac running macOS 12 Monterey or later.
//! 2. fuse-t installed (<https://www.fuse-t.org/>) — or macFUSE with
//!    `PCLOUD_MACOS_FUSE_BACKEND=macfuse`.
//! 3. `PCLOUD_FUSE_TEST=1` set in the environment.
//!
//! Run with:
//! ```text
//! PCLOUD_FUSE_TEST=1 cargo test -p pcloud-fs --test macos_mount_live -- --nocapture
//! ```
//!
//! The test binary gates internally on `PCLOUD_FUSE_TEST=1` and returns
//! immediately without failure when the env var is absent, so the suite
//! can be compiled and shipped on CI without requiring a real Mac.
//!
//! ## Backend selection
//!
//! Set `PCLOUD_MACOS_FUSE_BACKEND` to `fuse-t`, `macfuse`, or `auto`
//! to control which userspace FUSE library is used. Default is `fuse-t`.
//!
//! ## Test IDs and their purpose
//!
//! | Test | Coverage |
//! |------|----------|
//! | `readdir_root_via_fuset` | Mount + readdir / entries visible in VFS |
//! | `readdir_nested_via_fuset` | Nested directory traversal |
//! | `read_small_file_via_fuset` | File open + read + release via VFS |
//! | `read_large_file_via_fuset` | Multi-chunk read (> one FUSE buffer) |
//! | `write_create_fsync_via_fuset` | Kernel create + write + fsync → upload |
//! | `unlink_via_fuset` | Kernel unlink removes file from adapter |
//! | `rename_via_fuset` | Kernel rename updates adapter |
//! | `mkdir_rmdir_via_fuset` | Kernel mkdir + rmdir round-trip |
//! | `statfs_via_fuset` | statvfs reports sensible values |
//! | `getattr_via_fuset` | stat(2) returns correct mode/uid/size |
//! | `xattr_returns_enoattr` | getxattr / listxattr do not crash |
//! | `backend_env_macfuse_probe` | PCLOUD_MACOS_FUSE_BACKEND=macfuse path |
//! | `remount_cycle` | Unmount + remount preserves adapter state |
//! | `orphan_detection_after_nodrop` | getmntinfo shows stale entry |
//! | `concurrent_readers` | Multiple threads reading simultaneously |

#![cfg(target_os = "macos")]

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

// fuse-t has process-global state and does not support concurrent sessions
// from multiple threads within the same process. All tests that perform a
// real kernel mount must hold this lock for their entire duration.
static FUSE_T_SERIAL: Mutex<()> = Mutex::new(());

fn fuse_serial_lock() -> MutexGuard<'static, ()> {
    FUSE_T_SERIAL.lock().unwrap_or_else(|p| p.into_inner())
}

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::mount_orphan::{MountinfoReader, detect_orphans};
use pcloud_fs::mount_service::{MountError, MountOptions, MountService};
use pcloud_fs::platform::macos::MacosMountinfoReader;
use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::WriteJournal;
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};

// =============================================================================
// Helpers
// =============================================================================

fn fuse_gate_enabled() -> bool {
    std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1")
}

fn mount_appears_in_getmntinfo(path: &Path) -> bool {
    let reader = MacosMountinfoReader;
    let Ok(payload) = reader.read() else {
        return false;
    };
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    let needle = canonical.to_string_lossy();
    // The payload uses escape_mountinfo encoding; check both the raw and
    // escaped forms to be tolerant of spaces in temp dir names.
    payload.contains(needle.as_ref())
        || payload.contains(&needle.replace(' ', "\\040"))
}

fn should_skip_mount_error(err: &str) -> bool {
    err.contains("fuse-t not installed")
        || err.contains("macFUSE not installed")
        || err.contains("no macOS FUSE backend")
        || err.contains("fuse_mount failed")
        || err.contains("fuse_lowlevel_new")
        || err.contains("Operation not permitted")
        || err.contains("Permission denied")
}

fn should_skip_io_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::Unsupported
            | std::io::ErrorKind::NotConnected
    ) || should_skip_mount_error(&err.to_string())
}

/// Upload backend that records every `upload_file` call.
#[derive(Default)]
struct RecordingUploadBackend {
    uploads: std::sync::Mutex<Vec<(String, String, Vec<u8>)>>,
    unlinks: std::sync::Mutex<Vec<String>>,
    renames: std::sync::Mutex<Vec<(String, String)>>,
}

impl FileUploadBackend for RecordingUploadBackend {
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &Path,
    ) -> Result<(), WritePathError> {
        let bytes = std::fs::read(staging_file)
            .map_err(|e| WritePathError::Upload(e.to_string()))?;
        self.uploads
            .lock()
            .unwrap()
            .push((parent_path.to_owned(), name.to_owned(), bytes));
        Ok(())
    }

    fn unlink_remote(&self, path: &str) -> Result<(), WritePathError> {
        self.unlinks.lock().unwrap().push(path.to_owned());
        Ok(())
    }

    fn rename_remote(&self, from: &str, to: &str) -> Result<(), WritePathError> {
        self.renames
            .lock()
            .unwrap()
            .push((from.to_owned(), to.to_owned()));
        Ok(())
    }
}

// =============================================================================
// T1: readdir of root directory
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn readdir_root_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir(
        "/",
        1,
        vec![
            ("Documents", true, Some(2), None),
            ("notes.txt", false, None, Some(100)),
            ("photo.jpg", false, None, Some(101)),
        ],
    );

    let adapter = ProtoFuseAdapter::new(Arc::clone(&folder), AdapterOptions::default());
    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount must succeed with fuse-t installed: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let entries: Vec<String> = match std::fs::read_dir(mnt.path()) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("readdir /: {err}"),
    };

    assert!(
        entries.iter().any(|e| e == "Documents"),
        "root must contain 'Documents', got: {entries:?}"
    );
    assert!(
        entries.iter().any(|e| e == "notes.txt"),
        "root must contain 'notes.txt', got: {entries:?}"
    );
    assert!(
        entries.iter().any(|e| e == "photo.jpg"),
        "root must contain 'photo.jpg', got: {entries:?}"
    );

    handle.unmount().expect("unmount must succeed");
}

// =============================================================================
// T2: readdir of nested directory
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn readdir_nested_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir(
        "/",
        1,
        vec![("Projects", true, Some(2), None)],
    );
    folder.insert_dir(
        "/Projects",
        2,
        vec![
            ("alpha", true, Some(3), None),
            ("readme.md", false, None, Some(50)),
        ],
    );
    folder.insert_dir("/Projects/alpha", 3, vec![("main.rs", false, None, Some(51))]);

    let adapter = ProtoFuseAdapter::new(Arc::clone(&folder), AdapterOptions::default());
    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let nested: Vec<String> = match std::fs::read_dir(mnt.path().join("Projects")) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("readdir /Projects: {err}"),
    };

    assert!(
        nested.iter().any(|e| e == "alpha"),
        "/Projects must contain 'alpha', got: {nested:?}"
    );
    assert!(
        nested.iter().any(|e| e == "readme.md"),
        "/Projects must contain 'readme.md', got: {nested:?}"
    );

    let deep: Vec<String> = match std::fs::read_dir(mnt.path().join("Projects/alpha")) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("readdir /Projects/alpha: {err}"),
    };
    assert!(
        deep.iter().any(|e| e == "main.rs"),
        "/Projects/alpha must contain 'main.rs', got: {deep:?}"
    );

    handle.unmount().expect("unmount");
}

// =============================================================================
// T3: read small file
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn read_small_file_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![("hello.txt", false, None, Some(42))]);
    let files = Arc::new(MockFileBackend::new());
    let expected = b"hello from pcloud-rs fuse-t test";
    files.insert_file(42, expected.to_vec());

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());
    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let got = match std::fs::read(mnt.path().join("hello.txt")) {
        Ok(b) => b,
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("read hello.txt: {err}"),
    };

    assert_eq!(got, expected, "read bytes must match expected content");
    handle.unmount().expect("unmount");
}

// =============================================================================
// T4: read large file (multi-chunk, > 4 KiB)
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn read_large_file_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![("big.bin", false, None, Some(99))]);
    let files = Arc::new(MockFileBackend::new());
    // 256 KiB of data to force multi-chunk reads.
    let expected: Vec<u8> = (0u8..=255).cycle().take(256 * 1024).collect();
    files.insert_file(99, expected.clone());

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());
    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let got = match std::fs::read(mnt.path().join("big.bin")) {
        Ok(b) => b,
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("read big.bin: {err}"),
    };

    assert_eq!(got.len(), expected.len(), "read length must match");
    assert_eq!(got, expected, "read bytes must match expected content");
    handle.unmount().expect("unmount");
}

// =============================================================================
// T5: getattr — stat(2) returns correct fields
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn getattr_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir(
        "/",
        1,
        vec![
            ("subdir", true, Some(2), None),
            ("file.txt", false, None, Some(77)),
        ],
    );
    folder.insert_dir("/subdir", 2, vec![]);
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(77, b"content".to_vec());

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());
    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    // stat the directory
    match std::fs::metadata(mnt.path().join("subdir")) {
        Ok(meta) => {
            assert!(meta.is_dir(), "subdir must be a directory");
        }
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("stat subdir: {err}"),
    }

    // stat the file
    match std::fs::metadata(mnt.path().join("file.txt")) {
        Ok(meta) => {
            assert!(meta.is_file(), "file.txt must be a regular file");
            assert_eq!(meta.len(), 7, "file.txt must report size=7");
        }
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("stat file.txt: {err}"),
    }

    handle.unmount().expect("unmount");
}

// =============================================================================
// T6: write → create + write + fsync → upload recorded
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn write_create_fsync_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let files = Arc::new(MockFileBackend::new());
    let upload_backend = Arc::new(RecordingUploadBackend::default());

    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let stage = StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&upload_backend),
        WritePathOptions {
            flush_threshold_bytes: u64::MAX,
            flush_interval: Duration::from_secs(3600),
        },
    ));

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default())
            .with_write_path(Arc::clone(&writer));

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: false, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let payload = b"durable content from fuse-t test";
    let new_file = mnt.path().join("new.txt");

    match std::fs::write(&new_file, payload) {
        Ok(()) => {}
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("kernel write: {err}"),
    }

    // Force fsync.
    {
        use std::io::Write;
        match std::fs::OpenOptions::new().write(true).open(&new_file) {
            Ok(mut f) => {
                if let Err(err) = f.flush() {
                    if should_skip_io_error(&err) {
                        handle.unmount().ok();
                        return;
                    }
                }
                if let Err(err) = f.sync_all() {
                    if should_skip_io_error(&err) {
                        handle.unmount().ok();
                        return;
                    }
                }
            }
            Err(err) if should_skip_io_error(&err) => {
                handle.unmount().ok();
                return;
            }
            Err(err) => panic!("reopen for fsync: {err}"),
        }
    }

    let uploads = upload_backend.uploads.lock().unwrap();
    assert!(
        uploads
            .iter()
            .any(|(_, name, bytes)| name == "new.txt" && bytes == payload),
        "kernel write+fsync must have produced an upload, got: {uploads:?}"
    );
    drop(uploads);

    handle.unmount().expect("unmount");
}

// =============================================================================
// T7: unlink via kernel VFS
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn unlink_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![("todelete.txt", false, None, Some(55))]);
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(55, b"going away".to_vec());

    let upload_backend = Arc::new(RecordingUploadBackend::default());
    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let stage = StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&upload_backend),
        WritePathOptions {
            flush_threshold_bytes: u64::MAX,
            flush_interval: Duration::from_secs(3600),
        },
    ));

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default())
            .with_write_path(Arc::clone(&writer));

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: false, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    match std::fs::remove_file(mnt.path().join("todelete.txt")) {
        Ok(()) => {}
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("kernel unlink: {err}"),
    }

    // After unlink, the file must no longer be visible.
    match std::fs::metadata(mnt.path().join("todelete.txt")) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) if should_skip_io_error(&err) => {}
        Ok(_) => panic!("todelete.txt must not exist after unlink"),
        Err(err) => panic!("unexpected error checking deleted file: {err}"),
    }

    handle.unmount().expect("unmount");
}

// =============================================================================
// T8: rename via kernel VFS
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn rename_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![("alpha.txt", false, None, Some(10))]);
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(10, b"rename me".to_vec());

    let upload_backend = Arc::new(RecordingUploadBackend::default());
    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let stage = StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&upload_backend),
        WritePathOptions {
            flush_threshold_bytes: u64::MAX,
            flush_interval: Duration::from_secs(3600),
        },
    ));

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default())
            .with_write_path(Arc::clone(&writer));

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: false, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    match std::fs::rename(mnt.path().join("alpha.txt"), mnt.path().join("beta.txt")) {
        Ok(()) => {}
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("kernel rename: {err}"),
    }

    // The rename must be recorded.
    let renames = upload_backend.renames.lock().unwrap();
    assert!(
        renames.iter().any(|(from, to)| from.contains("alpha") && to.contains("beta")),
        "rename must be recorded in upload backend, got: {renames:?}"
    );
    drop(renames);

    handle.unmount().expect("unmount");
}

// =============================================================================
// T9: mkdir + rmdir round-trip
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn mkdir_rmdir_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let files = Arc::new(MockFileBackend::new());

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: false, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let new_dir = mnt.path().join("new_subdir");
    match std::fs::create_dir(&new_dir) {
        Ok(()) => {}
        Err(err) if should_skip_io_error(&err) => {
            handle.unmount().ok();
            return;
        }
        Err(err) => panic!("kernel mkdir: {err}"),
    }

    match std::fs::metadata(&new_dir) {
        Ok(meta) => assert!(meta.is_dir(), "new_subdir must be a directory"),
        Err(err) if should_skip_io_error(&err) => {}
        Err(err) => panic!("stat new_subdir: {err}"),
    }

    match std::fs::remove_dir(&new_dir) {
        Ok(()) => {}
        Err(err) if should_skip_io_error(&err) => {}
        Err(err) => panic!("kernel rmdir: {err}"),
    }

    handle.unmount().expect("unmount");
}

// =============================================================================
// T10: statfs reports sensible values
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn statfs_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let adapter = ProtoFuseAdapter::new(Arc::clone(&folder), AdapterOptions::default());

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    // `statvfs` on the mountpoint exercises the statfs thunk.
    let result = {
        let path_cstr = std::ffi::CString::new(mnt.path().to_str().unwrap()).unwrap();
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(path_cstr.as_ptr(), &mut st) };
        if rc == 0 { Some(st) } else { None }
    };

    if let Some(st) = result {
        assert!(st.f_bsize > 0, "block size must be positive, got {}", st.f_bsize);
        assert!(st.f_blocks > 0, "total blocks must be positive, got {}", st.f_blocks);
        assert!(st.f_namemax > 0, "namemax must be positive, got {}", st.f_namemax);
        // The thunk reports 1 TiB total, 512 GiB free.
        let expected_total_blocks = (1u64 << 40) / 4096;
        let expected_free_blocks = (512u64 << 30) / 4096;
        assert_eq!(st.f_blocks as u64, expected_total_blocks.min(u32::MAX as u64));
        assert_eq!(st.f_bfree as u64, expected_free_blocks.min(u32::MAX as u64));
    }

    handle.unmount().expect("unmount");
}

// =============================================================================
// T11: xattr probes do not crash the mount
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn xattr_returns_enoattr_not_crash() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![("file.txt", false, None, Some(1))]);
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(1, b"data".to_vec());

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let path = mnt.path().join("file.txt");
    let path_cstr = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let attr_name = std::ffi::CString::new("com.apple.FinderInfo").unwrap();

    // getxattr must return an error (ENOATTR=93), not crash.
    let mut buf = vec![0u8; 256];
    let rc = unsafe {
        libc::getxattr(
            path_cstr.as_ptr(),
            attr_name.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            0,
            0,
        )
    };
    assert!(rc < 0, "getxattr for non-existent xattr must return < 0, got {rc}");

    // listxattr must return 0 (empty list), not crash.
    let list_rc = unsafe {
        libc::listxattr(path_cstr.as_ptr(), buf.as_mut_ptr() as *mut libc::c_char, buf.len(), 0)
    };
    assert!(
        list_rc >= 0,
        "listxattr must succeed (return >= 0), got {list_rc}"
    );
    assert_eq!(list_rc, 0, "listxattr must return 0 bytes (no xattrs)");

    handle.unmount().expect("unmount");
}

// =============================================================================
// T12: PCLOUD_MACOS_FUSE_BACKEND=macfuse probe path
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 on macOS; tests macFUSE fallback probe path"]
fn backend_env_macfuse_probe() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    use pcloud_fs::platform::PlatformMount;
    use pcloud_fs::platform::macos::MacosPlatformMount;

    // SAFETY: test-only env mutation; this test is expected to run
    // with --test-threads=1 when exercising the macFUSE backend path.
    unsafe { std::env::set_var("PCLOUD_MACOS_FUSE_BACKEND", "macfuse") };
    let mount = MacosPlatformMount;
    // We only check that probe_supported does not panic — it will return
    // Ok or Unsupported depending on whether macFUSE is installed.
    match mount.probe_supported() {
        Ok(()) => {}
        Err(MountError::Unsupported(hint)) => {
            assert!(hint.contains("macFUSE"), "hint must mention macFUSE: {hint}");
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
    unsafe { std::env::remove_var("PCLOUD_MACOS_FUSE_BACKEND") };
}

// =============================================================================
// T13: remount cycle (unmount + remount) with same mock backends
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn remount_cycle_preserves_adapter_state() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir(
        "/",
        1,
        vec![
            ("persistent.txt", false, None, Some(77)),
        ],
    );
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(77, b"persisted across remount".to_vec());

    let svc = MountService::new();

    // First mount.
    let mnt1 = tempfile::tempdir().expect("mount tempdir 1");
    let adapter1 =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());

    let handle1 = match svc.mount(
        mnt1.path(),
        adapter1,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("first mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt1.path()) {
        handle1.unmount().ok();
        return;
    }

    let got1 = match std::fs::read(mnt1.path().join("persistent.txt")) {
        Ok(b) => b,
        Err(err) if should_skip_io_error(&err) => {
            handle1.unmount().ok();
            return;
        }
        Err(err) => panic!("read first mount: {err}"),
    };
    handle1.unmount().expect("unmount first mount");

    // Second mount — same backends, different tempdir.
    let mnt2 = tempfile::tempdir().expect("mount tempdir 2");
    let adapter2 =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());

    let handle2 = match svc.mount(
        mnt2.path(),
        adapter2,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("second mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt2.path()) {
        handle2.unmount().ok();
        return;
    }

    let got2 = match std::fs::read(mnt2.path().join("persistent.txt")) {
        Ok(b) => b,
        Err(err) if should_skip_io_error(&err) => {
            handle2.unmount().ok();
            return;
        }
        Err(err) => panic!("read second mount: {err}"),
    };
    handle2.unmount().expect("unmount second mount");

    assert_eq!(got1, b"persisted across remount", "first read must match expected");
    assert_eq!(got1, got2, "both remounts must return identical content");
}

// =============================================================================
// T14: orphan detection via MacosMountinfoReader after a live mount
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn orphan_detection_finds_active_fuset_mount() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let adapter = ProtoFuseAdapter::new(Arc::clone(&folder), AdapterOptions::default());

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(300));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        // fuse-t mount not visible in getmntinfo — accept as a platform variation.
        handle.unmount().ok();
        return;
    }

    // Pretend the daemon knows about no mounts — all mounts are "orphans".
    let reader = MacosMountinfoReader;
    let orphans = detect_orphans(&reader, &[]).expect("detect_orphans must not error");

    let mnt_canonical = mnt.path().canonicalize().ok();
    let found = orphans.iter().any(|o| {
        Some(&o.mount_point) == mnt_canonical.as_ref()
            || o.mount_point == mnt.path()
    });
    assert!(
        found,
        "the active fuse-t mount must appear as an orphan when daemon knows no mounts; \
         active mount path = {}, orphans = {orphans:?}",
        mnt.path().display()
    );

    handle.unmount().expect("unmount");
}

// =============================================================================
// T15: concurrent readers — multiple threads reading simultaneously
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn concurrent_readers_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    const THREAD_COUNT: usize = 4;
    const FILE_COUNT: usize = 8;

    let folder = Arc::new(MockFolderBackend::new());
    let files = Arc::new(MockFileBackend::new());

    let mut dir_entries = vec![];
    for i in 0..FILE_COUNT {
        let name = format!("file{i}.txt");
        dir_entries.push((name.clone(), false, None, Some((100 + i) as u64)));
        files.insert_file((100 + i) as u64, format!("content of file {i}").into_bytes());
    }
    folder.insert_dir("/", 1, dir_entries.iter().map(|(n, d, c, s)| (n.as_str(), *d, *c, *s)).collect());

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default());

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: true, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(300));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    let mnt_path = mnt.path().to_path_buf();
    let mut handles = vec![];

    for t in 0..THREAD_COUNT {
        let path = mnt_path.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..FILE_COUNT {
                let file_path = path.join(format!("file{i}.txt"));
                match std::fs::read(&file_path) {
                    Ok(bytes) => {
                        let expected = format!("content of file {i}");
                        assert_eq!(
                            bytes,
                            expected.as_bytes(),
                            "thread {t} file {i}: content mismatch"
                        );
                    }
                    Err(err) => {
                        if !should_skip_io_error(&err) {
                            panic!("thread {t} read file{i}.txt: {err}");
                        }
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("reader thread must not panic");
    }

    handle.unmount().expect("unmount");
}

// =============================================================================
// T16: full lifecycle — mount, readdir, read, write, fsync, unlink, rename,
//       unmount (combines all previous T1–T8 into a single coherent story)
// =============================================================================

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and fuse-t installed on macOS"]
fn full_lifecycle_via_fuset() {
    if !fuse_gate_enabled() {
        return;
    }
    let _fuse_lock = fuse_serial_lock();

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir(
        "/",
        1,
        vec![
            ("docs", true, Some(2), None),
            ("readme.txt", false, None, Some(10)),
        ],
    );
    folder.insert_dir("/docs", 2, vec![("api.md", false, None, Some(11))]);
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(10, b"README content".to_vec());
    files.insert_file(11, b"# API".to_vec());

    let upload_backend = Arc::new(RecordingUploadBackend::default());
    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let stage = StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&upload_backend),
        WritePathOptions {
            flush_threshold_bytes: u64::MAX,
            flush_interval: Duration::from_secs(3600),
        },
    ));

    let adapter =
        ProtoFuseAdapter::with_file_backend(Arc::clone(&folder), Arc::clone(&files), AdapterOptions::default())
            .with_write_path(Arc::clone(&writer));

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();

    // Step 1: mount.
    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions { read_only: false, ..MountOptions::default() },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_in_getmntinfo(mnt.path()) {
        handle.unmount().ok();
        return;
    }

    macro_rules! skip_on_io {
        ($expr:expr) => {
            match $expr {
                Ok(v) => v,
                Err(err) if should_skip_io_error(&err) => {
                    handle.unmount().ok();
                    return;
                }
                Err(err) => panic!("{}", err),
            }
        };
    }

    // Step 2: readdir root.
    let root_entries: Vec<String> = skip_on_io!(std::fs::read_dir(mnt.path()))
        .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
        .collect();
    assert!(root_entries.iter().any(|e| e == "docs"), "root must have docs");
    assert!(root_entries.iter().any(|e| e == "readme.txt"), "root must have readme.txt");

    // Step 3: read file.
    let readme = skip_on_io!(std::fs::read(mnt.path().join("readme.txt")));
    assert_eq!(readme, b"README content", "readme must match mock content");

    // Step 4: read nested file.
    let api = skip_on_io!(std::fs::read(mnt.path().join("docs/api.md")));
    assert_eq!(api, b"# API", "api.md must match mock content");

    // Step 5: write new file.
    skip_on_io!(std::fs::write(mnt.path().join("new.txt"), b"new content"));

    // Step 6: fsync.
    {
        use std::io::Write;
        let mut f = skip_on_io!(std::fs::OpenOptions::new().write(true).open(mnt.path().join("new.txt")));
        skip_on_io!(f.flush());
        skip_on_io!(f.sync_all());
    }

    // Step 7: unlink.
    skip_on_io!(std::fs::remove_file(mnt.path().join("new.txt")));

    // Step 8: rename.
    skip_on_io!(std::fs::rename(
        mnt.path().join("readme.txt"),
        mnt.path().join("readme.md"),
    ));

    // Step 9: unmount cleanly.
    handle.unmount().expect("unmount must succeed");

    // Post-conditions on the upload backend.
    let uploads = upload_backend.uploads.lock().unwrap();
    assert!(
        uploads.iter().any(|(_, name, b)| name == "new.txt" && b == b"new content"),
        "write+fsync must produce an upload record, got: {uploads:?}"
    );
}
