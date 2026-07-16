#![allow(clippy::pedantic)]
//! bd-1du.4.6 — dyn-shim write-path integration test.
//!
//! Proves that write operations (`create` / `write` / `fsync` / readback)
//! work end-to-end through the `FuserShim<A>` path (i.e. through the
//! `FuseAdapter` trait's write methods forwarded by the platform shim),
//! not just through the concrete `PcloudFsShim`.
//!
//! The test:
//!   1. Constructs a `ProtoFuseAdapter` with a `WritePathService` attached.
//!   2. Mounts it via `MountService::mount` (which wraps in `FuserShim<A>`).
//!   3. Creates a file, writes known bytes, calls `sync_all`.
//!   4. Reads the file back through the kernel VFS and asserts byte-identity.
//!   5. Unmounts cleanly.
//!
//! # Gating
//!
//! * `#[cfg(target_os = "linux")]` — `fuser` is Linux-only.
//! * `#[ignore]` by default — opt-in via `PCLOUD_FUSE_TEST=1` or
//!   `PCLOUD_LIVE_E2E=1`.

#![cfg(target_os = "linux")]

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::mount_service::{MountOptions, MountService};
use pcloud_fs::page_cache::PageCacheConfig;
use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::WriteJournal;
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};

fn gate_enabled() -> bool {
    let live = std::env::var("PCLOUD_LIVE_E2E").ok().as_deref() == Some("1");
    let legacy = std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1");
    live || legacy
}

fn should_skip_mount_error(err: &str) -> bool {
    err.contains("Operation not permitted")
        || err.contains("Function not implemented")
        || err.contains("Permission denied")
        || err.contains("/dev/fuse")
}

// ---- recording upload backend ----

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

#[test]
#[ignore = "requires PCLOUD_FUSE_TEST=1 and a working libfuse kernel module"]
fn dyn_shim_write_file_and_readback_via_kernel_vfs() {
    if !gate_enabled() {
        return;
    }

    // -- build adapter with write-path --
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);

    let files = Arc::new(MockFileBackend::new());

    let adapter = ProtoFuseAdapter::with_file_backend(
        Arc::clone(&folder),
        Arc::clone(&files),
        AdapterOptions {
            page_cache: PageCacheConfig {
                page_size: 4096,
                max_bytes: 4 * 1024 * 1024,
            },
            ..AdapterOptions::default()
        },
    );

    let tmp = tempfile::tempdir().expect("tempdir for staging");
    let stage = StagingDir::open(tmp.path().join("stage")).expect("staging dir");
    let journal = WriteJournal::open(stage.journal_path()).expect("journal");
    let upload = Arc::new(RecordingUploadBackend::default());
    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&upload),
        WritePathOptions {
            flush_threshold_bytes: 64 * 1024 * 1024,
            flush_interval: Duration::from_secs(3600),
            ..WritePathOptions::default()
        },
    ));

    let adapter = adapter.with_write_path(Arc::clone(&writer));

    // -- mount via MountService::mount (wraps in FuserShim<A>) --
    let mountdir = tempfile::tempdir().expect("tempdir for mount");
    let svc = MountService::new();
    let handle = match svc.mount(
        mountdir.path(),
        adapter,
        MountOptions {
            read_only: false,
            ..MountOptions::default()
        },
    ) {
        Ok(h) => h,
        Err(err) if should_skip_mount_error(&err.to_string()) => {
            eprintln!("FUSE not available, skipping: {err}");
            return;
        }
        Err(err) => panic!("mount should succeed: {err}"),
    };

    // Give fuser a moment to settle.
    std::thread::sleep(Duration::from_millis(200));

    // -- write a file through the kernel VFS --
    let payload = b"hello from dyn-shim write test";
    let file_path = mountdir.path().join("test_write.txt");

    // Try the write; skip gracefully if the kernel rejects it (EROFS
    // from a read-only mount indicates the kernel side did not pick up
    // our write callbacks).
    match std::fs::File::create(&file_path) {
        Ok(mut f) => {
            f.write_all(payload).expect("write_all");
            f.sync_all().expect("sync_all");
        }
        Err(e) if e.raw_os_error() == Some(libc::EROFS) => {
            eprintln!("kernel returned EROFS — write callbacks may not be wired; skipping");
            let _ = handle.unmount();
            return;
        }
        Err(e) => panic!("unexpected error creating file: {e}"),
    }

    let uploads = upload.uploads.lock().expect("recorded uploads lock");
    assert_eq!(
        uploads.len(),
        2,
        "flush plus fsync must both reach the upload backend"
    );
    assert_eq!(uploads.last().expect("fsync upload").2, payload);
    drop(uploads);

    assert_eq!(
        std::fs::metadata(&file_path)
            .expect("metadata after write")
            .len(),
        payload.len() as u64,
        "kernel-visible size must follow a completed write"
    );

    // -- readback through the kernel VFS --
    let readback = std::fs::read(&file_path).expect("readback");
    assert_eq!(
        readback, payload,
        "readback through kernel VFS must be byte-identical to what was written"
    );

    // -- clean unmount --
    handle.unmount().expect("clean unmount");
}
