#![allow(clippy::pedantic)]
//! bd-1du.4.6 footnote `[fuse-wiring]` integration test.
//!
//! Mounts a composed [`PcloudFsShim`] (read-side [`ProtoFuseAdapter`] +
//! live [`WritePathService`]) via [`MountService::mount_fuser`], creates
//! and writes a small file **under** `flush_threshold_bytes`, fsyncs it,
//! reads it back (best-effort; the mock read path does not auto-publish
//! post-create entries, so a readback miss is tolerated), unmounts, and
//! proves the upload finalized and its acknowledged `Create` + `Write` +
//! `FlushBarrier` records were checkpointed out of the replay journal.
//!
//! # Gating
//!
//! Matches the P0.5 convention in `fuse_kernel_e2e.rs`:
//!
//! * Linux/FreeBSD/NetBSD/OpenBSD/DragonFlyBSD only
//! * `#[ignore]` by default
//! * additionally gated on `PCLOUD_LIVE_E2E=1` (or legacy
//!   `PCLOUD_FUSE_TEST=1`)
//!
//! When the host refuses to mount FUSE (no `/dev/fuse`, unprivileged
//! container, EPERM, ENOSYS) the test skips gracefully.
//!
//! The in-memory write/flush/journal path without a live kernel mount is
//! covered by the unit tests in `fuser_shim.rs` and by
//! `write_path_replay.rs`; this test exists specifically to prove the
//! kernel → `PcloudFsShim` → `WritePathService` → journal wiring closes
//! under `MountService::mount_fuser` for the small-file case below the
//! 64 MiB flush threshold.

#![cfg(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly"
))]

// **PLATFORM:** Linux and supported BSD systems
// **GATING:** Linux/FreeBSD cfg plus explicit live-test environment.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pcloud_fs::backend::mock::{MockFileBackend, MockFolderBackend};
use pcloud_fs::fuse_adapter::{AdapterOptions, ProtoFuseAdapter};
use pcloud_fs::fuser_shim::PcloudFsShim;
use pcloud_fs::mount_service::{MountHandle, MountOptions, MountService};
use pcloud_fs::staging::StagingDir;
use pcloud_fs::write_journal::{WriteJournal, replay_path};
use pcloud_fs::write_path::{
    FileUploadBackend, WritePathError, WritePathOptions, WritePathService,
};

const SMALL_FILE_BYTES: usize = 4 * 1024; // 4 KiB — well under 64 MiB threshold.

fn e2e_gate_enabled() -> bool {
    let live = std::env::var("PCLOUD_LIVE_E2E").ok().as_deref() == Some("1");
    let legacy = std::env::var("PCLOUD_FUSE_TEST").ok().as_deref() == Some("1");
    live || legacy
}

fn strict_gate_enabled() -> bool {
    std::env::var("PCLOUD_STRICT_FUSE_TEST").ok().as_deref() == Some("1")
}

#[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "dragonfly"))]
fn dev_fuse_available() -> bool {
    std::path::Path::new("/dev/fuse").exists()
}

#[cfg(target_os = "netbsd")]
fn dev_fuse_available() -> bool {
    std::path::Path::new("/dev/puffs").exists() || std::path::Path::new("/dev/fuse").exists()
}

#[cfg(target_os = "openbsd")]
fn dev_fuse_available() -> bool {
    std::path::Path::new("/dev/fuse0").exists()
}

fn should_skip_mount_error(err: &str) -> bool {
    err.contains("Operation not permitted")
        || err.contains("Function not implemented")
        || err.contains("Permission denied")
        || err.contains("/dev/fuse")
        || err.contains("fusermount")
}

fn should_skip_io_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::Unsupported
            | std::io::ErrorKind::NotConnected
    ) || should_skip_mount_error(&err.to_string())
}

/// Recording upload backend — captures every `upload_file` so the test
/// can assert finalize byte-equality and journal alignment.
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

/// RAII unmount guard so a failing assertion does not leak a FUSE mount.
struct MountGuard {
    handle: Option<MountHandle>,
    path: std::path::PathBuf,
}

impl MountGuard {
    fn new(handle: MountHandle, path: std::path::PathBuf) -> Self {
        Self {
            handle: Some(handle),
            path,
        }
    }
    fn unmount(mut self) -> Result<(), String> {
        if let Some(h) = self.handle.take() {
            h.unmount().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        if let Some(h) = self.handle.take() {
            if let Err(e) = h.unmount() {
                eprintln!("[fuse_small_write_wiring] RAII unmount failed: {e}");
                let _ = std::process::Command::new("fusermount3")
                    .arg("-u")
                    .arg(&self.path)
                    .status();
                let _ = std::process::Command::new("fusermount")
                    .arg("-u")
                    .arg(&self.path)
                    .status();
                #[cfg(any(
                    target_os = "freebsd",
                    target_os = "netbsd",
                    target_os = "openbsd",
                    target_os = "dragonfly"
                ))]
                let _ = std::process::Command::new("unmount")
                    .arg("-f")
                    .arg(&self.path)
                    .status();
            }
        }
    }
}

fn mount_appears_active(path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
            return false;
        };
        let needle = path.to_string_lossy();
        mountinfo.lines().any(|line| line.contains(needle.as_ref()))
    }
    #[cfg(any(
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "dragonfly"
    ))]
    {
        use pcloud_fs::mount_orphan::MountinfoReader;
        let reader = pcloud_fs::platform::bsd::BsdMountinfoReader;
        reader
            .read()
            .map(|payload| {
                pcloud_fs::mount_orphan::parse_pcloud_mounts(&payload)
                    .iter()
                    .any(|entry| entry.mount_point == path)
            })
            .unwrap_or(false)
    }
}

