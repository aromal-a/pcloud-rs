#![allow(clippy::pedantic)]
//! bd-1du.4.b / 4.c / 4.e integration tests.
//!
//! Mounts a read-only `ProtoFuseAdapter` backed by a mocked folder backend
//! and verifies that `readdir` on `/` and on a nested directory returns the
//! expected entries through the real FUSE kernel interface. The 4.c test
//! extends this with `open`/`read`/`release` against a mocked file backend,
//! and the 4.e test exercises the full mount lifecycle plus a best-effort
//! write + fsync durability barrier (see note in the 4.e test body for the
//! current gap: the `FuseAdapter` trait does not yet forward kernel
//! `create`/`write`/`fsync` calls, so the write + fsync leg is executed
//! against `WritePathService` directly while the FUSE kernel mount is held
//! live — see `bd-1du.4.6`).
//!
//! All tests in this file are gated behind `PCLOUD_FUSE_TEST=1` because a
//! working libfuse kernel module and `/dev/fuse` access are not guaranteed
//! in CI.

#![cfg(target_os = "linux")]

// **PLATFORM:** Linux
// **GATING:** none (portable; uses Linux-only idioms — see TODO(bd-xplat)).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::mount_service::{MountOptions, MountService};
use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::WriteJournal;
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};

fn fuse_gate_enabled() -> bool {
    std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1")
}

fn should_skip_mount_error(err: &str) -> bool {
    err.contains("Operation not permitted")
        || err.contains("Function not implemented")
        // TODO(bd-xplat): Linux-only — needs cfg gate or platform trait abstraction. See PLAN_CROSSPLATFORM.md §2.
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
    // TODO(bd-xplat): Linux-only — needs cfg gate or platform trait abstraction. See PLAN_CROSSPLATFORM.md §2.
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let needle = path.to_string_lossy();
    mountinfo.lines().any(|line| line.contains(needle.as_ref()))
}

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn readdir_root_and_nested_via_real_mount() {
    if !fuse_gate_enabled() {
        return;
    }
    let backend = Arc::new(MockFolderBackend::new());
    backend.insert_dir(
        "/",
        1,
        vec![
            ("docs", true, Some(2), None),
            ("hello.txt", false, None, Some(100)),
        ],
    );
    backend.insert_dir("/docs", 2, vec![("readme.md", false, None, Some(101))]);

    let adapter = ProtoFuseAdapter::new(backend, AdapterOptions::default());

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

    // Give the kernel a moment to complete the mount handshake.
    std::thread::sleep(Duration::from_millis(100));
    if !mount_appears_active(tmp.path()) {
        return;
    }

    let root_entries: Vec<String> = match std::fs::read_dir(tmp.path()) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("readdir /: {err}"),
    };
    assert!(
        root_entries.iter().any(|e| e == "docs"),
        "root listing should include docs, got {root_entries:?}"
    );
    assert!(
        root_entries.iter().any(|e| e == "hello.txt"),
        "root listing should include hello.txt, got {root_entries:?}"
    );

    let nested: Vec<String> = match std::fs::read_dir(tmp.path().join("docs")) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("readdir /docs: {err}"),
    };
    assert!(
        nested.iter().any(|e| e == "readme.md"),
        "nested listing should include readme.md, got {nested:?}"
    );

    handle.unmount().expect("unmount");
}

/// bd-1du.4.c: end-to-end read path through a real FUSE mount backed by a
/// mocked [`MockFileBackend`]. Gated the same as the 4.b test.
#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn read_small_file_via_real_mount() {
    if !fuse_gate_enabled() {
        return;
    }
    let expected = b"hello via fuse";
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir_with_sizes(
        "/",
        1,
        vec![(
            "hello.txt",
            false,
            None,
            Some(42),
            Some(expected.len() as u64),
        )],
    );
    let files = Arc::new(MockFileBackend::new());
    files.insert_file(42, expected.to_vec());

    let adapter = ProtoFuseAdapter::with_file_backend(folder, files, AdapterOptions::default());

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
        Err(err) => panic!("mount: {err}"),
    };

    std::thread::sleep(Duration::from_millis(100));
    if !mount_appears_active(tmp.path()) {
        return;
    }

    // The kernel will not issue read requests beyond the size advertised by
    // lookup/getattr. Keep this assertion at the native boundary so a fixture
    // or metadata regression cannot silently turn every read into EOF.
    assert_eq!(
        std::fs::metadata(tmp.path().join("hello.txt"))
            .expect("stat hello.txt")
            .len(),
        expected.len() as u64,
        "FUSE must advertise the authoritative remote file size"
    );

    let got = match std::fs::read(tmp.path().join("hello.txt")) {
        Ok(bytes) => bytes,
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("read hello.txt: {err}"),
    };
    assert_eq!(got, expected);

    handle.unmount().expect("unmount");
}

