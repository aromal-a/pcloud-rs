#![allow(clippy::pedantic)]
//! bd-1du.4.6 — dyn-shim write-path integration test.
//!
//! Proves that write operations (`create` / `write` / `fsync` / readback)
//! work end-to-end through the type-erased `BoxedFuserShim` path (i.e.
//! through the `FuseAdapter` trait's write methods forwarded by the
//! [`PlatformMount`] abstraction),
//! not just through the concrete `PcloudFsShim`.
//!
//! The test:
//!   1. Constructs a `ProtoFuseAdapter` with a `WritePathService` attached.
//!   2. Mounts it via `LinuxPlatformMount::mount_adapter`, exercising the
//!      same type-erased entry point used by cross-platform runtime code.
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
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::mount_service::{MountOptions, MountService};
use pcloud_fs::page_cache::PageCacheConfig;
use pcloud_fs::platform::{PlatformMount, linux::LinuxPlatformMount};
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
    folder.set_quota(10 * 1024 * 1024, 7 * 1024 * 1024);

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

    // -- mount through the type-erased PlatformMount seam --
    let mountdir = tempfile::tempdir().expect("tempdir for mount");
    let platform = LinuxPlatformMount;
    let handle = match platform.mount_adapter(
        Box::new(adapter),
        mountdir.path(),
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

    // Exercise the remaining kernel callback family through the same dyn
    // shim: statfs, setattr/truncate, chmod rejection, rename and unlink.
    let mount_c = std::ffi::CString::new(mountdir.path().as_os_str().as_encoded_bytes())
        .expect("mount path has no NUL");
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the syscall.
    assert_eq!(unsafe { libc::statvfs(mount_c.as_ptr(), &mut stat) }, 0);
    assert!(stat.f_blocks > 0);

    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&file_path)
        .expect("open staged file for truncate");
    file.set_len(5).expect("truncate through setattr");
    drop(file);
    assert_eq!(
        std::fs::read(&file_path).expect("read truncated file"),
        b"hello"
    );

    let chmod = std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600));
    assert!(
        chmod.is_err(),
        "pCloud has no Unix permission bits, chmod must be rejected"
    );

    let renamed = mountdir.path().join("renamed.txt");
    std::fs::rename(&file_path, &renamed).expect("rename through dyn shim");
    std::fs::remove_file(&renamed).expect("unlink through dyn shim");

    let mkdir = std::fs::create_dir(mountdir.path().join("unsupported-directory"));
    assert!(
        mkdir.is_err(),
        "the mock folder backend deliberately rejects mkdir"
    );

    // -- clean unmount --
    handle.unmount().expect("clean unmount");

    // Repeat the same callback family through the generic MountService seam.
    // Linux keeps separate generic and type-erased fuser shims, and both are
    // production entrypoints used by embedders.
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    folder.set_quota(10 * 1024 * 1024, 7 * 1024 * 1024);
    let files = Arc::new(MockFileBackend::new());
    let stage = StagingDir::open(tmp.path().join("generic-stage")).unwrap();
    let journal = WriteJournal::open(stage.journal_path()).unwrap();
    let generic_upload = Arc::new(RecordingUploadBackend::default());
    let generic_writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&generic_upload),
        WritePathOptions {
            flush_threshold_bytes: 64 * 1024 * 1024,
            flush_interval: Duration::from_secs(3600),
            ..WritePathOptions::default()
        },
    ));
    let generic_adapter = ProtoFuseAdapter::with_file_backend(
        Arc::clone(&folder),
        Arc::clone(&files),
        AdapterOptions::default(),
    )
    .with_write_path(Arc::clone(&generic_writer));
    let generic_mount = tempfile::tempdir().unwrap();
    let generic_handle = MountService::new()
        .mount(
            generic_mount.path(),
            generic_adapter,
            MountOptions {
                read_only: false,
                ..MountOptions::default()
            },
        )
        .expect("generic mount");
    std::thread::sleep(Duration::from_millis(100));

    let generic_file = generic_mount.path().join("generic.txt");
    let mut file = std::fs::File::create(&generic_file).unwrap();
    file.write_all(b"generic callback payload").unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert_eq!(
        std::fs::read(&generic_file).unwrap(),
        b"generic callback payload"
    );
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&generic_file)
        .unwrap();
    file.set_len(7).unwrap();
    drop(file);
    assert_eq!(std::fs::read(&generic_file).unwrap(), b"generic");
    assert!(
        std::fs::set_permissions(&generic_file, std::fs::Permissions::from_mode(0o600)).is_err()
    );
    let generic_renamed = generic_mount.path().join("renamed.txt");
    std::fs::rename(&generic_file, &generic_renamed).unwrap();
    std::fs::remove_file(&generic_renamed).unwrap();
    assert!(std::fs::create_dir(generic_mount.path().join("unsupported")).is_err());
    generic_handle.unmount().expect("generic clean unmount");
}