#[test]
#[ignore = "requires PCLOUD_LIVE_E2E=1 (or PCLOUD_FUSE_TEST=1) and a working libfuse kernel module"]
fn small_file_below_threshold_roundtrip_checkpoints_journal() {
    if !e2e_gate_enabled() {
        eprintln!("[fuse_small_write_wiring] skip: PCLOUD_LIVE_E2E / PCLOUD_FUSE_TEST not set");
        return;
    }
    if !dev_fuse_available() {
        assert!(
            !strict_gate_enabled(),
            "strict live mount gate requires /dev/fuse"
        );
        eprintln!("[fuse_small_write_wiring] skip: /dev/fuse not available");
        return;
    }

    // --- backends --------------------------------------------------------
    let folder = Arc::new(MockFolderBackend::new());
    folder.insert_dir("/", 1, vec![]);
    let files = Arc::new(MockFileBackend::new());
    let upload_backend = Arc::new(RecordingUploadBackend::default());

    // --- staging + journal (tempfile per bd-1du.4.6 instruction) --------
    let stage_tmp = tempfile::tempdir().expect("stage tempdir");
    let journal_tmp = tempfile::tempdir().expect("journal tempdir");
    let stage = StagingDir::open(stage_tmp.path().join("stage")).expect("staging");
    let journal_path = journal_tmp.path().join("journal.bin");
    let journal = WriteJournal::open(&journal_path).expect("journal");

    let writer = Arc::new(WritePathService::new(
        stage,
        journal,
        Arc::clone(&upload_backend),
        // Default 64 MiB threshold — the 4 KiB write stays well below it,
        // so no size-based auto-flush fires; only the explicit kernel
        // fsync should produce the finalize upload.
        WritePathOptions::default(),
    ));

    // --- compose adapter + shim ------------------------------------------
    let adapter = Arc::new(
        ProtoFuseAdapter::with_file_backend(
            Arc::clone(&folder),
            Arc::clone(&files),
            AdapterOptions::default(),
        )
        .with_write_path(Arc::clone(&writer)),
    );
    let shim = PcloudFsShim::new(adapter, Arc::clone(&writer));

    // --- mount via MountService::mount_fuser -----------------------------
    let mnt = tempfile::tempdir().expect("mount tempdir");
    let svc = MountService::new();
    let handle = match svc.mount_fuser(
        mnt.path(),
        shim,
        MountOptions {
            read_only: false,
            ..MountOptions::default()
        },
    ) {
        Ok(handle) => handle,
        Err(err) if should_skip_mount_error(&err.to_string()) && !strict_gate_enabled() => {
            eprintln!("[fuse_small_write_wiring] skip: host refused FUSE mount: {err}");
            return;
        }
        Err(err) => panic!("mount_fuser: {err}"),
    };
    let guard = MountGuard::new(handle, mnt.path().to_path_buf());

    // Let the kernel complete the mount before probing via VFS.
    std::thread::sleep(Duration::from_millis(200));
    if !mount_appears_active(mnt.path()) {
        assert!(
            !strict_gate_enabled(),
            "mounted filesystem did not appear in the native mount table"
        );
        eprintln!("[fuse_small_write_wiring] skip: mount did not appear in the native mount table");
        return;
    }

    // --- create + write + fsync a 4 KiB file via the kernel VFS ---------
    let payload: Vec<u8> = (0..SMALL_FILE_BYTES).map(|i| (i as u8) ^ 0xA5).collect();
    let file_path = mnt.path().join("note.txt");
    {
        let mut f = match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&file_path)
        {
            Ok(f) => f,
            Err(err) if should_skip_io_error(&err) && !strict_gate_enabled() => return,
            Err(err) => panic!("open note.txt for write: {err}"),
        };
        use std::io::Write;
        if let Err(err) = f.write_all(&payload) {
            if should_skip_io_error(&err) && !strict_gate_enabled() {
                return;
            }
            panic!("write: {err}");
        }
        if let Err(err) = f.sync_all() {
            if should_skip_io_error(&err) && !strict_gate_enabled() {
                return;
            }
            panic!("sync_all: {err}");
        }
    }

    // --- best-effort readback (mock folder backend does not auto-publish
    // post-create entries; readback miss is tolerated, upload byte-equality
    // below is the authoritative signal for the wiring assertion). -------
    if let Ok(got) = std::fs::read(&file_path) {
        if got.len() == payload.len() {
            assert_eq!(got, payload, "readback mismatch for small file");
        }
    }

    // --- assert upload finalize hit the backend --------------------------
    {
        let uploads = upload_backend.uploads.lock().unwrap();
        let matched = uploads
            .iter()
            .find(|(_, n, b)| n == "note.txt" && b.len() == payload.len() && b == &payload);
        assert!(
            matched.is_some(),
            "kernel fsync should have finalized a 4 KiB upload; uploads={:?}",
            uploads
                .iter()
                .map(|(p, n, b)| (p.clone(), n.clone(), b.len()))
                .collect::<Vec<_>>()
        );
    }

    // --- unmount cleanly -------------------------------------------------
    guard.unmount().expect("clean unmount");

    // --- assert acknowledged records were checkpointed ------------------
    //
    // A successful backend upload is the commit point. Retaining its
    // Create/Write/FlushBarrier records would replay an already-acknowledged
    // mutation on the next mount. Reopen the on-disk journal as a fresh
    // remount would and require an empty replay set.
    let records = replay_path(&journal_path).expect("journal replay");
    assert!(
        records.is_empty(),
        "successful fsync/unmount must not replay acknowledged note.txt records; got {:?}",
        records.iter().map(|r| &r.op).collect::<Vec<_>>()
    );
}