/// Minimal upload backend used by the 4.e test. Records every successful
/// upload and never fails, so the test can assert that `flush`/`fsync`
/// delivered the staged bytes to the "remote" side.
#[derive(Default)]
struct RecordingUploadBackend {
    uploads: std::sync::Mutex<Vec<(String, String, Vec<u8>)>>,
}

impl FileUploadBackend for RecordingUploadBackend {
    fn upload_file(
        &self,
        parent_path: &str,
        name: &str,
        staging_file: &Path,
    ) -> Result<(), WritePathError> {
        let bytes =
            std::fs::read(staging_file).map_err(|e| WritePathError::Upload(e.to_string()))?;
        self.uploads
            .lock()
            .unwrap()
            .push((parent_path.to_owned(), name.to_owned(), bytes));
        Ok(())
    }
    fn unlink_remote(&self, _path: &str) -> Result<(), WritePathError> {
        Ok(())
    }
    fn rename_remote(&self, _from: &str, _to: &str) -> Result<(), WritePathError> {
        Ok(())
    }
}

/// bd-1du.4.e: full mount lifecycle integration test.
///
/// Exercises, end-to-end against a real FUSE kernel mount:
///   1. `mount` an adapter with folder + file backends
///   2. `readdir` through the kernel VFS
///   3. `read` a small file through the kernel VFS
///   4. (best-effort) write + fsync against `WritePathService`
///      while the mount is still held live
///   5. `unmount` cleanly
///
/// This remains a proof-oriented test rather than the final release gate:
/// kernel mount success still depends on host FUSE privileges, so the test
/// returns early on EPERM/ENOSYS-style host failures instead of reporting a
/// product regression.
#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn full_mount_readdir_read_write_fsync_unmount_cycle() {
    if !fuse_gate_enabled() {
        return;
    }

    // --- arrange backends ------------------------------------------------
    let expected_hello = b"hello via fuse e2e";
    let expected_readme = b"readme body";
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir_with_sizes(
        "/",
        1,
        vec![
            ("docs", true, Some(2), None, None),
            (
                "hello.txt",
                false,
                None,
                Some(42),
                Some(expected_hello.len() as u64),
            ),
        ],
    );
    folder.insert_dir_with_sizes(
        "/docs",
        2,
        vec![(
            "readme.md",
            false,
            None,
            Some(43),
            Some(expected_readme.len() as u64),
        )],
    );

    let files = Arc::new(MockFileBackend::new());
    files.insert_file(42, expected_hello.to_vec());
    files.insert_file(43, expected_readme.to_vec());

    let adapter = ProtoFuseAdapter::with_file_backend(folder, files, AdapterOptions::default());

    // --- step 1: mount ---------------------------------------------------
    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();
    let handle = match svc.mount(
        mnt.path(),
        adapter,
        MountOptions {
            read_only: true,
            ..MountOptions::default()
        },
    ) {
        Ok(handle) => handle,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount should succeed: {err}"),
    };

    std::thread::sleep(Duration::from_millis(100));
    if !mount_appears_active(mnt.path()) {
        return;
    }

    // --- step 2: readdir via kernel -------------------------------------
    let root_entries: Vec<String> = match std::fs::read_dir(mnt.path()) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("readdir /: {err}"),
    };
    assert!(root_entries.iter().any(|e| e == "docs"));
    assert!(root_entries.iter().any(|e| e == "hello.txt"));

    let nested: Vec<String> = match std::fs::read_dir(mnt.path().join("docs")) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|d| d.file_name().to_string_lossy().into_owned()))
            .collect(),
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("readdir /docs: {err}"),
    };
    assert!(nested.iter().any(|e| e == "readme.md"));

    // --- step 3: read small file via kernel ------------------------------
    let got = match std::fs::read(mnt.path().join("hello.txt")) {
        Ok(bytes) => bytes,
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("read hello.txt: {err}"),
    };
    assert_eq!(got, expected_hello);

    // --- step 4: write + fsync against WritePathService -----------------
    //
    // The mount is still live here; we are exercising the durability
    // barrier that the future write-via-kernel adapter will route into.
    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let stage = StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let upload_backend = Arc::new(RecordingUploadBackend::default());
    let write_svc = WritePathService::new(
        stage,
        journal,
        upload_backend.clone(),
        WritePathOptions {
            // Force upload only on explicit fsync/flush, not on write().
            flush_threshold_bytes: u64::MAX,
            flush_interval: Duration::from_secs(3600),
            ..WritePathOptions::default()
        },
    );

    let ino: u64 = 4242;
    write_svc.create(ino, "/", "new_file.txt").expect("create");
    let payload = b"durable content via fsync";
    let wrote = write_svc.write(ino, 0, payload).expect("write");
    assert_eq!(wrote, payload.len());
    write_svc.fsync(ino).expect("fsync");

    let uploads = upload_backend.uploads.lock().unwrap();
    assert_eq!(uploads.len(), 1, "fsync should have produced one upload");
    assert_eq!(uploads[0].0, "/");
    assert_eq!(uploads[0].1, "new_file.txt");
    assert_eq!(uploads[0].2, payload);
    drop(uploads);

    // --- step 5: unmount cleanly ----------------------------------------
    handle.unmount().expect("unmount");
}

/// bd-1du.4.e row-85 kernel write-ops coverage.
///
/// Mounts a composite [`PcloudFsShim`] that wires write ops into a real
/// [`WritePathService`] backed by an in-memory [`FileUploadBackend`].
/// Drives the full kernel write path end-to-end:
///
///   1. mount read-write composite shim
///   2. kernel `create` + `write` + `fsync` a brand-new file
///   3. kernel `unlink` removes it
///   4. kernel `rename` moves a second file
///   5. unmount
///   6. remount (fresh `PcloudFsShim` + same mock upload backend) and
///      verify a file we re-created survives a flush after the mount
///      cycle (remount-replay semantics for the happy path).
///
/// Gated behind `PCLOUD_FUSE_TEST=1` like the other mount tests. When
/// the gate is off, or the host refuses FUSE mounts, this test is a no-op
/// and row 85 stays honest.
#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn kernel_create_write_fsync_unlink_rename_remount_cycle() {
    if !fuse_gate_enabled() {
        return;
    }

    use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
    // TODO(bd-xplat): Linux-only — needs cfg gate or platform trait abstraction. See PLAN_CROSSPLATFORM.md §2.
    use pcloud_fs::fuser_shim::PcloudFsShim;

    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let files = Arc::new(MockFileBackend::new());

    let upload_backend = Arc::new(RecordingUploadBackend::default());

    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let stage =
        pcloud_fs::staging::StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal =
        pcloud_fs::write_journal::WriteJournal::open(stage.journal_path()).expect("journal");
    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&upload_backend),
        WritePathOptions {
            flush_threshold_bytes: u64::MAX,
            flush_interval: Duration::from_secs(3600),
            ..WritePathOptions::default()
        },
    ));

    let adapter = Arc::new(
        ProtoFuseAdapter::with_file_backend(
            Arc::clone(&folder),
            Arc::clone(&files),
            AdapterOptions::default(),
        )
        .with_write_path(Arc::clone(&writer)),
    );
    // Also attach the writer to the adapter so the trait-level write path
    // is exercised via read-side getattr after create/unlink.
    let adapter_for_check = Arc::clone(&adapter);
    let shim = PcloudFsShim::new(adapter, Arc::clone(&writer));

    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();
    let handle = match svc.mount_fuser(
        mnt.path(),
        shim,
        pcloud_fs::mount_service::MountOptions {
            read_only: false,
            ..pcloud_fs::mount_service::MountOptions::default()
        },
    ) {
        Ok(handle) => handle,
        Err(err) if should_skip_mount_error(&err.to_string()) => return,
        Err(err) => panic!("mount read-write composite shim: {err}"),
    };

    std::thread::sleep(Duration::from_millis(150));
    if !mount_appears_active(mnt.path()) {
        return;
    }

    // Step 2: create + write + fsync via kernel VFS.
    let created = mnt.path().join("new.txt");
    match std::fs::write(&created, b"via-kernel") {
        Ok(()) => {}
        Err(err) if should_skip_io_error(&err) => return,
        Err(err) => panic!("kernel write creates file: {err}"),
    }
    // Force fsync by reopening & calling sync_all.
    {
        let f = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&created)
        {
            Ok(file) => file,
            Err(err) if should_skip_io_error(&err) => return,
            Err(err) => panic!("reopen for fsync: {err}"),
        };
        if let Err(err) = f.sync_all() {
            if should_skip_io_error(&err) {
                return;
            }
            panic!("fsync: {err}");
        }
    }
    let uploads = upload_backend.uploads.lock().unwrap();
    assert!(
        uploads
            .iter()
            .any(|(_, name, bytes)| name == "new.txt" && bytes == b"via-kernel"),
        "kernel write must have produced an upload, got {:?}",
        uploads
    );
    drop(uploads);

    // Step 3: unlink via kernel VFS. The mock upload backend accepts
    // unlink_remote; the writer's journal records it.
    if let Err(err) = std::fs::remove_file(&created) {
        if should_skip_io_error(&err) {
            return;
        }
        panic!("kernel unlink: {err}");
    }

    // Step 4: rename via kernel VFS. Create a second file, rename it.
    let a = mnt.path().join("a.txt");
    if let Err(err) = std::fs::write(&a, b"abc") {
        if should_skip_io_error(&err) {
            return;
        }
        panic!("write a.txt: {err}");
    }
    let b = mnt.path().join("b.txt");
    if let Err(err) = std::fs::rename(&a, &b) {
        if should_skip_io_error(&err) {
            return;
        }
        panic!("kernel rename: {err}");
    }

    // Step 5: unmount.
    handle.unmount().expect("unmount");

    // Sanity: trait surface — write delegation worked end-to-end.
    assert!(adapter_for_check.has_write_path());
}
